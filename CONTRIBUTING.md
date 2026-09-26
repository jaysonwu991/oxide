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
| `cargo build -p oxide-desktop --features gui` | Desktop app (the `gui` feature is off by default so the plain build stays free of the Tauri tree). |
| `cd editors/vscode && pnpm test` | VS Code extension tests (`tsc -p .` then `node --test out/test/`). |

Before opening a pull request, make sure `cargo fmt`, `cargo clippy`, and
`cargo test` all pass. CI checks formatting with `cargo fmt --all -- --check`,
runs clippy and tests with `--locked`, and builds the release profile on Linux,
macOS, and Windows.

## Project layout

The repository is a Cargo workspace with three Cargo packages: `oxide-core`
(`crates/core`, shared agent core), `oxide` (`crates/cli`, the terminal binary:
`main.rs`, `tui/`, `theme.rs`, `uninstall.rs`), and `oxide-desktop`
(`crates/desktop`, the Tauri app, documented in
[`docs/desktop.md`](docs/desktop.md)). The VS Code extension under
`editors/vscode` is a separate pnpm/TypeScript package, not a Cargo workspace
member, documented in [`docs/vscode.md`](docs/vscode.md). Every path below is
relative to the repository root.

| Path | Responsibility |
| --- | --- |
| `crates/cli/src/main.rs` | CLI entry (clap), non-interactive `-p/--print`, `--mode json`, and `--mode rpc` modes, TUI dispatch, and the `mcp`, `sessions`, `plugin`, and `uninstall` subcommands. |
| `crates/core/src/cli.rs` | Non-interactive surface: `@file` expansion, tool filtering, JSON/RPC event framing. |
| `crates/core/src/config.rs` | Config loading, provider presets, per-provider model memory (`provider_models`), the `Reasoning` level, system prompt composition, `defaultProjectTrust`, context-compaction settings, and the context window. |
| `crates/core/src/auth.rs` | Multi-provider credential store backing the TUI `/login`, `/logout`, and `/connect` commands, with canonical provider names and aliases. |
| `crates/core/src/trust.rs` | Project trust: per-directory decisions and gating of project-local resources. |
| `crates/cli/src/theme.rs` | Semantic TUI color themes (built-in and custom JSON), including focus, speaker, success, tool, error, supporting-text, border, and tool background roles. |
| `crates/core/src/agent.rs` | Agent loop, parallel/sequential tool execution, steering and follow-up queues, terminate hint, and the agent-level tools. Tool calls, reasoning fragments (`ThinkingDelta`), stream retries (`Retrying`), subagent activity (`SubagentActivity`), usage, and finished history are reported through `AgentEvent`. |
| `crates/core/src/llm/` | Model clients: OpenAI-compatible and Anthropic, including reasoning effort, Anthropic adaptive/extended thinking, and transient-failure and empty-turn retries. |
| `crates/core/src/tools.rs` | Built-in tool specs and execution; `ToolOutput` (text/media/diff/terminate), streaming `Progress`, output truncation (line/byte caps, bash tail, saved full output), and `grep` (prefers `ripgrep`, with a parallel built-in fallback). |
| `crates/core/src/mcp.rs` | MCP runtime and remote tool exposure. |
| `crates/core/src/mcp_config.rs` | `oxide mcp` CLI: read/write MCP servers in `.oxide/mcp.json`, including OAuth fields and `oxide mcp auth`. |
| `crates/core/src/mcp_oauth.rs` | OAuth authorization-code + PKCE flow for remote MCP servers. |
| `crates/core/src/ecosystem/` | Discovery of the Oxide and Claude Code config ecosystems, including context files, prompt templates, trust-gated project resources, and plugin-packaged MCP servers (manifest `mcpServers` or a plugin-root `.mcp.json`). |
| `crates/core/src/permission.rs` | Permission rule parsing and decisions (allow/ask/deny, flat rules and wildcard patterns). |
| `crates/core/src/session.rs` | Pi-compatible JSONL session trees (`id`/`parentId` entries, compaction, branch summaries) and forking (`/fork`, `/clone`). |
| `crates/core/src/sessions.rs` | Non-interactive session management behind `oxide sessions list/delete/compact/merge`. |
| `crates/core/src/snapshots.rs` | Shadow-git snapshots backing `/undo` and `/redo`. |
| `crates/core/src/compact.rs` | Pi-style context compaction and branch summarization. |
| `crates/core/src/pricing.rs` | Model price table for the footer's `$cost` segment, overlaid by `modelPrices` in `settings.json`. |
| `crates/core/src/diff.rs` | Dependency-free LCS line diff for edit previews (context windows and gap markers). |
| `crates/core/src/html.rs` | Dependency-free HTML to Markdown/plain-text conversion for `webfetch`. |
| `crates/core/src/lsp.rs` | Minimal LSP client and diagnostics, including eviction and reconnection of a crashed server instead of reusing its broken pipe. |
| `crates/core/src/plugin.rs` | Plugin host and hooks: `tool.execute.before`/`after` (output rewriting, terminate hint) and `status` for footer statuses. |
| `crates/core/src/plugin_registry.rs` | Claude Code-style plugin packages and marketplaces: the `oxide plugin` CLI and TUI `/plugins` install lifecycle, manifests (`.oxide/*.json` preferred, `.claude-plugin/*.json` compatible), and hook-shim generation. |
| `crates/core/src/clipboard.rs` | System clipboard writes for the TUI: an OSC 52 sequence (tmux/screen-aware, so it survives SSH) plus a best-effort native helper. |
| `crates/core/src/memory.rs` | Cross-session memory store. |
| `crates/core/src/media.rs` | Image/PDF attachments and `@path` references. |
| `crates/core/src/notify.rs` | Best-effort desktop toast for a finished turn (Notification Center/`notify-send`/WinRT), with `notifyOnComplete`/`notifySound` settings. |
| `crates/core/src/runner.rs` | Shared `AgentRun` + `spawn_agent`: wires the runtime (MCP, plugins, session, snapshots, LSP), resolves attachments, and starts the agent loop for both the CLI and desktop. |
| `crates/core/src/theme_view.rs` | Built-in Dark/Light palettes (surface + semantic slots) plus `.oxide/themes/<name>.json` overrides, resolved to `#rrggbb` for the desktop front-end. |
| `crates/cli/src/uninstall.rs` | `oxide uninstall` install detection and cleanup. |
| `crates/core/src/portkey_usage.rs` | Portkey spend status bar behind `/usage`: settings in `portkey-usage.json`, spend from the Portkey analytics API. The TUI `/usage` dialog edits these settings in place. |
| `crates/cli/src/tui/` | ratatui + crossterm interface with incremental rendering, a stacked welcome banner (block-letter `OXIDE` wordmark above the ecosystem summary), a live state row, a Pi-style footer (path/branch/session, cumulative tokens with cache and cost, context `%`/window, model/thinking, and plugin statuses), a growing editor, background-filled tool panels (Ctrl+O collapses; state-colored with hanging-indented wrapped output, blank line before the body and `Took`), reasoning blocks (Ctrl+T collapses them to `✦ Thought for 1.4s`), inline user/assistant labels, `read` bodies, colored edit diffs, a dim `ChatItem::Status` tip line for idle feedback (copies, toggles), structured `ChatItem::Listing` blocks for `/mcps` and `/plugins`, modal dialogs (provider login, `/usage`, model/session pickers, marketplaces) that place a terminal cursor at the end of each input, Shift+Tab reasoning cycling, Alt+Enter follow-ups with `Alt+Up` to pull queued messages back into the editor, theme-aware project-trust/provider dialogs, and mid-run steering. |
| `crates/desktop/src/manager.rs` | Desktop multi-project state: the project registry (`desktop/projects.json`), session aggregation across projects, and the shared CLI config/trust loader. |
| `crates/desktop/src/turn.rs` | Starts an agent turn for a project (session resolution + `runner::spawn_agent`), returning the event stream, steering handles, and cancel flag. |
| `crates/desktop/src/approval.rs` / `approvals.rs` | Interactive approve/deny broker (emits `approval-request`, resolves `deny`/`once`/`always`) and the persisted per-project `allow` rules (`desktop/approvals.json`). |
| `crates/desktop/src/commands.rs` | Tauri commands (projects, sessions, turns, models, themes, providers, approvals); `crates/desktop/src/main.rs` is the `gui`-featured entry point and `ui/` the HTML/CSS/JS front-end. |
| `editors/vscode/` | VS Code extension (separate pnpm/TypeScript package): a chat webview and editor actions that drive the installed `oxide` binary as `oxide --mode json -p`; `src/core/` is webview-free and unit tested under `node --test`. |
| `.oxide/` | Project agents, commands, prompts, skills, and plugins (Oxide layout). |

