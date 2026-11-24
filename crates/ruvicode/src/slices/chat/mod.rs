use std::fmt::Write as _;
use std::sync::Arc;

use ruvi_core::{
    AgentMode, AppEvent, ChatMessage, CompletionRequest, CompletionResponse, CoreResult, EventBus,
    ExtensionRegistry, HarnessContext, MessageRole, SessionId, SessionRecord, ToolDefinition,
};
use serde_json::json;
use tokio::sync::mpsc;

const MAX_TOOL_STEPS: usize = 8;

/// Live events emitted during a streaming multi-step agent turn.
#[derive(Debug, Clone)]
pub enum ChatLoopEvent {
    /// Incremental assistant text chunk.
    Token(String),
    /// An [`ruvi_core::AgentTool`] invocation started.
    ToolStarted {
        /// Tool name.
        name: String,
        /// JSON arguments string.
        arguments: String,
    },
    /// An [`ruvi_core::AgentTool`] invocation finished.
    ToolFinished {
        /// Tool name.
        name: String,
        /// JSON result or error string.
        output: String,
    },
}

/// Configuration for running an isolated multi-step agent turn.
pub struct AgentLoopConfig {
    /// Target provider ID.
    pub provider_id: String,
    /// Target model ID.
    pub model: String,
    /// Active [`AgentMode`].
    pub mode: AgentMode,
    /// Initial messages including system prompt and user prompt.
    pub initial_messages: Vec<ChatMessage>,
}

/// Result of a completed multi-step agent turn.
pub struct AgentLoopOutcome {
    /// Final completion response returned by the provider.
    pub response: CompletionResponse,
    /// New messages generated during the turn (assistant tool calls, tool outputs, final reply).
    pub generated_messages: Vec<ChatMessage>,
}

/// Orchestrates multi-turn agent conversations via [`ExtensionRegistry`] and [`EventBus`].
pub struct ChatEngine {
    session_id: SessionId,
    mode: AgentMode,
    provider_id: String,
    model: String,
    messages: Vec<ChatMessage>,
    context: HarnessContext,
    registry: Arc<ExtensionRegistry>,
    bus: EventBus,
}

impl ChatEngine {
    /// Creates a new [`ChatEngine`] bound to an active provider, model, and workspace context.
    #[must_use]
    pub fn new(
        registry: Arc<ExtensionRegistry>,
        bus: EventBus,
        context: HarnessContext,
        provider_id: impl Into<String>,
        model: impl Into<String>,
    ) -> Self {
        Self {
            session_id: SessionId::new(),
            mode: AgentMode::Build,
            provider_id: provider_id.into(),
            model: model.into(),
            messages: Vec::new(),
            context,
            registry,
            bus,
        }
    }

    /// Returns the active [`SessionId`].
    #[must_use]
    pub const fn session_id(&self) -> SessionId {
        self.session_id
    }

    /// Returns the current [`AgentMode`].
    #[must_use]
    pub const fn mode(&self) -> AgentMode {
        self.mode
    }

    /// Returns the active provider identifier.
    #[must_use]
    pub fn provider_id(&self) -> &str {
        &self.provider_id
    }

    /// Returns the active model identifier.
    #[must_use]
    pub fn model(&self) -> &str {
        &self.model
    }

    /// Returns the conversation history (excluding the synthetic system prompt).
    #[must_use]
    pub fn messages(&self) -> &[ChatMessage] {
        &self.messages
    }

    /// Returns the discovered cross-harness context.
    #[must_use]
    pub const fn context(&self) -> &HarnessContext {
        &self.context
    }

    /// Toggles between [`AgentMode::Build`] and [`AgentMode::Plan`] and publishes an event.
    pub fn toggle_mode(&mut self) -> AgentMode {
        self.mode = self.mode.toggle();
        let _ = self.bus.publish(AppEvent::ModeChanged {
            session_id: self.session_id,
            mode: self.mode,
        });
        self.mode
    }

    /// Explicitly sets the active [`AgentMode`].
    pub fn set_mode(&mut self, mode: AgentMode) {
        self.mode = mode;
        let _ = self.bus.publish(AppEvent::ModeChanged {
            session_id: self.session_id,
            mode: self.mode,
        });
    }

    /// Switches the active provider and model and publishes an event.
    pub fn select_model(&mut self, provider_id: impl Into<String>, model: impl Into<String>) {
        self.provider_id = provider_id.into();
        self.model = model.into();
        let _ = self.bus.publish(AppEvent::ModelSelected {
            provider_id: self.provider_id.clone(),
            model: self.model.clone(),
        });
    }

