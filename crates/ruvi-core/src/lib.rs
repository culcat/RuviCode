//! Core domain contracts, pub/sub events, and Open/Closed extension registry for `RuviCode`.

pub mod bus;
pub mod domain;
pub mod error;
pub mod registry;
pub mod traits;

pub use bus::{AppEvent, EventBus};
pub use domain::{
    AgentMode, ChatMessage, CompletionRequest, CompletionResponse, HarnessContext, McpServerConfig,
    MessageRole, PluginManifest, RuleDocument, RuntimeKind, RuntimeStatus, SessionId,
    SessionRecord, SessionSummary, SkillDescriptor, ThemePalette, ToolCall, ToolDefinition,
};
pub use error::{CoreError, CoreResult};
pub use registry::ExtensionRegistry;
pub use traits::{
    AgentTool, ChannelGateway, ContextLoader, LlmProvider, LocalRuntimeManager, PluginHost,
    SessionStore, SpeechTranscriber, UiTheme,
};
