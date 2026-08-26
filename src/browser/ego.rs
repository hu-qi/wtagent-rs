use std::{
    env,
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};

use serde_json::Value;
use tokio::{io::AsyncWriteExt, process::Command};

use crate::{
    browser::provider::ProviderConfig,
    error::{Result, WtError},
};

const OUTPUT_MARKER: &str = "__WTAGENT_JSON__";
const OUTPUT_MARKER_CORE: &str = "WTAGENT_JSON";
const OUTPUT_MARKER_CORE_ESCAPED: &str = r"WTAGENT\_JSON";
const DEFAULT_COMMAND_TIMEOUT: Duration = Duration::from_secs(75);
const MAX_DIAGNOSTIC_LINES: usize = 8;

pub struct EgoClient {
    executable: PathBuf,
    task_space: String,
}

impl EgoClient {
    pub async fn launch(
        provider: &ProviderConfig,
        executable_override: Option<&Path>,
        task_space: String,
        preferred_url: Option<&str>,
    ) -> Result<Self> {
        let client = Self {
            executable: discover_ego(executable_override)?,
            task_space,
        };
        client
            .ensure_page(preferred_url.unwrap_or(provider.base_url))
            .await?;
        Ok(client)
    }

    pub async fn claim_task_space(
        executable_override: Option<&Path>,
        task_space: String,
    ) -> Result<()> {
        let client = Self {
            executable: discover_ego(executable_override)?,
            task_space,
        };
        client
            .run_json(
                "const __claimed = await claimTaskSpace(__task.id);\ncliLog('__WTAGENT_JSON__' + JSON.stringify({ done: true, taskSpaceId: __task.id, ownership: __claimed?.ownership ?? 'agent' }));",
            )
            .await?;
        Ok(())
    }

    pub fn executable(&self) -> &Path {
        &self.executable
    }

    pub fn task_space(&self) -> &str {
        &self.task_space
    }

    pub async fn call(&self, method: &str, params: Value) -> Result<Value> {
        let method = serde_json::to_string(method)?;
        let params = serde_json::to_string(&params)?;
        self.run_json(&format!(
            "const __result = await cdp({method}, {params});\ncliLog('{OUTPUT_MARKER}' + JSON.stringify(__result ?? {{}}));"
        ))
        .await
    }

    pub async fn handoff_for_manual_login(&self, timeout: Duration) -> Result<()> {
        let timeout_secs = timeout.as_secs().max(1);
        self.run_json_with_timeout(
            &format!(
                "const __handoff = await handOffTaskSpace(__task.id);\nif (__handoff?.done) {{\n  await waitForAgentControl(__task.id, {{ interval: 1, timeout: {timeout_secs} }});\n}}\ncliLog('{OUTPUT_MARKER}' + JSON.stringify({{ done: true, taskSpaceId: __task.id }}));"
            ),
            timeout.saturating_add(Duration::from_secs(15)),
        )
        .await?;
        Ok(())
    }

    async fn ensure_page(&self, url: &str) -> Result<()> {
        let url = serde_json::to_string(url)?;
        self.run_json(&format!(
            "const __tab = await openOrReuseTab({url}, {{ wait: true, timeout: 60 }});\ncliLog('{OUTPUT_MARKER}' + JSON.stringify({{ ok: true, targetId: __tab?.targetId ?? null }}));"
        ))
        .await?;
        Ok(())
    }

    async fn run_json(&self, code: &str) -> Result<Value> {
        self.run_json_with_timeout(code, DEFAULT_COMMAND_TIMEOUT)
            .await
    }

    async fn run_json_with_timeout(&self, code: &str, timeout: Duration) -> Result<Value> {
        let task_space = serde_json::to_string(&self.task_space)?;
        let script = format!("const __task = await useOrCreateTaskSpace({task_space});\n{code}\n");

        let mut child = Command::new(&self.executable)
            .arg("nodejs")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|error| {
                WtError::Browser(format!(
                    "failed to launch ego-browser at {}: {error}",
                    self.executable.display()
                ))
            })?;

        if let Some(mut stdin) = child.stdin.take() {
            stdin.write_all(script.as_bytes()).await?;
            stdin.shutdown().await?;
        }

        let output = tokio::time::timeout(timeout, child.wait_with_output())
            .await
            .map_err(|_| {
                WtError::Browser(format!(
                    "ego-browser command timed out after {} seconds",
                    timeout.as_secs()
                ))
            })??;
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);

        if !output.status.success() {
            let diagnostic = runtime_diagnostic(&stdout, &stderr);
            if is_user_control_diagnostic(&diagnostic) {
                return Err(WtError::Browser(
                    "ego-lite task space is controlled by the user. If you want WTAgent-RS to resume control, run `wtagent ego claim`, then retry or resume the task."
                        .into(),
                ));
            }
            return Err(WtError::Browser(format!(
                "ego-browser failed (exit={}): {diagnostic}",
                output.status
            )));
        }

        let payload = extract_marked_payload(&stdout).ok_or_else(|| {
            WtError::Browser(format!(
                "ego-browser produced no WTAgent result: {}",
                runtime_diagnostic(&stdout, &stderr)
            ))
        })?;
        serde_json::from_str(payload).map_err(WtError::Json)
    }
}

