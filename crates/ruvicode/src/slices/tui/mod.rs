pub mod theme;
mod view;

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyModifiers};
use ruvi_core::{
    AppEvent, ChatMessage, CoreResult, EventBus, ExtensionRegistry, MessageRole, RuntimeStatus,
    SessionId, SessionRecord, SessionSummary, ThemePalette, UiTheme,
};
use tokio::sync::mpsc;

use crate::slices::chat::{
    AgentLoopConfig, AgentLoopOutcome, ChatEngine, ChatLoopEvent, execute_agent_loop,
};

/// Built-in slash commands exposed in the Command Palette (`Ctrl+P` / `/help`).
pub const COMMANDS: &[(&str, &str)] = &[
    ("/models", "Select active LLM provider and model"),
    ("/agents", "Switch active cross-harness agent profile"),
    ("/sessions", "List and resume saved chat sessions"),
    (
        "/local",
        "Install or inspect Ollama, llama.cpp, and Whisper STT",
    ),
    ("/skills", "Show discovered skills across harnesses"),
    ("/mcp", "Inspect configured MCP servers and live MCP tools"),
    ("/plugins", "Inspect loaded WASM/native plugins and tools"),
    ("/bots", "Inspect Telegram & Discord bot gateways"),
    ("/voice", "Transcribe an audio file (`/voice <path>`)"),
    (
        "/mic",
        "Record microphone audio and transcribe (`/mic [seconds]`)",
    ),
    ("/theme", "Cycle active UI theme"),
    ("/mode", "Toggle BUILD / PLAN mode"),
    ("/clear", "Clear conversation history"),
    ("/help", "Open this command palette"),
];

/// Currently open overlay modal in the TUI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActiveModal {
    /// No modal open.
    None,
    /// Interactive command palette.
    CommandPalette {
        /// Highlighted command index.
        selected: usize,
    },
    /// Interactive provider/model selector.
    ModelSelector {
        /// `(provider_id, model_name)` list.
        entries: Vec<(String, String)>,
        /// Highlighted entry index.
        selected: usize,
    },
    /// Local AI runtime installer (`ollama`, `llama.cpp`, `whisper-cpp`).
    LocalInstaller {
        /// Highlighted runtime index.
        selected: usize,
        /// Live status message for install/pull tasks.
        busy_message: Option<String>,
    },
    /// Interactive picker for persisted sessions.
    SessionSelector {
        /// Available saved sessions.
        sessions: Vec<SessionSummary>,
        /// Highlighted session index.
        selected: usize,
    },
    /// Interactive picker for cross-harness agent profiles.
    AgentSelector {
        /// Highlighted agent profile index.
        selected: usize,
    },
    /// Read-only inspection modal for skills, plugins, or bots.
    Info {
        /// Modal title.
        title: String,
        /// Text lines displayed inside modal.
        lines: Vec<String>,
    },
}

enum BgResult {
    StreamEvent(ChatLoopEvent),
    ChatFinished(CoreResult<AgentLoopOutcome>),
    ModelsLoaded(Vec<(String, String)>),
    SessionsLoaded(Vec<SessionSummary>),
    SessionRestored(CoreResult<SessionRecord>),
    RuntimesRefreshed(Vec<RuntimeStatus>),
    RuntimeActionFinished {
        result: CoreResult<String>,
        statuses: Vec<RuntimeStatus>,
    },
    VoiceTranscribed(CoreResult<String>),
}

/// Interactive state for the `OpenCode`-inspired TUI.
pub struct TuiApp {
    /// Workspace path shown in Hero card.
    pub workspace_display: String,
    /// Current git branch label shown in top header.
    pub git_branch: String,
    /// Core chat engine managing history and active provider/model.
    pub engine: ChatEngine,
    /// OCP extension registry.
    pub registry: Arc<ExtensionRegistry>,
    /// Event bus for cross-slice notifications.
    pub bus: EventBus,
    /// Cached statuses of registered local runtimes.
    pub runtime_statuses: Vec<RuntimeStatus>,
    /// Currently active [`ruvi_core::UiTheme`] ID.
    pub theme_id: String,
    /// Currently active cross-harness agent profile name.
    pub active_agent: Option<String>,
    /// Prompt input buffer.
    pub input: String,
    /// Vertical scroll line offset for chat timeline.
    pub scroll_offset: u16,
    /// Whether the right Context & Extensions sidebar is visible.
    pub show_sidebar: bool,
    /// Active overlay modal.
    pub modal: ActiveModal,
    /// Footer status message.
    pub status_line: String,
    /// Whether an async completion is currently running.
    pub in_flight: bool,
    /// Live streaming tokens accumulated for the active assistant turn.
    pub streaming_buffer: String,
    /// Spinner animation frame counter.
    pub spinner_tick: usize,
    /// Exit flag.
    pub should_quit: bool,
    bg_tx: mpsc::UnboundedSender<BgResult>,
    bg_rx: mpsc::UnboundedReceiver<BgResult>,
}