## Conventions

- Rust 2021, 4-space indent, default `rustfmt` style.
- Use `anyhow::Result` with `.context(...)` / `.with_context(...)` for
  human-readable errors.
- Do not add comments unless they explain non-obvious intent.
- Keep changes small and focused; verify with `cargo test` and `cargo clippy`.
- Add tests near the code they cover using `#[cfg(test)]` modules.
- Keep streaming paths linear. Text that arrives a fragment at a time is
  accumulated in a `String` (or mutated in place when it has to live in a
  `serde_json::Value`), never rebuilt per fragment: copying the accumulated
  text on every delta is quadratic in the length of the response. For the same
  reason `read_sse` drains whole lines once per network chunk instead of after
  each one.

## Extending oxide

- **Tools.** Register built-in tools in `tools::specs(&McpRegistry)` and
  dispatch them in `tools::execute(call, cwd, &McpRegistry, &Progress)`; report
  incremental output through `Progress` and set `ToolOutput::terminate` to end
  the turn. Tool names are canonicalized with `tools::canonical_tool_name`,
  which accepts Pi names (`read`, `write`, `edit`, `ls`, `find`) and legacy
  aliases (`read_file`, `write_file`, `patch`, `list_dir`, `glob`); permission
  rules, concurrency checks, and TUI rendering must go through it. Read-only
  tools are listed in the `concurrency_safe` classifier in
  `crates/core/src/agent.rs` to run in parallel. Agent-level tools (`task`,
  `skill`, `command`, `memory`, `diagnostics`) are defined and dispatched in
  `crates/core/src/agent.rs`.
