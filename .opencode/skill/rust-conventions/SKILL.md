---
name: rust-conventions
description: Use when writing, editing, or reviewing Rust code in this repository. Covers error handling, style, async, testing, and the no-comments rule specific to oxide.
---

# Rust conventions for oxide

## Style

- Rust 2021 edition, 4-space indent, default `rustfmt`.
- Prefer iterators and combinators over manual index loops.
- Keep functions small; extract helpers when a function exceeds ~50 lines.
- Do **not** add comments unless they explain non-obvious intent.

## Error handling

- Fallible functions return `anyhow::Result<T>`.
- Add `.context("...")` or `.with_context(|| format!(...))` at every `?` where
  the underlying error alone is unclear.
- Never `unwrap()`/`expect()` on IO, parsing, network, or user input.
- `bail!` for early validation errors with an actionable message.

## Async

- Use `tokio`; never block the runtime with synchronous IO in an async fn.
- For subprocesses use `tokio::process::Command` and wrap with
  `tokio::time::timeout` where a hang is possible (see `tools::bash`).
- Stream data through `tokio::sync::mpsc` channels; keep the agent loop
  non-blocking and report progress via `AgentEvent`.

## Testing

- Put unit tests in a `#[cfg(test)] mod tests` at the bottom of the file.
- Async tests use `#[tokio::test]`.
- Use temp dirs under `std::env::temp_dir()` and clean up with
  `std::fs::remove_dir_all(...).ok()`.
- Every bug fix should come with a regression test.

## Extending tools

1. Add a spec in `tools::specs()` with a JSON Schema for its parameters.
2. Dispatch it in `tools::execute()`.
3. Return `String` output; truncate via the existing `truncate` helper.
4. Add a test covering both success and a failure path.