    /// Clears the current conversation transcript.
    pub fn clear_messages(&mut self) {
        self.messages.clear();
    }

    /// Appends a user message to the timeline.
    pub fn push_user_message(&mut self, text: impl Into<String>) {
        self.messages
            .push(ChatMessage::new(MessageRole::User, text));
    }

    /// Appends an assistant reply to the timeline.
    pub fn push_assistant_message(&mut self, text: impl Into<String>) {
        self.messages
            .push(ChatMessage::new(MessageRole::Assistant, text));
    }

    /// Appends a synthetic informational message to the timeline.
    pub fn push_system_note(&mut self, note: impl Into<String>) {
        self.messages
            .push(ChatMessage::new(MessageRole::System, note));
    }

    /// Appends a batch of generated messages (e.g., from an async agent loop).
    pub fn extend_messages(&mut self, msgs: Vec<ChatMessage>) {
        self.messages.extend(msgs);
    }

    /// Exports the current session state as a persistable [`SessionRecord`].
    #[must_use]
    pub fn export_record(&self) -> SessionRecord {
        let title = self
            .messages
            .iter()
            .find(|m| m.role == MessageRole::User)
            .map_or_else(
                || "Untitled session".to_string(),
                |m| m.content.chars().take(48).collect(),
            );
        SessionRecord {
            id: self.session_id,
            title,
            provider_id: self.provider_id.clone(),
            model: self.model.clone(),
            mode: self.mode,
            messages: self.messages.clone(),
        }
    }

    /// Restores session state from a loaded [`SessionRecord`].
    pub fn restore_record(&mut self, record: SessionRecord) {
        self.session_id = record.id;
        self.provider_id = record.provider_id;
        self.model = record.model;
        self.mode = record.mode;
        self.messages = record.messages;
    }

    /// Builds the system prompt reflecting active mode, rules, skills, MCP servers, and tools.
    #[must_use]
    pub fn build_system_prompt(&self) -> String {
        let mode_instruction = match self.mode {
            AgentMode::Build => {
                "Mode: BUILD. You may design, write, and verify changes concisely while adhering to project architecture rules."
            }
            AgentMode::Plan => {
                "Mode: PLAN (read-only analysis). Propose clear, step-by-step architectural and implementation plans before modifying files."
            }
        };

        let mut prompt =
            format!("You are RuviCode, a modular Rust AI coding agent.\n{mode_instruction}\n");

        if !self.context.rules.is_empty() {
            prompt.push_str("\nActive project rules:\n");
            for rule in &self.context.rules {
                let snippet: String = rule.content.chars().take(600).collect();
                let _ = writeln!(prompt, "- {}:\n{snippet}", rule.name);
            }
        }

        if !self.context.skills.is_empty() {
            prompt.push_str("\nLoaded skills:\n");
            for skill in &self.context.skills {
                let _ = writeln!(
                    prompt,
                    "- {} ({}): {}",
                    skill.name, skill.source_harness, skill.description
                );
            }
        }

        if !self.context.mcp_servers.is_empty() {
            prompt.push_str("\nConfigured MCP servers:\n");
            for server in &self.context.mcp_servers {
                let _ = writeln!(prompt, "- {} -> {}", server.name, server.target);
            }
        }

        let tools: Vec<_> = self
            .registry
            .tools()
            .into_iter()
            .filter(|tool| self.mode != AgentMode::Plan || tool.is_read_only())
            .collect();
        if !tools.is_empty() {
            prompt.push_str("\nRegistered tools:\n");
            for tool in tools {
                let _ = writeln!(prompt, "- {}: {}", tool.name(), tool.description());
            }
        }

        prompt
    }

    /// Sends a user prompt and runs the full multi-step streaming/tool-call loop.
    ///
    /// # Errors
    /// Returns [`ruvi_core::CoreError`] if the active provider is missing or fails completion.
    pub async fn send_user_message(
        &mut self,
        input: impl Into<String>,
    ) -> CoreResult<CompletionResponse> {
        let user_text = input.into();
        let _ = self.bus.publish(AppEvent::PromptSubmitted {
            session_id: self.session_id,
            text: user_text.clone(),
        });

        self.messages
            .push(ChatMessage::new(MessageRole::User, user_text));

        let mut request_messages = Vec::with_capacity(self.messages.len() + 1);
        request_messages.push(ChatMessage::new(
            MessageRole::System,
            self.build_system_prompt(),
        ));
        request_messages.extend(self.messages.iter().cloned());

        let outcome = execute_agent_loop(
            &self.registry,
            AgentLoopConfig {
                provider_id: self.provider_id.clone(),
                model: self.model.clone(),
                mode: self.mode,
                initial_messages: request_messages,
            },
            None,
        )
        .await?;

        self.messages.extend(outcome.generated_messages);
        Ok(outcome.response)
    }
}

