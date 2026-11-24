use std::path::Path;

use async_trait::async_trait;
use serde_json::Value;

use crate::bus::EventBus;
use crate::domain::{
    CompletionRequest, CompletionResponse, HarnessContext, PluginManifest, RuntimeStatus,
    SessionId, SessionRecord, SessionSummary, ThemePalette,
};
use crate::error::CoreResult;

/// Abstraction over any LLM inference backend (`OpenAI`, Ollama, `llama.cpp`, `OpenRouter`, etc.).
#[async_trait]
pub trait LlmProvider: Send + Sync {
    /// Stable provider identifier (e.g., `"ollama"`, `"openai"`).
    fn id(&self) -> &str;

    /// Human-readable display name.
    fn display_name(&self) -> &str;

    /// Lists available models (queries live endpoint when applicable, or returns configured defaults).
    ///
    /// # Errors
    /// Returns [`crate::CoreError::Provider`] if model discovery fails fatally.
    async fn list_models(&self) -> CoreResult<Vec<String>>;

    /// Executes a chat completion request.
    ///
    /// # Errors
    /// Returns [`crate::CoreError::Provider`] or [`crate::CoreError::Config`] on failure.
    async fn complete(&self, request: CompletionRequest) -> CoreResult<CompletionResponse>;

    /// Executes a streaming chat completion request, forwarding text chunks via `token_tx`.
    ///
    /// Default implementation delegates to [`LlmProvider::complete`] and emits the full reply once.
    ///
    /// # Errors
    /// Returns [`crate::CoreError::Provider`] or [`crate::CoreError::Config`] on failure.
    async fn complete_stream(
        &self,
        request: CompletionRequest,
        token_tx: tokio::sync::mpsc::UnboundedSender<String>,
    ) -> CoreResult<CompletionResponse> {
        let response = self.complete(request).await?;
        if !response.content.is_empty() {
            let _ = token_tx.send(response.content.clone());
        }
        Ok(response)
    }
}

/// Callable tool exposed to agents, implemented natively or backed by WASM/MCP plugins.
#[async_trait]
pub trait AgentTool: Send + Sync {
    /// Unique tool name.
    fn name(&self) -> &str;

    /// Concise tool description shown to models and users.
    fn description(&self) -> &str;

    /// JSON Schema describing accepted tool input arguments.
    fn parameters_schema(&self) -> Value;

    /// Returns `true` if the tool is read-only and permitted in [`crate::AgentMode::Plan`].
    fn is_read_only(&self) -> bool {
        true
    }

    /// Executes the tool with JSON arguments and returns JSON output.
    ///
    /// # Errors
    /// Returns [`crate::CoreError::Execution`] if execution fails.
    async fn execute(&self, input: Value) -> CoreResult<Value>;
}

/// Local or remote speech-to-text provider (e.g., local `whisper-cli`).
#[async_trait]
pub trait SpeechTranscriber: Send + Sync {
    /// Stable transcriber identifier.
    fn id(&self) -> &str;

    /// Human-readable display name.
    fn display_name(&self) -> &str;

    /// Checks whether the transcriber binary/runtime is currently ready.
    async fn is_available(&self) -> bool;

    /// Transcribes an audio file at `audio_path` into text.
    ///
    /// # Errors
    /// Returns [`crate::CoreError::Execution`] or [`crate::CoreError::Io`] on failure.
    async fn transcribe_file(&self, audio_path: &Path) -> CoreResult<String>;

    /// Actionable install instructions when the transcriber is not installed.
    fn install_instructions(&self) -> &str;
}

/// Lifecycle manager for local LLM engines (`ollama`, `llama.cpp`, etc.).
#[async_trait]
pub trait LocalRuntimeManager: Send + Sync {
    /// Stable runtime identifier.
    fn id(&self) -> &str;

    /// Human-readable display name.
    fn display_name(&self) -> &str;

    /// Queries installation status, running state, and local models.
    ///
    /// # Errors
    /// Returns [`crate::CoreError::Execution`] if inspecting the system fails unexpectedly.
    async fn status(&self) -> CoreResult<RuntimeStatus>;