impl TuiApp {
    /// Creates a new [`TuiApp`].
    #[must_use]
    pub fn new(
        workspace_display: String,
        git_branch: String,
        engine: ChatEngine,
        registry: Arc<ExtensionRegistry>,
        bus: EventBus,
        runtime_statuses: Vec<RuntimeStatus>,
    ) -> Self {
        let (bg_tx, bg_rx) = mpsc::unbounded_channel();
        Self {
            workspace_display,
            git_branch,
            engine,
            registry,
            bus,
            runtime_statuses,
            theme_id: "opencode-dark".to_string(),
            active_agent: None,
            input: String::new(),
            scroll_offset: 0,
            show_sidebar: true,
            modal: ActiveModal::None,
            status_line: "Ready".to_string(),
            in_flight: false,
            streaming_buffer: String::new(),
            spinner_tick: 0,
            should_quit: false,
            bg_tx,
            bg_rx,
        }
    }

    /// Resolves the active [`ThemePalette`] from [`ExtensionRegistry`].
    #[must_use]
    pub fn current_palette(&self) -> ThemePalette {
        if let Ok(theme) = self.registry.theme(&self.theme_id) {
            return theme.palette();
        }
        theme::OpenCodeDarkTheme.palette()
    }

    fn cycle_theme(&mut self) {
        let themes = self.registry.themes();
        if themes.is_empty() {
            return;
        }
        let current_idx = themes
            .iter()
            .position(|t| t.id() == self.theme_id)
            .unwrap_or(0);
        let next = &themes[(current_idx + 1) % themes.len()];
        self.theme_id = next.id().to_string();
        self.status_line = format!("Theme switched to {}", next.display_name());
    }

    fn spawn_model_discovery(&mut self) {
        let registry = Arc::clone(&self.registry);
        let tx = self.bg_tx.clone();
        self.status_line = "Discovering available models...".to_string();
        tokio::spawn(async move {
            let mut entries = Vec::new();
            for provider in registry.providers() {
                if let Ok(models) = provider.list_models().await {
                    for model in models {
                        entries.push((provider.id().to_string(), model));
                    }
                }
            }
            let _ = tx.send(BgResult::ModelsLoaded(entries));
        });
    }

    fn spawn_runtime_refresh(&mut self) {
        let registry = Arc::clone(&self.registry);
        let tx = self.bg_tx.clone();
        tokio::spawn(async move {
            let statuses = collect_runtime_statuses(&registry).await;
            let _ = tx.send(BgResult::RuntimesRefreshed(statuses));
        });
    }

    fn spawn_runtime_install(&mut self, runtime_id: String) {
        let registry = Arc::clone(&self.registry);
        let tx = self.bg_tx.clone();
        if let ActiveModal::LocalInstaller { busy_message, .. } = &mut self.modal {
            *busy_message = Some(format!("Installing `{runtime_id}`..."));
        }
        tokio::spawn(async move {
            let result = match registry.runtime(&runtime_id) {
                Ok(rt) => rt.install_runtime().await,
                Err(err) => Err(err),
            };
            let statuses = collect_runtime_statuses(&registry).await;
            let _ = tx.send(BgResult::RuntimeActionFinished { result, statuses });
        });
    }

    fn spawn_ollama_pull(&mut self, model: String) {
        let registry = Arc::clone(&self.registry);
        let tx = self.bg_tx.clone();
        if let ActiveModal::LocalInstaller { busy_message, .. } = &mut self.modal {
            *busy_message = Some(format!("Pulling `{model}` with Ollama..."));
        }
        tokio::spawn(async move {
            let result = match registry.runtime("ollama") {
                Ok(rt) => rt.pull_model(&model).await,
                Err(err) => Err(err),
            };
            let statuses = collect_runtime_statuses(&registry).await;
            let _ = tx.send(BgResult::RuntimeActionFinished { result, statuses });
        });
    }

    fn spawn_voice_transcribe(&mut self, audio_path: String) {
        let registry = Arc::clone(&self.registry);
        let tx = self.bg_tx.clone();
        self.status_line = format!("Transcribing {audio_path}...");
        tokio::spawn(async move {
            let transcriber = registry.transcribers().into_iter().next();
            let result = match transcriber {
                Some(t) => t.transcribe_file(Path::new(&audio_path)).await,
                None => Err(ruvi_core::CoreError::NotFound {
                    kind: "transcriber",
                    id: "whisper-cpp".to_string(),
                }),
            };
            let _ = tx.send(BgResult::VoiceTranscribed(result));
        });
    }

