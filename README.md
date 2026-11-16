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

A native Rust AI coding agent with a terminal UI, two independent desktop apps
— Oxide Desktop GPUI and Oxide Desktop Electron — and a VS Code extension.
Oxide streams from
OpenAI-compatible and Anthropic models, runs a tool-using agent loop against
your project, and understands its own `.oxide/` configuration layout out of the
box, with Claude Code configuration support for compatibility.

## Highlights

- **One core, several front-ends** — a ratatui terminal UI, the recommended
  native GPUI desktop app (`crates/desktop-gpui`), an independently supported
  Electron desktop app (`crates/desktop`), and a VS Code extension
  (`editors/vscode`), all sharing the same configuration, sessions, trust
  decisions, and MCP servers.
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

Oxide Desktop GPUI, Oxide Desktop Electron, and the VS Code extension are always
the newest build. GPUI links use `desktop-latest`, Electron links use its
independent `desktop-electron-latest` alias, and the extension uses
`extension-latest`; their files are replaced on every publish, so the links
themselves never change. Versioned `desktop-gpui-v*`, `desktop-v*`, and
`extension-v*` releases name the version a file is from.

| Surface | Platform | Installer |
| --- | --- | --- |
| Oxide Desktop GPUI | macOS, Apple Silicon | [Oxide-macos-arm64.dmg](https://github.com/jaysonwu991/oxide/releases/download/desktop-latest/Oxide-macos-arm64.dmg) |
| Oxide Desktop GPUI | macOS, Intel | [Oxide-macos-x64.dmg](https://github.com/jaysonwu991/oxide/releases/download/desktop-latest/Oxide-macos-x64.dmg) |
| Oxide Desktop GPUI | Windows x64 | [Oxide-win-x64-Setup.tar.gz](https://github.com/jaysonwu991/oxide/releases/download/desktop-latest/Oxide-win-x64-Setup.tar.gz) |
| Oxide Desktop GPUI | Linux x64 | [Oxide-linux-x64-Setup.tar.gz](https://github.com/jaysonwu991/oxide/releases/download/desktop-latest/Oxide-linux-x64-Setup.tar.gz) |
| Oxide Desktop GPUI | Linux arm64 | [Oxide-linux-arm64-Setup.tar.gz](https://github.com/jaysonwu991/oxide/releases/download/desktop-latest/Oxide-linux-arm64-Setup.tar.gz) |
| Oxide Desktop Electron | macOS, Apple Silicon | [Oxide-macos-arm64.dmg](https://github.com/jaysonwu991/oxide/releases/download/desktop-electron-latest/Oxide-macos-arm64.dmg) |
| Oxide Desktop Electron | macOS, Intel | [Oxide-macos-x64.dmg](https://github.com/jaysonwu991/oxide/releases/download/desktop-electron-latest/Oxide-macos-x64.dmg) |
| Oxide Desktop Electron | Windows x64 | [Oxide-win-x64-Setup.exe](https://github.com/jaysonwu991/oxide/releases/download/desktop-electron-latest/Oxide-win-x64-Setup.exe) |
| Oxide Desktop Electron | Linux x64 | [AppImage](https://github.com/jaysonwu991/oxide/releases/download/desktop-electron-latest/Oxide-linux-x86_64-Setup.AppImage) · [tar.gz](https://github.com/jaysonwu991/oxide/releases/download/desktop-electron-latest/Oxide-linux-x64-Setup.tar.gz) |
| Oxide Desktop Electron | Linux arm64 | [AppImage](https://github.com/jaysonwu991/oxide/releases/download/desktop-electron-latest/Oxide-linux-arm64-Setup.AppImage) · [tar.gz](https://github.com/jaysonwu991/oxide/releases/download/desktop-electron-latest/Oxide-linux-arm64-Setup.tar.gz) |
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

## Oxide Desktop GPUI

`crates/desktop-gpui` is the primary desktop app: the same agent drawn with
[GPUI](https://gpui.rs) in one native process. It links the desktop engine
in-process (`oxide_desktop::commands::dispatch`), so it reads and writes the same
projects, sessions, approvals, configuration, themes and language as the
terminal and editor integration. It manages multiple projects and their threads
from one window, with streamed turns, reviews, worktrees, terminals, plugins,
MCP servers and AccessKit accessibility.

**Oxide Desktop GPUI is the recommended desktop app.** It owns the
`desktop-latest` installer links and receives new desktop feature work first.
Oxide Desktop Electron remains a fully supported, independent desktop app with
the same Rust engine and shared configuration, sessions, approvals and MCP
servers; it has its own release train so either app can be installed and updated
separately.

The package is its own Cargo workspace:

```sh
cd crates/desktop-gpui && cargo run
```

Prebuilt installers are in [Installers](#installers). See
[docs/desktop-gpui.md](docs/desktop-gpui.md) for the complete surface,
accessibility, signing and packaging.

### Oxide Desktop Electron

`crates/desktop` is the independent [Electron](https://electronjs.org) desktop
app. It drives the same Rust engine through its JSON bridge and shares the same
stores and project state while keeping its own window implementation, installer
and `desktop-v*` release train. GPUI is recommended for new installs, but the
Electron app is a supported first-class frontend rather than a fallback.

```sh
cd crates/desktop && pnpm install && pnpm start
```

See [docs/desktop.md](docs/desktop.md) for the Electron desktop app and its packaging.

## Oxide Computer Use

**`Oxide Computer Use` is a development build and has not been signed with an
Apple Developer ID or notarized by Apple.** It uses ad-hoc code signing when no
identity is supplied, so macOS Gatekeeper can warn about or block it, and OS
permissions may need to be granted again after rebuilding. Do not treat it as an
Apple-verified production distribution. See the
[helper guide](crates/desktop/computer-use/README.md) before installing it.

## VS Code extension

Install the CLI first: the extension is a chat panel over your installed `oxide`
binary and bundles no engine of its own.

The `editors/vscode` package is a TypeScript extension (a separate pnpm package,
not a Cargo workspace member) that drives the same `oxide` binary from a chat
panel and from editor actions. It shells out to `oxide --mode rpc`, so it reads
the same provider logins, `config.json`, `sessions/`, `trust.json`, `AGENTS.md`,
agents, skills, plugins, and MCP servers as the terminal and both desktop
front-ends:
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
| [docs/install.md](docs/install.md) | Install and update the CLI, Oxide Desktop GPUI, Oxide Desktop Electron, and the extension; supported platforms, `oxide update`, `oxide uninstall`. |
| [docs/tui.md](docs/tui.md) | Terminal layout, keyboard shortcuts, copying. |
| [docs/modes.md](docs/modes.md) | `-p/--print`, `--mode json`, `--mode rpc`, `oxide session`, a turn's changes. |
| [docs/configuration.md](docs/configuration.md) | `config.json`, CLI flags, environment variables, providers, trust, themes, compaction, data locations. |
| [docs/ecosystem.md](docs/ecosystem.md) | The `.oxide/` layout, Claude Code compatibility, built-in slash commands. |
| [docs/tools.md](docs/tools.md) | Tool parameters, behavior, output caps, concurrency. |
| [docs/cli.md](docs/cli.md) | Task-by-task guide: MCP servers, subagents, commands, skills, plugins, permissions, reasoning, attachments, notifications. |
| [docs/desktop-gpui.md](docs/desktop-gpui.md) | Primary native Oxide Desktop GPUI. |
| [docs/desktop.md](docs/desktop.md) | Oxide Desktop Electron architecture, interface, packaging, and signing. |
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
`oxide` terminal binary). `crates/desktop-gpui` and `crates/desktop` are
separate Cargo workspaces: GPUI is the recommended native desktop app, while
Electron is an independently supported desktop app packaged with
electron-builder. The VS
Code extension is a separate pnpm package:

```sh
cd crates/desktop-gpui && cargo test && cargo clippy --all-targets -- -D warnings
cd crates/desktop && pnpm install && cargo test
node crates/desktop/check-app.mjs && node crates/desktop/check-shell.mjs
cd editors/vscode && pnpm test
```

See [CONTRIBUTING.md](CONTRIBUTING.md) for architecture notes and guidelines.

## License

MIT
