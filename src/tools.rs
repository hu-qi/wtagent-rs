use std::{
    collections::{HashMap, HashSet},
    path::Path,
    process::Stdio,
    sync::Arc,
    time::{Duration, Instant},
};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::{
    io::AsyncReadExt,
    process::{Child, Command},
    sync::Mutex,
};
use uuid::Uuid;

use crate::{
    config::Limits,
    error::{Result, WtError},
    policy::PolicyEngine,
    protocol::ToolCall,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolRisk {
    Read,
    Write,
    Execute,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResult {
    pub name: String,
    pub ok: bool,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

impl ToolResult {
    pub fn denied(name: &str, message: impl Into<String>) -> Self {
        Self {
            name: name.to_string(),
            ok: false,
            message: message.into(),
            data: None,
        }
    }

    pub fn to_value(&self) -> Value {
        serde_json::to_value(self).unwrap_or_else(
            |_| json!({"name": self.name, "ok": false, "message": "result serialization failed"}),
        )
    }
}

struct ManagedProcess {
    child: Child,
    command: String,
    started_at: Instant,
    output: Arc<Mutex<Vec<u8>>>,
}

pub struct ToolExecutor {
    policy: PolicyEngine,
    limits: Limits,
    processes: Arc<Mutex<HashMap<String, ManagedProcess>>>,
}

impl ToolExecutor {
    pub fn new(policy: PolicyEngine, limits: Limits) -> Self {
        Self {
            policy,
            limits,
            processes: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn risk(name: &str) -> Option<ToolRisk> {
        match name {
            "fs.list" | "fs.read" | "fs.search" | "process.read" | "process.list" => {
                Some(ToolRisk::Read)
            }
            "fs.write" | "fs.edit" => Some(ToolRisk::Write),
            "terminal.exec" | "process.start" | "process.stop" => Some(ToolRisk::Execute),
            _ => None,
        }
    }

    pub fn project_root(&self) -> &Path {
        self.policy.project_root()
    }

    pub fn policy(&self) -> &PolicyEngine {
        &self.policy
    }

    pub async fn execute(&self, call: &ToolCall) -> ToolResult {
        let result = match call.name.as_str() {
            "fs.list" => self.fs_list(&call.args).await,
            "fs.read" => self.fs_read(&call.args).await,
            "fs.search" => self.fs_search(&call.args).await,
            "fs.write" => self.fs_write(&call.args).await,
            "fs.edit" => self.fs_edit(&call.args).await,
            "terminal.exec" => self.terminal_exec(&call.args).await,
            "process.start" => self.process_start(&call.args).await,
            "process.read" => self.process_read(&call.args).await,
            "process.list" => self.process_list().await,
            "process.stop" => self.process_stop(&call.args).await,
            other => Err(WtError::Tool(format!("unknown local tool: {other}"))),
        };
        match result {
            Ok(mut value) => {
                value.name = call.name.clone();
                value
            }
            Err(error) => ToolResult {
                name: call.name.clone(),
                ok: false,
                message: error.to_string(),
                data: None,
            },
        }
    }

    async fn fs_list(&self, args: &Value) -> Result<ToolResult> {
        let raw = string_arg(args, "path").unwrap_or_else(|| ".".into());
        let depth = usize_arg(args, "depth").unwrap_or(2).min(5);
        let include_hidden = bool_arg(args, "include_hidden").unwrap_or(false);
        let target = self.policy.resolve_read_path(&raw)?;
        if !target.is_dir() {
            return Err(WtError::Tool(format!("not a directory: {raw}")));
        }
        let root = self.policy.project_root().to_path_buf();
        let max_entries = 2_000usize;
        let entries = tokio::task::spawn_blocking(move || {
            let mut output = Vec::<Value>::new();
            walk_tree(
                &root,
                &target,
                depth,
                include_hidden,
                max_entries,
                &mut output,
            )?;
            Ok::<_, std::io::Error>(output)
        })
        .await
        .map_err(|e| WtError::Tool(format!("directory task failed: {e}")))??;
        let truncated = entries.len() >= max_entries;
        Ok(ToolResult {
            name: String::new(),
            ok: true,
            message: format!("Listed {} entries.", entries.len()),
            data: Some(json!({"entries": entries, "truncated": truncated})),
        })
    }

    async fn fs_read(&self, args: &Value) -> Result<ToolResult> {
        let raw = required_string(args, "path")?;
        let offset = usize_arg(args, "offset").unwrap_or(0);
        let max_bytes = usize_arg(args, "max_bytes")
            .unwrap_or(self.limits.max_file_read_bytes)
            .min(self.limits.max_file_read_bytes);
        let target = self.policy.resolve_read_path(&raw)?;
        let data = tokio::fs::read(&target).await?;
        let start = offset.min(data.len());
        let end = (start + max_bytes).min(data.len());
        let content = String::from_utf8_lossy(&data[start..end]).into_owned();
        Ok(ToolResult {
            name: String::new(),
            ok: true,
            message: format!("Read {raw}."),
            data: Some(json!({
                "content": content,
                "offset": start,
                "bytes_read": end - start,
                "next_offset": end,
                "truncated": end < data.len(),
                "size": data.len()
            })),
        })
    }

    async fn fs_search(&self, args: &Value) -> Result<ToolResult> {
        let query = required_string(args, "query")?;
        let raw_path = string_arg(args, "path").unwrap_or_else(|| ".".into());
        let regex = bool_arg(args, "regex").unwrap_or(false);
        let max_results = usize_arg(args, "max_results").unwrap_or(200).min(1_000);
        let target = self.policy.resolve_read_path(&raw_path)?;
        let pattern = if regex {
            Some(
                regex::Regex::new(&query)
                    .map_err(|e| WtError::Tool(format!("invalid regex: {e}")))?,
            )
        } else {
            None
        };
        let excludes: HashSet<String> = [
            ".git",
            "node_modules",
            "target",
            "dist",
            "build",
            ".next",
            ".cache",
            "vendor",
        ]
        .into_iter()
        .map(str::to_string)
        .collect();
        let matches = tokio::task::spawn_blocking(move || {
            search_tree(&target, &query, pattern.as_ref(), max_results, &excludes)
        })
        .await
        .map_err(|e| WtError::Tool(format!("search task failed: {e}")))??;
        Ok(ToolResult {
            name: String::new(),
            ok: true,
            message: if matches.is_empty() {
                "No matches.".into()
            } else {
                format!("Found {} matches.", matches.len())
            },
            data: Some(json!({"matches": matches, "truncated": matches.len() >= max_results})),
        })
    }

    async fn fs_write(&self, args: &Value) -> Result<ToolResult> {
        let raw = required_string(args, "path")?;
        let content = string_arg(args, "content").unwrap_or_default();
        let mode = string_arg(args, "mode").unwrap_or_else(|| "overwrite".into());
        let target = self.policy.resolve_write_path(&raw)?;
        if let Some(parent) = target.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        if mode == "append" {
            use tokio::io::AsyncWriteExt;
            let mut options = tokio::fs::OpenOptions::new();
            options.create(true).append(true);
            let mut file = options.open(&target).await?;
            file.write_all(content.as_bytes()).await?;
            file.flush().await?;
        } else if mode == "overwrite" {
            atomic_write(&target, content.as_bytes()).await?;
        } else {
            return Err(WtError::Tool(format!("unsupported write mode: {mode}")));
        }
        Ok(ToolResult {
            name: String::new(),
            ok: true,
            message: format!("Wrote {raw}."),
            data: Some(json!({"bytes": content.len(), "mode": mode})),
        })
    }

    async fn fs_edit(&self, args: &Value) -> Result<ToolResult> {
        let raw = required_string(args, "path")?;
        let target = self.policy.resolve_write_path(&raw)?;
        let mut content = tokio::fs::read_to_string(&target).await?;
        let edits = args
            .get("edits")
            .and_then(Value::as_array)
            .ok_or_else(|| WtError::Tool("fs.edit requires an edits array".into()))?;
        if edits.is_empty() {
            return Err(WtError::Tool("fs.edit requires at least one edit".into()));
        }

        for (index, edit) in edits.iter().enumerate() {
            let old = required_string(edit, "old_text")?;
            let new = string_arg(edit, "new_text").unwrap_or_default();
            let replace_all = bool_arg(edit, "replace_all").unwrap_or(false);
            if old == new {
                return Err(WtError::Tool(format!("edit {} makes no change", index + 1)));
            }
            let occurrences = content.matches(&old).count();
            if occurrences == 0 {
                return Err(WtError::Tool(format!(
                    "edit {} old_text was not found",
                    index + 1
                )));
            }
            if !replace_all && occurrences != 1 {
                return Err(WtError::Tool(format!(
                    "edit {} matched {occurrences} times; make it unique or set replace_all",
                    index + 1
                )));
            }
            content = if replace_all {
                content.replace(&old, &new)
            } else {
                content.replacen(&old, &new, 1)
            };
        }
        atomic_write(&target, content.as_bytes()).await?;
        Ok(ToolResult {
            name: String::new(),
            ok: true,
            message: format!("Edited {raw}."),
            data: Some(json!({"edits_applied": edits.len()})),
        })
    }

    async fn terminal_exec(&self, args: &Value) -> Result<ToolResult> {
        let program = required_string(args, "program")?;
        let argv = string_array_arg(args, "argv")?;
        let raw_cwd = string_arg(args, "cwd").unwrap_or_else(|| ".".into());
        let cwd = self.policy.resolve_read_path(&raw_cwd)?;
        if !cwd.is_dir() {
            return Err(WtError::Tool(format!("cwd is not a directory: {raw_cwd}")));
        }
        let requested =
            u64_arg(args, "timeout_ms").unwrap_or(self.limits.command_timeout.as_millis() as u64);
        let timeout = Duration::from_millis(requested).min(self.limits.command_timeout);
        let inherit_sensitive = bool_arg(args, "inherit_sensitive_env").unwrap_or(false);
        let started = Instant::now();
        let mut command = build_command(&program, &argv);
        command
            .current_dir(cwd)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        apply_safe_env(&mut command, inherit_sensitive);

        let output = tokio::time::timeout(timeout, command.output()).await;
        match output {
            Ok(Ok(output)) => {
                let stdout = truncate_utf8(&output.stdout, self.limits.max_tool_output_bytes / 2);
                let stderr = truncate_utf8(&output.stderr, self.limits.max_tool_output_bytes / 2);
                Ok(ToolResult {
                    name: String::new(),
                    ok: output.status.success(),
                    message: format!("Command exited with {:?}.", output.status.code()),
                    data: Some(json!({
                        "program": program,
                        "argv": argv,
                        "exit_code": output.status.code(),
                        "stdout": stdout,
                        "stderr": stderr,
                        "duration_ms": started.elapsed().as_millis()
                    })),
                })
            }
            Ok(Err(error)) => Err(WtError::Tool(format!("failed to run {program}: {error}"))),
            Err(_) => Ok(ToolResult {
                name: String::new(),
                ok: false,
                message: format!("Command timed out after {} ms.", timeout.as_millis()),
                data: Some(json!({"timed_out": true, "completion_unknown": false})),
            }),
        }
    }

    async fn process_start(&self, args: &Value) -> Result<ToolResult> {
        let program = required_string(args, "program")?;
        let argv = string_array_arg(args, "argv")?;
        let raw_cwd = string_arg(args, "cwd").unwrap_or_else(|| ".".into());
        let cwd = self.policy.resolve_read_path(&raw_cwd)?;
        let inherit_sensitive = bool_arg(args, "inherit_sensitive_env").unwrap_or(false);
        let mut command = build_command(&program, &argv);
        command
            .current_dir(cwd)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        apply_safe_env(&mut command, inherit_sensitive);
        let mut child = command
            .spawn()
            .map_err(|e| WtError::Tool(format!("failed to start {program}: {e}")))?;
        let output = Arc::new(Mutex::new(Vec::<u8>::new()));
        if let Some(stdout) = child.stdout.take() {
            capture_stream(stdout, output.clone(), 128 * 1024);
        }
        if let Some(stderr) = child.stderr.take() {
            capture_stream(stderr, output.clone(), 128 * 1024);
        }
        let process_id = format!("proc_{}", &Uuid::new_v4().simple().to_string()[..12]);
        self.processes.lock().await.insert(
            process_id.clone(),
            ManagedProcess {
                child,
                command: format_command(&program, &argv),
                started_at: Instant::now(),
                output,
            },
        );
        Ok(ToolResult {
            name: String::new(),
            ok: true,
            message: format!("Started {program} as {process_id}."),
            data: Some(json!({"process_id": process_id, "program": program, "argv": argv})),
        })
    }

    async fn process_read(&self, args: &Value) -> Result<ToolResult> {
        let process_id = required_string(args, "process_id")?;
        let mut processes = self.processes.lock().await;
        let entry = processes
            .get_mut(&process_id)
            .ok_or_else(|| WtError::Tool(format!("unknown process_id: {process_id}")))?;
        let status = entry
            .child
            .try_wait()
            .map_err(|e| WtError::Tool(format!("cannot inspect process: {e}")))?;
        let bytes = entry.output.lock().await.clone();
        let output = truncate_utf8(&bytes, self.limits.max_tool_output_bytes);
        Ok(ToolResult {
            name: String::new(),
            ok: true,
            message: format!("Read process {process_id}."),
            data: Some(json!({
                "process_id": process_id,
                "command": entry.command,
                "running": status.is_none(),
                "exit_code": status.and_then(|s| s.code()),
                "output": output,
                "duration_ms": entry.started_at.elapsed().as_millis()
            })),
        })
    }

    async fn process_list(&self) -> Result<ToolResult> {
        let mut processes = self.processes.lock().await;
        let mut items = Vec::new();
        for (id, entry) in processes.iter_mut() {
            let status = entry.child.try_wait().ok().flatten();
            items.push(json!({
                "process_id": id,
                "command": entry.command,
                "running": status.is_none(),
                "exit_code": status.and_then(|s| s.code()),
                "duration_ms": entry.started_at.elapsed().as_millis()
            }));
        }
        Ok(ToolResult {
            name: String::new(),
            ok: true,
            message: format!("Listed {} managed processes.", items.len()),
            data: Some(json!({"processes": items})),
        })
    }

    async fn process_stop(&self, args: &Value) -> Result<ToolResult> {
        let process_id = required_string(args, "process_id")?;
        let mut entry = self
            .processes
            .lock()
            .await
            .remove(&process_id)
            .ok_or_else(|| WtError::Tool(format!("unknown process_id: {process_id}")))?;
        let _ = entry.child.kill().await;
        let _ = entry.child.wait().await;
        let bytes = entry.output.lock().await.clone();
        Ok(ToolResult {
            name: String::new(),
            ok: true,
            message: format!("Stopped process {process_id}."),
            data: Some(json!({
                "process_id": process_id,
                "output": truncate_utf8(&bytes, self.limits.max_tool_output_bytes)
            })),
        })
    }
}

fn walk_tree(
    root: &Path,
    directory: &Path,
    depth: usize,
    include_hidden: bool,
    max_entries: usize,
    output: &mut Vec<Value>,
) -> std::io::Result<()> {
    if output.len() >= max_entries {
        return Ok(());
    }
    let mut entries: Vec<_> = std::fs::read_dir(directory)?
        .filter_map(|e| e.ok())
        .collect();
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        if output.len() >= max_entries {
            break;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        if !include_hidden && name.starts_with('.') {
            continue;
        }
        if matches!(name.as_str(), ".git" | "node_modules" | "target" | "vendor") {
            continue;
        }
        let path = entry.path();
        let metadata = std::fs::symlink_metadata(&path)?;
        let kind = if metadata.file_type().is_symlink() {
            "symlink"
        } else if metadata.is_dir() {
            "directory"
        } else {
            "file"
        };
        output.push(json!({
            "path": path.strip_prefix(root).unwrap_or(&path).to_string_lossy(),
            "type": kind
        }));
        if metadata.is_dir() && depth > 0 {
            walk_tree(root, &path, depth - 1, include_hidden, max_entries, output)?;
        }
    }
    Ok(())
}

fn search_tree(
    target: &Path,
    query: &str,
    regex: Option<&regex::Regex>,
    max_results: usize,
    excludes: &HashSet<String>,
) -> std::io::Result<Vec<Value>> {
    let mut matches = Vec::new();
    search_path(target, query, regex, max_results, excludes, &mut matches)?;
    Ok(matches)
}

fn search_path(
    path: &Path,
    query: &str,
    regex: Option<&regex::Regex>,
    max_results: usize,
    excludes: &HashSet<String>,
    matches: &mut Vec<Value>,
) -> std::io::Result<()> {
    if matches.len() >= max_results {
        return Ok(());
    }
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(_) => return Ok(()),
    };
    if metadata.file_type().is_symlink() {
        return Ok(());
    }
    if metadata.is_dir() {
        for entry in std::fs::read_dir(path)?.filter_map(|e| e.ok()) {
            let name = entry.file_name().to_string_lossy().to_string();
            if excludes.contains(&name) {
                continue;
            }
            search_path(&entry.path(), query, regex, max_results, excludes, matches)?;
            if matches.len() >= max_results {
                break;
            }
        }
        return Ok(());
    }
    if metadata.len() > 2 * 1024 * 1024 {
        return Ok(());
    }
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(_) => return Ok(()),
    };
    if bytes.iter().take(8_192).any(|byte| *byte == 0) {
        return Ok(());
    }
    let text = String::from_utf8_lossy(&bytes);
    for (index, line) in text.lines().enumerate() {
        let matched = regex
            .map(|r| r.is_match(line))
            .unwrap_or_else(|| line.contains(query));
        if matched {
            matches.push(json!({
                "path": path.to_string_lossy(),
                "line": index + 1,
                "text": line.chars().take(500).collect::<String>()
            }));
            if matches.len() >= max_results {
                break;
            }
        }
    }
    Ok(())
}

async fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| WtError::Tool(format!("write target has no parent: {}", path.display())))?;
    tokio::fs::create_dir_all(parent).await?;
    let temp = parent.join(format!(
        ".{}.wtagent-{}.tmp",
        path.file_name().and_then(|n| n.to_str()).unwrap_or("file"),
        Uuid::new_v4()
    ));
    tokio::fs::write(&temp, bytes).await?;
    if let Err(first) = tokio::fs::rename(&temp, path).await {
        if path.exists() {
            tokio::fs::remove_file(path).await?;
            tokio::fs::rename(&temp, path).await?;
        } else {
            let _ = tokio::fs::remove_file(&temp).await;
            return Err(first.into());
        }
    }
    Ok(())
}

fn build_command(program: &str, argv: &[String]) -> Command {
    let mut command = Command::new(program);
    command.args(argv);
    command
}

fn apply_safe_env(command: &mut Command, inherit_sensitive: bool) {
    if inherit_sensitive {
        return;
    }
    command.env_clear();
    for (key, value) in std::env::vars_os() {
        let upper = key.to_string_lossy().to_ascii_uppercase();
        let sensitive = [
            "TOKEN",
            "SECRET",
            "PASSWORD",
            "PASSWD",
            "API_KEY",
            "APIKEY",
            "COOKIE",
            "AUTHORIZATION",
            "CREDENTIAL",
            "PRIVATE_KEY",
        ]
        .iter()
        .any(|marker| upper.contains(marker));
        if !sensitive {
            command.env(key, value);
        }
    }
}

fn capture_stream<R>(mut reader: R, output: Arc<Mutex<Vec<u8>>>, max_bytes: usize)
where
    R: tokio::io::AsyncRead + Unpin + Send + 'static,
{
    tokio::spawn(async move {
        let mut buffer = [0u8; 4_096];
        loop {
            match reader.read(&mut buffer).await {
                Ok(0) | Err(_) => break,
                Ok(read) => {
                    let mut target = output.lock().await;
                    let remaining = max_bytes.saturating_sub(target.len());
                    if remaining > 0 {
                        target.extend_from_slice(&buffer[..read.min(remaining)]);
                    }
                }
            }
        }
    });
}

fn truncate_utf8(bytes: &[u8], max_bytes: usize) -> String {
    if bytes.len() <= max_bytes {
        return String::from_utf8_lossy(bytes).into_owned();
    }
    let mut end = max_bytes;
    while end > 0 && std::str::from_utf8(&bytes[..end]).is_err() {
        end -= 1;
    }
    let prefix = String::from_utf8_lossy(&bytes[..end]);
    format!("{prefix}\n…[truncated {} bytes]", bytes.len() - end)
}

fn format_command(program: &str, argv: &[String]) -> String {
    std::iter::once(program)
        .chain(argv.iter().map(String::as_str))
        .map(|part| {
            if part.contains(char::is_whitespace) {
                format!("{part:?}")
            } else {
                part.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn required_string(value: &Value, key: &str) -> Result<String> {
    string_arg(value, key).ok_or_else(|| WtError::Tool(format!("missing string argument: {key}")))
}

fn string_arg(value: &Value, key: &str) -> Option<String> {
    let item = value.get(key)?;
    match item {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

fn bool_arg(value: &Value, key: &str) -> Option<bool> {
    let item = value.get(key)?;
    match item {
        Value::Bool(v) => Some(*v),
        Value::String(v) if v.eq_ignore_ascii_case("true") => Some(true),
        Value::String(v) if v.eq_ignore_ascii_case("false") => Some(false),
        _ => None,
    }
}

fn usize_arg(value: &Value, key: &str) -> Option<usize> {
    value
        .get(key)
        .and_then(|v| v.as_u64().or_else(|| v.as_str()?.parse().ok()))
        .map(|v| v as usize)
}

fn u64_arg(value: &Value, key: &str) -> Option<u64> {
    value
        .get(key)
        .and_then(|v| v.as_u64().or_else(|| v.as_str()?.parse().ok()))
}

fn string_array_arg(value: &Value, key: &str) -> Result<Vec<String>> {
    let Some(item) = value.get(key) else {
        return Ok(Vec::new());
    };
    match item {
        Value::Array(items) => items
            .iter()
            .map(|item| match item {
                Value::String(value) => Ok(value.clone()),
                other => Err(WtError::Tool(format!(
                    "{key} must contain strings, got {other}"
                ))),
            })
            .collect(),
        Value::String(text) if text.trim().is_empty() => Ok(Vec::new()),
        Value::String(text) if text.trim_start().starts_with('[') => {
            serde_json::from_str::<Vec<String>>(text)
                .map_err(|e| WtError::Tool(format!("invalid {key} array: {e}")))
        }
        Value::String(text) => Ok(vec![text.clone()]),
        other => Err(WtError::Tool(format!(
            "{key} must be an array of strings, got {other}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ApprovalMode;

    #[tokio::test]
    async fn read_and_edit_project_file() {
        let temp = tempfile::tempdir().unwrap();
        tokio::fs::write(temp.path().join("a.txt"), "hello\n")
            .await
            .unwrap();
        let policy = PolicyEngine::new(temp.path().canonicalize().unwrap(), ApprovalMode::Auto);
        let executor = ToolExecutor::new(policy, Limits::default());
        let read = executor
            .execute(&ToolCall {
                name: "fs.read".into(),
                args: json!({"path":"a.txt"}),
            })
            .await;
        assert!(read.ok);
        let edit = executor
            .execute(&ToolCall {
                name: "fs.edit".into(),
                args: json!({"path":"a.txt","edits":[{"old_text":"hello","new_text":"world"}]}),
            })
            .await;
        assert!(edit.ok);
        assert_eq!(
            tokio::fs::read_to_string(temp.path().join("a.txt"))
                .await
                .unwrap(),
            "world\n"
        );
    }
}
