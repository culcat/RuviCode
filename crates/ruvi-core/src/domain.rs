use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Strongly typed identifier for an agent conversation session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SessionId(Uuid);

impl SessionId {
    /// Generates a new random [`SessionId`].
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for SessionId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for SessionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Execution mode matching `OpenCode`'s Build/Plan dichotomy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum AgentMode {
    /// Full coding and tool-execution mode.
    #[default]
    Build,
    /// Read-only architecture and step-by-step planning mode.
    Plan,
}

impl AgentMode {
    /// Flips between [`AgentMode::Build`] and [`AgentMode::Plan`].
    #[must_use]
    pub const fn toggle(self) -> Self {
        match self {
            Self::Build => Self::Plan,
            Self::Plan => Self::Build,
        }
    }

    /// Short uppercase badge label used in UI headers and status lines.
    #[must_use]
    pub const fn badge(self) -> &'static str {
        match self {
            Self::Build => "BUILD",
            Self::Plan => "PLAN",
        }
    }
}

/// Participant role in an agent conversation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MessageRole {
    /// System instructions injected by the harness.
    System,
    /// Human or bot-channel user prompt.
    User,
    /// Model response.
    Assistant,
    /// Tool output message.
    Tool,
}

impl MessageRole {
    /// Human-readable uppercase label.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::System => "SYSTEM",
            Self::User => "USER",
            Self::Assistant => "ASSISTANT",
            Self::Tool => "TOOL",
        }
    }
}

/// Tool specification sent to an LLM provider that supports function calling.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolDefinition {
    /// Unique tool name.
    pub name: String,
    /// Concise tool description.
    pub description: String,
    /// JSON Schema for tool arguments.
    pub parameters: serde_json::Value,
}

/// Tool invocation requested by an LLM response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolCall {
    /// Provider-assigned call identifier.
    pub id: String,
    /// Name of the registered [`crate::AgentTool`] to invoke.
    pub name: String,
    /// JSON arguments object.
    pub arguments: serde_json::Value,
}

/// Single message inside a session transcript.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatMessage {
    /// Author role.
    pub role: MessageRole,
    /// Plain-text or markdown message content.
    pub content: String,
    /// Optional tool calls requested by an assistant message.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ToolCall>,
    /// Identifier of the originating [`ToolCall`] when `role` is [`MessageRole::Tool`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
}

impl ChatMessage {
    /// Creates a new [`ChatMessage`].
    #[must_use]
    pub fn new(role: MessageRole, content: impl Into<String>) -> Self {
        Self {
            role,
            content: content.into(),
            tool_calls: Vec::new(),
            tool_call_id: None,
        }
    }

    /// Creates an assistant message carrying requested [`ToolCall`] entries.
    #[must_use]
    pub fn assistant_tool_calls(content: impl Into<String>, tool_calls: Vec<ToolCall>) -> Self {
        Self {
            role: MessageRole::Assistant,
            content: content.into(),
            tool_calls,
            tool_call_id: None,
        }
    }

    /// Creates a [`MessageRole::Tool`] response message linked to `tool_call_id`.
    #[must_use]
    pub fn tool_result(tool_call_id: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            role: MessageRole::Tool,
            content: content.into(),
            tool_calls: Vec::new(),
            tool_call_id: Some(tool_call_id.into()),
        }
    }
}

/// Provider-agnostic completion request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct CompletionRequest {
    /// Target model identifier understood by the selected provider.
    pub model: String,
    /// Ordered conversation history including system instructions.
    pub messages: Vec<ChatMessage>,
    /// Current agent mode (`Build` or `Plan`).
    pub mode: AgentMode,
    /// Callable tool definitions exposed for this request.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<ToolDefinition>,
}

impl CompletionRequest {
    /// Creates a [`CompletionRequest`] with an empty tool list.
    #[must_use]
    pub fn new(model: impl Into<String>, messages: Vec<ChatMessage>, mode: AgentMode) -> Self {
        Self {
            model: model.into(),
            messages,
            mode,
            tools: Vec::new(),
        }
    }
}

