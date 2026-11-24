# RuviCode (`ruvi`)

A modular, OpenCode-inspired AI Agent Harness written in Rust.

## Features

- **OpenCode-Style TUI (`ratatui` + `crossterm`):** Centered hero welcome screen, interactive chat timeline, `[BUILD]` / `[PLAN]` mode toggle (`Tab`), collapsible context sidebar (`Ctrl+B`), command palette (`Ctrl+P`), model picker (`/models`), and swappable themes (`/theme`).
- **Multi-Provider LLM Engine:** OpenAI-compatible adapter supporting Ollama, `llama.cpp`, OpenAI, OpenRouter, Anthropic-compatible endpoints, plus a zero-config `ruvi-demo` offline fallback.
- **In-UI Local Runtime Installer (`/local`):** Detect and install `ollama`, `llama.cpp`, and `whisper-cpp`, or pull local models directly from the TUI.
- **Local Voice Input:** Whisper CLI speech-to-text integration via `SpeechTranscriber` (`/voice`).
- **Cross-Harness Config Discovery:** Loads `agent.md`, `AGENTS.md`, `CLAUDE.md`, `.agents/rules/*.md`, `.agents/skills/*/SKILL.md`, `.claude/skills/*/SKILL.md`, `.opencode/skills/*/SKILL.md`, `.mcp.json`, and `opencode.json`.
- **Modular Plugin Architecture (OCP + WASM):** Extend or replace any subsystem via `ruvi-core` traits or drop sandboxed `.wasm` / `.wat` plugins into `.ruvi/plugins/`.
- **Native Bot Gateways:** Built-in Telegram and Discord adapters connected through `EventBus`.

## Architecture

See [agent.md](./agent.md) for full SOLID/OCP and Vertical Slice conventions, and [rust/MAP.md](./rust/MAP.md) for the unit-by-unit OpenCode porting roadmap.

```text
ruvi-core (Domain, Events, OCP Traits, ExtensionRegistry)
    ▲
    │ depends inward
ruvicode/src/slices/
  ├── chat/           ├── providers/       ├── local_llm/
  ├── voice/          ├── harness_compat/  ├── plugins_wasm/
  ├── bots/           └── tui/
```

## Quickstart

```bash
# Launch interactive OpenCode-style TUI
cargo run -p ruvicode

# Inspect discovered rules, skills, MCP servers, local runtimes, plugins, and bots
cargo run -p ruvicode -- doctor

# Execute a single-shot prompt from CLI
cargo run -p ruvicode -- prompt "Summarize active skills" --provider ruvi-demo --mode plan
```

## Keybindings (TUI)

| Key | Action |
|-----|--------|
| `Enter` | Send prompt or execute `/command` |
| `Tab` | Toggle `[BUILD]` / `[PLAN]` mode |
| `Ctrl+P` / `/help` | Open Command Palette |
| `Ctrl+L` / `/local` | Open Local AI Installer (`ollama`, `llama.cpp`, `whisper-cpp`) |
| `Ctrl+B` | Toggle right context sidebar |
| `Up` / `Down` / `PgUp` / `PgDn` | Scroll chat timeline or navigate modals |
| `Esc` | Close active modal |
| `Ctrl+Q` / `Ctrl+C` | Quit RuviCode |

## Contributing (Git Flow)

- `master`: stable releases
- `dev`: integration branch
- `Feature/<name>` / `Fix/<name>`: branched from `dev`, merged back via `git merge --no-ff`
- Commit format: Conventional Commits without scope (e.g., `feat: add ollama streaming adapter`)
