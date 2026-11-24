use std::path::{Path, PathBuf};
use std::process::Command as StdCommand;
use std::sync::Arc;

use anyhow::Result;
use clap::{Parser, Subcommand, ValueEnum};
use ruvi_core::{AgentMode, EventBus, SessionId};
use ruvicode::bootstrap::build_registry;
use ruvicode::slices::bots::start_configured_bots;
use ruvicode::slices::chat::ChatEngine;
use ruvicode::slices::providers::detect_initial_provider_and_model;
use ruvicode::slices::tui::{TuiApp, collect_runtime_statuses, run_tui};

#[derive(Debug, Parser)]
#[command(
    name = "ruvi",
    version,
    about = "RuviCode — modular AI agent harness with an OpenCode-inspired TUI"
)]
struct Cli {
    /// Workspace directory path (defaults to current working directory).
    #[arg(short, long)]
    workspace: Option<PathBuf>,

    /// Resume a persisted session by UUID when launching the TUI.
    #[arg(long)]
    resume: Option<String>,

    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Debug, Subcommand)]
enum Commands {
    /// Inspect discovered rules, skills, MCP servers, providers, local runtimes, plugins, and bots.
    Doctor,
    /// Execute a single-shot prompt without launching the TUI.
    Prompt {
        /// Prompt text to send to the agent.
        message: String,
        /// Override LLM provider ID (e.g. `ollama`, `openai`, `openrouter`, `ruvi-demo`).
        #[arg(long)]
        provider: Option<String>,
        /// Override model identifier.
        #[arg(long)]
        model: Option<String>,
        /// Agent mode (`build` or `plan`).
        #[arg(long, value_enum, default_value_t = CliMode::Build)]
        mode: CliMode,
    },
    /// Run configured Telegram/Discord bot gateways in headless daemon mode.
    Bots {
        /// Override LLM provider ID (e.g. `ollama`, `openai`, `openrouter`, `ruvi-demo`).
        #[arg(long)]
        provider: Option<String>,
        /// Override model identifier.
        #[arg(long)]
        model: Option<String>,
    },
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum CliMode {
    Build,
    Plan,
}

impl From<CliMode> for AgentMode {
    fn from(value: CliMode) -> Self {
        match value {
            CliMode::Build => Self::Build,
            CliMode::Plan => Self::Plan,
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    if cli.command.is_some() || std::env::var_os("RUST_LOG").is_some() {
        tracing_subscriber::fmt()
            .with_env_filter(
                tracing_subscriber::EnvFilter::try_from_default_env()
                    .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
            )
            .with_writer(std::io::stderr)
            .init();
    }

    let workspace = match cli.workspace {
        Some(path) => path,
        None => std::env::current_dir()?,
    };

    let (registry, context) = build_registry(&workspace).await?;
    let bus = EventBus::default();

    match cli.command {
        None => {
            let (provider_id, model) = detect_initial_provider_and_model(&registry).await;
            let statuses = collect_runtime_statuses(&registry).await;
            let mut engine = ChatEngine::new(
                Arc::clone(&registry),
                bus.clone(),
                context,
                provider_id,
                model,
            );
            let bot_system_prompt = engine.build_system_prompt();
            if let Some(session_str) = cli.resume
                && let Some(store) = registry.session_store()
            {
                let id = SessionId::parse(&session_str)?;
                let record = store.load(id).await?;
                engine.restore_record(record);
            }
            if let Err(err) = start_configured_bots(
                Arc::clone(&registry),
                bus.clone(),
                bot_system_prompt,
                engine.provider_id().to_string(),
                engine.model().to_string(),
            )
            .await
            {
                tracing::warn!("failed to start configured bot gateways: {err}");
            }
            let branch = detect_git_branch(&workspace);
            let app = TuiApp::new(
                workspace.display().to_string(),
                branch,
                engine,
                registry,
                bus,
                statuses,
            );
            run_tui(app).await?;
        }
        Some(Commands::Doctor) => {
            print_doctor_report(&workspace, &registry, &context).await;
        }
        Some(Commands::Prompt {
            message,
            provider,
            model,
            mode,
        }) => {
            let (chosen_provider, chosen_model) =
                resolve_provider_and_model(&registry, provider, model).await?;

            let mut engine = ChatEngine::new(
                Arc::clone(&registry),
                bus,
                context,
                chosen_provider,
                chosen_model,
            );
            engine.set_mode(mode.into());
            let reply = engine.send_user_message(message).await?;
            println!("{}", reply.content);
        }
        Some(Commands::Bots { provider, model }) => {
            run_bots_daemon(registry, bus, context, provider, model).await?;
        }
    }

    Ok(())
}

async fn run_bots_daemon(
    registry: Arc<ruvi_core::ExtensionRegistry>,
    bus: EventBus,
    context: ruvi_core::HarnessContext,
    provider: Option<String>,
    model: Option<String>,
) -> Result<()> {
    let (chosen_provider, chosen_model) =
        resolve_provider_and_model(&registry, provider, model).await?;
    let engine = ChatEngine::new(
        Arc::clone(&registry),
        bus.clone(),
        context,
        chosen_provider.clone(),
        chosen_model.clone(),
    );
    let started = start_configured_bots(
        Arc::clone(&registry),
        bus,
        engine.build_system_prompt(),
        chosen_provider.clone(),
        chosen_model.clone(),
    )
    .await?;

    if started.is_empty() {
        println!("No bot gateways are configured.");
        println!("Configure at least one gateway via environment variables:");
        println!("  Telegram: RUVI_TELEGRAM_BOT_TOKEN (optional: RUVI_TELEGRAM_CHAT_ID)");
        println!("  Discord:  RUVI_DISCORD_BOT_TOKEN and RUVI_DISCORD_CHANNEL_ID");
        return Ok(());
    }

    println!(
        "RuviCode bots running ({chosen_provider}/{chosen_model}) for gateways: {}",
        started.join(", ")
    );
    println!("Press Ctrl+C to stop.");
    tokio::signal::ctrl_c().await?;
    Ok(())
}

async fn resolve_provider_and_model(
    registry: &ruvi_core::ExtensionRegistry,
    provider: Option<String>,
    model: Option<String>,
) -> Result<(String, String)> {
    let (default_provider, default_model) = detect_initial_provider_and_model(registry).await;
    let chosen_provider = provider.unwrap_or_else(|| default_provider.clone());
    let chosen_model = match model {
        Some(m) => m,
        None if chosen_provider == default_provider => default_model,
        None if chosen_provider == "ruvi-demo" => "ruvi-demo-v1".to_string(),
        None => {
            let prov = registry.provider(&chosen_provider)?;
            prov.list_models()
                .await?
                .into_iter()
                .next()
                .ok_or_else(|| {
                    anyhow::anyhow!("provider `{chosen_provider}` returned no available models")
                })?
        }
    };
    Ok((chosen_provider, chosen_model))
}

async fn print_doctor_report(
    workspace: &Path,
    registry: &ruvi_core::ExtensionRegistry,
    context: &ruvi_core::HarnessContext,
) {
    println!("RuviCode Doctor");
    println!("Workspace: {}", workspace.display());
    println!("Git branch: {}", detect_git_branch(workspace));

    println!("\n[Rules ({})]", context.rules.len());
    for rule in &context.rules {
        println!("  - {}", rule.name);
    }

    println!("\n[Skills ({})]", context.skills.len());
    for skill in &context.skills {
        println!(
            "  - {} [{}] — {}",
            skill.name, skill.source_harness, skill.description
        );
    }

    println!("\n[MCP Servers ({})]", context.mcp_servers.len());
    for server in &context.mcp_servers {
        println!(
            "  - {} ({}) -> {}",
            server.name, server.source_file, server.target
        );
    }

    println!("\n[LLM Providers ({})]", registry.providers().len());
    for provider in registry.providers() {
        println!("  - {} ({})", provider.id(), provider.display_name());
    }

    let statuses = collect_runtime_statuses(registry).await;
    println!("\n[Local Runtimes ({})]", statuses.len());
    for rt in statuses {
        let state = if rt.running {
            "running"
        } else if rt.installed {
            "installed (daemon stopped)"
        } else {
            "not installed"
        };
        let ver = rt.version.as_deref().unwrap_or("-");
        println!("  - {} ({}): {state} [ver: {ver}]", rt.display_name, rt.id);
        if !rt.models.is_empty() {
            println!("    models: {}", rt.models.join(", "));
        }
    }

    println!(
        "\n[WASM Plugins ({}) / Tools ({})]",
        registry.plugin_manifests().len(),
        registry.tools().len()
    );
    for plugin in registry.plugin_manifests() {
        println!(
            "  - {} v{} ({})",
            plugin.name, plugin.version, plugin.source
        );
    }

    println!("\n[Bot Gateways ({})]", registry.channels().len());
    for ch in registry.channels() {
        let state = if ch.is_configured() {
            "configured"
        } else {
            "not configured"
        };
        println!("  - {} ({}): {state}", ch.display_name(), ch.id());
    }
}

fn detect_git_branch(workspace: &Path) -> String {
    StdCommand::new("git")
        .arg("-C")
        .arg(workspace)
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .output()
        .ok()
        .filter(|out| out.status.success())
        .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_string())
        .filter(|branch| !branch.is_empty())
        .unwrap_or_else(|| "no-git".to_string())
}

#[cfg(test)]
mod tests {
    use async_trait::async_trait;
    use ruvi_core::{
        CompletionRequest, CompletionResponse, CoreResult, ExtensionRegistry, LlmProvider,
    };

