---
name: oxide-architecture
description: Use when navigating or modifying the oxide internals — the agent loop, LLM streaming client, tool dispatch, config loading, or TUI. Explains how a request flows through the crate.
---

# oxide architecture

## Request flow

1. `src/main.rs` parses CLI args (clap), handles the `mcp` subcommand, resolves
   the working directory, and loads `Config`. Provider login and logout are TUI
   slash commands rather than CLI subcommands.
2. Non-interactive mode (`-p/--print` or a positional prompt) reads the prompt
   and calls `run_print`; otherwise the TUI starts via `tui::run`.
3. `agent::run(config, cwd, history, tx, runtime)` drives a turn:
   - Prepends `Message::system(config.compose_system_prompt())`.
   - `history` is the session context (`SessionLog::messages`, the leaf path
     with the latest compaction applied). Auto-compaction runs when the context
     approaches the model window, appending a `compaction` entry to the session
     branch and refreshing `history`.
   - Calls `LlmClient::stream_chat` with `StreamHooks`, forwarding text as
     `AgentEvent::Text`, reasoning as `ThinkingDelta`, and retry notices as
     `Retrying` through an unbounded mpsc channel. The client retries transient
     failures and turns with neither text nor tool calls with backoff while no
     text has been emitted.
   - Pushes the assistant message (with any tool calls and thinking blocks) onto
     history.
   - If there are no tool calls, drains the steering and follow-up queues and
     continues if either produced a message; otherwise emits
     `AgentEvent::Finished(messages)`.
   - Otherwise checks permissions, executes each call (`dispatch` routes the
     agent-level `task`/`skill`/`command`/`memory`/`diagnostics` tools,
     built-in/MCP tools go through `tools::execute`), emits
     `ToolCall`/`ToolResult`, appends a `Message::tool` to the session, and
     loops. A nested `task` forwards its own `SubagentActivity` so the parent
     view is not silent.
   - Loops until the model stops calling tools; a batch whose results all set
     `output.terminate` ends the turn. There is no fixed step cap.
4. A leading `/command` is resolved by `Config::resolve_command` into an expanded
   prompt plus optional `agent`/`subtask` routing. Commands with `subtask: true`
   run through `agent::run_subagent` in an isolated context and report their
   final text back to the main conversation.

## Modules

- `src/llm/client.rs` — `LlmClient::stream_chat` and the OpenAI-compatible /
  Anthropic stream paths; `StreamHooks` (text/thinking/retry), the retry budget
  (transient failures plus empty turns), and `read_sse`/`drain_lines` live here.
- `src/llm/types.rs` — OpenAI-compatible request/response, `Message`, and
  `Usage` (input/output plus cache tokens and cost).
- `src/llm/mod.rs` — module re-exports (`LlmClient`, `Message`, `ToolSpec`, ...).
- `src/auth.rs` — multi-provider credential store in `auth.json`, behind the
  TUI `/login`, `/logout`, and `/connect` commands.
- `src/lsp.rs` — `LspManager`/`Server`: a cached language server that exits or
  whose pipe breaks is evicted and reconnected before the call is retried.
- `src/agent.rs` — the agent loop (`run`, `run_loop`, `dispatch`) and
  `run_subagent` for `subtask` commands.
- `src/tools.rs` — built-in/MCP `specs()` and `execute()`; `grep` prefers
  `ripgrep` (`rg`) when present and otherwise uses a parallel built-in walker
  that sniffs binary files; agent-level tools are wired in `src/agent.rs`.
- `src/ecosystem/mod.rs` — Oxide (`.oxide/`, `AGENTS.md`) and Claude Code
  (`.claude/`, `CLAUDE.md`, `.mcp.json`) layout discovery; `frontmatter.rs`
  parses Markdown frontmatter; `resolve_command` returns command prompt +
  `agent`/`subtask` routing.
- `src/config.rs` — `Config`, `load`, `activate_agent`, `resolve_command`,
  `config_path`, `require_api_key`, `context_window`, `supports_reasoning`, and
  the `compaction`/`prices` settings.
- `src/diff.rs` — `preview(old, new)` LCS line diff with context windows and gap
  markers, used for the TUI's colored edit previews.
- `src/session.rs` — Pi-compatible JSONL session trees: typed entries with
  `id`/`parentId`, compaction and branch-summary entries, leaf-path context
  building (`messages`, `context_ids`), per-entry usage, forking, and picker
  metadata.
- `src/pricing.rs` — `modelPrices` lookup for the footer's `$cost` segment.
- `src/compact.rs` — Pi-style compaction: `prepare`, `generate`,
  `summarize_branch`, `needs_compaction`, and token estimation.
- `src/mcp.rs` / `src/mcp_config.rs` / `src/mcp_oauth.rs` — MCP runtime
  (`McpRegistry`, stdio/HTTP), the `oxide mcp` CLI that reads/writes
  `.oxide/mcp.json`, and the OAuth authorization-code + PKCE flow for remote
  servers.
- `src/tui/` — `run` entry plus `app`/`ui` for rendering and input; renders the
  welcome banner as the block-letter `OXIDE` wordmark stacked above the
  ecosystem summary (falling back to plain `oxide` text on narrow terminals),
  renders each tool call as one background-filled panel (header, blank line,
  body, and `Took` footer) colored by state, shows bodies by default and `read`
  file contents (Ctrl+O collapses), wraps long actions and tool output with a
  hanging indent, times slow non-shell tools, renders concise `Run`/`Ran` shell
  actions, shows inline user/assistant labels and colored edit diffs, and draws
  the Pi-style footer (path/branch/session, cumulative tokens with cache and
  cost, context `%`/window, model/thinking, and plugin statuses).

## Adding a model provider

Provider presets are resolved by `ProviderPreset::for_name` in `src/config.rs`;
each sets a default `model` and `base_url`. Override them with `OXIDE_PROVIDER`,
`OXIDE_MODEL`, `OXIDE_BASE_URL`, or `OXIDE_API_KEY`, or the provider-specific
`*_API_KEY` / `*_BASE_URL` variables (`OPENAI_*`, `DEEPSEEK_*`, `ANTHROPIC_*`,
`PORTKEY_*`). Portkey additionally supports `PORTKEY_CONFIG` and
`PORTKEY_MODELS`.
OpenAI-compatible and Anthropic APIs are dispatched in `src/llm/client.rs` (see
`src/llm/anthropic.rs`). Config on disk lives in the oxide config dir
(`dirs::config_dir()/oxide/config.json`; e.g. `~/.config/oxide` on Linux,
`~/Library/Application Support/oxide` on macOS; see `Config::config_path`).

## Invariants

- Streaming stays linear: accumulated text is appended to a `String` (or mutated
  in place when it lives in a `serde_json::Value`), and `read_sse` drains whole
  lines once per network chunk instead of after every line.
- The system prompt is always index 0 of the request, never stored in history.
- History is the single source of truth passed to `Finished`.
- Sessions are append-only trees; `messages` returns the leaf path with the
  latest compaction applied, and appending always adds a child of the leaf.
- Tool output is capped before entering history. The general limit is
  `MAX_OUTPUT_LINES` (250) lines and `MAX_OUTPUT_BYTES` (6,000) bytes, with
  smaller per-tool limits for shell, search, listing, fetch, and edit results.
  `bash` keeps its tail (so the exit code survives), other tools keep the head,
  and dropped content is saved under `truncated/` in the config dir
  (`OXIDE_TRUNCATION_DIR`) with a pointer in the result.
