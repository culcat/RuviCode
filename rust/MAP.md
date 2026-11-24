# RuviCode Port & Feature Map (`MAP.md`)

> Follows `rust-anything` wave tracking alongside RuviCode Git Flow branches (`Feature/*`).
> Note: Upstream `anomalyco/opencode` serves as reference architecture; `packages/tui` is intentionally replaced by native `ratatui` + `crossterm` (`ADR-002`).

## Wave 1 — Foundation & Core Kernel (`Feature/core-kernel`)
| Unit / Reference | RuviCode Target | State | Notes |
|---|---|---|---|
| Domain types, `bus.ts`, `agent.ts` | `crates/ruvi-core/src/{domain,bus,error}.rs` | `done` | Pure domain structs, `SessionId`, `AgentMode`, `EventBus` |
| Extension contracts (`provider.ts`, `tool.ts`, `plugin.ts`) | `crates/ruvi-core/src/{traits,registry}.rs` | `done` | OCP `ExtensionRegistry` & segregated object-safe traits |

## Wave 2 — Core Vertical Slices & OpenCode-Style TUI MVP (`Feature/mvp-vertical-slices`)
| Unit / Reference | RuviCode Target | State | Notes |
|---|---|---|---|
| `packages/core/src/session.ts` | `slices/chat/` | `done` | `ChatEngine` with `Build`/`Plan` prompt assembly |
| `packages/core/src/provider.ts` | `slices/providers/` | `done` | `OpenAiCompatibleProvider` (Ollama, llama.cpp, OpenAI, OpenRouter, Anthropic-compat) + `LocalEchoProvider` |
| RuviCode native | `slices/local_llm/` | `done` | `OllamaRuntimeManager`, `LlamaCppRuntimeManager` + in-UI installer |
| `packages/core/src/voice.ts` | `slices/voice/` | `done` | `WhisperCliTranscriber` + install command support |
| `packages/core/src/{skill,instruction-discovery,mcp}.ts` | `slices/harness_compat/` | `done` | Multi-harness Rules / Skills / MCP loader |
| `packages/core/src/plugin.ts` | `slices/plugins_wasm/` | `done` | `WasmiPluginHost` loading `.wasm`/`.wat` as `AgentTool` |
| RuviCode native | `slices/bots/` | `done` | `TelegramBotGateway` & `DiscordBotGateway` |
| `packages/tui/src/{app,routes,component,theme}` | `slices/tui/` | `done` | OpenCode-style Ratatui TUI + `TestBackend` tests |

## Wave 3 — Post-MVP Hardening & Deep Parity (`Future Feature/*` branches)
| Unit / Reference | Planned Branch | State | Notes |
|---|---|---|---|
| Streaming SSE token chunks & tool-call loop | `Feature/streaming-tool-loop` | `done` | Multi-step agentic tool execution + streaming tokens + built-in `read_file`/`list_dir` tools |
| SQLite persistent session history (`database/`) | `Feature/session-persistence` | `done` | `SessionStore` OCP trait + `JsonSessionStore` (`.ruvi/sessions/*.json`), `/sessions`, `--resume` |
| Live MCP stdio/SSE client transport (`mcp/`) | `Feature/mcp-client-runtime` | `done` | Live stdio JSON-RPC 2.0 MCP client registering `mcp__<srv>__<tool>` as `AgentTool` |
| Live audio capture (`cpal`) -> Whisper STT | `Feature/voice-mic-capture` | `open` | Direct microphone push-to-talk in TUI |
| Extism / Component-Model WASM plugin ABI v2 | `Feature/wasm-plugin-abi-v2` | `open` | Rich host functions & UI panel extensions |

## Using `rust-anything` CLI (Optional)
If porting specific reference utilities from TypeScript/Python into standalone crates, run:
```bash
cargo install rust-anything
rust-anything doctor
```
