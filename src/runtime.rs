use std::{
    io::{self, Write},
    path::PathBuf,
    sync::Arc,
};

use async_trait::async_trait;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::{
    browser::{
        adapter::{AuthState, WebAdapter},
        throttle::{ProviderOutcome, RateController},
    },
    config::Limits,
    error::{Result, WtError},
    protocol::{
        build_bootstrap_prompt, build_follow_up, build_protocol_correction,
        parse_agent_response, serialize_tool_results, ToolCall,
    },
    session::{EffectStatus, SessionStore},
    tools::{ToolExecutor, ToolResult, ToolRisk},
};

#[async_trait]
pub trait ApprovalHandler: Send + Sync {
    async fn approve(&self, call: &ToolCall, risk: ToolRisk) -> Result<bool>;
}

pub struct TerminalApproval;

#[async_trait]
impl ApprovalHandler for TerminalApproval {
    async fn approve(&self, call: &ToolCall, risk: ToolRisk) -> Result<bool> {
        let call = call.clone();
        tokio::task::spawn_blocking(move || {
            eprintln!("\nWTAgent-RS requests a {risk:?} tool:");
            eprintln!("  {} {}", call.name, call.args);
            eprint!("Approve? [y/N] ");
            io::stderr().flush().map_err(WtError::Io)?;
            let mut input = String::new();
            io::stdin().read_line(&mut input).map_err(WtError::Io)?;
            Ok(matches!(input.trim().to_ascii_lowercase().as_str(), "y" | "yes"))
        })
        .await
        .map_err(|e| WtError::Policy(format!("approval prompt failed: {e}")))?
    }
}

pub struct AgentRuntime {
    adapter: Box<dyn WebAdapter>,
    tools: ToolExecutor,
    session: SessionStore,
    rate: RateController,
    approval: Arc<dyn ApprovalHandler>,
    limits: Limits,
}

impl AgentRuntime {
    pub fn new(
        adapter: Box<dyn WebAdapter>,
        tools: ToolExecutor,
        session: SessionStore,
        rate: RateController,
        approval: Arc<dyn ApprovalHandler>,
        limits: Limits,
    ) -> Self {
        Self {
            adapter,
            tools,
            session,
            rate,
            approval,
            limits,
        }
    }

    pub fn session_id(&self) -> &str {
        &self.session.state.session_id
    }

    pub async fn run(
        mut self,
        resume: bool,
        instruction: Option<String>,
        files: Vec<PathBuf>,
        requested_mode: Option<String>,
    ) -> Result<String> {
        self.session.update_phase("initializing").await?;
        self.session
            .append_event("runtime.initializing", json!({"resume": resume}))
            .await?;

        let preferred = if resume {
            self.session.state.conversation_url.as_deref()
        } else {
            None
        };
        self.adapter.launch(preferred).await?;

        if self.adapter.auth_state().await? != AuthState::Authenticated {
            self.session.update_phase("auth_required").await?;
            eprintln!(
                "Open the dedicated {} Chrome window and sign in. Security challenges must be completed manually.",
                self.adapter.provider_label()
            );
            self.adapter
                .wait_for_manual_login(std::time::Duration::from_secs(10 * 60))
                .await?;
        }

        self.adapter
            .start_conversation(if resume {
                self.session.state.conversation_url.as_deref()
            } else {
                None
            })
            .await?;
        let active_mode = self.adapter.select_mode(requested_mode.as_deref()).await?;
        self.session.set_active_mode(active_mode.clone()).await?;
        self.session
            .append_event(
                "conversation.started",
                json!({
                    "url": self.adapter.conversation_url().await?,
                    "mode": active_mode,
                    "provider": self.adapter.provider_label()
                }),
            )
            .await?;

        let initial = if resume {
            match instruction.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
                Some(text) => build_follow_up(text),
                None => build_follow_up(
                    "Continue the saved task from the existing conversation. Do not replay completed local side effects.",
                ),
            }
        } else {
            build_bootstrap_prompt(
                &self.session.state.task,
                &self.tools.project_root().display().to_string(),
            )
        };
        self.send_web(&initial, &files).await?;

