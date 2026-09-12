---
name: oxide-architecture
description: Use when navigating or modifying the oxide internals — the agent loop, LLM streaming client, tool dispatch, config loading, or TUI. Explains how a request flows through the crate.
---

# oxide architecture

## Request flow

1. `src/main.rs` parses CLI args (clap), resolves the working directory, and
   loads `Config`.
2. Non-interactive mode (`-p/--print` or a positional prompt) reads the prompt
   and calls `run_print`; otherwise the TUI starts via `tui::run`.
3. `agent::run(config, cwd, history, tx, runtime)` drives a turn:
   - Prepends `Message::system(config.system_prompt)`.
   - Calls `LlmClient::stream_chat`, forwarding text deltas as
     `AgentEvent::Text` through an unbounded mpsc channel.
   - Pushes the assistant message (with any tool calls) onto history.
   - If there are no tool calls, emits `AgentEvent::Finished(messages)`.
   - Otherwise checks permissions, executes each call via `tools::execute`,
     emits `ToolCall`/`ToolResult`, appends a `Message::tool`, and loops.
   - Stops after `MAX_STEPS = 25` iterations with an error event.

## Modules

- `src/llm/client.rs` — `LlmClient::stream_chat`; SSE parsing and turn assembly.
- `src/llm/types.rs` — OpenAI-compatible request/response and `Message` types.
- `src/llm/mod.rs` — module re-exports (`LlmClient`, `Message`, `ToolSpec`, ...).
- `src/tools.rs` — `specs()` and `execute()`; the only place tools are wired.
- `src/config.rs` — `Config`, `load`, `config_path`, `require_api_key`.
- `src/tui/` — `run` entry plus `app`/`ui` for rendering and input.

## Adding a model provider

`Config` is OpenAI-compatible: set `provider`, `model`, and `base_url`. Env
overrides are `OXIDE_MODEL`, `OXIDE_PROVIDER`, `OPENAI_BASE_URL`, and
`OPENAI_API_KEY`. Config on disk lives at
`~/.config/oxide/config.json` (see `Config::config_path`).

## Invariants

- The system prompt is always index 0 of the request, never stored in history.
- History is the single source of truth passed to `Finished`.
- Tool output is truncated (`MAX_OUTPUT = 30_000` bytes) before entering history.
