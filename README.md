<div align="center">
<pre>
 ██████╗  ██╗  ██╗ ██╗ ██████╗  ███████╗
██╔═══██╗ ╚██╗██╔╝ ██║ ██╔══██╗ ██╔════╝
██║   ██║  ╚███╔╝  ██║ ██║  ██║ █████╗  
██║   ██║  ██╔██╗  ██║ ██║  ██║ ██╔══╝  
╚██████╔╝ ██╔╝ ██╗ ██║ ██████╔╝ ███████╗
 ╚═════╝  ╚═╝  ╚═╝ ╚═╝ ╚═════╝  ╚══════╝
</pre>
</div>

# Oxide

A native Rust AI coding agent with a terminal UI, a Tauri desktop app, and a
VS Code extension. Oxide streams from OpenAI-compatible and Anthropic models,
runs a tool-using agent loop against your project, and understands its own
`.oxide/` configuration layout out of the box, with Claude Code configuration
support for compatibility.

## Highlights

- **Three front-ends, one core** — a ratatui terminal UI, a Tauri desktop app
  (`crates/desktop`), and a VS Code extension (`editors/vscode`), all sharing the
  same configuration, sessions, trust decisions, and MCP servers.
- **Providers** — OpenAI-compatible (OpenAI, DeepSeek, Portkey, Z.AI/GLM, custom)
  and the Anthropic Messages API, with reasoning effort from `auto` to `high` and
  resilient streaming that retries transient failures.
- **Tools** — `read`, `write`, `edit`, `bash`, `grep`, `find`, `ls`, `webfetch`
  plus agent-level `task`, `skill`, `command`, `memory`, `diagnostics`, and `ask`;
  MCP servers over stdio or HTTP, loaded on demand with OAuth discovery.
- **Project ecosystem** — instructions, commands, prompt templates, agents,
  skills, MCP servers, and plugin packages from `.oxide/` (and the Claude Code
  layout), gated behind per-project trust.
- **Sessions you can go back through** — Pi-compatible JSONL session trees,
  automatic context compaction, branch summarization, and shadow-git `/undo`.
- **Evidence-based completion** — the system prompt's Definition of Done makes a
  run confirm the outcome of anything it changed before claiming success, and a
  Scope rule keeps commits limited to the task.

The full list is in [docs/features.md](docs/features.md), and
[docs/comparison.md](docs/comparison.md) compares Oxide with Codex, OpenCode, and
Claude Code.

## Install

macOS and Linux:

```sh
curl -fsSL https://raw.githubusercontent.com/jaysonwu991/oxide/main/install.sh | bash
```

Windows (x86_64):

```powershell
irm https://raw.githubusercontent.com/jaysonwu991/oxide/main/install.ps1 | iex
```

From source (stable Rust, edition 2021):

```sh
cargo install --path crates/cli
```

A prebuilt install keeps itself current with `oxide update`, and each released
surface updates its own train: the desktop app installs the newest `desktop-v*`
bundle over itself, and the VS Code panel installs the newest `extension-v*`
`.vsix` — so updating one never pulls a release meant for another. Supported
platforms, installer overrides, the desktop bundle, the VS Code VSIX, updating,
and uninstalling are all covered in [docs/install.md](docs/install.md).

## Quick start

```sh
# Launch the TUI in the current project
oxide

# Then connect a provider from inside the TUI
/login
```

Or bring your own key from the environment:

```sh
export OPENAI_API_KEY=sk-...
oxide
```

Non-interactive:

```sh
oxide -p "summarize this repository"
echo "explain src/main.rs" | oxide -p
oxide -t read,grep,find -p "review the diff"
```

Manage MCP servers and plugins:

```sh
oxide mcp add filesystem npx -y @modelcontextprotocol/server-filesystem .
oxide mcp add --transport http atlassian https://mcp.atlassian.com/v1/mcp
oxide plugin marketplace add <url|path|owner/repo>
oxide plugin install <name>[@marketplace]
```

