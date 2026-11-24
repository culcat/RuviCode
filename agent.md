# RuviCode Architectural Conventions (`agent.md`)

> **Repository:** <https://github.com/culcat/RuviCode>
> **Language:** Rust (Edition 2024, MSRV 1.85)

---

## 1. Vision & Scope

**RuviCode** (`ruvi`) is a modular AI Agent Harness written in Rust with an OpenCode-inspired terminal user interface (`ratatui` + `crossterm`).

Core capabilities:
- **Interactive Agent Chat:** Multi-turn streaming/async conversations with `[BUILD]` and `[PLAN]` modes (`Tab` to toggle).
- **Multi-Provider LLM Support:** Unified OpenAI-compatible and custom provider abstraction (Ollama, `llama.cpp`, OpenAI, OpenRouter, Anthropic-compatible endpoints, plus zero-config offline demo fallback).
- **Local LLM Management & In-UI Installer:** Native status detection, model pulling, and runtime installation for `ollama` and `llama.cpp` directly from the TUI (`/local`).
- **Local Voice Input (STT):** Native local Whisper integration (`whisper-cli` / `whisper`) with in-UI installation support.
- **Cross-Harness Compatibility:** Automatic discovery of rules, skills, agent configs, and MCP server definitions compatible with other harnesses (`agent.md`, `AGENTS.md`, `CLAUDE.md`, `opencode.json`, `.mcp.json`, `.agents/skills/`, `.claude/skills/`, `.opencode/skills/`).
- **Modular Plugin System:** Every subsystem is replaceable through Rust trait contracts and sandboxed WASM user plugins (`wasmi` / `.wat` & `.wasm`, extensible toward Extism/WIT).
- **Native Bot Channels:** Built-in Telegram and Discord bot gateways bridging external chat channels to the same agent core via `EventBus`.

---

## 2. Communication & Commit Rules

- **User Communication:** Always communicate with the user in **Russian**.
- **Code & Technical Artifacts:** All source code, docstrings, comments, architectural docs, commit messages, and pull requests MUST be written in **English**.
- **Commit Format:** Strict Conventional Commits **without scope**:
  - `feat: <description>`
  - `fix: <description>`
  - `refactor: <description>`
  - `chore: <description>`
  - `docs: <description>`
  - `test: <description>`

---

## 3. Git Flow Workflow

All repository work follows **Git Flow**:

- `master` — Production-ready releases only. Never commit feature work directly here.
- `dev` — Main integration branch. Every completed feature or fix merges here with `--no-ff`.
- `Feature/<name>` — Branched from `dev` for any new capability or vertical slice (e.g., `Feature/core-kernel`, `Feature/mvp-vertical-slices`).
- `Fix/<name>` — Branched from `dev` for bug fixes.
- `Release/<version>` — Branched from `dev` when preparing a release, merged into `master` and `dev`.
- `Hotfix/<name>` — Branched from `master` for urgent production patches, merged into both `master` and `dev`.

Merge discipline:
```bash
git checkout dev
git merge --no-ff Feature/<name> -m "chore: merge Feature/<name> into dev"
```

---

## 4. SOLID Principles & Open/Closed Principle (OCP)

RuviCode strictly enforces **SOLID**, placing primary emphasis on the **Open/Closed Principle**:

- **`crates/ruvi-core` is CLOSED for modification** when adding new LLM providers, agent tools, speech transcribers, local LLM runtimes, harness context loaders, bot channels, UI themes, or plugin hosts.
- **`crates/ruvi-core` is OPEN for extension** through segregated, object-safe `Send + Sync` traits registered in `ExtensionRegistry`:
  - `LlmProvider` — completion & model listing
  - `AgentTool` — tool schema & execution
  - `SpeechTranscriber` — audio-to-text transcription
  - `LocalRuntimeManager` — local LLM lifecycle & installation
  - `ContextLoader` — discovery of rules, skills, and MCP configs
  - `ChannelGateway` — external messaging bots (Telegram, Discord, etc.)
  - `PluginHost` — runtime discovery/loading of user plugins
  - `UiTheme` — TUI color palettes
