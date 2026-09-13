# oxide

An optimized AI coding agent CLI written in Rust.

## Layout

- `src/main.rs` — CLI entry (clap). Non-interactive `-p/--print` mode and TUI dispatch; `auth` and `mcp` subcommands.
- `src/mcp_config.rs` — `oxide mcp add/list/get/add-json/remove/auth`: reads and writes MCP servers in the native `.oxide/mcp.json` files (`--scope project|global`), merging sources for listing. `add` also accepts the Claude Code-style `oauth` fields (`--oauth-client-id`, `--oauth-client-secret`, `--callback-port`, `--oauth-scope`, `--redirect-uri`).
- `src/config.rs` — `Config` struct; loads `config.json` from the oxide config dir (`dirs::config_dir()/oxide`, e.g. `~/.config/oxide` on Linux, `~/Library/Application Support/oxide` on macOS) plus `OXIDE_*` / provider env overrides. Provider presets cover OpenAI/GPT, DeepSeek and Anthropic (`OPENAI_API_KEY`, `DEEPSEEK_API_KEY`, `ANTHROPIC_API_KEY`). Composes the system prompt, resolves `/commands` (including `agent`/`subtask` routing), and loads the context-pruning config from `.oxide/dcp.json`.
- `src/auth.rs` — `auth login` / `list` / `logout`: stores provider API keys in `auth.json` in the oxide config dir (mode 0600); keys are resolved after env vars and before the config file.
- `src/ecosystem/` — discovers and loads configuration from project and global scope: the native Oxide layout (`.oxide/`, `AGENTS.md`, `.oxide/mcp.json`) plus the Claude Code layout (`.claude/`, `CLAUDE.md`, `.mcp.json`) for compatibility. Covers rules, memory, commands, agents/subagents, skills (`SKILL.md`), MCP servers and plugins. This repository keeps its own agents, commands, skills and plugins in `.oxide/`; user-facing configuration guidance lives in `docs/configuration.md`.
- `src/agent.rs` — agent loop: stream the model, execute requested tools, feed results back (`MAX_STEPS = 25`). Read-only tool calls in a batch run in parallel (results kept in call order) while mutating tools stay sequential; a `Steering` queue injects user messages typed mid-run before the next model call, and a batch whose results all set `terminate` ends the turn. Hosts the agent-level `task` (subagent), `skill` (on-demand load), `memory`, `diagnostics` and `compress` tools; `run` returns a boxed future so subagents can recurse (capped by `MAX_TASK_DEPTH`), and `run_subagent` runs a `subtask` command in an isolated context. A `Runtime` bundles the MCP registry, plugin host, session log, snapshots, LSP manager, steering queue and the approval callback.
- `src/llm/` — model client. `client.rs` dispatches by provider: OpenAI-compatible chat completions (OpenAI, DeepSeek, custom) and the Anthropic Messages API (`anthropic.rs`, converting internal messages to content blocks and parsing its SSE events).
- `src/mcp.rs` — MCP runtime: connects configured servers over stdio or HTTP, initializes JSON-RPC sessions, and exposes remote tools (`<server>__<tool>`). Remote servers with an `oauth` block attach a bearer token per request.
- `src/mcp_oauth.rs` — OAuth 2.0 authorization-code + PKCE for remote MCP servers: metadata discovery, loopback callback, token exchange/refresh, and `mcp-oauth/<server>.json` storage (mode 0600).
- `src/media.rs` — multimodal attachments: dependency-free base64, image/PDF MIME detection, `@path` reference extraction, and a best-effort OS clipboard image grab (`pngpaste` / `wl-paste` / `xclip`).
- `src/memory.rs` — persistent cross-session memory store (project + user scope) under `memory/` in the oxide config dir, with dependency-free tf-idf search.
- `src/session.rs` — durable append-only session log (JSONL) under `sessions/<project>/` in the oxide config dir; model history is reconstructed from it and resumed via `-c/--continue` or `--resume <id>`. Also stores dynamic-context-pruning compression records so a resumed session rebuilds the same pruned view.
- `src/snapshots.rs` — shadow-git snapshots (bare repo under `snapshots/<project>/` in the oxide config dir) committed after each agent step; `/undo` and `/redo` in the TUI restore file changes.
- `src/permission.rs` — parses per-agent `permission` rules into allow/ask/deny and decides per tool + subject; `ask` prompts in the TUI unless `auto_approve` is set.
- `src/compact.rs` — summarizes long conversation history with the model, keeping the most recent messages; runs automatically when context pruning is disabled, and via `/compact`.
- `src/dcp.rs` — dynamic context pruning: loads `.oxide/dcp.json`, builds the pruned outgoing view (model `compress` summaries, tool-output deduplication, errored-output purging), injects context-limit nudges, and persists compression records through the session log. History is never modified.
- `src/lsp.rs` — minimal LSP client (rust-analyzer, typescript-language-server, pyright, gopls) that reports diagnostics; a `diagnostics` tool is exposed and `write_file` appends diagnostics.
- `src/plugin.rs` — plugin runtime: loads discovered JS/TS plugins under bun/node with an embedded harness and dispatches `tool.execute.before` / `tool.execute.after` hooks. An `after` hook can rewrite the output and set `output.terminate` to end the turn.
- `src/tools.rs` — tool specs and execution (`read_file`, `write_file`, `list_dir`, `bash`, `glob`, `grep`, `patch`, `webfetch`) plus connected MCP tools. `execute(call, cwd, &McpRegistry, &Progress)` returns a `ToolOutput` (text plus optional image/PDF media parts and a `terminate` flag; `read_file` attaches images/PDFs); `bash` streams stdout/stderr through `Progress`.
- `src/tui/` — ratatui + crossterm interface; renders conversation lines incrementally (only changed items are re-wrapped) and routes Enter while busy into the steering queue.

## Commands

- `cargo build`
- `cargo test`
- `cargo clippy --all-targets -- -D warnings`
- `cargo fmt`

## Conventions

- Rust 2021 edition, 4-space indent, default rustfmt style.
- Errors use `anyhow::Result` with `.context(...)` / `.with_context(...)` for human-readable messages.
- Do not add comments unless they explain non-obvious intent.
- Keep changes small and focused; verify with `cargo test` and `cargo clippy`.
- Built-in file/shell tools and connected MCP tools are registered in `tools::specs(&McpRegistry)` and dispatched in `tools::execute(call, cwd, &McpRegistry, &Progress)`. The agent-level `task`, `skill`, `memory` and `diagnostics` tools are defined and dispatched in `src/agent.rs`.