        let mut protocol_errors = 0usize;
        for _ in 0..self.limits.max_steps {
            let turn_number = self.session.next_turn().await?;
            self.session.update_phase("waiting_model").await?;
            let turn = match self
                .adapter
                .wait_for_turn(self.limits.model_turn_timeout, self.limits.stable_window)
                .await
            {
                Ok(turn) => {
                    self.rate.record_outcome(ProviderOutcome::Success).await;
                    turn
                }
                Err(error @ WtError::UsageLimit(_)) => {
                    self.rate.record_outcome(ProviderOutcome::UsageLimit).await;
                    self.session
                        .append_event("model.usage_limit", json!({"error": error.to_string()}))
                        .await?;
                    return Err(error);
                }
                Err(error @ WtError::RateLimit(_)) => {
                    self.rate.record_outcome(ProviderOutcome::RateLimited).await;
                    self.session
                        .append_event("model.rate_limit", json!({"error": error.to_string()}))
                        .await?;
                    return Err(error);
                }
                Err(error @ WtError::Challenge(_)) => {
                    self.rate.record_outcome(ProviderOutcome::Challenge).await;
                    return Err(error);
                }
                Err(error) => {
                    self.rate
                        .record_outcome(ProviderOutcome::GenerationFailure)
                        .await;
                    return Err(error);
                }
            };

            self.session
                .set_conversation(
                    Some(self.adapter.conversation_url().await?),
                    turn.assistant_id.clone(),
                )
                .await?;
            self.session
                .append_event(
                    "model.message_complete",
                    json!({
                        "turn": turn_number,
                        "assistant_id": turn.assistant_id,
                        "bytes": turn.text.len()
                    }),
                )
                .await?;

            if !turn.text.contains("<agent_response") {
                let answer = clean_plain_answer(&turn.text);
                self.session.complete(answer.clone()).await?;
                return Ok(answer);
            }

            let parsed = match parse_agent_response(&turn.text) {
                Ok(parsed) => {
                    protocol_errors = 0;
                    parsed
                }
                Err(error) => {
                    protocol_errors += 1;
                    self.session
                        .append_event(
                            "protocol.invalid",
                            json!({"count": protocol_errors, "error": error.to_string()}),
                        )
                        .await?;
                    if protocol_errors >= 2 {
                        return Err(error);
                    }
                    self.send_web(&build_protocol_correction(&error.to_string()), &[])
                        .await?;
                    continue;
                }
            };

            if parsed.done {
                let answer = parsed.message.trim().to_string();
                self.session.complete(answer.clone()).await?;
                return Ok(answer);
            }

            if parsed.tool_calls.is_empty() {
                // Do not create a retry storm just because the model forgot done=true.
                // Returning control to the user is safer and uses fewer provider turns.
                let answer = if parsed.message.trim().is_empty() {
                    "The model returned no actionable tool call. Resume with a more specific instruction if needed."
                        .to_string()
                } else {
                    parsed.message.trim().to_string()
                };
                self.session.complete(answer.clone()).await?;
                return Ok(answer);
            }

            if let Err(message) = self.validate_batch(&parsed.tool_calls) {
                protocol_errors += 1;
                if protocol_errors >= 2 {
                    return Err(WtError::Protocol(message));
                }
                self.send_web(&build_protocol_correction(&message), &[]).await?;
                continue;
            }

            let mut results = Vec::with_capacity(parsed.tool_calls.len());
            for (index, call) in parsed.tool_calls.iter().enumerate() {
                let result = self
                    .execute_tool(call, index, turn_number, &turn.text, turn.assistant_id.as_deref())
                    .await?;
                results.push(result.to_value());
            }

            let compacted = compact_results(results, self.limits.max_tool_result_bytes);
            let message = serialize_tool_results(&compacted);
            self.send_web(&message, &[]).await?;
            self.session
                .append_event(
                    "tool.results_sent",
                    json!({"count": compacted.len(), "turn": turn_number}),
                )
                .await?;
        }