    use super::*;

    struct StubProvider {
        id: &'static str,
        models: Vec<String>,
    }

    #[async_trait]
    impl LlmProvider for StubProvider {
        fn id(&self) -> &str {
            self.id
        }

        fn display_name(&self) -> &'static str {
            "Stub"
        }

        async fn list_models(&self) -> CoreResult<Vec<String>> {
            Ok(self.models.clone())
        }

        async fn complete(&self, _request: CompletionRequest) -> CoreResult<CompletionResponse> {
            unreachable!()
        }
    }

    #[tokio::test]
    async fn resolve_provider_and_model_preserves_defaults_and_errors_on_empty_models() {
        let mut registry = ExtensionRegistry::new();
        registry.register_provider(Arc::new(StubProvider {
            id: "custom",
            models: vec!["custom-model-1".to_string()],
        }));
        registry.register_provider(Arc::new(StubProvider {
            id: "empty",
            models: Vec::new(),
        }));

        let (prov, model) = resolve_provider_and_model(&registry, Some("custom".into()), None)
            .await
            .expect("custom provider with model should resolve");
        assert_eq!(prov, "custom");
        assert_eq!(model, "custom-model-1");

        let (explicit_p, explicit_m) =
            resolve_provider_and_model(&registry, Some("empty".into()), Some("pinned".into()))
                .await
                .expect("explicit model override should succeed");
        assert_eq!(explicit_p, "empty");
        assert_eq!(explicit_m, "pinned");

        let err = resolve_provider_and_model(&registry, Some("empty".into()), None)
            .await
            .expect_err("provider with empty models must fail cleanly");
        assert!(err.to_string().contains("no available models"));
    }
}
