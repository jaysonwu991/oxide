---
name: oxide-architecture
description: Use when navigating or modifying the oxide internals — the agent loop, LLM streaming client, tool dispatch, config loading, or TUI. Explains how a request flows through the crate.
---

# oxide architecture

## Request flow

1. `src/main.rs` parses CLI args (clap), handles the `auth` and `mcp`
   subcommands, resolves the working directory, and loads `Config`.
2. Non-interactive mode (`-p/--print` or a positional prompt) reads the prompt
   and calls `run_print`; otherwise the TUI starts via `tui::run`.
3. `agent::run(config, cwd, history, tx, runtime)` drives a turn:
   - Prepends `Message::system(config.compose_system_prompt())`.
   - At depth 0 with pruning enabled, builds the outgoing request from
     `dcp::prune(history, state, config.dcp)` and appends a `dcp::nudge` when the
     estimated context is large. History itself is never modified.
   - Calls `LlmClient::stream_chat`, forwarding text deltas as
     `AgentEvent::Text` through an unbounded mpsc channel.
   - Pushes the assistant message (with any tool calls) onto history.
   - If there are no tool calls, emits `AgentEvent::Finished(messages)`.
   - Otherwise checks permissions, executes each call (`dispatch` routes the
     agent-level `task`/`skill`/`memory`/`diagnostics` tools, else
     `tools::execute`), emits `ToolCall`/`ToolResult`, appends a
     `Message::tool`, and loops. A `compress` call is handled in the loop:
     `dcp::apply_compress` updates the in-memory state and appends a record to
     the session log.
   - Stops after `MAX_STEPS = 25` iterations with an error event.
4. A leading `/command` is resolved by `Config::resolve_command` into an expanded
   prompt plus optional `agent`/`subtask` routing. Commands with `subtask: true`
   run through `agent::run_subagent` in an isolated context and report their
   final text back to the main conversation.

## Modules

- `src/llm/client.rs` — `LlmClient::stream_chat`; SSE parsing and turn assembly.
- `src/llm/types.rs` — OpenAI-compatible request/response and `Message` types.
- `src/llm/mod.rs` — module re-exports (`LlmClient`, `Message`, `ToolSpec`, ...).
- `src/agent.rs` — the agent loop (`run`, `run_loop`, `dispatch`) and
  `run_subagent` for `subtask` commands.
- `src/tools.rs` — `specs()` and `execute()`; the only place tools are wired.
- `src/ecosystem/mod.rs` — Oxide (`.oxide/`, `AGENTS.md`) and Claude Code
  (`.claude/`, `CLAUDE.md`, `.mcp.json`) layout discovery; `frontmatter.rs`
  parses Markdown frontmatter; `resolve_command` returns command prompt +
  `agent`/`subtask` routing.
- `src/config.rs` — `Config`, `load`, `activate_agent`, `resolve_command`,
  `config_path`, `require_api_key`.
- `src/dcp.rs` — dynamic context pruning: `DcpConfig` (`.oxide/dcp.json`),
  `DcpState`, `prune`, `nudge`, `apply_compress`, `compress_spec`.
- `src/session.rs` — durable JSONL session log; also stores DCP compression
  records (`append_dcp`, `dcp_state`).
- `src/mcp.rs` / `src/mcp_config.rs` / `src/mcp_oauth.rs` — MCP runtime
  (`McpRegistry`, stdio/HTTP), the `oxide mcp` CLI that reads/writes
  `.oxide/mcp.json`, and the OAuth authorization-code + PKCE flow for remote
  servers.
- `src/tui/` — `run` entry plus `app`/`ui` for rendering and input.

## Adding a model provider

Provider presets are resolved by `ProviderPreset::for_name` in `src/config.rs`;
each sets a default `model` and `base_url`. Override them with `OXIDE_PROVIDER`,
`OXIDE_MODEL`, `OXIDE_BASE_URL`, or `OXIDE_API_KEY`, or the provider-specific
`*_API_KEY` / `*_BASE_URL` variables (`OPENAI_*`, `DEEPSEEK_*`, `ANTHROPIC_*`).
OpenAI-compatible and Anthropic APIs are dispatched in `src/llm/client.rs` (see
`src/llm/anthropic.rs`). Config on disk lives in the oxide config dir
(`dirs::config_dir()/oxide/config.json`; e.g. `~/.config/oxide` on Linux,
`~/Library/Application Support/oxide` on macOS; see `Config::config_path`).

## Invariants

- The system prompt is always index 0 of the request, never stored in history.
- History is the single source of truth passed to `Finished`.
- Context pruning only changes the outgoing request; history and the session log
  keep every original message.
- Tool output is truncated (`MAX_OUTPUT = 30_000` bytes) before entering history.
