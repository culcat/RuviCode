use std::fmt::Write as _;
use std::sync::Arc;

use ruvi_core::{
    AgentMode, AppEvent, ChatMessage, CompletionRequest, CompletionResponse, CoreResult, EventBus,
    ExtensionRegistry, HarnessContext, MessageRole, SessionId,
};

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

        let tools = self.registry.tools();
        if !tools.is_empty() {
            prompt.push_str("\nRegistered tools:\n");
            for tool in tools {
                let _ = writeln!(prompt, "- {}: {}", tool.name(), tool.description());
            }
        }

        prompt
    }

    /// Sends a user prompt to the active [`ruvi_core::LlmProvider`] and records both messages.
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

        let provider = self.registry.provider(&self.provider_id)?;
        let response = provider
            .complete(CompletionRequest {
                model: self.model.clone(),
                messages: request_messages,
                mode: self.mode,
            })
            .await?;

        self.messages.push(ChatMessage::new(
            MessageRole::Assistant,
            response.content.clone(),
        ));
        Ok(response)
    }
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
            })
        }
    }

    #[tokio::test]
    async fn chat_engine_toggles_mode_and_completes_via_registry() {
        let mut registry = ExtensionRegistry::new();
        registry.register_provider(Arc::new(FakeProvider));

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

        assert_eq!(engine.toggle_mode(), AgentMode::Plan);
        let reply = engine
            .send_user_message("hello ruvi")
            .await
            .expect("completion should succeed");

        assert_eq!(reply.content, "[PLAN] echo: hello ruvi");
        assert_eq!(engine.messages().len(), 2);
        assert!(engine.build_system_prompt().contains("ponytail"));
    }
}