/// Provider-agnostic completion response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompletionResponse {
    /// Model name actually used by the provider.
    pub model: String,
    /// Assistant reply content.
    pub content: String,
    /// Optional tool calls requested by the model.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ToolCall>,
}

/// Descriptor for a discovered `SKILL.md` package from any compatible harness.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillDescriptor {
    /// Skill name (frontmatter `name` or folder name).
    pub name: String,
    /// Concise summary from YAML frontmatter.
    pub description: String,
    /// Filesystem path to the skill directory or `SKILL.md`.
    pub path: PathBuf,
    /// Harness convention origin (e.g., `agents`, `claude`, `opencode`).
    pub source_harness: String,
}

/// Discovered project rule or instruction file (`agent.md`, `AGENTS.md`, `CLAUDE.md`, etc.).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuleDocument {
    /// Display name or relative file path.
    pub name: String,
    /// Raw markdown content.
    pub content: String,
    /// Absolute or workspace-relative file path.
    pub path: PathBuf,
}

/// Discovered Model Context Protocol server configuration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpServerConfig {
    /// Logical server name.
    pub name: String,
    /// Executable command or remote URL.
    pub target: String,
    /// Optional command arguments.
    pub args: Vec<String>,
    /// Source file that declared this server (`opencode.json`, `.mcp.json`, etc.).
    pub source_file: String,
}

/// Aggregated cross-harness context discovered from the workspace and user config.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HarnessContext {
    /// Discovered rule files.
    pub rules: Vec<RuleDocument>,
    /// Discovered reusable skills.
    pub skills: Vec<SkillDescriptor>,
    /// Discovered MCP server entries.
    pub mcp_servers: Vec<McpServerConfig>,
}

impl HarnessContext {
    /// Merges entries from another [`HarnessContext`] into `self`.
    pub fn merge(&mut self, other: Self) {
        self.rules.extend(other.rules);
        self.skills.extend(other.skills);
        self.mcp_servers.extend(other.mcp_servers);
    }
}

/// Category of local AI inference runtime.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum RuntimeKind {
    /// Ollama local server & CLI.
    Ollama,
    /// `llama.cpp` server & CLI.
    LlamaCpp,
    /// Custom user-registered runtime.
    Custom(String),
}

/// Live availability and model status of a local runtime.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeStatus {
    /// Manager ID.
    pub id: String,
    /// Human-friendly runtime name.
    pub display_name: String,
    /// Runtime kind.
    pub kind: RuntimeKind,
    /// Whether the CLI/server binary was found on `PATH`.
    pub installed: bool,
    /// Whether the background daemon/endpoint responded.
    pub running: bool,
    /// Reported version string if installed.
    pub version: Option<String>,
    /// Locally pulled or available models.
    pub models: Vec<String>,
    /// Actionable install command shown in UI/doctor.
    pub install_hint: String,
}

/// Metadata describing a loaded plugin (native or WASM).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginManifest {
    /// Unique plugin identifier.
    pub id: String,
    /// Display name.
    pub name: String,
    /// Semantic version string.
    pub version: String,
    /// Capability tags (e.g., `tool`, `wasm`).
    pub capabilities: Vec<String>,
    /// Source path or origin.
    pub source: String,
}

/// Color palette consumed by the TUI slice.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ThemePalette {
    /// Primary canvas background RGB.
    pub background: (u8, u8, u8),
    /// Elevated panel/modal background RGB.
    pub surface: (u8, u8, u8),
    /// Box border RGB.
    pub border: (u8, u8, u8),
    /// Primary text RGB.
    pub text: (u8, u8, u8),
    /// Secondary/muted text RGB.
    pub muted: (u8, u8, u8),
    /// Primary brand accent RGB.
    pub accent: (u8, u8, u8),
    /// Success / `[BUILD]` badge RGB.
    pub success: (u8, u8, u8),
    /// Warning / `[PLAN]` badge RGB.
    pub warning: (u8, u8, u8),
}