- **Sessions and context.** Session trees, entry appends, leaf-path context
  building, and compaction/branch-summary entries live in
  `crates/core/src/session.rs`; summarization lives in
  `crates/core/src/compact.rs`. The model sees `SessionLog::messages` (the leaf
  path with the latest compaction applied).
- **Providers.** Add a preset in `ProviderPreset::for_name` in
  `crates/core/src/config.rs`, the name and key URL in `KNOWN_PROVIDERS` in
  `crates/core/src/auth.rs`, and any aliases in `canonical_provider`; if the API
  is not OpenAI-compatible, extend the dispatch in
  `crates/core/src/llm/client.rs` (see `crates/core/src/llm/anthropic.rs`).
  OpenAI-compatible providers map explicit reasoning levels to
  `reasoning_effort`; Portkey-hosted newer Claude models and the Anthropic
  client use adaptive thinking where supported, with an Anthropic
  extended-thinking `budget_tokens` fallback for older models. A provider with
  no model-listing endpoint ships a fallback catalog in
  `crates/core/src/config.rs` (see `GLM_FALLBACK_MODELS`).
- **Ecosystem sources.** Parsing lives in `crates/core/src/ecosystem/mod.rs`;
  frontmatter handling is in `crates/core/src/ecosystem/frontmatter.rs`. The
  native Oxide layout (`.oxide/`, `AGENTS.md`) and Claude Code layout
  (`.claude/`, `CLAUDE.md`, `.mcp.json`) are supported at project and global
  scope. Claude-compatible entries load first so native Oxide entries win on
  name collisions. Context files are collected by walking ancestor directories;
  `ecosystem::load_opts` controls whether project resources load.
- **Project trust.** `crates/core/src/trust.rs` owns the decision store and
  resource detection; untrusted runs reload via `Config::reload_ecosystem`.
- **Footer and usage.** `App` accumulates cumulative usage from
  `AgentEvent::Usage` and reloads it from `SessionLog::usage_totals` on resume;
  `crates/cli/src/tui/ui.rs::draw_footer` renders it. Model prices live in
  `crates/core/src/pricing.rs` (built-in table plus `modelPrices` in settings).