- **Forbidden Anti-Pattern:** Never add hardcoded `match provider_name` or `match channel_name` dispatch branches in core orchestration (`ChatEngine`, `ExtensionRegistry`, or `ruvi-core`). Always register a trait implementation by ID and resolve dynamically.
- **Dependency Inversion:** High-level domain logic depends exclusively on `ruvi-core` traits; concrete HTTP clients, subprocess wrappers, WASM runtimes, and terminal backends depend inward on `ruvi-core`.

---

## 5. Vertical Slice Architecture

The workspace separates pure domain contracts from self-contained vertical feature slices:

```text
crates/
├── ruvi-core/                 # Pure domain models, CoreError, EventBus, OCP traits, ExtensionRegistry
│                              # Zero HTTP, TUI, WASM, or bot SDK dependencies.
└── ruvicode/                  # Application binary (`ruvi`) organized by Vertical Slice
    └── src/
        ├── main.rs            # CLI entrypoint (`clap`) + TUI/command dispatch
        ├── bootstrap.rs       # Composition root wiring slices into ExtensionRegistry
        └── slices/
            ├── chat/          # ChatEngine, session state, Build/Plan mode prompt assembly
            ├── providers/     # OpenAI-compatible HTTP adapter & zero-config fallback provider
            ├── local_llm/     # Ollama & llama.cpp status, model pull, and installer
            ├── voice/         # Local Whisper STT adapter & installer
            ├── harness_compat/# Multi-harness Rules / Skills / MCP discovery
            ├── plugins_wasm/  # Sandboxed WASM/WAT PluginHost (`wasmi`) -> AgentTool bridge
            ├── bots/          # Telegram & Discord ChannelGateway adapters
            └── tui/           # OpenCode-style Ratatui UI, modals, themes, async event loop
```

Slice boundaries:
1. Each vertical slice owns its configuration parsing, adapter types, domain mapping, and unit tests.
2. Slices **must not** reach into private internals of sibling slices.
3. Cross-slice coordination flows exclusively through `ruvi-core` traits, `ExtensionRegistry`, and `EventBus` (`AppEvent`).

---

## 6. Rust Engineering Standards

- **Toolchain:** `edition = "2024"`, `rust-version = "1.85"`.
- **Workspace Lints:**
  - `unsafe_code = "warn"` (every `unsafe` block requires a `// SAFETY:` justification).
  - `clippy::all = "warn"` and `clippy::pedantic = "warn"`. Fix warnings cleanly at the source; do not blanket-suppress lints.
- **Error Handling:**
  - Use `thiserror` (`CoreError` / `CoreResult<T>`) across core and slice boundaries.
  - Use `anyhow::Result` only at the CLI/binary entrypoint (`main.rs`).
  - **Zero `.unwrap()`** in production/library code; `.expect(...)` only for statically verified invariants or tests.
- **Concurrency:** Use `tokio` for async I/O (`LlmProvider`, `ChannelGateway`, `LocalRuntimeManager`, TUI event loop). Never hold mutex guards across `.await` points.

---

## 7. Active Development Skills (`ponytail` & `rust-anything`)

Project skills live in `.agents/skills/`:
- **`ponytail` / `ponytail-review`:** Pragmatic minimalism — build the smallest complete, rock-solid implementation without speculative wrappers, dead abstractions, or unrequested bloat. Every non-trivial slice leaves focused self-tests.
- **`rust-anything`:** Structured OpenCode-to-Rust porting methodology tracked in `rust/MAP.md`, `rust/DECISIONS.md`, and `rust/GLOSSARY.md`. Every ported or native unit moves cleanly through design decisions, trait bridging, implementation, and verified gating (`cargo fmt`, `cargo clippy`, `cargo test`).