    fn spawn_mic_capture(&mut self, seconds: u8) {
        let registry = Arc::clone(&self.registry);
        let tx = self.bg_tx.clone();
        self.status_line = format!("Recording microphone for {seconds}s...");
        tokio::spawn(async move {
            let Some(transcriber) = registry.transcribers().into_iter().next() else {
                let _ = tx.send(BgResult::VoiceTranscribed(Err(
                    ruvi_core::CoreError::NotFound {
                        kind: "transcriber",
                        id: "whisper-cpp".to_string(),
                    },
                )));
                return;
            };
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos());
            let temp_path = std::env::temp_dir()
                .join(format!("ruvicode-mic-{}-{nanos}.wav", std::process::id()));
            let recorder = crate::slices::voice::MicrophoneRecorder;
            let result = match recorder.record_wav(&temp_path, seconds).await {
                Ok(()) => transcriber.transcribe_file(&temp_path).await,
                Err(err) => Err(err),
            };
            let _ = tokio::fs::remove_file(&temp_path).await;
            let _ = tx.send(BgResult::VoiceTranscribed(result));
        });
    }

    fn spawn_session_list(&mut self) {
        let registry = Arc::clone(&self.registry);
        let tx = self.bg_tx.clone();
        self.status_line = "Loading saved sessions...".to_string();
        tokio::spawn(async move {
            let sessions = match registry.session_store() {
                Some(store) => store.list().await.unwrap_or_default(),
                None => Vec::new(),
            };
            let _ = tx.send(BgResult::SessionsLoaded(sessions));
        });
    }

    fn spawn_session_load(&mut self, id: SessionId) {
        let registry = Arc::clone(&self.registry);
        let tx = self.bg_tx.clone();
        tokio::spawn(async move {
            let result = match registry.session_store() {
                Some(store) => store.load(id).await,
                None => Err(ruvi_core::CoreError::NotFound {
                    kind: "session_store",
                    id: "default".to_string(),
                }),
            };
            let _ = tx.send(BgResult::SessionRestored(result));
        });
    }

    fn spawn_session_autosave(&self) {
        let Some(store) = self.registry.session_store() else {
            return;
        };
        let record = self.engine.export_record();
        tokio::spawn(async move {
            let _ = store.save(&record).await;
        });
    }

    fn submit_prompt(&mut self, text: &str) {
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return;
        }
        if trimmed.starts_with('/') {
            self.execute_slash_command(trimmed);
            return;
        }
        if self.in_flight {
            self.status_line = "Wait for active completion to finish".to_string();
            return;
        }

        let _ = self.bus.publish(AppEvent::PromptSubmitted {
            session_id: self.engine.session_id(),
            text: trimmed.to_string(),
        });

        let mut request_messages = Vec::with_capacity(self.engine.messages().len() + 2);
        request_messages.push(ChatMessage::new(
            MessageRole::System,
            self.engine.build_system_prompt(),
        ));
        request_messages.extend(self.engine.messages().iter().cloned());
        request_messages.push(ChatMessage::new(MessageRole::User, trimmed));

        let provider_id = self.engine.provider_id().to_string();
        let model = self.engine.model().to_string();
        let mode = self.engine.mode();
        self.engine.push_user_message(trimmed);

        self.in_flight = true;
        self.streaming_buffer.clear();
        self.status_line = format!("Running {provider_id}/{model}...");
        let registry = Arc::clone(&self.registry);
        let tx = self.bg_tx.clone();
        tokio::spawn(async move {
            let (ev_tx, mut ev_rx) = mpsc::unbounded_channel();
            let fwd_tx = tx.clone();
            let fwd = tokio::spawn(async move {
                while let Some(ev) = ev_rx.recv().await {
                    if fwd_tx.send(BgResult::StreamEvent(ev)).is_err() {
                        break;
                    }
                }
            });

            let res = execute_agent_loop(
                &registry,
                AgentLoopConfig {
                    provider_id,
                    model,
                    mode,
                    initial_messages: request_messages,
                },
                Some(ev_tx),
            )
            .await;
            let _ = fwd.await;
            let _ = tx.send(BgResult::ChatFinished(res));
        });
    }

    fn execute_slash_command(&mut self, cmd_line: &str) {
        let mut parts = cmd_line.splitn(2, ' ');
        let cmd = parts.next().unwrap_or("");
        let arg = parts.next().unwrap_or("").trim();

        match cmd {
            "/help" => self.modal = ActiveModal::CommandPalette { selected: 0 },
            "/models" => self.spawn_model_discovery(),
            "/agents" => self.modal = ActiveModal::AgentSelector { selected: 0 },
            "/sessions" => self.spawn_session_list(),
            "/local" => {
                self.modal = ActiveModal::LocalInstaller {
                    selected: 0,
                    busy_message: None,
                };
                self.spawn_runtime_refresh();
            }
            "/skills" => self.open_skills_modal(),
            "/mcp" => self.open_mcp_modal(),
            "/plugins" => self.open_plugins_modal(),
            "/bots" => self.open_bots_modal(),
            "/voice" => self.handle_voice_command(arg),
            "/mic" => self.handle_mic_command(arg),
            "/theme" => self.cycle_theme(),
            "/mode" => {
                let mode = self.engine.toggle_mode();
                self.status_line = format!("Switched to [{}] mode", mode.badge());
            }
            "/clear" => {
                self.engine.clear_messages();
                self.scroll_offset = 0;
                self.status_line = "Cleared session history".to_string();
            }
            other => self.status_line = format!("Unknown command `{other}` (try /help)"),
        }
    }

    fn open_skills_modal(&mut self) {
        let mut lines: Vec<String> = self
            .engine
            .context()
            .skills
            .iter()
            .map(|s| format!("• {} [{}] — {}", s.name, s.source_harness, s.description))
            .collect();
        if lines.is_empty() {
            lines.push(
                "No skills found in .agents/skills, .claude/skills, or .opencode/skills."
                    .to_string(),
            );
        }
        self.modal = ActiveModal::Info {
            title: "Discovered Harness Skills".to_string(),
            lines,
        };
    }

    fn open_mcp_modal(&mut self) {
        let mut lines: Vec<String> = self
            .engine
            .context()
            .mcp_servers
            .iter()
            .map(|s| {
                format!(
                    "• {} ({}) -> {} {}",
                    s.name,
                    s.source_file,
                    s.target,
                    s.args.join(" ")
                )
            })
            .collect();
        for tool in self
            .registry
            .tools()
            .into_iter()
            .filter(|t| t.name().starts_with("mcp__"))
        {
            lines.push(format!(
                "Live Tool: {} — {}",
                tool.name(),
                tool.description()
            ));
        }
        if lines.is_empty() {
            lines.push("No MCP servers configured in .mcp.json or opencode.json.".to_string());
        }
        self.modal = ActiveModal::Info {
            title: "MCP Servers & Live Tools".to_string(),
            lines,
        };
    }

    fn open_plugins_modal(&mut self) {
        let mut lines = Vec::new();
        for manifest in self.registry.plugin_manifests() {
            lines.push(format!(
                "• {} v{} ({}) — {}",
                manifest.name,
                manifest.version,
                manifest.capabilities.join(","),
                manifest.source
            ));
        }
        for tool in self.registry.tools() {
            lines.push(format!("Tool: {} — {}", tool.name(), tool.description()));
        }
        if lines.is_empty() {
            lines.push(
                "No plugins loaded. Place .wasm or .wat plugins in .ruvi/plugins/.".to_string(),
            );
        }
        self.modal = ActiveModal::Info {
            title: "Plugins & Tools".to_string(),
            lines,
        };
    }

    fn open_bots_modal(&mut self) {
        let lines = self
            .registry
            .channels()
            .into_iter()
            .map(|ch| {
                let status = if ch.is_configured() {
                    "CONFIGURED"
                } else {
                    "UNCONFIGURED"
                };
                format!("• {} ({}) — {status}", ch.display_name(), ch.id())
            })
            .chain([
                String::new(),
                "Env vars:".to_string(),
                "  Telegram: RUVI_TELEGRAM_BOT_TOKEN, RUVI_TELEGRAM_CHAT_ID".to_string(),
                "  Discord:  RUVI_DISCORD_BOT_TOKEN, RUVI_DISCORD_CHANNEL_ID".to_string(),
            ])
            .collect();
        self.modal = ActiveModal::Info {
            title: "Messaging Bot Gateways".to_string(),
            lines,
        };
    }

    fn handle_voice_command(&mut self, arg: &str) {
        if arg.is_empty() {
            self.modal = ActiveModal::Info {
                title: "Voice Input (Whisper STT)".to_string(),
                lines: vec![
                    "Usage: /voice <path-to-wav-or-mp3>".to_string(),
                    "Install local Whisper via `/local` or `brew install whisper-cpp`.".to_string(),
                ],
            };
        } else {
            self.spawn_voice_transcribe(arg.to_string());
        }
    }

    fn handle_mic_command(&mut self, arg: &str) {
        const DEFAULT_MIC_SECONDS: u8 = 5;
        let seconds = if arg.is_empty() {
            DEFAULT_MIC_SECONDS
        } else {
            match arg.parse::<u8>() {
                Ok(secs) if secs > 0 => secs,
                Ok(_) => {
                    self.status_line = "Recording duration must be at least 1 second".to_string();
                    return;
                }
                Err(_) => {
                    self.status_line =
                        format!("Invalid duration `{arg}`. Usage: /mic [seconds] (1-255)");
                    return;
                }
            }
        };
        self.spawn_mic_capture(seconds);
    }

    fn handle_key(&mut self, key: KeyEvent) {
        if key.modifiers.contains(KeyModifiers::CONTROL)
            && matches!(key.code, KeyCode::Char('c' | 'q'))
        {
            self.should_quit = true;
            return;
        }

        if !matches!(self.modal, ActiveModal::None) {
            self.handle_modal_key(key);
            return;
        }

        if key.modifiers.contains(KeyModifiers::CONTROL) {
            match key.code {
                KeyCode::Char('p') => {
                    self.modal = ActiveModal::CommandPalette { selected: 0 };
                    return;
                }
                KeyCode::Char('l') => {
                    self.execute_slash_command("/local");
                    return;
                }
                KeyCode::Char('b') => {
                    self.show_sidebar = !self.show_sidebar;
                    return;
                }
                _ => {}
            }
        }

        match key.code {
            KeyCode::Tab => {
                let mode = self.engine.toggle_mode();
                self.status_line = format!("Switched to [{}] mode", mode.badge());
            }
            KeyCode::Enter => {
                let text = std::mem::take(&mut self.input);
                self.submit_prompt(&text);
            }
            KeyCode::Backspace => {
                self.input.pop();
            }
            KeyCode::Up => {
                self.scroll_offset = self.scroll_offset.saturating_sub(1);
            }
            KeyCode::Down => {
                self.scroll_offset = self.scroll_offset.saturating_add(1);
            }
            KeyCode::PageUp => {
                self.scroll_offset = self.scroll_offset.saturating_sub(8);
            }
            KeyCode::PageDown => {
                self.scroll_offset = self.scroll_offset.saturating_add(8);
            }
            KeyCode::Char(c) => {
                self.input.push(c);
            }
            _ => {}
        }
    }

    fn handle_modal_key(&mut self, key: KeyEvent) {
        if key.code == KeyCode::Esc {
            self.modal = ActiveModal::None;
            return;
        }

        match &mut self.modal {
            ActiveModal::CommandPalette { selected } => match key.code {
                KeyCode::Up => *selected = selected.saturating_sub(1),
                KeyCode::Down => {
                    if *selected + 1 < COMMANDS.len() {
                        *selected += 1;
                    }
                }
                KeyCode::Enter => {
                    let cmd = COMMANDS[*selected].0;
                    self.modal = ActiveModal::None;
                    self.execute_slash_command(cmd);
                }
                _ => {}
            },
            ActiveModal::ModelSelector { entries, selected } => match key.code {
                KeyCode::Up => *selected = selected.saturating_sub(1),
                KeyCode::Down => {
                    if *selected + 1 < entries.len() {
                        *selected += 1;
                    }
                }
                KeyCode::Enter => {
                    if let Some((provider, model)) = entries.get(*selected).cloned() {
                        self.engine.select_model(&provider, &model);
                        self.status_line = format!("Selected {provider} / {model}");
                    }
                    self.modal = ActiveModal::None;
                }
                _ => {}
            },
            ActiveModal::LocalInstaller { selected, .. } => match key.code {
                KeyCode::Up => *selected = selected.saturating_sub(1),
                KeyCode::Down => {
                    if *selected + 1 < self.runtime_statuses.len() {
                        *selected += 1;
                    }
                }
                KeyCode::Char('r') => self.spawn_runtime_refresh(),
                KeyCode::Char('i') => {
                    if let Some(rt) = self.runtime_statuses.get(*selected) {
                        let id = rt.id.clone();
                        self.spawn_runtime_install(id);
                    }
                }
                KeyCode::Char('p') => {
                    self.spawn_ollama_pull("qwen2.5-coder:7b".to_string());
                }
                _ => {}
            },
            ActiveModal::SessionSelector { sessions, selected } => match key.code {
                KeyCode::Up => *selected = selected.saturating_sub(1),
                KeyCode::Down => {
                    if *selected + 1 < sessions.len() {
                        *selected += 1;
                    }
                }
                KeyCode::Enter => {
                    if let Some(summary) = sessions.get(*selected) {
                        let id = summary.id;
                        self.modal = ActiveModal::None;
                        self.spawn_session_load(id);
                    } else {
                        self.modal = ActiveModal::None;
                    }
                }
                _ => {}
            },
            ActiveModal::AgentSelector { selected } => match key.code {
                KeyCode::Up => *selected = selected.saturating_sub(1),
                KeyCode::Down => {
                    if *selected + 1 < self.engine.context().agents.len() {
                        *selected += 1;
                    }
                }
                KeyCode::Enter => {
                    let idx = *selected;
                    self.modal = ActiveModal::None;
                    self.activate_agent_profile(idx);
                }
                _ => {}
            },
            ActiveModal::Info { .. } | ActiveModal::None => {}
        }
    }

    fn activate_agent_profile(&mut self, index: usize) {
        let Some(profile) = self.engine.context().agents.get(index).cloned() else {
            return;
        };
        self.active_agent = Some(profile.name.clone());
        if let Some(mode) = profile.mode {
            self.engine.set_mode(mode);
        }
        if let Some(raw_model) = profile
            .model
            .as_deref()
            .map(str::trim)
            .filter(|m| !m.is_empty())
        {
            if let Some((provider, model)) = raw_model
                .split_once('/')
                .filter(|(p, m)| !p.is_empty() && !m.is_empty())
            {
                self.engine.select_model(provider, model);
            } else {
                let current_provider = self.engine.provider_id().to_string();
                self.engine.select_model(current_provider, raw_model);
            }
        }
        let mut note = format!("Activated agent profile `{}`", profile.name);
        if !profile.description.is_empty() {
            note.push_str(" — ");
            note.push_str(&profile.description);
        }
        if !profile.system_prompt.is_empty() {
            note.push_str("\n\n");
            note.push_str(&profile.system_prompt);
        }
        self.engine.push_system_note(note);
        self.status_line = format!("Switched to agent `{}`", profile.name);
    }

    fn handle_bg_result(&mut self, msg: BgResult) {
        match msg {
            BgResult::StreamEvent(ev) => match ev {
                ChatLoopEvent::Token(tok) => self.streaming_buffer.push_str(&tok),
                ChatLoopEvent::ToolStarted { name, .. } => {
                    self.streaming_buffer.clear();
                    self.status_line = format!("Executing tool `{name}`...");
                }
                ChatLoopEvent::ToolFinished { name, .. } => {
                    self.status_line = format!("Tool `{name}` finished");
                }
            },
            BgResult::ChatFinished(Ok(outcome)) => {
                self.in_flight = false;
                self.streaming_buffer.clear();
                self.engine.extend_messages(outcome.generated_messages);
                self.spawn_session_autosave();
                self.status_line = format!("Completed via {}", outcome.response.model);
            }
            BgResult::ChatFinished(Err(err)) => {
                self.in_flight = false;
                self.streaming_buffer.clear();
                self.engine
                    .push_system_note(format!("Provider error: {err}"));
                self.status_line = format!("Error: {err}");
            }
            BgResult::ModelsLoaded(entries) => {
                self.modal = ActiveModal::ModelSelector {
                    entries,
                    selected: 0,
                };
                self.status_line = "Loaded available models".to_string();
            }
            BgResult::SessionsLoaded(sessions) => {
                self.modal = ActiveModal::SessionSelector {
                    sessions,
                    selected: 0,
                };
                self.status_line = "Loaded saved sessions".to_string();
            }
            BgResult::SessionRestored(Ok(record)) => {
                let id = record.id;
                self.engine.restore_record(record);
                self.scroll_offset = 0;
                self.status_line = format!("Resumed session {id}");
            }
            BgResult::SessionRestored(Err(err)) => {
                self.status_line = format!("Failed to load session: {err}");
            }
            BgResult::RuntimesRefreshed(statuses) => {
                self.runtime_statuses = statuses;
                self.status_line = "Refreshed local runtime statuses".to_string();
            }
            BgResult::RuntimeActionFinished { result, statuses } => {
                self.runtime_statuses = statuses;
                let msg = match result {
                    Ok(ok) => ok,
                    Err(err) => format!("Failed: {err}"),
                };
                if let ActiveModal::LocalInstaller { busy_message, .. } = &mut self.modal {
                    *busy_message = Some(msg.clone());
                }
                self.status_line = msg;
            }
            BgResult::VoiceTranscribed(Ok(transcript)) => {
                self.input = transcript;
                self.status_line = "Voice transcribed into prompt input".to_string();
            }
            BgResult::VoiceTranscribed(Err(err)) => {
                self.status_line = format!("Voice error: {err}");
            }
        }
    }
}

