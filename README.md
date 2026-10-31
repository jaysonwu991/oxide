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

A native Rust AI coding agent with a terminal UI, an Electron desktop app, and a
VS Code extension. Oxide streams from OpenAI-compatible and Anthropic models,
runs a tool-using agent loop against your project, and understands its own
`.oxide/` configuration layout out of the box, with Claude Code configuration
support for compatibility.

## Highlights

- **Three front-ends, one core** — a ratatui terminal UI, an Electron desktop
  app (`crates/desktop`) whose window drives the same Rust engine, and a VS Code
  extension (`editors/vscode`), all sharing the same configuration, sessions,
  trust decisions, and MCP servers. All three draw the interface in British
  English or Simplified Chinese from one shared setting.
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

The desktop app and the VS Code extension, always the newest build. Each link
points at the `desktop-latest` or `extension-latest` release, whose files are
replaced on every publish, so the links themselves never change; the versioned
`desktop-v*` and `extension-v*` releases name the version a file is from.

| Surface | Platform | Installer |
| --- | --- | --- |
| Desktop | macOS, Apple Silicon | [Oxide-macos-arm64.dmg](https://github.com/jaysonwu991/oxide/releases/download/desktop-latest/Oxide-macos-arm64.dmg) |
| Desktop | macOS, Intel | [Oxide-macos-x64.dmg](https://github.com/jaysonwu991/oxide/releases/download/desktop-latest/Oxide-macos-x64.dmg) |
| Desktop | Windows x64 | [Oxide-win-x64-Setup.exe](https://github.com/jaysonwu991/oxide/releases/download/desktop-latest/Oxide-win-x64-Setup.exe) |
| Desktop | Linux x64 | [Oxide-linux-x86_64-Setup.AppImage](https://github.com/jaysonwu991/oxide/releases/download/desktop-latest/Oxide-linux-x86_64-Setup.AppImage) · [Oxide-linux-x64-Setup.tar.gz](https://github.com/jaysonwu991/oxide/releases/download/desktop-latest/Oxide-linux-x64-Setup.tar.gz) |
| Desktop | Linux arm64 | [Oxide-linux-arm64-Setup.AppImage](https://github.com/jaysonwu991/oxide/releases/download/desktop-latest/Oxide-linux-arm64-Setup.AppImage) · [Oxide-linux-arm64-Setup.tar.gz](https://github.com/jaysonwu991/oxide/releases/download/desktop-latest/Oxide-linux-arm64-Setup.tar.gz) |
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
oxide mcp add --transport http atlassian https://mcp.atlassian.com/v1/mcp
oxide plugin marketplace add <url|path|owner/repo>
oxide plugin install <name>[@marketplace]
```

Keyboard shortcuts, the transcript layout, and copy behavior are in
[docs/tui.md](docs/tui.md); print/JSON/RPC modes and `oxide session` are in
[docs/modes.md](docs/modes.md).

## Desktop app

The `oxide-desktop` package (`crates/desktop`) is an [Electron](https://electronjs.org)
front-end for the same `oxide-core` agent, with the window written in TypeScript
and the engine it talks to the same Rust harness the terminal runs. It shares the
CLI's configuration (`config.json`,
`auth.json`, `settings.json`) and its session store, and adds a multi-project
sidebar: any folder can be added, every project you have run the CLI in is
discovered from its sessions, and each project lists its own threads alongside a
cross-repo view of recent ones. Chat runs the same agent loop through
`oxide-core`, streaming text, tool calls, and token usage over the window's own
bridge to the engine beside it.

The package is its own workspace (`exclude`d from the root one) with its own pnpm
tree, so it is built from its own directory:

```sh
cd crates/desktop && pnpm install && pnpm start
```

Prebuilt installers are in [Installers](#installers). See
[docs/desktop.md](docs/desktop.md) for the layout, approvals, change cards,
shortcuts, signing, and packaging.

## VS Code extension

Install the CLI first: the extension is a chat panel over your installed `oxide`
binary and bundles no engine of its own.

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
| [docs/install.md](docs/install.md) | Install and update the CLI, the desktop app and the extension; supported platforms, `oxide update`, `oxide uninstall`. |
| [docs/tui.md](docs/tui.md) | Terminal layout, keyboard shortcuts, copying. |
| [docs/modes.md](docs/modes.md) | `-p/--print`, `--mode json`, `--mode rpc`, `oxide session`, a turn's changes. |
| [docs/configuration.md](docs/configuration.md) | `config.json`, CLI flags, environment variables, providers, trust, themes, compaction, data locations. |
| [docs/ecosystem.md](docs/ecosystem.md) | The `.oxide/` layout, Claude Code compatibility, built-in slash commands. |
| [docs/tools.md](docs/tools.md) | Tool parameters, behavior, output caps, concurrency. |
| [docs/cli.md](docs/cli.md) | Task-by-task guide: MCP servers, subagents, commands, skills, plugins, permissions, reasoning, attachments, notifications. |
| [docs/desktop.md](docs/desktop.md) | Desktop app architecture, interface, packaging, signing. |
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
`oxide` terminal binary). `crates/desktop` is a workspace of its own — excluded
from the root one, because its binary is packaged by electron-builder and
versioned with the app — so the plain workspace build stays free of the window's
dependencies; the VS Code extension is a separate pnpm package:

```sh
cd crates/desktop && pnpm install && cargo test
node crates/desktop/check-app.mjs && node crates/desktop/check-shell.mjs
cd editors/vscode && pnpm test
```

See [CONTRIBUTING.md](CONTRIBUTING.md) for architecture notes and guidelines.

## License

MIT