    /// Installs or bootstraps the runtime using platform package tooling.
    ///
    /// # Errors
    /// Returns [`crate::CoreError::Execution`] if installation fails.
    async fn install_runtime(&self) -> CoreResult<String>;

    /// Downloads/pulls a model artifact by name.
    ///
    /// # Errors
    /// Returns [`crate::CoreError::Execution`] if pulling the model fails.
    async fn pull_model(&self, model: &str) -> CoreResult<String>;
}

/// Discovers rules, skills, and MCP definitions from workspace or global harness directories.
#[async_trait]
pub trait ContextLoader: Send + Sync {
    /// Stable loader identifier.
    fn id(&self) -> &str;

    /// Loads context items rooted at `workspace`.
    ///
    /// # Errors
    /// Returns [`crate::CoreError::Io`] or [`crate::CoreError::Config`] on unrecoverable errors.
    async fn load(&self, workspace: &Path) -> CoreResult<HarnessContext>;
}

/// External messaging gateway (Telegram, Discord, etc.) bridging chat platforms to [`EventBus`].
#[async_trait]
pub trait ChannelGateway: Send + Sync {
    /// Stable gateway identifier (`"telegram"`, `"discord"`).
    fn id(&self) -> &str;

    /// Human-readable display name.
    fn display_name(&self) -> &str;

    /// Returns `true` when required credentials/environment configuration are present.
    fn is_configured(&self) -> bool;

    /// Starts listening for inbound messages and publishes events to `bus`.
    ///
    /// # Errors
    /// Returns [`crate::CoreError::Config`] or [`crate::CoreError::Execution`] on failure.
    async fn start(&self, bus: EventBus) -> CoreResult<()>;

    /// Sends an agent reply back to `destination`.
    ///
    /// # Errors
    /// Returns [`crate::CoreError::Execution`] if delivery fails.
    async fn send_reply(&self, destination: &str, text: &str) -> CoreResult<()>;
}

/// Discovers and instantiates external user plugins (e.g., WASM modules), returning tools & manifests.
#[async_trait]
pub trait PluginHost: Send + Sync {
    /// Stable plugin host identifier.
    fn id(&self) -> &str;

    /// Scans `plugin_dirs` and returns loaded plugin manifests alongside instantiated [`AgentTool`]s.
    ///
    /// # Errors
    /// Returns [`crate::CoreError::Execution`] on host initialization failures.
    async fn load_plugins(
        &self,
        plugin_dirs: &[&Path],
    ) -> CoreResult<(Vec<PluginManifest>, Vec<std::sync::Arc<dyn AgentTool>>)>;
}

/// Swappable TUI color palette adhering to the Open/Closed Principle.
pub trait UiTheme: Send + Sync {
    /// Stable theme identifier.
    fn id(&self) -> &str;

    /// Human-readable theme name.
    fn display_name(&self) -> &str;

    /// Theme color palette.
    fn palette(&self) -> ThemePalette;
}

/// Persistent storage backend for agent conversation sessions (JSON files, `SQLite`, etc.).
#[async_trait]
pub trait SessionStore: Send + Sync {
    /// Stable store identifier.
    fn id(&self) -> &str;

    /// Saves or overwrites a [`SessionRecord`].
    ///
    /// # Errors
    /// Returns [`crate::CoreError::Io`] or [`crate::CoreError::Execution`] if persistence fails.
    async fn save(&self, record: &SessionRecord) -> CoreResult<()>;

    /// Loads a [`SessionRecord`] by [`SessionId`].
    ///
    /// # Errors
    /// Returns [`crate::CoreError::NotFound`] or [`crate::CoreError::Io`] if missing/unreadable.
    async fn load(&self, id: SessionId) -> CoreResult<SessionRecord>;

    /// Lists available persisted sessions.
    ///
    /// # Errors
    /// Returns [`crate::CoreError::Io`] if listing fails.
    async fn list(&self) -> CoreResult<Vec<SessionSummary>>;
}