/// Queries live statuses for every registered [`ruvi_core::LocalRuntimeManager`].
pub async fn collect_runtime_statuses(registry: &ExtensionRegistry) -> Vec<RuntimeStatus> {
    let mut statuses = Vec::new();
    for rt in registry.runtimes() {
        if let Ok(status) = rt.status().await {
            statuses.push(status);
        }
    }
    statuses
}

/// Runs the interactive `ratatui` event loop until the user quits (`Ctrl+Q` / `Ctrl+C`).
///
/// # Errors
/// Returns [`std::io::Error`] if terminal rendering fails.
pub async fn run_tui(mut app: TuiApp) -> std::io::Result<()> {
    let mut terminal = ratatui::init();
    let (key_tx, mut key_rx) = mpsc::unbounded_channel::<KeyEvent>();
    let running = Arc::new(AtomicBool::new(true));
    let reader_running = Arc::clone(&running);

    let reader_handle = std::thread::spawn(move || {
        while reader_running.load(Ordering::Relaxed) {
            if event::poll(Duration::from_millis(80)).unwrap_or(false)
                && let Ok(Event::Key(key)) = event::read()
                && key_tx.send(key).is_err()
            {
                break;
            }
        }
    });

    let mut tick = tokio::time::interval(Duration::from_millis(120));
    while !app.should_quit {
        terminal.draw(|frame| view::render(frame, &app))?;

        tokio::select! {
            Some(key) = key_rx.recv() => {
                app.handle_key(key);
            }
            Some(bg) = app.bg_rx.recv() => {
                app.handle_bg_result(bg);
            }
            _ = tick.tick() => {
                if app.in_flight {
                    app.spinner_tick = app.spinner_tick.wrapping_add(1);
                }
            }
        }
    }

    running.store(false, Ordering::Relaxed);
    let _ = reader_handle.join();
    ratatui::restore();
    Ok(())
}