/// Runs a streaming multi-step completion + [`ruvi_core::AgentTool`] execution loop.
///
/// # Errors
/// Returns [`ruvi_core::CoreError`] if the provider is missing or fails transport.
pub async fn execute_agent_loop(
    registry: &ExtensionRegistry,
    config: AgentLoopConfig,
    event_tx: Option<mpsc::UnboundedSender<ChatLoopEvent>>,
) -> CoreResult<AgentLoopOutcome> {
    let provider = registry.provider(&config.provider_id)?;
    let tools: Vec<ToolDefinition> = registry
        .tools()
        .into_iter()
        .filter(|tool| config.mode != AgentMode::Plan || tool.is_read_only())
        .map(|tool| ToolDefinition {
            name: tool.name().to_string(),
            description: tool.description().to_string(),
            parameters: tool.parameters_schema(),
        })
        .collect();

    let mut conversation = config.initial_messages;
    let mut generated_messages = Vec::new();

    for _step in 0..MAX_TOOL_STEPS {
        let (tok_tx, mut tok_rx) = mpsc::unbounded_channel();
        let forwarder = event_tx.as_ref().map(|out_tx| {
            let out_tx = out_tx.clone();
            tokio::spawn(async move {
                while let Some(tok) = tok_rx.recv().await {
                    if out_tx.send(ChatLoopEvent::Token(tok)).is_err() {
                        break;
                    }
                }
            })
        });

        let req = CompletionRequest {
            model: config.model.clone(),
            messages: conversation.clone(),
            mode: config.mode,
            tools: tools.clone(),
        };

        let response = provider.complete_stream(req, tok_tx).await?;
        if let Some(handle) = forwarder {
            let _ = handle.await;
        }

        if response.tool_calls.is_empty() {
            let assistant_msg = ChatMessage::new(MessageRole::Assistant, response.content.clone());
            generated_messages.push(assistant_msg);
            return Ok(AgentLoopOutcome {
                response,
                generated_messages,
            });
        }

        let assistant_call_msg = ChatMessage::assistant_tool_calls(
            response.content.clone(),
            response.tool_calls.clone(),
        );
        conversation.push(assistant_call_msg.clone());
        generated_messages.push(assistant_call_msg);

        for call in &response.tool_calls {
            if let Some(tx) = &event_tx {
                let _ = tx.send(ChatLoopEvent::ToolStarted {
                    name: call.name.clone(),
                    arguments: call.arguments.to_string(),
                });
            }

            let output_json = match registry.tool(&call.name) {
                Ok(tool) if config.mode == AgentMode::Plan && !tool.is_read_only() => {
                    json!({ "error": format!("tool `{}` is disabled in PLAN (read-only) mode", call.name) })
                }
                Ok(tool) => match tool.execute(call.arguments.clone()).await {
                    Ok(val) => val,
                    Err(err) => json!({ "error": err.to_string() }),
                },
                Err(err) => json!({ "error": err.to_string() }),
            };

            let output_str = output_json.to_string();
            if let Some(tx) = &event_tx {
                let _ = tx.send(ChatLoopEvent::ToolFinished {
                    name: call.name.clone(),
                    output: output_str.clone(),
                });
            }

            let tool_msg = ChatMessage::tool_result(call.id.clone(), output_str);
            conversation.push(tool_msg.clone());
            generated_messages.push(tool_msg);
        }
    }

    let final_msg = ChatMessage::new(
        MessageRole::Assistant,
        format!("Reached maximum tool-call steps ({MAX_TOOL_STEPS})."),
    );
    generated_messages.push(final_msg.clone());
    Ok(AgentLoopOutcome {
        response: CompletionResponse {
            model: config.model,
            content: final_msg.content,
            tool_calls: Vec::new(),
        },
        generated_messages,
    })
}

#[cfg(test)]
mod tests {
    use async_trait::async_trait;
    use ruvi_core::{LlmProvider, SkillDescriptor};
    use std::path::PathBuf;

    use super::*;

    struct FakeProvider;

    #[async_trait]
    impl LlmProvider for FakeProvider {
        fn id(&self) -> &'static str {
            "fake"
        }

