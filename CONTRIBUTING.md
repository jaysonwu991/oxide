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
| `src/main.rs` | CLI entry (clap), non-interactive `-p/--print`, `--mode json`, and `--mode rpc` modes, TUI dispatch, and the `mcp` subcommand. |
| `src/cli.rs` | Non-interactive surface: `@file` expansion, tool filtering, JSON/RPC event framing. |
| `src/config.rs` | Config loading, provider presets, agent `Mode` and `Reasoning`, system prompt composition, and `defaultProjectTrust`. |
| `src/auth.rs` | Credential store backing the TUI `/login` and `/logout` commands. |
| `src/trust.rs` | Project trust: per-directory decisions and gating of project-local resources. |
| `src/theme.rs` | TUI color themes (built-in and custom JSON). |
| `src/agent.rs` | Agent loop, parallel/sequential tool execution, steering and follow-up queues, terminate hint, and the agent-level tools. |
| `src/llm/` | Model clients: OpenAI-compatible and Anthropic, including reasoning effort / extended thinking. |
| `src/tools.rs` | Built-in tool specs and execution; `ToolOutput` (text/media/diff/terminate), streaming `Progress`, and output truncation (line/byte caps, bash tail, saved full output). |
| `src/mcp.rs` | MCP runtime and remote tool exposure. |
| `src/mcp_config.rs` | `oxide mcp` CLI: read/write MCP servers in `.oxide/mcp.json`, including OAuth fields and `oxide mcp auth`. |
| `src/mcp_oauth.rs` | OAuth authorization-code + PKCE flow for remote MCP servers. |
| `src/ecosystem/` | Discovery of the Oxide and Claude Code config ecosystems, including context files, prompt templates, and trust-gated project resources. |
| `src/permission.rs` | Permission rule parsing and decisions, including `build`/`plan`/`auto-edit` mode overrides. |
| `src/session.rs` | Durable JSONL session log and forking (`/fork`, `/clone`). |
| `src/snapshots.rs` | Shadow-git snapshots backing `/undo` and `/redo`. |
| `src/compact.rs` | Conversation summarization. |
| `src/dcp.rs` | Dynamic context pruning: config, pruned view, nudges, compression records. |
| `src/diff.rs` | Dependency-free LCS line diff for edit previews (context windows and gap markers). |
| `src/html.rs` | Dependency-free HTML to Markdown/plain-text conversion for `webfetch`. |
| `src/lsp.rs` | Minimal LSP client and diagnostics. |
| `src/plugin.rs` | Plugin host and tool hooks, including output rewriting and the terminate hint. |
| `src/memory.rs` | Cross-session memory store. |
| `src/media.rs` | Image/PDF attachments and `@path` references. |
| `src/tui/` | ratatui + crossterm interface with incremental rendering, a Pi-style footer/status row, hidden-by-default tool output (Ctrl+O), `$ command` shell display, colored edit diffs, per-turn thought timing, Shift+Tab mode and Ctrl+R reasoning cycling, project-trust and theme dialogs, and mid-run steering. |
| `.oxide/` | Project agents, commands, prompts, skills, and plugins (Oxide layout). |

## Conventions

- Rust 2021, 4-space indent, default `rustfmt` style.
- Use `anyhow::Result` with `.context(...)` / `.with_context(...)` for
  human-readable errors.
- Do not add comments unless they explain non-obvious intent.
- Keep changes small and focused; verify with `cargo test` and `cargo clippy`.
- Add tests near the code they cover using `#[cfg(test)]` modules.

## Extending oxide

- **Tools.** Register built-in tools in `tools::specs(&McpRegistry)` and
  dispatch them in `tools::execute(call, cwd, &McpRegistry, &Progress)`; report
  incremental output through `Progress` and set `ToolOutput::terminate` to end
  the turn. Tool names are canonicalized with `tools::canonical_tool_name`, which
  accepts Pi names (`read`, `write`, `edit`, `ls`, `find`) and legacy aliases
  (`read_file`, `write_file`, `patch`, `list_dir`, `glob`); permission rules,
  concurrency checks, and TUI rendering must go through it. Read-only tools are
  listed in the `concurrency_safe` classifier in `src/agent.rs` to run in
  parallel. Agent-level tools (`task`, `skill`, `memory`, `diagnostics`,
  `compress`) are defined and dispatched in `src/agent.rs`; `compress` and the
  pruned request view live in `src/dcp.rs`.
- **Context pruning.** Configuration, deduplication, error purging, and nudges
  live in `src/dcp.rs`; compression records are persisted through
  `SessionLog::append_dcp` / `dcp_state`. The raw history is never modified, only
  the outgoing request.
- **Providers.** Add a preset in `ProviderPreset::for_name` in `src/config.rs`
  and, if the API is not OpenAI-compatible, extend the dispatch in
  `src/llm/client.rs` (see `src/llm/anthropic.rs`). Reasoning levels come from
  `Config::effective_reasoning`; OpenAI maps them to `reasoning_effort`
  (`src/llm/client.rs`) and Anthropic to extended-thinking `budget_tokens`
  (`src/llm/anthropic.rs`).
- **Ecosystem sources.** Parsing lives in `src/ecosystem/mod.rs`; frontmatter
  handling is in `src/ecosystem/frontmatter.rs`. The native Oxide layout
  (`.oxide/`, `AGENTS.md`) is read first and the Claude Code layout
  (`.claude/`, `CLAUDE.md`, `.mcp.json`) is supported for compatibility, both at
  project and global scope. Context files are collected by walking ancestor
  directories; `ecosystem::load_opts` controls whether project resources load.
- **Project trust.** `src/trust.rs` owns the decision store and resource
  detection; untrusted runs reload via `Config::reload_ecosystem`.
- **Themes.** Add a slot in `Theme`/`ThemeFile` in `src/theme.rs` and use it from
  `src/tui/ui.rs` via `app.theme`.

## Commits and pull requests

- Make focused commits with clear, imperative messages.
- Describe the motivation and behavior change in the PR body.
- Include tests for bug fixes and new behavior where practical.
- Ensure CI is green before requesting review.

## Releases

Releases are automated by GitHub Actions:

1. `ci.yml` runs formatting, clippy, tests, and a release build on pushes to
   `main` and on pull requests.
2. `release.yml` triggers on `v*` tags, builds the supported targets (including
   `x86_64-pc-windows-msvc`), packages each binary with a `.sha256` checksum,
   and publishes a GitHub Release with `install.sh` and `install.ps1` attached.

To cut a release, make sure the version in `Cargo.toml` matches the tag, then
push a `vX.Y.Z` tag:

```sh
git tag vX.Y.Z
git push origin vX.Y.Z
```