#[cfg(test)]
mod tests {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ruvi_core::{HarnessContext, RuntimeKind};

    use super::*;
    use crate::slices::providers::register_default_providers;

    fn build_test_app() -> TuiApp {
        let mut registry = ExtensionRegistry::new();
        register_default_providers(&mut registry);
        theme::register_default_themes(&mut registry);
        let registry = Arc::new(registry);
        let bus = EventBus::default();
        let engine = ChatEngine::new(
            Arc::clone(&registry),
            bus.clone(),
            HarnessContext::default(),
            "ruvi-demo",
            "ruvi-demo-v1",
        );

        let runtimes = vec![RuntimeStatus {
            id: "ollama".to_string(),
            display_name: "Ollama".to_string(),
            kind: RuntimeKind::Ollama,
            installed: true,
            running: false,
            version: Some("0.6.0".to_string()),
            models: vec!["qwen2.5-coder:7b".to_string()],
            install_hint: "brew install ollama".to_string(),
        }];

        TuiApp::new(
            "/workspace/RuviCode".to_string(),
            "dev".to_string(),
            engine,
            registry,
            bus,
            runtimes,
        )
    }

    fn buffer_text(terminal: &Terminal<TestBackend>) -> String {
        let buf = terminal.backend().buffer();
        let mut text = String::new();
        for y in 0..buf.area.height {
            for x in 0..buf.area.width {
                text.push_str(buf[(x, y)].symbol());
            }
            text.push('\n');
        }
        text
    }

