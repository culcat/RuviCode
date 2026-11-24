# Domain Glossary (`GLOSSARY.md`)

Mapping between upstream `anomalyco/opencode` (`packages/core/src/*`, `packages/tui/src/*`) and RuviCode Rust domain concepts.

| OpenCode Concept | Meaning | RuviCode Rust Equivalent | Crate / Module |
|---|---|---|---|
| `Session` | Active conversation state and message history | `SessionId`, `ChatEngine` | `ruvi_core::domain`, `ruvicode::slices::chat` |
| `Agent` (`build` / `plan`) | Execution mode controlling edit vs read-only planning | `AgentMode::{Build, Plan}` | `ruvi_core::domain` |
| `Provider` / `Model` | LLM inference backend & selected model | `LlmProvider` trait, `OpenAiCompatibleProvider` | `ruvi_core::traits`, `ruvicode::slices::providers` |
| `Tool` | Callable capability exposed to the agent | `AgentTool` trait | `ruvi_core::traits` |
| `Skill` | Reusable prompt/workflow package (`SKILL.md`) | `SkillDescriptor` | `ruvi_core::domain`, `ruvicode::slices::harness_compat` |
| `Instructions` / `Rules` | Repository rules (`AGENTS.md`, `CLAUDE.md`, `agent.md`) | `RuleDocument`, `HarnessContext` | `ruvi_core::domain`, `ruvicode::slices::harness_compat` |
| `Mcp` | Model Context Protocol server configuration | `McpServerConfig` | `ruvi_core::domain`, `ruvicode::slices::harness_compat` |
| `Bus` / `Event` | Pub/sub notification channel across subsystems | `EventBus`, `AppEvent` | `ruvi_core::bus` |
| `Plugin` | User-supplied runtime extension | `PluginHost` trait, `PluginManifest`, `WasmiPluginHost` | `ruvi_core::traits`, `ruvicode::slices::plugins_wasm` |
| `Theme` | TUI color palette | `UiTheme` trait, `ThemePalette` | `ruvi_core::traits`, `ruvicode::slices::tui` |
| `Voice` | Local audio transcription | `SpeechTranscriber` trait, `WhisperCliTranscriber` | `ruvi_core::traits`, `ruvicode::slices::voice` |
| Local Runtime (native) | Local LLM engine & installer (`ollama`, `llama.cpp`) | `LocalRuntimeManager` trait | `ruvi_core::traits`, `ruvicode::slices::local_llm` |
| Bot Channel (native) | External messaging bridge (`Telegram`, `Discord`) | `ChannelGateway` trait | `ruvi_core::traits`, `ruvicode::slices::bots` |