fn is_user_control_diagnostic(diagnostic: &str) -> bool {
    let diagnostic_lower = diagnostic.to_ascii_lowercase();
    diagnostic.contains("EGO_TASK_SPACE_USER_IN_CONTROL")
        || diagnostic_lower.contains("user is controlling")
        || diagnostic_lower.contains("user-owned")
        || diagnostic_lower.contains("you now control this task space")
        || diagnostic_lower.contains("claimtaskspace")
}

fn runtime_diagnostic(stdout: &str, stderr: &str) -> String {
    let mut lines: Vec<&str> = stderr
        .lines()
        .chain(stdout.lines())
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect();
    if lines.is_empty() {
        return "ego-browser exited without diagnostic output".into();
    }
    if lines.len() > MAX_DIAGNOSTIC_LINES {
        lines.drain(..lines.len() - MAX_DIAGNOSTIC_LINES);
    }
    lines.join(" | ")
}

fn extract_marked_payload(stdout: &str) -> Option<&str> {
    stdout.lines().rev().find_map(|line| {
        let line = line.trim();
        let (marker_at, marker_len) = find_marker_core(line)?;
        let prefix = &line[..marker_at];
        if !prefix
            .chars()
            .all(|ch| ch.is_whitespace() || matches!(ch, '_' | '*' | '`'))
        {
            return None;
        }

        let payload = line[marker_at + marker_len..].trim_start_matches(|ch: char| {
            ch.is_whitespace() || matches!(ch, '_' | '*' | '`' | ':')
        });
        (!payload.is_empty()).then_some(payload)
    })
}

fn find_marker_core(line: &str) -> Option<(usize, usize)> {
    let plain = line
        .find(OUTPUT_MARKER_CORE)
        .map(|index| (index, OUTPUT_MARKER_CORE.len()));
    let escaped = line
        .find(OUTPUT_MARKER_CORE_ESCAPED)
        .map(|index| (index, OUTPUT_MARKER_CORE_ESCAPED.len()));

    match (plain, escaped) {
        (Some(plain), Some(escaped)) => Some(if plain.0 <= escaped.0 { plain } else { escaped }),
        (Some(plain), None) => Some(plain),
        (None, Some(escaped)) => Some(escaped),
        (None, None) => None,
    }
}

pub fn discover_ego(override_path: Option<&Path>) -> Result<PathBuf> {
    if let Some(path) = override_path {
        if path.is_file() {
            return Ok(path.to_path_buf());
        }
        return Err(WtError::Config(format!(
            "configured ego-browser path does not exist: {}",
            path.display()
        )));
    }

    let binary = if cfg!(target_os = "windows") {
        "ego-browser.exe"
    } else {
        "ego-browser"
    };

    if let Some(path) = env::var_os("PATH") {
        for base in env::split_paths(&path) {
            let candidate = base.join(binary);
            if candidate.is_file() {
                return Ok(candidate);
            }
        }
    }

    if let Some(home) = dirs::home_dir() {
        let candidate = home.join(".local/bin").join(binary);
        if candidate.is_file() {
            return Ok(candidate);
        }
    }

    Err(WtError::Config(
        "ego-browser was not found. Install ego lite and finish onboarding, add ~/.local/bin to PATH, or pass --ego-path."
            .into(),
    ))
}

#[cfg(test)]
mod tests {
    use super::{extract_marked_payload, is_user_control_diagnostic, runtime_diagnostic};

    #[test]
    fn parses_original_marker() {
        assert_eq!(
            extract_marked_payload("__WTAGENT_JSON__{\"ok\":true}"),
            Some("{\"ok\":true}")
        );
    }

    #[test]
    fn parses_markdown_decorated_marker_from_ego_browser() {
        assert_eq!(
            extract_marked_payload("**WTAGENT_JSON**{\"ok\":true,\"targetId\":\"abc\"}"),
            Some("{\"ok\":true,\"targetId\":\"abc\"}")
        );
    }

    #[test]
    fn parses_markdown_escaped_marker_from_ego_browser() {
        assert_eq!(
            extract_marked_payload(r#"**WTAGENT\_JSON**{"done":true,"ownership":"agent"}"#),
            Some(r#"{"done":true,"ownership":"agent"}"#)
        );
    }

    #[test]
    fn ignores_marker_mentions_in_prose() {
        assert_eq!(
            extract_marked_payload("warning: expected WTAGENT_JSON marker"),
            None
        );
    }

    #[test]
    fn keeps_useful_runtime_error_context() {
        let diagnostic = runtime_diagnostic(
            "some stdout\n",
            "Error: task space is user-owned\nego's nodejs process exited with code 1\n",
        );
        assert!(diagnostic.contains("task space is user-owned"));
        assert!(diagnostic.contains("nodejs process exited"));
    }

    #[test]
    fn recognizes_claim_guidance_as_user_control() {
        assert!(is_user_control_diagnostic(
            "await claimTaskSpace(id) | You now control this task space."
        ));
    }
}