        fn display_name(&self) -> &'static str {
            "Fake"
        }

        async fn list_models(&self) -> CoreResult<Vec<String>> {
            Ok(vec!["fake-model".to_string()])
        }

        async fn complete(&self, request: CompletionRequest) -> CoreResult<CompletionResponse> {
            let last_user = request
                .messages
                .iter()
                .rfind(|msg| msg.role == MessageRole::User)
                .map_or("", |msg| msg.content.as_str());
            Ok(CompletionResponse {
                model: request.model,
                content: format!("[{}] echo: {last_user}", request.mode.badge()),
                tool_calls: Vec::new(),
            })
        }
    }

    struct ToolLoopProvider;

    #[async_trait]
    impl LlmProvider for ToolLoopProvider {
        fn id(&self) -> &'static str {
            "tool-loop"
        }

        fn display_name(&self) -> &'static str {
            "Tool Loop Mock"
        }

        async fn list_models(&self) -> CoreResult<Vec<String>> {
            Ok(vec!["loop-1".to_string()])
        }

        async fn complete(&self, request: CompletionRequest) -> CoreResult<CompletionResponse> {
            if let Some(tool_msg) = request
                .messages
                .iter()
                .rfind(|m| m.role == MessageRole::Tool)
            {
                return Ok(CompletionResponse {
                    model: request.model,
                    content: format!("observed tool output: {}", tool_msg.content),
                    tool_calls: Vec::new(),
                });
            }

            Ok(CompletionResponse {
                model: request.model,
                content: String::new(),
                tool_calls: vec![ruvi_core::ToolCall {
                    id: "call_42".to_string(),
                    name: "upper".to_string(),
                    arguments: json!({ "text": "ruvi" }),
                }],
            })
        }
    }

    struct UpperTool;

    #[async_trait]
    impl ruvi_core::AgentTool for UpperTool {
        fn name(&self) -> &'static str {
            "upper"
        }

        fn description(&self) -> &'static str {
            "Uppercases input text"
        }

        fn parameters_schema(&self) -> serde_json::Value {
            json!({ "type": "object" })
        }

        async fn execute(&self, input: serde_json::Value) -> CoreResult<serde_json::Value> {
            let text = input["text"].as_str().unwrap_or("");
            Ok(json!({ "upper": text.to_uppercase() }))
        }
    }

    #[tokio::test]
    async fn chat_engine_toggles_mode_and_completes_via_registry() {
        let mut registry = ExtensionRegistry::new();
        registry.register_provider(Arc::new(FakeProvider));
        registry.register_tool(Arc::new(UpperTool));
        registry.register_tool(Arc::new(MutatingSpyTool {
            executed: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        }));

        let context = HarnessContext {
            skills: vec![SkillDescriptor {
                name: "ponytail".to_string(),
                description: "Minimal complete code".to_string(),
                path: PathBuf::from(".agents/skills/ponytail/SKILL.md"),
                source_harness: "agents".to_string(),
            }],
            ..HarnessContext::default()
        };

        let mut engine = ChatEngine::new(
            Arc::new(registry),
            EventBus::default(),
            context,
            "fake",
            "fake-model",
        );

        let build_prompt = engine.build_system_prompt();
        assert!(build_prompt.contains("- upper:"));
        assert!(build_prompt.contains("- mutate:"));

        assert_eq!(engine.toggle_mode(), AgentMode::Plan);
        let reply = engine
            .send_user_message("hello ruvi")
            .await
            .expect("completion should succeed");

        assert_eq!(reply.content, "[PLAN] echo: hello ruvi");
        assert_eq!(engine.messages().len(), 2);
        let plan_prompt = engine.build_system_prompt();
        assert!(plan_prompt.contains("ponytail"));
        assert!(plan_prompt.contains("- upper:"));
        assert!(!plan_prompt.contains("- mutate:"));
    }

    #[tokio::test]
    async fn chat_engine_executes_multi_step_tool_loop() {
        let mut registry = ExtensionRegistry::new();
        registry.register_provider(Arc::new(ToolLoopProvider));
        registry.register_tool(Arc::new(UpperTool));

        let mut engine = ChatEngine::new(
            Arc::new(registry),
            EventBus::default(),
            HarnessContext::default(),
            "tool-loop",
            "loop-1",
        );

        let reply = engine
            .send_user_message("run upper")
            .await
            .expect("tool loop should succeed");

        assert!(reply.content.contains("RUVI"));
        assert!(
            engine
                .messages()
                .iter()
                .any(|m| m.role == MessageRole::Tool)
        );
    }

    struct MutatingSpyTool {
        executed: Arc<std::sync::atomic::AtomicBool>,
    }

    #[async_trait]
    impl ruvi_core::AgentTool for MutatingSpyTool {
        fn name(&self) -> &'static str {
            "mutate"
        }

        fn description(&self) -> &'static str {
            "Mutating test spy tool"
        }

        fn parameters_schema(&self) -> serde_json::Value {
            json!({ "type": "object" })
        }

        fn is_read_only(&self) -> bool {
            false
        }

        async fn execute(&self, _input: serde_json::Value) -> CoreResult<serde_json::Value> {
            self.executed
                .store(true, std::sync::atomic::Ordering::SeqCst);
            Ok(json!({ "mutated": true }))
        }
    }

    struct ModeCheckingProvider {
        seen_tools: Arc<std::sync::Mutex<Vec<Vec<String>>>>,
    }

    #[async_trait]
    impl LlmProvider for ModeCheckingProvider {
        fn id(&self) -> &'static str {
            "mode-check"
        }

        fn display_name(&self) -> &'static str {
            "Mode Checking Provider"
        }

        async fn list_models(&self) -> CoreResult<Vec<String>> {
            Ok(vec!["mode-1".to_string()])
        }

        async fn complete(&self, request: CompletionRequest) -> CoreResult<CompletionResponse> {
            let names: Vec<String> = request.tools.iter().map(|t| t.name.clone()).collect();
            self.seen_tools
                .lock()
                .expect("seen_tools mutex should not be poisoned")
                .push(names);

            let tool_outputs: Vec<&str> = request
                .messages
                .iter()
                .filter(|m| m.role == MessageRole::Tool)
                .map(|m| m.content.as_str())
                .collect();
            if !tool_outputs.is_empty() {
                return Ok(CompletionResponse {
                    model: request.model,
                    content: tool_outputs.join(" | "),
                    tool_calls: Vec::new(),
                });
            }

            Ok(CompletionResponse {
                model: request.model,
                content: String::new(),
                tool_calls: vec![
                    ruvi_core::ToolCall {
                        id: "call_ro".to_string(),
                        name: "upper".to_string(),
                        arguments: json!({ "text": "ok" }),
                    },
                    ruvi_core::ToolCall {
                        id: "call_rw".to_string(),
                        name: "mutate".to_string(),
                        arguments: json!({}),
                    },
                ],
            })
        }
    }

    #[tokio::test]
    async fn execute_agent_loop_filters_and_blocks_mutating_tools_in_plan_mode() {
        let executed = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let seen_tools = Arc::new(std::sync::Mutex::new(Vec::new()));

        let mut registry = ExtensionRegistry::new();
        registry.register_provider(Arc::new(ModeCheckingProvider {
            seen_tools: Arc::clone(&seen_tools),
        }));
        registry.register_tool(Arc::new(UpperTool));
        registry.register_tool(Arc::new(MutatingSpyTool {
            executed: Arc::clone(&executed),
        }));

        let plan_outcome = execute_agent_loop(
            &registry,
            AgentLoopConfig {
                provider_id: "mode-check".to_string(),
                model: "mode-1".to_string(),
                mode: AgentMode::Plan,
                initial_messages: vec![ChatMessage::new(MessageRole::User, "plan only")],
            },
            None,
        )
        .await
        .expect("plan loop should complete");

        assert!(
            !executed.load(std::sync::atomic::Ordering::SeqCst),
            "mutating tool must never execute in Plan mode"
        );
        assert!(plan_outcome.response.content.contains("OK"));
        assert!(
            plan_outcome
                .response
                .content
                .contains("tool `mutate` is disabled in PLAN (read-only) mode")
        );
        {
            let recorded = seen_tools
                .lock()
                .expect("seen_tools mutex should not be poisoned");
            assert_eq!(recorded[0], vec!["upper".to_string()]);
        }

        let build_outcome = execute_agent_loop(
            &registry,
            AgentLoopConfig {
                provider_id: "mode-check".to_string(),
                model: "mode-1".to_string(),
                mode: AgentMode::Build,
                initial_messages: vec![ChatMessage::new(MessageRole::User, "build now")],
            },
            None,
        )
        .await
        .expect("build loop should complete");

        assert!(executed.load(std::sync::atomic::Ordering::SeqCst));
        assert!(build_outcome.response.content.contains("\"mutated\":true"));
        let recorded = seen_tools
            .lock()
            .expect("seen_tools mutex should not be poisoned");
        assert_eq!(
            recorded.last().expect("build request recorded"),
            &vec!["mutate".to_string(), "upper".to_string()]
        );
    }
}
