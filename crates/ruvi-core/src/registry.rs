use std::collections::BTreeMap;
use std::sync::Arc;

use crate::domain::PluginManifest;
use crate::error::{CoreError, CoreResult};
use crate::traits::{
    AgentTool, ChannelGateway, ContextLoader, LlmProvider, LocalRuntimeManager, PluginHost,
    SpeechTranscriber, UiTheme,
};

/// Central Open/Closed extension container holding all replaceable `RuviCode` subsystems.
///
/// New implementations can be registered or swapped by `id`/`name` at runtime
/// without modifying any logic inside `ruvi-core`.
#[derive(Default)]
pub struct ExtensionRegistry {
    providers: BTreeMap<String, Arc<dyn LlmProvider>>,
    tools: BTreeMap<String, Arc<dyn AgentTool>>,
    transcribers: BTreeMap<String, Arc<dyn SpeechTranscriber>>,
    runtimes: BTreeMap<String, Arc<dyn LocalRuntimeManager>>,
    context_loaders: BTreeMap<String, Arc<dyn ContextLoader>>,
    channels: BTreeMap<String, Arc<dyn ChannelGateway>>,
    plugin_hosts: BTreeMap<String, Arc<dyn PluginHost>>,
    themes: BTreeMap<String, Arc<dyn UiTheme>>,
    plugin_manifests: Vec<PluginManifest>,
}

impl ExtensionRegistry {
    /// Creates an empty [`ExtensionRegistry`].
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers or replaces an [`LlmProvider`] keyed by [`LlmProvider::id`].
    pub fn register_provider(&mut self, provider: Arc<dyn LlmProvider>) {
        self.providers.insert(provider.id().to_string(), provider);
    }

    /// Registers or replaces an [`AgentTool`] keyed by [`AgentTool::name`].
    pub fn register_tool(&mut self, tool: Arc<dyn AgentTool>) {
        self.tools.insert(tool.name().to_string(), tool);
    }

    /// Registers or replaces a [`SpeechTranscriber`] keyed by [`SpeechTranscriber::id`].
    pub fn register_transcriber(&mut self, transcriber: Arc<dyn SpeechTranscriber>) {
        self.transcribers
            .insert(transcriber.id().to_string(), transcriber);
    }

    /// Registers or replaces a [`LocalRuntimeManager`] keyed by [`LocalRuntimeManager::id`].
    pub fn register_runtime(&mut self, runtime: Arc<dyn LocalRuntimeManager>) {
        self.runtimes.insert(runtime.id().to_string(), runtime);
    }

    /// Registers or replaces a [`ContextLoader`] keyed by [`ContextLoader::id`].
    pub fn register_context_loader(&mut self, loader: Arc<dyn ContextLoader>) {
        self.context_loaders.insert(loader.id().to_string(), loader);
    }

    /// Registers or replaces a [`ChannelGateway`] keyed by [`ChannelGateway::id`].
    pub fn register_channel(&mut self, channel: Arc<dyn ChannelGateway>) {
        self.channels.insert(channel.id().to_string(), channel);
    }

    /// Registers or replaces a [`PluginHost`] keyed by [`PluginHost::id`].
    pub fn register_plugin_host(&mut self, host: Arc<dyn PluginHost>) {
        self.plugin_hosts.insert(host.id().to_string(), host);
    }

    /// Registers or replaces a [`UiTheme`] keyed by [`UiTheme::id`].
    pub fn register_theme(&mut self, theme: Arc<dyn UiTheme>) {
        self.themes.insert(theme.id().to_string(), theme);
    }

    /// Records manifests of dynamically loaded plugins.
    pub fn extend_plugin_manifests(&mut self, manifests: Vec<PluginManifest>) {
        self.plugin_manifests.extend(manifests);
    }

    /// Resolves an [`LlmProvider`] by `id`.
    ///
    /// # Errors
    /// Returns [`CoreError::NotFound`] if no provider is registered under `id`.
    pub fn provider(&self, id: &str) -> CoreResult<Arc<dyn LlmProvider>> {
        self.providers
            .get(id)
            .cloned()
            .ok_or_else(|| CoreError::NotFound {
                kind: "provider",
                id: id.to_string(),
            })
    }

    /// Returns all registered [`LlmProvider`] instances ordered by ID.
    #[must_use]
    pub fn providers(&self) -> Vec<Arc<dyn LlmProvider>> {
        self.providers.values().cloned().collect()
    }

    /// Resolves an [`AgentTool`] by `name`.
    ///
    /// # Errors
    /// Returns [`CoreError::NotFound`] if no tool is registered under `name`.
    pub fn tool(&self, name: &str) -> CoreResult<Arc<dyn AgentTool>> {
        self.tools
            .get(name)
            .cloned()
            .ok_or_else(|| CoreError::NotFound {
                kind: "tool",
                id: name.to_string(),
            })
    }

    /// Returns all registered [`AgentTool`] instances ordered by name.
    #[must_use]
    pub fn tools(&self) -> Vec<Arc<dyn AgentTool>> {
        self.tools.values().cloned().collect()
    }

    /// Returns all registered [`SpeechTranscriber`] instances ordered by ID.
    #[must_use]
    pub fn transcribers(&self) -> Vec<Arc<dyn SpeechTranscriber>> {
        self.transcribers.values().cloned().collect()
    }

    /// Resolves a [`LocalRuntimeManager`] by `id`.
    ///
    /// # Errors
    /// Returns [`CoreError::NotFound`] if no runtime is registered under `id`.
    pub fn runtime(&self, id: &str) -> CoreResult<Arc<dyn LocalRuntimeManager>> {
        self.runtimes
            .get(id)
            .cloned()
            .ok_or_else(|| CoreError::NotFound {
                kind: "runtime",
                id: id.to_string(),
            })
    }

