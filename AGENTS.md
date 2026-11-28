# oxide

An optimized AI coding agent CLI written in Rust.

## Layout

- `crates/core/` (`oxide-core`): shared config, sessions, agent loop, tools, providers,
  approvals, trust, snapshots, MCP, skills and plugins.
- `crates/cli/` (`oxide`): terminal binary, RPC mode and ratatui TUI.
- `crates/desktop/` (`oxide-desktop`): the native Tauri desktop app, a separate
  Cargo workspace. `src/` holds the in-process engine and Tauri host, `ui/` the
  desktop page, `assets/` and `icons/` the packaging resources, and `package.sh`
  the platform packaging. It is released by `desktop.yml`.
- `editors/vscode/`: separate pnpm/TypeScript package. Runs the installed CLI with
  `oxide --mode rpc`; shares its configuration and session store.
- `.github/`: CI, release workflows and packaging.

Detailed behavior contracts and file-by-file notes are in
[docs/architecture-notes.md](docs/architecture-notes.md). Search that reference
for the subsystem being changed; do not load it in full. Other focused guides
live in `docs/` (`tools.md`, `configuration.md`, `desktop.md`, `vscode.md`,
`tui.md`, `ecosystem.md`). Keep this file concise; put implementation history
and detailed explanations in the relevant guide.

## Shared contracts

- Put shared behavior in `oxide-core`; CLI, Desktop and VS Code must agree on
  configuration, session storage, tool names and tool semantics.
- Project trust gates loading local resources, independently of tool permissions.
  Every front-end uses the shared approval store. Preserve explicit user choices.
- Desktop conversations own their run, transcript, queue, usage and change cards.
  A background run must never write into another conversation's state.
- Desktop and extension share icon paths and editor behavior; their existing
  checks hold that contract. Preserve keyboard access, native click activation,
  scroll behavior and independent chat state.
- Report tool failures accurately. Pending CI and empty searches are results,
  not failed jobs. Read back changes and run focused checks before claiming success.
- Inspect relevant code and tests together, batch independent reads, then act.
  Reuse results while their source is unchanged. A review can finish without
  edits; do not invent optimizations to justify the review.

## Commands

- `cargo build`
- `cargo test`
- `cargo clippy --all-targets -- -D warnings`
- `cargo fmt`
- `cd crates/desktop && pnpm install` (the page's Mermaid and Monaco dependencies, once; pnpm 12 and the app's own lockfile are what CI installs from)
- `cd crates/desktop && node ../../scripts/prepare-diagrams.mjs desktop && cargo run` (the Tauri window; install the page dependency first with `pnpm install`)
- `cd crates/desktop && cargo build --release && ./package.sh` (this platform's packages under `artifacts/`)
- `cd crates/desktop && cargo test` (desktop manager, turn, command, bridge, host and updater tests)
- `node crates/desktop/check-app.mjs` (front-end behavior against a stubbed DOM and bridge)
- `node crates/desktop/check-shell.mjs` (the Tauri host, page bridge, native services, menu, release train and packaging)
- `cd crates/desktop && cargo test && cargo clippy --all-targets -- -D warnings` (the host's packet routing, native services, menu and engine contract; release CI also runs `--check` against the packaged binary)
- `cd editors/vscode && pnpm test` (VS Code extension; `tsc -p .` then `node --test out/test/`)

## Conventions

- Rust 2021 edition, 4-space indent, default rustfmt style.
- Errors use `anyhow::Result` with `.context(...)` / `.with_context(...)` for human-readable messages.
- Do not add comments unless they explain non-obvious intent.
- Keep changes small and focused; verify with `cargo test` and `cargo clippy`.
- Built-in file/shell tools and connected MCP tools are registered in `tools::specs(&McpRegistry)` and dispatched in `tools::execute(call, cwd, &McpRegistry, &Progress)`. A tool's name is the name it is registered under: permission rules, the concurrency classifier and the TUI compare it directly, so a tool renamed in `specs` has to be renamed in each of them. `edit` is targeted text replacement (`edits[].oldText` must match uniquely against the original file). Matching is byte-exact first, then a line-wise pass that ignores trailing whitespace, and a block copied from a numbered `read` listing has its `N|`/`N+|` prefixes stripped (from the replacement text too); leading indentation is never ignored, so a replacement cannot silently lose its indentation. A non-unique match is still rejected, naming the occurrence line numbers, an absent one names the closest region (its line range and raw text, so copying it back applies exactly) and the first line of the block that differs from it, both sides escaped — a block that reads identically in a transcript can still hold a tab where the file holds spaces, or a newline where the file holds `\n`, and every failing edit in a batch is reported together while nothing is written. `find`/`grep` honor `.gitignore`; `grep` prefers `ripgrep` when available and otherwise uses a parallel built-in walker. The agent-level `task`, `skill`, `command`, `memory`, `diagnostics`, and `ask` tools are defined and dispatched in `crates/core/src/agent.rs`.