- **Agent events.** Add a variant to `AgentEvent` in `crates/core/src/agent.rs`,
  forward it from the stream/loop, and handle it in whichever surfaces consume
  the event: the TUI in `crates/cli/src/tui/mod.rs::handle_agent_event`, the
  JSON and RPC framings in `crates/core/src/cli.rs`, and the print-mode reporter
  in `crates/cli/src/main.rs`. An event that reaches a live transcript also needs
  the mapping in `event_json` plus the desktop UI (`crates/desktop/ui/app.js`)
  and the extension's `editors/vscode/src/core/protocol.ts` (and `media/main.js`
  when it introduces a new view message). Nested subagents must forward the
  event themselves (`task_inner`/`run_subagent`) or the parent view stays silent
  until the subagent returns.
- **Plugin hooks.** Handlers are dispatched by name in the embedded harness in
  `crates/core/src/plugin.rs`; add one by exposing it on the plugin object and
  calling `Host::call` from a `PluginHost` method (see `status`).
- **Themes.** Add a slot in `Theme`/`ThemeFile` in `crates/cli/src/theme.rs` and
  use it from `crates/cli/src/tui/ui.rs` via `app.theme`. Keep state
  understandable without color, preserve readable dark/light defaults, document
  the slot in the README and `docs/cli.md`, invalidate the render cache when a
  visual setting changes, and mirror the slot in
  `crates/core/src/theme_view.rs` so the desktop palette keeps the same semantic
  names.

## Commits and pull requests

- Make focused commits with clear, imperative messages.
- Describe the motivation and behavior change in the PR body.
- Include tests for bug fixes and new behavior where practical.
- Ensure CI is green before requesting review.

## Releases

Releases are automated by GitHub Actions:

1. `ci.yml` runs on pushes to `main` and on pull requests: formatting, clippy,
   and the test suite plus a release build for the Rust workspace; a clippy and
   `gui`-feature build of the desktop app on Linux, macOS, and Windows; and the
   VS Code extension's type check, unit tests, and `vsce package`.
2. `cli.yml` triggers on `v*` tags, builds the supported CLI targets
   (including `x86_64-pc-windows-msvc`), packages each binary with a `.sha256`
   checksum, and publishes a GitHub Release with `install.sh` and `install.ps1`
   attached.
3. `desktop.yml` triggers on `desktop-v*` tags (and manually) and builds the
   macOS, Linux, and Windows desktop bundles, drafting a release.

The CLI and the desktop app release independently. A `vX.Y.Z` tag releases the
CLI only; a `desktop-vX.Y.Z` tag builds and drafts the desktop bundles. The
component that did not change keeps its previous version, so it does not need a
new tag or release.

Release notes are drafted on every push to `main`, one draft per component:
`release-drafter.cli.yml` drafts the CLI release and
`release-drafter.desktop.yml` drafts the desktop release. Each config is pinned
to its component's tag prefix (`v` and `desktop-v`), so a run only sees its own
draft and resolves its next version from its own last release; the draft it
produces already carries the tag to push. Their categories match
conventional-commit PR titles (`feat:`, `fix:`, `perf:`, …) directly and by the
type label the `release-drafter/autolabeler` step derives from that same title,
so keep the PR title in that form and no manual labelling is needed. The type
labels it applies are created by `labels.yml`.

A PR whose changes are limited to `crates/cli/` is labeled `cli`, and one
limited to `crates/desktop/` is labeled `desktop`; a change that also touches
shared code (for example `crates/core/`) or both components carries neither
label.

The repository keeps a placeholder version (`0.0.0`). On a tag, the matching
workflow runs `scripts/set-version.sh "$GITHUB_REF_NAME"` followed by
`cargo update --workspace`, so the built CLI binary or desktop bundle reports the
tag version — there is no manual version bump. To cut a release, push the tag:

```sh
git tag vX.Y.Z             # CLI release
git push origin vX.Y.Z

git tag desktop-vX.Y.Z     # desktop release
git push origin desktop-vX.Y.Z
```
