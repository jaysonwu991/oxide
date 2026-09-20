# Contributing to oxide

Thanks for your interest in improving oxide. This document covers the local
workflow, project layout, and conventions. For a user-facing overview see
[README.md](README.md).

## Prerequisites

- A stable Rust toolchain (edition 2021) with `rustfmt` and `clippy`.
- `git`.
- Optional, only for the features that use them:
  - `bun` or `node` for plugin development.
  - `ripgrep` (`rg`) to accelerate `grep`; without it oxide uses its built-in
    parallel walker.
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
`cargo test` all pass. CI checks formatting with `cargo fmt --all -- --check`,
runs clippy and tests with `--locked`, and builds the release profile on Linux,
macOS, and Windows.

## Project layout

| Path | Responsibility |
| --- | --- |
| `src/main.rs` | CLI entry (clap), non-interactive `-p/--print`, `--mode json`, and `--mode rpc` modes, TUI dispatch, and the `mcp`, `sessions`, `plugin`, and `uninstall` subcommands. |
| `src/cli.rs` | Non-interactive surface: `@file` expansion, tool filtering, JSON/RPC event framing. |
| `src/config.rs` | Config loading, provider presets, agent `Mode` and `Reasoning`, system prompt composition, `defaultProjectTrust`, context-compaction settings, and the context window. |
| `src/auth.rs` | Credential store backing the TUI `/login` and `/logout` commands. |
| `src/trust.rs` | Project trust: per-directory decisions and gating of project-local resources. |
| `src/theme.rs` | Semantic TUI color themes (built-in and custom JSON), including focus, speaker, success, tool, error, supporting-text, border, and tool background roles. |
| `src/agent.rs` | Agent loop, parallel/sequential tool execution, steering and follow-up queues, terminate hint, and the agent-level tools. |
| `src/llm/` | Model clients: OpenAI-compatible and Anthropic, including reasoning effort and Anthropic adaptive/extended thinking. |
| `src/tools.rs` | Built-in tool specs and execution; `ToolOutput` (text/media/diff/terminate), streaming `Progress`, output truncation (line/byte caps, bash tail, saved full output), and `grep` (prefers `ripgrep`, with a parallel built-in fallback). |
| `src/mcp.rs` | MCP runtime and remote tool exposure. |
| `src/mcp_config.rs` | `oxide mcp` CLI: read/write MCP servers in `.oxide/mcp.json`, including OAuth fields and `oxide mcp auth`. |
| `src/mcp_oauth.rs` | OAuth authorization-code + PKCE flow for remote MCP servers. |
| `src/ecosystem/` | Discovery of the Oxide and Claude Code config ecosystems, including context files, prompt templates, and trust-gated project resources. |
| `src/permission.rs` | Permission rule parsing and decisions, including `build`/`plan`/`auto-edit` mode overrides. |
| `src/session.rs` | Pi-compatible JSONL session trees (`id`/`parentId` entries, compaction, branch summaries) and forking (`/fork`, `/clone`). |
| `src/snapshots.rs` | Shadow-git snapshots backing `/undo` and `/redo`. |
| `src/compact.rs` | Pi-style context compaction and branch summarization. |
| `src/pricing.rs` | Model price table for the footer's `$cost` segment, overlaid by `modelPrices` in `settings.json`. |
| `src/diff.rs` | Dependency-free LCS line diff for edit previews (context windows and gap markers). |
| `src/html.rs` | Dependency-free HTML to Markdown/plain-text conversion for `webfetch`. |
| `src/lsp.rs` | Minimal LSP client and diagnostics. |
| `src/plugin.rs` | Plugin host and hooks: `tool.execute.before`/`after` (output rewriting, terminate hint) and `status` for footer statuses. |
| `src/plugin_registry.rs` | Claude Code-style plugin packages and marketplaces: `oxide plugin`/`/plugin` install lifecycle, manifests (`.oxide/*.json` preferred, `.claude-plugin/*.json` compatible), and hook-shim generation. |
| `src/memory.rs` | Cross-session memory store. |
| `src/media.rs` | Image/PDF attachments and `@path` references. |
| `src/uninstall.rs` | `oxide uninstall` install detection and cleanup. |
| `src/tui/` | ratatui + crossterm interface with incremental rendering, a two-column welcome banner, a live state row, a Pi-style footer (path/branch/session, cumulative tokens with cache and cost, context `%`/window, model/thinking, and plugin statuses), a growing labeled editor, background-filled tool panels (Ctrl+O collapses; state-colored with hanging-indented wrapped output, blank line before the body and `Took`), inline user/assistant labels, `read` bodies, colored edit diffs, Shift+Tab mode and Ctrl+R reasoning cycling, theme-aware project-trust/provider dialogs, and mid-run steering. |
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
  parallel. Agent-level tools (`task`, `skill`, `memory`, `diagnostics`) are
  defined and dispatched in `src/agent.rs`.
- **Sessions and context.** Session trees, entry appends, leaf-path context
  building, and compaction/branch-summary entries live in `src/session.rs`;
  summarization lives in `src/compact.rs`. The model sees `SessionLog::messages`
  (the leaf path with the latest compaction applied).
- **Providers.** Add a preset in `ProviderPreset::for_name` in `src/config.rs`
  and, if the API is not OpenAI-compatible, extend the dispatch in
  `src/llm/client.rs` (see `src/llm/anthropic.rs`). OpenAI-compatible providers
  map explicit reasoning levels to `reasoning_effort`; Portkey-hosted newer
  Claude models and the Anthropic client use adaptive thinking where supported,
  with an Anthropic extended-thinking `budget_tokens` fallback for older
  models.
- **Ecosystem sources.** Parsing lives in `src/ecosystem/mod.rs`; frontmatter
  handling is in `src/ecosystem/frontmatter.rs`. The native Oxide layout
  (`.oxide/`, `AGENTS.md`) and Claude Code layout (`.claude/`, `CLAUDE.md`,
  `.mcp.json`) are supported at project and global scope. Claude-compatible
  entries load first so native Oxide entries win on name collisions. Context
  files are collected by walking ancestor directories; `ecosystem::load_opts`
  controls whether project resources load.
- **Project trust.** `src/trust.rs` owns the decision store and resource
  detection; untrusted runs reload via `Config::reload_ecosystem`.
- **Footer and usage.** `App` accumulates cumulative usage from
  `AgentEvent::Usage` and reloads it from `SessionLog::usage_totals` on resume;
  `src/tui/ui.rs::draw_footer` renders it. Model prices live in `src/pricing.rs`
  (built-in table plus `modelPrices` in settings).
- **Plugin hooks.** Handlers are dispatched by name in the embedded harness in
  `src/plugin.rs`; add one by exposing it on the plugin object and calling
  `Host::call` from a `PluginHost` method (see `status`).
- **Themes.** Add a slot in `Theme`/`ThemeFile` in `src/theme.rs` and use it from
  `src/tui/ui.rs` via `app.theme`. Keep state understandable without color,
  preserve readable dark/light defaults, document the slot in the README and
  configuration guide, and invalidate the render cache when a visual setting
  changes.

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