        Err(WtError::Protocol(format!(
            "agent exceeded the maximum of {} model turns",
            self.limits.max_steps
        )))
    }

    fn validate_batch(&self, calls: &[ToolCall]) -> std::result::Result<(), String> {
        if calls.len() > self.limits.max_batch_read_calls {
            return Err(format!(
                "at most {} read-only tools may be batched in one reply",
                self.limits.max_batch_read_calls
            ));
        }
        if calls.len() > 1 {
            for call in calls {
                if ToolExecutor::risk(&call.name) != Some(ToolRisk::Read) {
                    return Err(
                        "only read-only tools may be batched; request write/execute tools one at a time"
                            .into(),
                    );
                }
            }
        }
        Ok(())
    }

    async fn execute_tool(
        &mut self,
        call: &ToolCall,
        index: usize,
        turn_number: u64,
        raw_turn: &str,
        assistant_id: Option<&str>,
    ) -> Result<ToolResult> {
        let Some(risk) = ToolExecutor::risk(&call.name) else {
            return Ok(ToolResult::denied(
                &call.name,
                format!("unknown local tool: {}", call.name),
            ));
        };

        self.session
            .append_event(
                "tool.proposed",
                json!({"name": call.name, "args": call.args, "risk": format!("{risk:?}")}),
            )
            .await?;

        if let Err(error) = self.tools.policy().requires_confirmation(risk) {
            return Ok(ToolResult::denied(&call.name, error.to_string()));
        }
        if self.tools.policy().requires_confirmation(risk)?
            && !self.approval.approve(call, risk).await?
        {
            return Ok(ToolResult::denied(&call.name, "user denied this tool call"));
        }

        if risk == ToolRisk::Read {
            let result = self.tools.execute(call).await;
            self.session
                .append_event("tool.completed", json!({"name": call.name, "ok": result.ok}))
                .await?;
            return Ok(result);
        }

        let (effect_key, fingerprint) = effect_identity(
            call,
            index,
            turn_number,
            raw_turn,
            assistant_id,
        );
        if let Some(existing) = self.session.effect(&effect_key) {
            return match (&existing.status, &existing.result) {
                (EffectStatus::Completed, Some(result)) => Ok(result.clone()),
                _ => Ok(ToolResult::denied(
                    &call.name,
                    "This side effect may already have started before an interruption. It will not be replayed automatically; inspect local state first.",
                )),
            };
        }

        self.session
            .mark_effect_started(effect_key.clone(), call.name.clone(), fingerprint)
            .await?;
        let result = self.tools.execute(call).await;
        self.session
            .mark_effect_completed(&effect_key, result.clone())
            .await?;
        self.session
            .append_event("tool.completed", json!({"name": call.name, "ok": result.ok}))
            .await?;
        Ok(result)
    }

    async fn send_web(&mut self, message: &str, files: &[PathBuf]) -> Result<()> {
        let bytes = message.len();
        if bytes > self.limits.max_browser_message_bytes {
            return Err(WtError::Protocol(format!(
                "outbound browser message is {bytes} bytes; limit is {}",
                self.limits.max_browser_message_bytes
            )));
        }
        self.rate.before_send().await;
        match self.adapter.send_message(message, files).await {
            Ok(()) => {
                let url = self.adapter.conversation_url().await.ok();
                self.session.set_conversation(url, None).await?;
                self.session
                    .append_event("model.message_sent", json!({"bytes": bytes}))
                    .await?;
                Ok(())
            }
            Err(error @ WtError::UsageLimit(_)) => {
                self.rate.record_outcome(ProviderOutcome::UsageLimit).await;
                Err(error)
            }
            Err(error @ WtError::RateLimit(_)) => {
                self.rate.record_outcome(ProviderOutcome::RateLimited).await;
                Err(error)
            }
            Err(error @ WtError::Challenge(_)) => {
                self.rate.record_outcome(ProviderOutcome::Challenge).await;
                Err(error)
            }
            Err(error) => Err(error),
        }
    }
}