Keyboard shortcuts, the transcript layout, and copy behavior are in
[docs/tui.md](docs/tui.md); print/JSON/RPC modes and `oxide sessions` are in
[docs/modes.md](docs/modes.md).

## Desktop app

The `oxide-desktop` package (`crates/desktop`) is a Tauri front-end for the
same `oxide-core` agent. It shares the CLI's configuration (`config.json`,
`auth.json`, `settings.json`) and its session store, and adds a multi-project
sidebar: any folder can be added, every project you have run the CLI in is
discovered from its sessions, and each project lists its own threads alongside a
cross-repo view of recent ones. Chat runs the same agent loop through
`oxide-core`, streaming text, tool calls, and token usage over the window's own
bridge to the Rust command layer.

```sh
cargo run -p oxide-desktop --features gui
```

Prebuilt bundles are drafted under `desktop-v*`
[releases](https://github.com/jaysonwu991/oxide/releases). See
[docs/desktop.md](docs/desktop.md) for the layout, approvals, change cards,
shortcuts, signing, and packaging.

## VS Code extension

The `editors/vscode` package is a TypeScript extension (a separate pnpm package,
not a Cargo workspace member) that drives the same `oxide` binary from a chat
panel and from editor actions. It shells out to `oxide --mode rpc`, so it reads
the same provider logins, `config.json`, `sessions/`, `trust.json`, `AGENTS.md`,
agents, skills, plugins, and MCP servers as the terminal and the desktop app:
streaming Markdown replies with `✦ Thinking` blocks and tool cards, editor
actions for the selection, the CLI's own session list, and `@path` references
that attach a file's text or an image.

```sh
cd editors/vscode
pnpm install
pnpm test
```

See [editors/vscode/README.md](editors/vscode/README.md) for the command and
setting tables, and [docs/vscode.md](docs/vscode.md) for the architecture.

## Documentation

| Guide | Contents |
| --- | --- |
| [docs/features.md](docs/features.md) | The full feature list behind the highlights above. |
| [docs/install.md](docs/install.md) | Installers, supported platforms, `oxide update`, `oxide uninstall`. |
| [docs/tui.md](docs/tui.md) | Terminal layout, keyboard shortcuts, copying. |
| [docs/modes.md](docs/modes.md) | `-p/--print`, `--mode json`, `--mode rpc`, `oxide sessions`, a turn's changes. |
| [docs/configuration.md](docs/configuration.md) | `config.json`, CLI flags, environment variables, providers, trust, themes, compaction, data locations. |
| [docs/ecosystem.md](docs/ecosystem.md) | The `.oxide/` layout, Claude Code compatibility, built-in slash commands. |
| [docs/tools.md](docs/tools.md) | Tool parameters, behavior, output caps, concurrency. |
| [docs/cli.md](docs/cli.md) | Task-by-task guide: MCP servers, subagents, commands, skills, plugins, permissions, reasoning, attachments, notifications. |
| [docs/desktop.md](docs/desktop.md) | Desktop app architecture, interface, packaging, signing. |
| [docs/vscode.md](docs/vscode.md) | VS Code extension architecture, protocol, testing. |
| [docs/comparison.md](docs/comparison.md) | Oxide next to Codex, OpenCode, and Claude Code. |
| [CONTRIBUTING.md](CONTRIBUTING.md) | Architecture notes, conventions, releases. |

## Development

```sh
cargo build
cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt
```

The workspace members are `crates/core` (shared agent core), `crates/cli` (the
`oxide` terminal binary), and `crates/desktop` (the Tauri desktop app). The
desktop's `gui` feature is off by default, so the plain workspace build stays
free of the Tauri dependency tree; the VS Code extension is a separate pnpm
package:

```sh
cargo build -p oxide-desktop --features gui
node crates/desktop/check-app.mjs && node crates/desktop/check-shell.mjs
cd editors/vscode && pnpm test
```

See [CONTRIBUTING.md](CONTRIBUTING.md) for architecture notes and guidelines.

## License

MIT
