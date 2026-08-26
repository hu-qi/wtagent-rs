use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::error::{Result, WtError};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolCall {
    pub name: String,
    #[serde(default)]
    pub args: Value,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AgentResponse {
    pub message: String,
    pub done: bool,
    pub tool_calls: Vec<ToolCall>,
}

pub fn parse_agent_response(raw: &str) -> Result<AgentResponse> {
    let xml = extract_envelope(raw).ok_or_else(|| {
        WtError::Protocol("assistant reply did not contain <agent_response>".into())
    })?;
    let doc = roxmltree::Document::parse(xml)
        .map_err(|e| WtError::Protocol(format!("invalid agent XML: {e}")))?;
    let root = doc.root_element();
    if root.tag_name().name() != "agent_response" {
        return Err(WtError::Protocol(
            "root element must be <agent_response>".into(),
        ));
    }

    let message = child_text(root, "message").unwrap_or_default();
    let done_text = child_text(root, "done").unwrap_or_else(|| "false".into());
    let done = match done_text.trim().to_ascii_lowercase().as_str() {
        "true" | "1" => true,
        "false" | "0" | "" => false,
        other => {
            return Err(WtError::Protocol(format!(
                "<done> must be true or false, got {other:?}"
            )))
        }
    };

    let mut tool_calls = Vec::new();
    for node in root.descendants().filter(|n| n.has_tag_name("tool_call")) {
        // Only accept tool_call elements directly under tool_calls or the root.
        let parent_name = node.parent_element().map(|p| p.tag_name().name());
        if !matches!(parent_name, Some("tool_calls") | Some("agent_response")) {
            continue;
        }
        let name = node
            .attribute("name")
            .map(ToOwned::to_owned)
            .or_else(|| child_text(node, "name"))
            .filter(|s| !s.trim().is_empty())
            .ok_or_else(|| WtError::Protocol("tool_call is missing a name".into()))?;
        let args = parse_args(node)?;
        tool_calls.push(ToolCall { name, args });
    }

    if done && !tool_calls.is_empty() {
        return Err(WtError::Protocol(
            "done=true cannot include local tool calls".into(),
        ));
    }
    if done && message.trim().is_empty() {
        return Err(WtError::Protocol(
            "done=true requires a non-empty <message>".into(),
        ));
    }

    Ok(AgentResponse {
        message: message.trim().to_string(),
        done,
        tool_calls,
    })
}

fn parse_args(tool_call: roxmltree::Node<'_, '_>) -> Result<Value> {
    let Some(args_node) = tool_call.children().find(|n| n.has_tag_name("args")) else {
        return Ok(Value::Object(Map::new()));
    };

    let text = args_node.text().unwrap_or_default().trim();
    if !text.is_empty() && (text.starts_with('{') || text.starts_with('[')) {
        return serde_json::from_str(text)
            .map_err(|e| WtError::Protocol(format!("tool args JSON is invalid: {e}")));
    }

    let element_children: Vec<_> = args_node.children().filter(|n| n.is_element()).collect();
    if element_children.is_empty() {
        if text.is_empty() {
            return Ok(Value::Object(Map::new()));
        }
        return Ok(Value::String(text.to_string()));
    }

    let mut object = Map::new();
    for child in element_children {
        insert_xml_value(&mut object, child.tag_name().name(), xml_node_to_json(child));
    }
    Ok(Value::Object(object))
}

fn xml_node_to_json(node: roxmltree::Node<'_, '_>) -> Value {
    let children: Vec<_> = node.children().filter(|n| n.is_element()).collect();
    if children.is_empty() {
        return scalar(node.text().unwrap_or_default().trim());
    }

    // XML arrays produced by the original WTAgent use <item> children.
    if children.iter().all(|child| child.has_tag_name("item")) {
        return Value::Array(children.into_iter().map(xml_node_to_json).collect());
    }

    let mut object = Map::new();
    for child in children {
        insert_xml_value(&mut object, child.tag_name().name(), xml_node_to_json(child));
    }
    Value::Object(object)
}

fn insert_xml_value(object: &mut Map<String, Value>, key: &str, value: Value) {
    match object.get_mut(key) {
        None => {
            object.insert(key.to_string(), value);
        }
        Some(Value::Array(items)) => items.push(value),
        Some(existing) => {
            let previous = std::mem::replace(existing, Value::Null);
            *existing = Value::Array(vec![previous, value]);
        }
    }
}

fn scalar(text: &str) -> Value {
    match text {
        "true" => Value::Bool(true),
        "false" => Value::Bool(false),
        _ => text
            .parse::<i64>()
            .map(Value::from)
            .or_else(|_| text.parse::<f64>().map(Value::from))
            .unwrap_or_else(|_| Value::String(text.to_string())),
    }
}

fn child_text(node: roxmltree::Node<'_, '_>, name: &str) -> Option<String> {
    node.children()
        .find(|child| child.has_tag_name(name))
        .map(|child| child.text().unwrap_or_default().to_string())
}

pub fn extract_envelope(raw: &str) -> Option<&str> {
    let start = raw.find("<agent_response")?;
    let end_tag = "</agent_response>";
    let end = raw[start..].find(end_tag)? + start + end_tag.len();
    Some(&raw[start..end])
}

pub fn build_bootstrap_prompt(task: &str, project_root: &str) -> String {
    format!(
        r#"You are the reasoning component of WTAgent-RS, a local coding agent.
The local Rust runtime — not the web page — is the authority for tools, permissions, and state.

Task:
{task}

Project root:
{project_root}

Reply using this XML envelope only when requesting local work:
<agent_response>
  <message>short progress or final answer</message>
  <done>false</done>
  <tool_calls>
    <tool_call name="fs.read"><args>{{"path":"src/main.rs","offset":0,"max_bytes":16384}}</args></tool_call>
  </tool_calls>
</agent_response>

Rules:
- Set <done>true</done> only when the task is complete; then omit <tool_calls>.
- Prefer the smallest useful local operation.
- Up to 4 READ-ONLY calls may be batched in one <tool_calls> block. This reduces web turns and provider load.
- WRITE/EXECUTE/PROCESS side effects must be requested one at a time and can require user approval.
- Never assume a tool succeeded until its tool result is returned.
- Never ask to bypass a CAPTCHA, anti-bot challenge, usage limit, or provider policy. The runtime will stop and return control to the user.
- Available tools: fs.list, fs.read, fs.search, fs.write, fs.edit, terminal.exec, process.start, process.read, process.list, process.stop.
- terminal.exec takes a program and argv array; do not send a shell command string.
- Keep tool output requests narrow; large results are deterministically compacted before being sent back.
"#
    )
}

pub fn build_follow_up(instruction: &str) -> String {
    format!(
        "{instruction}\n\nContinue from the existing conversation context. Use <agent_response> XML for local tool requests."
    )
}

pub fn build_protocol_correction(error: &str) -> String {
    format!(
        "The previous reply attempted WTAgent XML but could not be parsed: {}. Reply once more with one complete <agent_response> envelope. Do not repeat any local side effect whose result is already visible.",
        truncate_chars(error, 240)
    )
}

pub fn serialize_tool_results(results: &[Value]) -> String {
    let json = serde_json::to_string(results).unwrap_or_else(|_| "[]".to_string());
    format!(
        "<tool_results><result_json>{}</result_json></tool_results>\nContinue the task. Reply with <agent_response> XML if more local work is needed.",
        escape_xml(&json)
    )
}

fn escape_xml(input: &str) -> String {
    input
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn truncate_chars(value: &str, max: usize) -> String {
    let mut chars = value.chars();
    let prefix: String = chars.by_ref().take(max).collect();
    if chars.next().is_some() {
        format!("{prefix}…")
    } else {
        prefix
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_json_args_and_batch() {
        let raw = r#"<agent_response><message>inspect</message><done>false</done><tool_calls><tool_call name="fs.read"><args>{"path":"Cargo.toml"}</args></tool_call><tool_call name="fs.list"><args>{"path":"."}</args></tool_call></tool_calls></agent_response>"#;
        let parsed = parse_agent_response(raw).unwrap();
        assert_eq!(parsed.tool_calls.len(), 2);
        assert_eq!(parsed.tool_calls[0].args["path"], "Cargo.toml");
    }

    #[test]
    fn accepts_legacy_nested_xml_args() {
        let raw = r#"<agent_response><message>x</message><done>false</done><tool_calls><tool_call name="terminal.exec"><args><program>cargo</program><argv><item>test</item><item>--all</item></argv><timeout_ms>1000</timeout_ms></args></tool_call></tool_calls></agent_response>"#;
        let parsed = parse_agent_response(raw).unwrap();
        assert_eq!(parsed.tool_calls[0].args["program"], "cargo");
        assert_eq!(parsed.tool_calls[0].args["argv"][0], "test");
        assert_eq!(parsed.tool_calls[0].args["timeout_ms"], 1000);
    }

    #[test]
    fn extracts_envelope_from_markdown_noise() {
        let raw = "prefix\n```xml\n<agent_response><message>done</message><done>true</done></agent_response>\n```";
        assert!(parse_agent_response(raw).unwrap().done);
    }
}
