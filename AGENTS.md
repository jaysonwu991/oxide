# oxide

An optimized AI coding agent CLI written in Rust.

## Layout

- `src/main.rs` — CLI entry (clap). Non-interactive `-p/--print` mode and TUI dispatch.
- `src/config.rs` — `Config` struct; loads `config.json` from the oxide config dir (`dirs::config_dir()/oxide`, e.g. `~/.config/oxide` on Linux, `~/Library/Application Support/oxide` on macOS) plus `OXIDE_*` / provider env overrides. Provider presets cover OpenAI/GPT, DeepSeek and Anthropic (`OPENAI_API_KEY`, `DEEPSEEK_API_KEY`, `ANTHROPIC_API_KEY`). Composes the system prompt and expands `/commands`.
- `src/auth.rs` — `auth login` / `list` / `logout`: stores provider API keys in `auth.json` in the oxide config dir (mode 0600); keys are resolved after env vars and before the config file.
- `src/ecosystem/` — discovers and loads configuration from project and global scope: the native Oxide layout (`.oxide/`, `AGENTS.md`) plus the Claude Code layout (`.claude/`, `CLAUDE.md`, `.mcp.json`) for compatibility. Covers rules, memory, commands, agents/subagents, skills (`SKILL.md`), MCP servers and plugins. This repository keeps its own agents, commands, skills and plugins in `.oxide/`.
- `src/agent.rs` — agent loop: stream the model, execute requested tools, feed results back (`MAX_STEPS = 25`). Hosts the agent-level `task` (subagent), `skill` (on-demand load), `memory` and `diagnostics` tools; `run` returns a boxed future so subagents can recurse (capped by `MAX_TASK_DEPTH`). A `Runtime` bundles the MCP registry, plugin host, session log, snapshots, LSP manager and the approval callback.
- `src/llm/` — model client. `client.rs` dispatches by provider: OpenAI-compatible chat completions (OpenAI, DeepSeek, custom) and the Anthropic Messages API (`anthropic.rs`, converting internal messages to content blocks and parsing its SSE events).
- `src/mcp.rs` — MCP runtime: connects configured servers over stdio or HTTP, initializes JSON-RPC sessions, and exposes remote tools (`<server>__<tool>`).
- `src/media.rs` — multimodal attachments: dependency-free base64, image/PDF MIME detection, `@path` reference extraction, and a best-effort OS clipboard image grab (`pngpaste` / `wl-paste` / `xclip`).
- `src/memory.rs` — persistent cross-session memory store (project + user scope) under `memory/` in the oxide config dir, with dependency-free tf-idf search.
- `src/session.rs` — durable append-only session log (JSONL) under `sessions/<project>/` in the oxide config dir; model history is reconstructed from it and resumed via `-c/--continue` or `--resume <id>`.
- `src/snapshots.rs` — shadow-git snapshots (bare repo under `snapshots/<project>/` in the oxide config dir) committed after each agent step; `/undo` and `/redo` in the TUI restore file changes.
- `src/permission.rs` — parses per-agent `permission` rules into allow/ask/deny and decides per tool + subject; `ask` prompts in the TUI unless `auto_approve` is set.
- `src/compact.rs` — summarizes long conversation history with the model, keeping the most recent messages; runs automatically and via `/compact`.
- `src/lsp.rs` — minimal LSP client (rust-analyzer, typescript-language-server, pyright, gopls) that reports diagnostics; a `diagnostics` tool is exposed and `write_file` appends diagnostics.
- `src/plugin.rs` — plugin runtime: loads discovered JS/TS plugins under bun/node with an embedded harness and dispatches `tool.execute.before` / `tool.execute.after` hooks.
- `src/tools.rs` — tool specs and execution (`read_file`, `write_file`, `list_dir`, `bash`, `glob`, `grep`, `patch`, `webfetch`) plus connected MCP tools. `execute` returns a `ToolOutput` (text plus optional image/PDF media parts; `read_file` attaches images/PDFs).
- `src/tui/` — ratatui + crossterm interface.

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
- Built-in file/shell tools and connected MCP tools are registered in `tools::specs(&McpRegistry)` and dispatched in `tools::execute(call, cwd, &McpRegistry)`. The agent-level `task`, `skill`, `memory` and `diagnostics` tools are defined and dispatched in `src/agent.rs`.