fn effect_identity(
    call: &ToolCall,
    index: usize,
    turn_number: u64,
    raw_turn: &str,
    assistant_id: Option<&str>,
) -> (String, String) {
    let canonical = canonical_json(&call.args);
    let fingerprint = hex_hash(&format!("{}\0{canonical}", call.name));
    let message_key = assistant_id
        .map(|id| format!("message:{id}"))
        .unwrap_or_else(|| format!("turn:{turn_number}:{}", hex_hash(raw_turn)));
    let key = hex_hash(&format!("{message_key}\0{index}\0{fingerprint}"));
    (key, fingerprint)
}

fn canonical_json(value: &Value) -> String {
    match value {
        Value::Object(map) => {
            let mut keys: Vec<_> = map.keys().collect();
            keys.sort();
            let body = keys
                .into_iter()
                .map(|key| format!("{}:{}", serde_json::to_string(key).unwrap(), canonical_json(&map[key])))
                .collect::<Vec<_>>()
                .join(",");
            format!("{{{body}}}")
        }
        Value::Array(items) => format!(
            "[{}]",
            items.iter().map(canonical_json).collect::<Vec<_>>().join(",")
        ),
        _ => serde_json::to_string(value).unwrap_or_else(|_| "null".into()),
    }
}

fn hex_hash(input: &str) -> String {
    let digest = Sha256::digest(input.as_bytes());
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn compact_results(results: Vec<Value>, budget: usize) -> Vec<Value> {
    let serialized = serde_json::to_vec(&results).unwrap_or_default();
    if serialized.len() <= budget {
        return results;
    }

    let share = (budget / results.len().max(1)).saturating_sub(256).max(256);
    results
        .into_iter()
        .map(|value| {
            let name = value.get("name").cloned().unwrap_or(Value::Null);
            let ok = value.get("ok").cloned().unwrap_or(Value::Bool(false));
            let message = value.get("message").and_then(Value::as_str).unwrap_or_default();
            let preview = value
                .get("data")
                .map(|data| serde_json::to_string(data).unwrap_or_default())
                .unwrap_or_default();
            json!({
                "name": name,
                "ok": ok,
                "message": truncate_string(message, 512),
                "data_preview": truncate_string(&preview, share),
                "compacted": true
            })
        })
        .collect()
}

fn truncate_string(value: &str, max_bytes: usize) -> String {
    if value.len() <= max_bytes {
        return value.to_string();
    }
    let mut end = max_bytes.min(value.len());
    while !value.is_char_boundary(end) {
        end = end.saturating_sub(1);
    }
    format!("{}…[truncated]", &value[..end])
}

fn clean_plain_answer(raw: &str) -> String {
    raw.lines()
        .filter(|line| {
            let trimmed = line.trim();
            !matches!(trimmed, "Copy" | "Retry" | "Regenerate" | "复制" | "重试")
        })
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compacts_large_results() {
        let results = vec![json!({"name":"fs.read","ok":true,"message":"ok","data":{"content":"x".repeat(20_000)}})];
        let compacted = compact_results(results, 2_000);
        assert_eq!(compacted[0]["compacted"], true);
    }

    #[test]
    fn effect_identity_is_stable_for_object_key_order() {
        let a = ToolCall { name: "fs.write".into(), args: json!({"b":2,"a":1}) };
        let b = ToolCall { name: "fs.write".into(), args: json!({"a":1,"b":2}) };
        assert_eq!(
            effect_identity(&a, 0, 1, "raw", Some("m1")),
            effect_identity(&b, 0, 1, "raw", Some("m1"))
        );
    }
}
