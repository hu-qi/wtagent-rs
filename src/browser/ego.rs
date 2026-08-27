use std::{
    env,
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};

use serde_json::Value;
use tokio::{io::AsyncWriteExt, process::Command};
use tracing::debug;

use crate::{
    browser::provider::ProviderConfig,
    error::{Result, WtError},
};

const OUTPUT_MARKER: &str = "__WTAGENT_JSON__";
const DEFAULT_COMMAND_TIMEOUT: Duration = Duration::from_secs(75);
const MAX_DIAGNOSTIC_LINES: usize = 8;
const MAX_DEBUG_EXCERPT_CHARS: usize = 2_048;

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

        debug!(
            executable = %self.executable.display(),
            task_space = %self.task_space,
            timeout_ms = timeout.as_millis(),
            script_len = script.len(),
            parser = "normalized-marker-v3-stream-aware",
            "ego command start"
        );

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

        debug!(
            status = %output.status,
            stdout_len = output.stdout.len(),
            stderr_len = output.stderr.len(),
            stdout = %debug_excerpt(&stdout),
            stderr = %debug_excerpt(&stderr),
            "ego command completed"
        );

        if !output.status.success() {
            let diagnostic = runtime_diagnostic(&stdout, &stderr);
            if is_user_control_diagnostic(&diagnostic) {
                return Err(WtError::Browser(
                    "ego-lite task space is controlled by the user. Browser automation has stopped and WTAgent-RS will not take control back automatically. When you explicitly want WTAgent-RS to continue, run `wtagent ego claim`, then retry or resume the task."
                        .into(),
                ));
            }
            return Err(WtError::Browser(format!(
                "ego-browser failed (exit={}): {diagnostic}",
                output.status
            )));
        }

        let (source, payload) = extract_result_payload(&stdout, &stderr).ok_or_else(|| {
            WtError::Browser(format!(
                "ego-browser produced no WTAgent result: {}",
                runtime_diagnostic(&stdout, &stderr)
            ))
        })?;
        debug!(
            source,
            payload_len = payload.len(),
            "ego WTAgent payload matched"
        );
        serde_json::from_str(payload).map_err(WtError::Json)
    }
}

fn is_user_control_diagnostic(diagnostic: &str) -> bool {
    let diagnostic_lower = diagnostic.to_ascii_lowercase();
    diagnostic.contains("EGO_TASK_SPACE_USER_IN_CONTROL")
        || diagnostic_lower.contains("user is controlling")
        || diagnostic_lower.contains("user has taken control")
        || diagnostic_lower.contains("taken control of this task space")
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

fn extract_result_payload<'a>(stdout: &'a str, stderr: &'a str) -> Option<(&'static str, &'a str)> {
    extract_marked_payload(stdout)
        .map(|payload| ("stdout", payload))
        .or_else(|| extract_marked_payload(stderr).map(|payload| ("stderr", payload)))
}

fn extract_marked_payload(stream: &str) -> Option<&str> {
    stream.lines().rev().find_map(|line| {
        let line = line.trim();
        let Some(json_at) = line.find('{') else {
            debug!(line = %line.escape_debug(), "ego parser skipped line without JSON object");
            return None;
        };
        let marker = line[..json_at].trim();
        let normalized = normalize_marker(marker);
        let accepted = matches!(
            normalized.as_str(),
            "__WTAGENT_JSON__" | "**WTAGENT_JSON**" | "WTAGENT_JSON"
        );

        debug!(
            line = %line.escape_debug(),
            marker = %marker.escape_debug(),
            normalized_marker = %normalized.escape_debug(),
            json_at,
            accepted,
            "ego parser inspected result line"
        );

        accepted.then_some(&line[json_at..])
    })
}

fn normalize_marker(marker: &str) -> String {
    let mut normalized = String::with_capacity(marker.len());
    let mut chars = marker.chars().peekable();

    while let Some(ch) = chars.next() {
        if ch == '\\' && matches!(chars.peek(), Some('_' | '*' | '`')) {
            if let Some(escaped) = chars.next() {
                normalized.push(escaped);
            }
            continue;
        }
        normalized.push(ch);
    }

    normalized
}

fn debug_excerpt(value: &str) -> String {
    let mut escaped = value.escape_debug().to_string();
    if escaped.chars().count() > MAX_DEBUG_EXCERPT_CHARS {
        escaped = escaped
            .chars()
            .take(MAX_DEBUG_EXCERPT_CHARS)
            .collect::<String>();
        escaped.push_str("…[truncated]");
    }
    escaped
}

pub fn discover_ego(explicit: Option<&Path>) -> Result<PathBuf> {
    if let Some(path) = explicit {
        if path.is_file() {
            return Ok(path.to_path_buf());
        }
        return Err(WtError::Config(format!(
            "ego-browser executable does not exist: {}",
            path.display()
        )));
    }

    if let Some(path) = find_on_path("ego-browser") {
        return Ok(path);
    }

    if let Some(home) = dirs::home_dir() {
        let local = home.join(".local/bin/ego-browser");
        if local.is_file() {
            return Ok(local);
        }
    }

    Err(WtError::Config(
        "ego-browser was not found. Install ego-lite and finish its onboarding so `ego-browser` is registered on PATH (normally ~/.local/bin/ego-browser)."
            .into(),
    ))
}

fn find_on_path(name: &str) -> Option<PathBuf> {
    env::var_os("PATH").and_then(|paths| {
        env::split_paths(&paths)
            .map(|dir| dir.join(name))
            .find(|path| path.is_file())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_unexpected_user_takeover() {
        assert!(is_user_control_diagnostic(
            "The user has taken control of this task space, so browser commands are paused."
        ));
    }

    #[test]
    fn recognizes_claim_guidance_as_user_control() {
        assert!(is_user_control_diagnostic(
            "You now control this task space. await claimTaskSpace(id)"
        ));
    }

    #[test]
    fn parses_raw_marker() {
        assert_eq!(
            extract_marked_payload("__WTAGENT_JSON__{\"ok\":true}"),
            Some("{\"ok\":true}")
        );
    }

    #[test]
    fn parses_markdown_decorated_marker() {
        assert_eq!(
            extract_marked_payload("**WTAGENT_JSON**{\"ok\":true}"),
            Some("{\"ok\":true}")
        );
    }

    #[test]
    fn parses_markdown_escaped_marker() {
        assert_eq!(
            extract_marked_payload("\\_\\_WTAGENT\\_JSON\\_\\_{\"ok\":true}"),
            Some("{\"ok\":true}")
        );
    }

    #[test]
    fn extracts_stderr_result_when_stdout_is_empty() {
        let stderr = "**WTAGENT\\_JSON**{\"ok\":true}\n\n";
        assert_eq!(
            extract_result_payload("", stderr),
            Some(("stderr", "{\"ok\":true}"))
        );
    }
}
