use std::path::{Path, PathBuf};
use std::sync::Arc;

use ruvi_core::{CoreResult, ExtensionRegistry, HarnessContext};

use crate::slices::bots::{DiscordBotGateway, TelegramBotGateway};
use crate::slices::harness_compat::MultiHarnessContextLoader;
use crate::slices::local_llm::{LlamaCppRuntimeManager, OllamaRuntimeManager};
use crate::slices::mcp::register_mcp_tools;
use crate::slices::plugins_wasm::WasmiPluginHost;
use crate::slices::providers::register_default_providers;
use crate::slices::tools::register_builtin_tools;
use crate::slices::tui::theme;
use crate::slices::voice::{WhisperCliTranscriber, WhisperRuntimeManager};

/// Composition root wiring all vertical slices into [`ExtensionRegistry`] and loading workspace context/plugins.
///
/// # Errors
/// Returns [`ruvi_core::CoreError`] if a context loader or plugin host fails fatally.
pub async fn build_registry(
    workspace: &Path,
) -> CoreResult<(Arc<ExtensionRegistry>, HarnessContext)> {
    let mut registry = ExtensionRegistry::new();

    register_default_providers(&mut registry);
    register_builtin_tools(&mut registry, workspace);

    registry.register_runtime(Arc::new(OllamaRuntimeManager::new()));
    registry.register_runtime(Arc::new(LlamaCppRuntimeManager::new()));
    registry.register_runtime(Arc::new(WhisperRuntimeManager));

    registry.register_transcriber(Arc::new(WhisperCliTranscriber));
    registry.register_context_loader(Arc::new(MultiHarnessContextLoader));
    registry.register_plugin_host(Arc::new(WasmiPluginHost::new()));

    registry.register_channel(Arc::new(TelegramBotGateway::from_env()));
    registry.register_channel(Arc::new(DiscordBotGateway::from_env()));

    theme::register_default_themes(&mut registry);

    let mut context = HarnessContext::default();
    for loader in registry.context_loaders() {
        context.merge(loader.load(workspace).await?);
    }

    let _mcp_warnings = register_mcp_tools(&mut registry, &context.mcp_servers).await;

    let mut plugin_dirs: Vec<PathBuf> = vec![workspace.join(".ruvi").join("plugins")];
    if let Ok(home) = std::env::var("HOME")
        && !home.trim().is_empty()
    {
        plugin_dirs.push(
            PathBuf::from(home)
                .join(".config")
                .join("ruvi")
                .join("plugins"),
        );
    }
    let dir_refs: Vec<&Path> = plugin_dirs.iter().map(PathBuf::as_path).collect();

    for host in registry.plugin_hosts() {
        let (manifests, tools) = host.load_plugins(&dir_refs).await?;
        registry.extend_plugin_manifests(manifests);
        for tool in tools {
            registry.register_tool(tool);
        }
    }

    Ok((Arc::new(registry), context))
}
