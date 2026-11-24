# Architecture Decision Records (`DECISIONS.md`)

> Maintained per `rust-anything` whole-app porting methodology.

## ADR-001: Vertical Slice Architecture + Open/Closed Trait Kernel
- **Status:** accepted (2026-11-24)
- **Context:** RuviCode needs extreme modularity so providers, local LLM runtimes, speech transcribers, harness config loaders, plugins, UI themes, and chat bots can be replaced or extended independently without modifying core code.
- **Decision:** Split the workspace into `crates/ruvi-core` (pure domain models, `CoreError`, `EventBus`, segregated `Send + Sync` object-safe traits, `ExtensionRegistry`) and `crates/ruvicode/src/slices/*` (self-contained vertical slices depending inward on `ruvi-core`).
- **Consequences:** Adding a new LLM provider, bot gateway, or theme requires implementing a trait and registering it in `ExtensionRegistry` with zero edits to `ruvi-core`.

## ADR-002: Ratatui + Crossterm Replacing OpenCode Solid/TS TUI
- **Status:** accepted (2026-11-24)
- **Context:** Upstream `anomalyco/opencode` implements `packages/tui` using TypeScript/Solid (`@opentui`). RuviCode targets a single fast native Rust binary (`ruvi`).
- **Decision:** Replace `packages/tui` with `ratatui` `0.30` and `crossterm` `0.29` inside `slices/tui`, preserving OpenCode's signature layout: header with `[BUILD]`/`[PLAN]` mode pill, centered hero welcome view, scrollable timeline, collapsible context sidebar, command palette, model selector, and theme switching (`UiTheme`).
- **Consequences:** Eliminates Node/Bun runtime overhead and allows headless `TestBackend` verification in `cargo test`.

## ADR-003: Hybrid Rust Trait Registry + Sandboxed WASM Plugins + MCP
- **Status:** accepted (2026-11-24)
- **Context:** Users must be able to extend and replace components dynamically without recompiling RuviCode while keeping compiled-in extensions zero-overhead.
- **Decision:** Combine compile-time/runtime Rust trait registration (`ExtensionRegistry`) with sandboxed WebAssembly plugins hosted via `wasmi` `2.0` (`wat` + `.wasm` support, extensible to `extism` / component model) and external MCP tool servers.
- **Consequences:** Sandboxed user plugins in `.ruvi/plugins` and `~/.config/ruvi/plugins` load safely at startup and expose `AgentTool` implementations seamlessly.

## ADR-004: Cross-Harness Config, Rules, Skills, and MCP Discovery
- **Status:** accepted (2026-11-24)
- **Context:** Users migrate from OpenCode, Claude Code, Cursor, or Antigravity and expect existing project instructions, skills, and MCP configs to work out of the box.
- **Decision:** Implement `MultiHarnessContextLoader` (`ContextLoader` trait) to discover and merge `agent.md`, `AGENTS.md`, `CLAUDE.md`, `.agents/rules/*.md`, `.cursor/rules/*.md`, `*/skills/*/SKILL.md` across `.agents`, `.claude`, `.opencode`, plus `.mcp.json` and `opencode.json`.
- **Consequences:** RuviCode reads both its own native conventions and existing multi-harness repositories immediately.

## ADR-005: Native Local AI (Ollama, llama.cpp, Whisper) & Bot Gateways (Telegram, Discord)
- **Status:** accepted (2026-11-24)
- **Context:** Offline local execution, in-UI installation, voice input, and remote chat access via Telegram/Discord are first-class RuviCode features not present natively in upstream OpenCode TUI.
- **Decision:** Model local runtime management (`LocalRuntimeManager`), speech transcription (`SpeechTranscriber`), and messaging bridges (`ChannelGateway`) as OCP slices (`local_llm`, `voice`, `bots`) wired to `EventBus` and surfaced inside the TUI (`/local`, `/voice`, `/bots`).
- **Consequences:** Users can inspect, install, and pull local models or wire Telegram/Discord bots without leaving the harness contracts.