    #[test]
    fn renders_hero_screen_with_opencode_layout() {
        let app = build_test_app();
        let backend = TestBackend::new(120, 36);
        let mut terminal = Terminal::new(backend).expect("test terminal must init");

        terminal
            .draw(|frame| view::render(frame, &app))
            .expect("draw must succeed");

        let rendered = buffer_text(&terminal);
        assert!(rendered.contains("RUVICODE"));
        assert!(rendered.contains("[BUILD]"));
        assert!(rendered.contains("Context & Extensions"));
        assert!(rendered.contains("/local"));
    }

    #[test]
    fn renders_chat_timeline_and_plan_mode() {
        let mut app = build_test_app();
        app.engine.toggle_mode();
        app.engine.push_user_message("Explain Vertical Slice");
        app.engine
            .push_assistant_message("Each slice owns its vertical stack.");

        let backend = TestBackend::new(120, 36);
        let mut terminal = Terminal::new(backend).expect("test terminal must init");
        terminal
            .draw(|frame| view::render(frame, &app))
            .expect("draw must succeed");

        let rendered = buffer_text(&terminal);
        assert!(rendered.contains("[PLAN]"));
        assert!(rendered.contains("Explain Vertical Slice"));
        assert!(rendered.contains("Each slice owns its vertical stack."));
    }

