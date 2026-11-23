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

A native Rust AI coding agent with a terminal UI, a Tauri desktop app, and a VS
Code extension. Oxide streams from
OpenAI-compatible and Anthropic models, runs a tool-using agent loop against
your project, and understands its own `.oxide/` configuration layout out of the
box, with Claude Code configuration support for compatibility.

## Highlights

- **One core, several front-ends** — a ratatui terminal UI, the native Tauri
  desktop app (`crates/desktop`), and a VS Code extension (`editors/vscode`),
  all sharing the same configuration, sessions, trust decisions, and MCP
  servers.
- **Providers** — 35 built-in providers behind one table: every OpenAI-compatible
  endpoint (OpenAI, DeepSeek, Groq, Mistral, xAI, OpenRouter, Z.AI/GLM, …), the
  Anthropic Messages API, Gemini and Vertex AI, Amazon Bedrock, Azure OpenAI,
  GitHub Copilot (device-flow login), GitLab Duo (the AI gateway's proxy) and the
  Portkey gateway (with its spend bar in the footer), plus any custom endpoint.
  Reasoning effort from `auto`/`off` up to `max`, narrowed to the levels the
  model's own listing or the pi.dev catalog advertises, and resilient streaming
  that retries transient failures.
  See [Providers](docs/configuration.md#providers).
- **Tools** — `read`, `write`, `edit`, `bash`, `grep`, `find`, `ls`, `webfetch`
  plus agent-level `task`, `skill`, `command`, `memory`, and `ask`;
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
[docs/comparison.md](docs/comparison.md) compares Oxide with Claude Code, Codex,
Pi Coding Agent, and OpenCode.

## Install

macOS and Linux:

```sh
curl -fsSL https://raw.githubusercontent.com/jaysonwu991/oxide/main/install.sh | sh
```

Windows (x86_64):

```powershell
irm https://raw.githubusercontent.com/jaysonwu991/oxide/main/install.ps1 | iex
```

From source (stable Rust, edition 2021):

```sh
cargo install --path crates/cli
```

A prebuilt install keeps current with `oxide update`, and each released surface
updates its own train, so updating one never pulls a release meant for another.
Supported platforms, the installer overrides, the desktop build, `oxide update`
and `oxide uninstall` are in [docs/install.md](docs/install.md).

## Installers

Oxide Desktop and the VS Code extension are always the newest build. Desktop
links use `desktop-latest`, and the extension uses `extension-latest`; their
files are replaced on every publish, so the links themselves never change.
Versioned `desktop-v*` and `extension-v*` releases name the version a file is
from.

| Surface | Platform | Installer |
| --- | --- | --- |
| Oxide Desktop | macOS, Apple Silicon | [Oxide-macos-arm64.dmg](https://github.com/jaysonwu991/oxide/releases/download/desktop-latest/Oxide-macos-arm64.dmg) |
| Oxide Desktop | macOS, Intel | [Oxide-macos-x64.dmg](https://github.com/jaysonwu991/oxide/releases/download/desktop-latest/Oxide-macos-x64.dmg) |
| Oxide Desktop | Windows x64 | [Oxide-win-x64-Setup.tar.gz](https://github.com/jaysonwu991/oxide/releases/download/desktop-latest/Oxide-win-x64-Setup.tar.gz) |
| Oxide Desktop | Linux x64 | [Oxide-linux-x64-Setup.tar.gz](https://github.com/jaysonwu991/oxide/releases/download/desktop-latest/Oxide-linux-x64-Setup.tar.gz) |
| Oxide Desktop | Linux arm64 | [Oxide-linux-arm64-Setup.tar.gz](https://github.com/jaysonwu991/oxide/releases/download/desktop-latest/Oxide-linux-arm64-Setup.tar.gz) |
| VS Code | any | [oxide-vscode.vsix](https://github.com/jaysonwu991/oxide/releases/download/extension-latest/oxide-vscode.vsix) |

On macOS, an app that was not notarized — a build made without both the
Developer ID certificate and the App Store Connect key — opens as **damaged**
once the browser quarantines it. Drag the app into `/Applications` first if it
is still on the disk image, then clear the flag and open it again:

```sh
xattr -dr com.apple.quarantine /Applications/Oxide.app
```

A notarized release needs none of this.

## Quick start

```sh
# Launch the TUI in the current project
oxide

# Then connect a provider from inside the TUI
/connect
```

Or bring a key from the environment. A provider reads the variable its own
listing names — `OPENAI_API_KEY`, `DEEPSEEK_API_KEY`, `ANTHROPIC_API_KEY`,
`GEMINI_API_KEY`, … — or `OXIDE_API_KEY`, which every provider reads, so the
provider a run uses has to be the one that key belongs to (see
[docs/configuration.md](docs/configuration.md#providers)):

```sh
export OPENAI_API_KEY=sk-...   # or DEEPSEEK_API_KEY, ANTHROPIC_API_KEY, …
oxide --provider openai        # the provider whose key that is
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
oxide mcp add --transport http atlassian https://mcp.atlassian.com/v2/mcp
oxide plugin marketplace add <url|path|owner/repo>
oxide plugin install <name>[@marketplace]
```

Keyboard shortcuts, the transcript layout, and copy behavior are in
[docs/tui.md](docs/tui.md); print/JSON/RPC modes and `oxide session` are in
[docs/modes.md](docs/modes.md).

## Oxide Desktop

`crates/desktop` is the desktop app: the same agent in a
[Tauri](https://tauri.rs) window. It links the desktop engine
in-process (`oxide_desktop::commands::dispatch`), so it reads and writes the same
projects, sessions, approvals, configuration, themes and language as the
terminal and editor integration. It manages multiple projects and their threads
from one window, with streamed turns, reviews, worktrees, terminals, plugins,
MCP servers and platform webview accessibility.

Oxide Desktop owns the `desktop-latest` installer links and the `desktop-v*`
release train.

The package is its own Cargo workspace:

```sh
cd crates/desktop
pnpm install
node ../../scripts/prepare-diagrams.mjs desktop
cargo run
```

Prebuilt installers are in [Installers](#installers). See
[docs/desktop.md](docs/desktop.md) for the complete surface,
accessibility, signing and packaging.

## VS Code extension

Install the CLI first: the extension is a chat panel over your installed `oxide`
binary and bundles no engine of its own.

The `editors/vscode` package is a TypeScript extension (a separate pnpm package,
not a Cargo workspace member) that drives the same `oxide` binary from a chat
panel and from editor actions. It shells out to `oxide --mode rpc`, so it reads
the same provider logins, `config.json`, `sessions/`, `trust.json`, `AGENTS.md`,
agents, skills, plugins, and MCP servers as the terminal and desktop app:
streaming Markdown replies, progress commentary and tool cards,
editor actions for the selection, the CLI's own session list, and `@path`
references that attach a file's text or an image.

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
| [docs/install.md](docs/install.md) | Install and update the CLI, Oxide Desktop, and the extension; supported platforms, `oxide update`, `oxide uninstall`. |
| [docs/tui.md](docs/tui.md) | Terminal layout, keyboard shortcuts, copying. |
| [docs/modes.md](docs/modes.md) | `-p/--print`, `--mode json`, `--mode rpc`, `oxide session`, a turn's changes. |
| [docs/configuration.md](docs/configuration.md) | `config.json`, CLI flags, environment variables, providers, trust, themes, compaction, data locations. |
| [docs/ecosystem.md](docs/ecosystem.md) | The `.oxide/` layout, Claude Code compatibility, built-in slash commands. |
| [docs/tools.md](docs/tools.md) | Tool parameters, behavior, output caps, concurrency. |
| [docs/cli.md](docs/cli.md) | Task-by-task guide: MCP servers, subagents, commands, skills, plugins, permissions, reasoning, attachments, notifications. |
| [docs/desktop.md](docs/desktop.md) | Oxide Desktop architecture, interface, packaging, and signing. |
| [docs/vscode.md](docs/vscode.md) | VS Code extension architecture, protocol, testing. |
| [docs/comparison.md](docs/comparison.md) | Oxide next to Claude Code, Codex, Pi Coding Agent, and OpenCode. |
| [CONTRIBUTING.md](CONTRIBUTING.md) | Architecture notes, conventions, releases. |

## Development

```sh
cargo build
cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt
```

The workspace members are `crates/core` (shared agent core) and `crates/cli` (the
`oxide` terminal binary). `crates/desktop` is a separate Cargo workspace for the
native Tauri app. The VS Code extension is a separate pnpm package:

```sh
cd crates/desktop && pnpm install && cargo test && cargo clippy --all-targets -- -D warnings
node crates/desktop/check-app.mjs && node crates/desktop/check-shell.mjs
cd editors/vscode && pnpm test
```

See [CONTRIBUTING.md](CONTRIBUTING.md) for architecture notes and guidelines.

## License

MIT