    /// Returns all registered [`LocalRuntimeManager`] instances ordered by ID.
    #[must_use]
    pub fn runtimes(&self) -> Vec<Arc<dyn LocalRuntimeManager>> {
        self.runtimes.values().cloned().collect()
    }

    /// Returns all registered [`ContextLoader`] instances ordered by ID.
    #[must_use]
    pub fn context_loaders(&self) -> Vec<Arc<dyn ContextLoader>> {
        self.context_loaders.values().cloned().collect()
    }

    /// Returns all registered [`ChannelGateway`] instances ordered by ID.
    #[must_use]
    pub fn channels(&self) -> Vec<Arc<dyn ChannelGateway>> {
        self.channels.values().cloned().collect()
    }

    /// Returns all registered [`PluginHost`] instances ordered by ID.
    #[must_use]
    pub fn plugin_hosts(&self) -> Vec<Arc<dyn PluginHost>> {
        self.plugin_hosts.values().cloned().collect()
    }

    /// Resolves a [`UiTheme`] by `id`.
    ///
    /// # Errors
    /// Returns [`CoreError::NotFound`] if no theme is registered under `id`.
    pub fn theme(&self, id: &str) -> CoreResult<Arc<dyn UiTheme>> {
        self.themes
            .get(id)
            .cloned()
            .ok_or_else(|| CoreError::NotFound {
                kind: "theme",
                id: id.to_string(),
            })
    }

    /// Returns all registered [`UiTheme`] instances ordered by ID.
    #[must_use]
    pub fn themes(&self) -> Vec<Arc<dyn UiTheme>> {
        self.themes.values().cloned().collect()
    }

    /// Returns manifests of all loaded plugins.
    #[must_use]
    pub fn plugin_manifests(&self) -> &[PluginManifest] {
        &self.plugin_manifests
    }
}

#[cfg(test)]
mod tests {
    use async_trait::async_trait;
    use serde_json::{Value, json};

    use super::*;
    use crate::bus::{AppEvent, EventBus};
    use crate::domain::{
        AgentMode, CompletionRequest, CompletionResponse, SessionId, ThemePalette,
    };

    struct MockProvider {
        id: &'static str,
        reply: &'static str,
    }

    #[async_trait]
    impl LlmProvider for MockProvider {
        fn id(&self) -> &str {
            self.id
        }

        fn display_name(&self) -> &'static str {
            "Mock Provider"
        }

        async fn list_models(&self) -> CoreResult<Vec<String>> {
            Ok(vec!["mock-1".to_string()])
        }

        async fn complete(&self, request: CompletionRequest) -> CoreResult<CompletionResponse> {
            Ok(CompletionResponse {
                model: request.model,
                content: self.reply.to_string(),
            })
        }
    }

    struct MockTool;

    #[async_trait]
    impl AgentTool for MockTool {
        fn name(&self) -> &'static str {
            "echo_tool"
        }

        fn description(&self) -> &'static str {
            "Echoes arguments"
        }

        fn parameters_schema(&self) -> Value {
            json!({"type": "object"})
        }

        async fn execute(&self, input: Value) -> CoreResult<Value> {
            Ok(json!({ "echo": input }))
        }
    }

    struct MockTheme(&'static str);

    impl UiTheme for MockTheme {
        fn id(&self) -> &str {
            self.0
        }

        fn display_name(&self) -> &'static str {
            "Mock Theme"
        }

        fn palette(&self) -> ThemePalette {
            ThemePalette {
                background: (0, 0, 0),
                surface: (10, 10, 10),
                border: (40, 40, 40),
                text: (255, 255, 255),
                muted: (120, 120, 120),
                accent: (0, 200, 255),
                success: (0, 220, 100),
                warning: (255, 180, 0),
            }
        }
    }

    #[tokio::test]
    async fn registry_supports_open_closed_extension_and_replacement() {
        let mut registry = ExtensionRegistry::new();
        registry.register_provider(Arc::new(MockProvider {
            id: "custom",
            reply: "v1",
        }));
        registry.register_tool(Arc::new(MockTool));
        registry.register_theme(Arc::new(MockTheme("custom-theme")));

        let first = registry
            .provider("custom")
            .expect("provider should be registered")
            .complete(CompletionRequest {
                model: "mock-1".to_string(),
                messages: vec![],
                mode: AgentMode::Build,
            })
            .await
            .expect("completion should succeed");
        assert_eq!(first.content, "v1");

        // Replace existing extension without touching ruvi-core
        registry.register_provider(Arc::new(MockProvider {
            id: "custom",
            reply: "v2",
        }));
        let second = registry
            .provider("custom")
            .expect("replaced provider should exist")
            .complete(CompletionRequest {
                model: "mock-1".to_string(),
                messages: vec![],
                mode: AgentMode::Plan,
            })
            .await
            .expect("completion should succeed");
        assert_eq!(second.content, "v2");

        let tool_output = registry
            .tool("echo_tool")
            .expect("tool should exist")
            .execute(json!({"hello": "ruvi"}))
            .await
            .expect("tool should run");
        assert_eq!(tool_output["echo"]["hello"], "ruvi");
        assert_eq!(registry.themes().len(), 1);
    }

    #[tokio::test]
    async fn event_bus_delivers_events_to_subscribers() {
        let bus = EventBus::default();
        let mut rx = bus.subscribe();
        let session_id = SessionId::new();
        let delivered = bus.publish(AppEvent::ModeChanged {
            session_id,
            mode: AgentMode::Plan,
        });
        assert_eq!(delivered, 1);

        let received = rx.recv().await.expect("event should arrive");
        assert_eq!(
            received,
            AppEvent::ModeChanged {
                session_id,
                mode: AgentMode::Plan,
            }
        );
    }
}