    #[test]
    fn renders_local_installer_modal() {
        let mut app = build_test_app();
        app.modal = ActiveModal::LocalInstaller {
            selected: 0,
            busy_message: None,
        };

        let backend = TestBackend::new(120, 36);
        let mut terminal = Terminal::new(backend).expect("test terminal must init");
        terminal
            .draw(|frame| view::render(frame, &app))
            .expect("draw must succeed");

        let rendered = buffer_text(&terminal);
        assert!(rendered.contains("Local AI Runtime Installer"));
        assert!(rendered.contains("Ollama"));
        assert!(rendered.contains("qwen2.5-coder:7b"));
    }

    #[test]
    fn mic_command_registered_and_validates_duration() {
        assert!(COMMANDS.iter().any(|(cmd, _)| *cmd == "/mic"));

        let mut app = build_test_app();
        app.execute_slash_command("/mic 0");
        assert!(app.status_line.contains("at least 1 second"));

        app.execute_slash_command("/mic abc");
        assert!(app.status_line.contains("Invalid duration"));
    }

    #[test]
    fn renders_agent_selector_modal_and_activates_profile() {
        use ruvi_core::{AgentMode, AgentProfile};

        assert!(COMMANDS.iter().any(|(cmd, _)| *cmd == "/agents"));

        let mut registry = ExtensionRegistry::new();
        register_default_providers(&mut registry);
        theme::register_default_themes(&mut registry);
        let registry = Arc::new(registry);
        let bus = EventBus::default();
        let context = HarnessContext {
            agents: vec![
                AgentProfile {
                    name: "builder".to_string(),
                    description: "Fast implementation agent".to_string(),
                    system_prompt: "Write concise Rust code.".to_string(),
                    model: Some("local-coder".to_string()),
                    mode: Some(AgentMode::Build),
                    source_harness: "opencode".to_string(),
                },
                AgentProfile {
                    name: "reviewer".to_string(),
                    description: "Strict code reviewer".to_string(),
                    system_prompt: "Inspect diffs for edge cases.".to_string(),
                    model: Some("openai/gpt-4o".to_string()),
                    mode: Some(AgentMode::Plan),
                    source_harness: "agents".to_string(),
                },
            ],
            ..HarnessContext::default()
        };
        let engine = ChatEngine::new(
            Arc::clone(&registry),
            bus.clone(),
            context,
            "ruvi-demo",
            "ruvi-demo-v1",
        );
        let mut app = TuiApp::new(
            "/workspace/RuviCode".to_string(),
            "dev".to_string(),
            engine,
            registry,
            bus,
            Vec::new(),
        );

        app.execute_slash_command("/agents");
        assert_eq!(app.modal, ActiveModal::AgentSelector { selected: 0 });

        let backend = TestBackend::new(120, 36);
        let mut terminal = Terminal::new(backend).expect("test terminal must init");
        terminal
            .draw(|frame| view::render(frame, &app))
            .expect("draw must succeed");
        let rendered = buffer_text(&terminal);
        assert!(rendered.contains("Select Agent Profile"));
        assert!(rendered.contains("reviewer [agents]"));
        assert!(rendered.contains("Strict code reviewer"));

        // Move down to `reviewer` and activate with Enter.
        app.handle_key(KeyEvent::from(KeyCode::Down));
        app.handle_key(KeyEvent::from(KeyCode::Enter));
        assert_eq!(app.modal, ActiveModal::None);
        assert_eq!(app.active_agent.as_deref(), Some("reviewer"));
        assert_eq!(app.engine.mode(), AgentMode::Plan);
        assert_eq!(app.engine.provider_id(), "openai");
        assert_eq!(app.engine.model(), "gpt-4o");
        assert!(app.engine.messages().iter().any(|m| {
            m.role == MessageRole::System
                && m.content.contains("Strict code reviewer")
                && m.content.contains("Inspect diffs for edge cases.")
        }));

        terminal
            .draw(|frame| view::render(frame, &app))
            .expect("draw after selection must succeed");
        let after = buffer_text(&terminal);
        assert!(after.contains("agent:reviewer"));
        assert!(after.contains("Agents (2)"));
        assert!(after.contains("*reviewer [agents]"));

        // Selecting a profile with a model without `provider/` keeps the active provider.
        app.execute_slash_command("/agents");
        app.handle_key(KeyEvent::from(KeyCode::Enter));
        assert_eq!(app.active_agent.as_deref(), Some("builder"));
        assert_eq!(app.engine.mode(), AgentMode::Build);
        assert_eq!(app.engine.provider_id(), "openai");
        assert_eq!(app.engine.model(), "local-coder");
    }
}
