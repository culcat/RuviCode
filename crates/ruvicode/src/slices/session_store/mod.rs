use std::path::{Path, PathBuf};

use async_trait::async_trait;
use ruvi_core::{CoreError, CoreResult, SessionId, SessionRecord, SessionStore, SessionSummary};
use tokio::fs;

/// File-backed JSON [`SessionStore`] persisting conversations under `<root>/.ruvi/sessions/`.
pub struct JsonSessionStore {
    sessions_dir: PathBuf,
}

impl JsonSessionStore {
    /// Creates a [`JsonSessionStore`] storing files under `workspace/.ruvi/sessions`.
    #[must_use]
    pub fn for_workspace(workspace: &Path) -> Self {
        Self {
            sessions_dir: workspace.join(".ruvi").join("sessions"),
        }
    }

    fn session_path(&self, id: SessionId) -> PathBuf {
        self.sessions_dir.join(format!("{id}.json"))
    }
}

#[async_trait]
impl SessionStore for JsonSessionStore {
    fn id(&self) -> &'static str {
        "json-session-store"
    }

    async fn save(&self, record: &SessionRecord) -> CoreResult<()> {
        fs::create_dir_all(&self.sessions_dir).await?;
        let data = serde_json::to_vec_pretty(record).map_err(|err| CoreError::Execution {
            component: self.id().to_string(),
            message: format!("failed to serialize session: {err}"),
        })?;
        fs::write(self.session_path(record.id), data).await?;
        Ok(())
    }

    async fn load(&self, id: SessionId) -> CoreResult<SessionRecord> {
        let path = self.session_path(id);
        if !path.is_file() {
            return Err(CoreError::NotFound {
                kind: "session",
                id: id.to_string(),
            });
        }
        let raw = fs::read_to_string(path).await?;
        serde_json::from_str(&raw).map_err(|err| CoreError::Execution {
            component: self.id().to_string(),
            message: format!("invalid session JSON: {err}"),
        })
    }

    async fn list(&self) -> CoreResult<Vec<SessionSummary>> {
        if !self.sessions_dir.is_dir() {
            return Ok(Vec::new());
        }
        let mut entries = fs::read_dir(&self.sessions_dir).await?;
        let mut summaries = Vec::new();
        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            if let Ok(raw) = fs::read_to_string(&path).await
                && let Ok(rec) = serde_json::from_str::<SessionRecord>(&raw)
            {
                summaries.push(SessionSummary {
                    id: rec.id,
                    title: rec.title,
                    provider_id: rec.provider_id,
                    model: rec.model,
                    message_count: rec.messages.len(),
                });
            }
        }
        summaries.sort_by(|a, b| a.title.cmp(&b.title));
        Ok(summaries)
    }
}

#[cfg(test)]
mod tests {
    use ruvi_core::{AgentMode, ChatMessage, MessageRole};

    use super::*;

    #[tokio::test]
    async fn saves_lists_and_loads_session_record() {
        let temp_root =
            std::env::temp_dir().join(format!("ruvi-session-test-{}", SessionId::new()));
        let store = JsonSessionStore::for_workspace(&temp_root);

        let id = SessionId::new();
        let record = SessionRecord {
            id,
            title: "Build MCP client".to_string(),
            provider_id: "ruvi-demo".to_string(),
            model: "ruvi-demo-v1".to_string(),
            mode: AgentMode::Plan,
            messages: vec![ChatMessage::new(MessageRole::User, "hello")],
        };

        store.save(&record).await.expect("save should succeed");
        let list = store.list().await.expect("list should succeed");
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].id, id);

        let loaded = store.load(id).await.expect("load should succeed");
        assert_eq!(loaded, record);

        let _ = std::fs::remove_dir_all(&temp_root);
    }
}
