use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::io::AsyncWriteExt;
use uuid::Uuid;

use crate::{
    browser::provider::ProviderId,
    error::{Result, WtError},
    tools::ToolResult,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EffectStatus {
    Started,
    Completed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EffectRecord {
    pub status: EffectStatus,
    pub tool_name: String,
    pub fingerprint: String,
    pub result: Option<ToolResult>,
    pub updated_at_ms: u128,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionState {
    pub schema_version: u32,
    pub session_id: String,
    pub provider: ProviderId,
    pub project_root: PathBuf,
    pub task: String,
    pub phase: String,
    pub conversation_url: Option<String>,
    pub last_assistant_id: Option<String>,
    pub active_mode: Option<String>,
    pub turn: u64,
    pub last_message: Option<String>,
    pub created_at_ms: u128,
    pub updated_at_ms: u128,
    #[serde(default)]
    pub effects: HashMap<String, EffectRecord>,
}

pub struct SessionStore {
    directory: PathBuf,
    state_path: PathBuf,
    events_path: PathBuf,
    pub state: SessionState,
}

impl SessionStore {
    pub async fn create(
        app_data_dir: &Path,
        provider: ProviderId,
        project_root: &Path,
        task: String,
    ) -> Result<Self> {
        let session_id = Uuid::new_v4().simple().to_string();
        let directory = app_data_dir.join("sessions").join(&session_id);
        tokio::fs::create_dir_all(&directory).await?;
        let now = timestamp_ms();
        let state = SessionState {
            schema_version: 1,
            session_id,
            provider,
            project_root: project_root.to_path_buf(),
            task,
            phase: "created".into(),
            conversation_url: None,
            last_assistant_id: None,
            active_mode: None,
            turn: 0,
            last_message: None,
            created_at_ms: now,
            updated_at_ms: now,
            effects: HashMap::new(),
        };
        let state_path = directory.join("state.json");
        let events_path = directory.join("events.jsonl");
        let store = Self {
            directory,
            state_path,
            events_path,
            state,
        };
        store.save().await?;
        store.append_event("session.created", json!({})).await?;
        Ok(store)
    }

    pub async fn load(app_data_dir: &Path, session_id: &str) -> Result<Self> {
        validate_session_id(session_id)?;
        let directory = app_data_dir.join("sessions").join(session_id);
        let state_path = directory.join("state.json");
        let events_path = directory.join("events.jsonl");
        let content = tokio::fs::read_to_string(&state_path)
            .await
            .map_err(|e| WtError::Session(format!("cannot load session {session_id}: {e}")))?;
        let state: SessionState = serde_json::from_str(&content)?;
        Ok(Self {
            directory,
            state_path,
            events_path,
            state,
        })
    }

    pub fn directory(&self) -> &Path {
        &self.directory
    }

    pub async fn save(&self) -> Result<()> {
        let bytes = serde_json::to_vec_pretty(&self.state)?;
        atomic_write(&self.state_path, &bytes).await
    }

    pub async fn update_phase(&mut self, phase: &str) -> Result<()> {
        self.state.phase = phase.to_string();
        self.touch_and_save().await
    }

    pub async fn set_conversation(
        &mut self,
        url: Option<String>,
        assistant_id: Option<String>,
    ) -> Result<()> {
        if let Some(url) = url {
            self.state.conversation_url = Some(url);
        }
        self.state.last_assistant_id = assistant_id;
        self.touch_and_save().await
    }

    pub async fn set_active_mode(&mut self, mode: Option<String>) -> Result<()> {
        self.state.active_mode = mode;
        self.touch_and_save().await
    }

    pub async fn next_turn(&mut self) -> Result<u64> {
        self.state.turn = self.state.turn.saturating_add(1);
        self.touch_and_save().await?;
        Ok(self.state.turn)
    }

    pub async fn complete(&mut self, message: String) -> Result<()> {
        self.state.phase = "idle".into();
        self.state.last_message = Some(message.clone());
        self.touch_and_save().await?;
        self.append_event("run.completed", json!({"message": message}))
            .await
    }

    pub fn effect(&self, key: &str) -> Option<&EffectRecord> {
        self.state.effects.get(key)
    }

    pub async fn mark_effect_started(
        &mut self,
        key: String,
        tool_name: String,
        fingerprint: String,
    ) -> Result<()> {
        self.state.effects.insert(
            key,
            EffectRecord {
                status: EffectStatus::Started,
                tool_name,
                fingerprint,
                result: None,
                updated_at_ms: timestamp_ms(),
            },
        );
        self.touch_and_save().await
    }

    pub async fn mark_effect_completed(&mut self, key: &str, result: ToolResult) -> Result<()> {
        let record = self
            .state
            .effects
            .get_mut(key)
            .ok_or_else(|| WtError::Session(format!("effect {key} was not started")))?;
        record.status = EffectStatus::Completed;
        record.result = Some(result);
        record.updated_at_ms = timestamp_ms();
        self.touch_and_save().await
    }

    pub async fn append_event(&self, event_type: &str, payload: Value) -> Result<()> {
        if let Some(parent) = self.events_path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        let event = json!({
            "timestamp_ms": timestamp_ms(),
            "session_id": self.state.session_id,
            "type": event_type,
            "payload": payload
        });
        let mut file = tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.events_path)
            .await?;
        file.write_all(serde_json::to_string(&event)?.as_bytes())
            .await?;
        file.write_all(b"\n").await?;
        file.flush().await?;
        Ok(())
    }

    async fn touch_and_save(&mut self) -> Result<()> {
        self.state.updated_at_ms = timestamp_ms();
        self.save().await
    }
}

pub async fn list_sessions(app_data_dir: &Path, limit: usize) -> Result<Vec<SessionState>> {
    let root = app_data_dir.join("sessions");
    let mut output = Vec::new();
    let mut entries = match tokio::fs::read_dir(&root).await {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(output),
        Err(error) => return Err(error.into()),
    };
    while let Some(entry) = entries.next_entry().await? {
        let path = entry.path().join("state.json");
        let Ok(content) = tokio::fs::read_to_string(path).await else {
            continue;
        };
        if let Ok(state) = serde_json::from_str::<SessionState>(&content) {
            output.push(state);
        }
    }
    output.sort_by_key(|state| std::cmp::Reverse(state.updated_at_ms));
    output.truncate(limit);
    Ok(output)
}

pub async fn latest_session_for_project(
    app_data_dir: &Path,
    project_root: &Path,
) -> Result<Option<SessionState>> {
    let sessions = list_sessions(app_data_dir, usize::MAX).await?;
    Ok(sessions
        .into_iter()
        .find(|state| state.project_root == project_root))
}

pub async fn delete_session(app_data_dir: &Path, session_id: &str) -> Result<()> {
    validate_session_id(session_id)?;
    let directory = app_data_dir.join("sessions").join(session_id);
    match tokio::fs::remove_dir_all(&directory).await {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Err(WtError::Session(format!(
            "cannot delete session {session_id}: session does not exist"
        ))),
        Err(error) => Err(error.into()),
    }
}

fn validate_session_id(session_id: &str) -> Result<()> {
    if session_id.is_empty()
        || session_id.contains('/')
        || session_id.contains('\\')
        || session_id.contains("..")
    {
        return Err(WtError::Session("invalid session id".into()));
    }
    Ok(())
}

fn timestamp_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

async fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| WtError::Session("state path has no parent".into()))?;
    tokio::fs::create_dir_all(parent).await?;
    let temp = parent.join(format!(".state-{}.tmp", Uuid::new_v4()));
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

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn persists_session_state() {
        let temp = tempfile::tempdir().unwrap();
        let project = temp.path().join("project");
        tokio::fs::create_dir_all(&project).await.unwrap();
        let mut store =
            SessionStore::create(temp.path(), ProviderId::Chatgpt, &project, "test".into())
                .await
                .unwrap();
        store.update_phase("running").await.unwrap();
        let id = store.state.session_id.clone();
        let loaded = SessionStore::load(temp.path(), &id).await.unwrap();
        assert_eq!(loaded.state.phase, "running");
    }

    #[tokio::test]
    async fn finds_latest_session_for_project() {
        let temp = tempfile::tempdir().unwrap();
        let project = temp.path().join("project");
        let other = temp.path().join("other");
        tokio::fs::create_dir_all(&project).await.unwrap();
        tokio::fs::create_dir_all(&other).await.unwrap();

        SessionStore::create(temp.path(), ProviderId::Chatgpt, &other, "other".into())
            .await
            .unwrap();
        let mut expected =
            SessionStore::create(temp.path(), ProviderId::Chatgpt, &project, "current".into())
                .await
                .unwrap();
        expected.update_phase("idle").await.unwrap();

        let latest = latest_session_for_project(temp.path(), &project)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(latest.session_id, expected.state.session_id);
    }

    #[tokio::test]
    async fn deletes_session_directory() {
        let temp = tempfile::tempdir().unwrap();
        let project = temp.path().join("project");
        tokio::fs::create_dir_all(&project).await.unwrap();
        let store =
            SessionStore::create(temp.path(), ProviderId::Chatgpt, &project, "delete".into())
                .await
                .unwrap();
        let id = store.state.session_id.clone();
        assert!(store.directory().exists());

        delete_session(temp.path(), &id).await.unwrap();
        assert!(!store.directory().exists());
    }
}
