# Contributing to oxide

Thanks for your interest in improving oxide. This document covers the local
workflow, project layout, and conventions. For a user-facing overview see
[README.md](README.md).

## Prerequisites

- A stable Rust toolchain (edition 2021) with `rustfmt` and `clippy`.
- `git`.
- Optional, only for the features that use them:
  - `bun` or `node` for plugin development.
  - Language servers for the `diagnostics` tool: `rust-analyzer`,
    `typescript-language-server`, `pyright`, or `gopls`.

## Getting started

```sh
git clone https://github.com/jaysonwu991/oxide.git
cd oxide
cargo build
cargo test
```

Run the binary against a project:

```sh
cargo run -- -p "summarize this repository"
```

## Commands

| Command | Purpose |
| --- | --- |
| `cargo build` | Debug build. |
| `cargo test` | Run the test suite. |
| `cargo clippy --all-targets -- -D warnings` | Lint; warnings are errors. |
| `cargo fmt` | Format the code. |

Before opening a pull request, make sure `cargo fmt`, `cargo clippy`, and
`cargo test` all pass. CI runs the same checks.

## Project layout

| Path | Responsibility |
| --- | --- |
| `src/main.rs` | CLI entry (clap), `-p/--print` mode, TUI dispatch. |
| `src/config.rs` | Config loading, provider presets, system prompt composition. |
| `src/auth.rs` | `auth login` / `list` / `logout` and the credential store. |
| `src/agent.rs` | Agent loop, tool execution, and the agent-level tools. |
| `src/llm/` | Model clients: OpenAI-compatible and Anthropic. |
| `src/tools.rs` | Built-in tool specs and execution. |
| `src/mcp.rs` | MCP runtime and remote tool exposure. |
| `src/ecosystem/` | Discovery of the Oxide and Claude Code config ecosystems. |
| `src/permission.rs` | Permission rule parsing and decisions. |
| `src/session.rs` | Durable JSONL session log. |
| `src/snapshots.rs` | Shadow-git snapshots backing `/undo` and `/redo`. |
| `src/compact.rs` | Conversation summarization. |
| `src/lsp.rs` | Minimal LSP client and diagnostics. |
| `src/plugin.rs` | Plugin host and tool hooks. |
| `src/memory.rs` | Cross-session memory store. |
| `src/media.rs` | Image/PDF attachments and `@path` references. |
| `src/tui/` | ratatui + crossterm interface. |
| `.oxide/` | Project agents, commands, skills, and plugins (Oxide layout). |

## Conventions

- Rust 2021, 4-space indent, default `rustfmt` style.
- Use `anyhow::Result` with `.context(...)` / `.with_context(...)` for
  human-readable errors.
- Do not add comments unless they explain non-obvious intent.
- Keep changes small and focused; verify with `cargo test` and `cargo clippy`.
- Add tests near the code they cover using `#[cfg(test)]` modules.

## Extending oxide

- **Tools.** Register built-in tools in `tools::specs(&McpRegistry)` and
  dispatch them in `tools::execute(call, cwd, &McpRegistry)`. Agent-level tools
  (`task`, `skill`, `memory`, `diagnostics`) are defined and dispatched in
  `src/agent.rs`.
- **Providers.** Add a preset in `ProviderPreset::for_name` in `src/config.rs`
  and, if the API is not OpenAI-compatible, extend the dispatch in
  `src/llm/client.rs` (see `src/llm/anthropic.rs`).
- **Ecosystem sources.** Parsing lives in `src/ecosystem/mod.rs`; frontmatter
  handling is in `src/ecosystem/frontmatter.rs`. The native Oxide layout
  (`.oxide/`, `AGENTS.md`) is read first and the Claude Code layout
  (`.claude/`, `CLAUDE.md`, `.mcp.json`) is supported for compatibility, both at
  project and global scope.

## Commits and pull requests

- Make focused commits with clear, imperative messages.
- Describe the motivation and behavior change in the PR body.
- Include tests for bug fixes and new behavior where practical.
- Ensure CI is green before requesting review.

## Releases

Releases are automated by GitHub Actions:

1. `ci.yml` runs formatting, clippy, tests, and a release build on pushes to
   `main` and on pull requests.
2. `release.yml` triggers on `v*` tags, builds the supported targets, packages
   each binary with a `.sha256` checksum, and publishes a GitHub Release with
   `install.sh` attached.

To cut a release:

```sh
git tag v0.1.0
git push origin v0.1.0
```

The version in `Cargo.toml` should match the tag.
