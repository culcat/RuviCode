# AGENTS.md — RuviCode Harness Entrypoint

Follow **[agent.md](./agent.md)** as the authoritative architectural specification for this repository.

## Non-Negotiable Rules

1. **Language:** Communicate with the user in **Russian**. Write all code, comments, documentation, commits, and PRs in **English**.
2. **Git Flow:** Use `master`, `dev`, `Feature/<name>`, `Fix/<name>`, `Release/<version>`, `Hotfix/<name>`. Always merge completed work into `dev` with `--no-ff`.
3. **Commits:** Conventional Commits **without scope** (`feat: ...`, `fix: ...`, `refactor: ...`, `chore: ...`, `docs: ...`, `test: ...`).
4. **Architecture:** Follow **SOLID** (especially **Open/Closed Principle** via `crates/ruvi-core` traits and `ExtensionRegistry`) and **Vertical Slice Architecture** (`crates/ruvicode/src/slices/*`). Never add provider/tool switch statements to `ruvi-core`.
5. **Rust Hygiene:** `edition = "2024"`, `rust-version = "1.85"`, `clippy::all` + `clippy::pedantic` clean, no `.unwrap()` in production code, `thiserror` in libraries, `anyhow` only in `main.rs`.
6. **Project Skills:** Apply `.agents/skills/ponytail/SKILL.md` (minimal complete code, no bloat) and `.agents/skills/rust-anything/SKILL.md` (unit tracking in `rust/MAP.md`, `rust/DECISIONS.md`, `rust/GLOSSARY.md`).
