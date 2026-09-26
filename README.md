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

## Features

- Interactive TUI (ratatui) plus non-interactive `-p/--print`, `--mode json`
  (JSONL event stream), and `--mode rpc` (JSONL over stdin/stdout) modes. In the
  TUI, `/login` (`/connect`) and `/logout` manage provider credentials.
- Desktop app (`oxide-desktop`, Tauri) that manages multiple projects and shows
  the shared session store, using the same configuration as the CLI (see
  [docs/desktop.md](docs/desktop.md)).
- VS Code extension (`editors/vscode`) that drives the same `oxide` binary from
  a chat panel in the activity bar, with editor actions for the selection and
  the CLI's own sessions and configuration (see [docs/vscode.md](docs/vscode.md)).
- OpenAI-compatible (OpenAI, DeepSeek, Portkey, Z.AI/GLM, custom) and
  Anthropic Messages API clients.
- Built-in tools under Pi-style names: `read`, `write`, `edit`, `bash`, `grep`,
  `find`, `ls`, `webfetch`. Compatibility names (`read_file`, `write_file`,
  `list_dir`, `glob`) and the unified-diff `patch` tool are accepted everywhere,
  including in permission rules.
- Agent-level tools: `task` (subagents), `skill` (on-demand skill loading),
  `command` (model-invoked commands), `memory` (cross-session notes), and
  `diagnostics` (LSP diagnostics).
- MCP servers over stdio or Streamable HTTP, loaded on demand with automatic
  tool selection, OAuth discovery, and session handling, exposed as
  `<server>__<tool>`.
- Multimodal prompts: attach images/PDFs with `--image` or `@path` references,
  and pass prompt files as `oxide @file "message"`.
- Project + global ecosystem discovery: instructions, commands, prompt
  templates, agents, skills, MCP servers, and plugins from `.oxide/` (plus the
  Claude Code layout).
- Pi-compatible sessions stored as JSONL trees (`id`/`parentId` entries with
  in-file compaction and branch summaries), shadow-git snapshots (`/undo`,
  `/redo`), Pi-style context compaction that runs automatically near the model
  window (`/compact`), and branch summarization when branching (`/tree <n>`,
  `/fork <n>`).
- LSP diagnostics via rust-analyzer, typescript-language-server, pyright, gopls;
  a language server that crashes or closes its pipe is evicted and respawned on
  the next edit instead of poisoning every later call with a broken pipe.
- Plugin hooks (`tool.execute.before` / `tool.execute.after`, plus `status` for
  a footer status row) run under bun/node, and an `after` hook can end the turn
  by setting `output.terminate = true`.
- Claude Code-style plugin packages and marketplaces: install plugins that
  bundle commands, agents, skills, MCP servers, and command hooks behind a
  `.oxide/plugin.json` (or `.claude-plugin/plugin.json`) manifest, from a
  marketplace declared by `.oxide/marketplace.json` (or
  `.claude-plugin/marketplace.json`). MCP servers can come from the manifest's
  `mcpServers` or a plugin-root `.mcp.json`. Manage them with `oxide plugin`
  and `/plugins`, or browse marketplaces and their plugins interactively with
  `/marketplaces`.
- Agent-harness niceties: read-only tool calls in a batch run in parallel while
  preserving model order, `bash` output streams into the UI as it arrives, and
  typing while the agent works steers it between steps. Enter queues a steering
  message while busy; Alt+Enter queues a follow-up delivered after all work
  finishes.
- Evidence-based completion: the system prompt's Definition of Done requires the
  model to confirm the outcome of any state-changing action before claiming
  success — edits on disk and builds/tests, a pull request's CI and
  mergeability, posted comments and reviews, releases, and deployments. A code
  change made in response to a pull request or review is not delivered until it
  is committed and pushed to the branch under review, so a review reply is held
  until the fix is pushed and an uncommitted fix is
  never reported as an addressed review. A run
  that tries to finish with unconfirmed edits or unchecked side effects gets one
  hidden reminder to verify before it can summarize; a reminder the provider
  answers with nothing ends the run with the summary the model already wrote,
  rather than reporting an empty response on top of a finished answer. A
  separate Scope rule keeps
  commits limited to the task: blanket staging (`git add -A`, `git commit -a`)
  is held until the model reviews the staged files, so local-only files like
  `.claude/settings.local.json` stay out of the PR.
- Read-only runs via the tool allowlist, e.g.
  `oxide -t read,grep,find,ls -p "review this"`.
- Reasoning effort: `auto` (default), `off`, `low`, `medium`, or `high`, cycled
  in the TUI with Shift+Tab or set with `--reasoning` / `OXIDE_REASONING`. `auto`
  uses the provider/model's native behavior; explicit levels map to
  OpenAI-compatible effort, Anthropic adaptive thinking, or legacy extended
  thinking as appropriate.
- Compact, bounded output: tool bodies render as background-filled panels with
  a short, readable preview by default (compact JSON is expanded, and long output
  is cut to a per-tool budget: shell tail 5 lines, `read` 10, `grep` 15,
  `find`/`ls` 20), colored by state (pending, success, or error), with long
  action lines and wrapped output continuations aligned so the full text stays
  readable, and tool results are capped by lines and bytes before they enter the
  model's context. Capped output is saved to disk with a pointer so it stays
  recoverable.
- Compact agent transcript: shell calls render as `→ Run <command>` and finish
  as `→ Ran <command> · exit <code>` (`→ Run failed …` on a non-zero exit),
  with a `(timeout Ns)` hint when the call
  sets one, a live `Elapsed Ns` while it runs, and a `Took Nms` duration
  afterwards (any other tool that runs for at least 500 ms is timed too).
  Long output is previewed with a
  `⋯ <lines> more/earlier lines · Ctrl+O to expand` affordance, `read` results
  preview the file contents, file edits show a colored line-numbered diff, user
  and assistant turns render their label inline with the message text, and the
  system prompt nudges the model to batch reads instead of re-reading the same
  paths.
- Visible work in progress: reasoning streams into the transcript as a muted
  italic `✦ Thinking` block that closes with `✦ Thought for 1.4s`, so work done
  before the answer is no longer invisible. Ctrl+T collapses reasoning blocks to
  their label (`· Ctrl+T to expand`); set `hideThinkingBlock` in `settings.json`
  to start collapsed, as in Pi. A running `task` subagent reports its own
  progress — a live `Elapsed` on any tool panel, a
  `↳ <agent> · <activity> · <n> call(s)` line, and a status row naming the
  subagent and its current tool. The status row also reports the phase of the
  current step (`thinking...`, `running tool...`, `compacting...`, `summarizing
  branch...`) and surfaces stream retries as `retrying (n/3) in Ns...`.
- Desktop notifications: a finished agent turn raises a system toast with a
  short snippet of the reply and plays the platform alert sound (Notification
  Center on macOS, `notify-send` plus the freedesktop `complete` sound on Linux,
  a Windows toast), so long runs can finish while you are in another window;
  internal work like `/compact` stays silent. Enabled by default and tuned with
  `/notify [on|off]` and `/notify sound [on|off]` in the TUI, the
  `notifyOnComplete` / `notifySound` keys in `settings.json`, or
  `OXIDE_NOTIFY_ON_COMPLETE` / `OXIDE_NOTIFY_SOUND`.
- Resilient streaming: transient failures (network errors, truncated streams,
  429, and 5xx responses) and a turn that comes back with neither text nor tool
  calls are retried with backoff. A stream that drops after it has already
  emitted part of the reply is retried too — the failed attempt is discarded so
  the retry streams fresh instead of extending the partial text — and only
  after that budget is exhausted do empty or truncated responses and in-band
  stream errors surface as errors instead of silently ending the turn. Text the
  last attempt streamed is kept in the session, so the next message can
  continue from what you already saw.
- Tool selection: `--tools`/`-t` allowlists and `--exclude-tools`/`-x`
  disables tools (accepting both Pi and legacy names); disabled tools are hidden
  from the model and refused if requested.
- Pi-style session flags: `--session <path|id>`, `--no-session`, `--name`,
  `-c`/`--continue`, `-r`/`--resume` (browse past sessions), and
  `--fork <path|id>`, plus TUI commands `/new`, `/session`, `/resume`, `/name`,
  `/model`, `/thinking`, `/export`, `/reload`, and `/hotkeys`.
- Session branching: `/tree` lists user messages and `/tree <n>` branches the
  current session in place (summarizing the abandoned path), `/fork <n>`
  branches a new session from one, and `/clone` duplicates the current session.
- Project trust: project-local resources (agents, commands, prompts, skills,
  plugins, `SYSTEM.md`) load only after the project is trusted; decisions are
  saved per directory in `trust.json`, `defaultProjectTrust` sets the fallback,
  `--approve`/`-a` and `--no-approve` override for one run, and `/trust` saves a
  decision.
- Themes: built-in `dark` and `light` plus custom `.oxide/themes/<name>.json`,
  selected with `--use-theme` or `/theme`. The built-in palettes are shared with
  the desktop app, so the CLI and desktop render the same colors.
- Portkey spend bar: with a Portkey login, `/usage` opens a settings dialog
  that adds a full-width bar at the bottom of the screen showing the user, this
  session's cost, and today's and the month's spend from the Portkey analytics
  API against an optional monthly budget in `$` or `¥`.
- Focused terminal layout: the welcome banner stacks the block-letter `OXIDE`
  wordmark above a short summary of the loaded ecosystem, context files, and
  MCP/plugin/memory state; current activity and elapsed time live in the status
  row, and the footer shows the abbreviated working directory with the git
  branch and session name, cumulative usage (`↑`/`↓`, `R`/`W` cache tokens and
  `CH` hit rate when reported, `$cost` from the model price table, including
  summary generation), context usage as `%`/window with an `(auto)` marker, and
  the right-aligned model and thinking level; plugins can add a third status
  row, and the Portkey spend bar adds a final one when it is enabled. The editor
  matches Pi: full-width top and bottom rules colored by the thinking level that
  grow to 12 rows, and semantic colors keep dark, light, and custom themes
  consistent.

## Comparison

Oxide is a small, native terminal agent that deliberately borrows the Claude
Code configuration layout so existing `.claude/` setups keep working. The table
below compares the high-level shape of the four tools; feature sets move fast,
so check each project's documentation for the current details.

| Capability | Oxide | [Codex](https://github.com/openai/codex) | [OpenCode](https://opencode.ai) | [Claude Code](https://docs.claude.com/en/docs/claude-code/overview) |
| --- | --- | --- | --- | --- |
| Distribution | Native Rust core + Tauri desktop app + VS Code extension | Open-source CLI (Rust) + IDE extension | Open-source CLI (Node/Bun) | Proprietary CLI + apps |
| License | MIT | Apache-2.0 | Open source | Proprietary |
| Model providers | OpenAI-compatible (OpenAI, DeepSeek, Portkey, Z.AI/GLM, custom) + Anthropic Messages API | OpenAI models (GPT-5-Codex family) + custom providers | Any provider (bring your own keys) | Claude (Anthropic API, Bedrock, Vertex, third-party) |
| Interfaces | Terminal TUI, `-p` print, JSON/RPC modes, desktop app, VS Code extension | Terminal CLI, IDE (VS Code, Cursor) | Terminal, desktop, IDE, web | Terminal, IDE, desktop, web |
| Project config | `.oxide/` + `AGENTS.md` (also reads `.claude/`) | `AGENTS.md` + `~/.codex/config.toml` | `opencode.json` + `AGENTS.md` | `CLAUDE.md` + `.claude/` |
| Subagents | `--agent`, `task`, command routing | Subagents | Agents | Subagents, background agents |
| Reasoning effort | `auto` / `off` / `low` / `medium` / `high` (Shift+Tab, `--reasoning`) | `--reasoning-effort` (model-dependent) | Model-dependent | Extended thinking |
| Slash commands | `.oxide/commands` + `.oxide/prompts`, `agent`/`subtask` routing | Commands (`~/.codex/prompts`, `prompts/`) | Commands | Commands |
| Skills | `SKILL.md` | `SKILL.md` | Agent Skills | Skills |
| MCP servers | stdio + HTTP + OAuth, managed with `oxide mcp` | MCP servers (`codex mcp`, config.toml) | MCP servers | MCP servers |
| Plugins / hooks | Hooks + plugin packages & marketplaces | — | Plugins | Hooks, plugins, Agent SDK |
| LSP diagnostics | Built in (rust-analyzer, TS, pyright, gopls) | — | Built in (LSP servers) | — |
| Undo file changes | Shadow-git `/undo`, `/redo` | Git checkpoints (`codex checkpoint`) | `/undo`, `/redo` | Git / checkpoints |
| Sessions | Pi-style JSONL trees, `-c` / `-r`, `/resume` / `/tree` / `/fork` / `/clone` | Sessions (`codex --resume`) | Sessions, share links | Sessions across surfaces |
| Project trust | `trust.json`, `--approve` / `/trust` | Sandbox + approval modes | — | — |
| Themes | Built-in `dark` / `light`, custom `.oxide/themes` | Built-in themes (`codex themes`) | Themes | — |
| Context management | Auto-compaction + branch summarization | Auto-compaction | Auto-compaction + DCP plugin | Auto-compaction |
| Multimodal input | Images and PDFs (`--image`, `@path`) | Images | Images | Images |

A dash indicates no first-class built-in equivalent. Where Oxide differs most:
it is a dependency-light Rust core with a terminal binary and a Tauri desktop
app, it speaks both the OpenAI-compatible and Anthropic APIs directly, its plugin
packages reuse the same on-disk commands, agents, skills, and MCP servers the
ecosystem already reads, and it is compatible with the Claude Code on-disk layout
while using its own `.oxide/` format.

## Installation

### Prebuilt binary

#### macOS and Linux

```sh
curl -fsSL https://github.com/jaysonwu991/oxide/releases/latest/download/install.sh | bash
```

The installer detects your OS/arch, downloads the matching release, verifies its
SHA-256 checksum, and installs `oxide` to `~/.local/bin` by default.

Overrides:

| Variable | Purpose |
| --- | --- |
| `OXIDE_VERSION` | Version to install (with or without a leading `v`). Defaults to the latest release. |
| `OXIDE_INSTALL_DIR` | Install directory. Defaults to `$HOME/.local/bin`. |
| `OXIDE_REPO` | GitHub repo slug. Defaults to `jaysonwu991/oxide`. |

#### Windows (x86_64)

```powershell
irm https://github.com/jaysonwu991/oxide/releases/latest/download/install.ps1 | iex
```

The PowerShell installer detects your OS/arch, downloads the matching release,
verifies its SHA-256 checksum, and installs `oxide.exe` to
`%LOCALAPPDATA%\Programs\Oxide` by default. The same script also runs on macOS
and Linux under PowerShell (`pwsh`), installing `oxide` to `~/.local/bin`.

Overrides:

| Variable | Purpose |
| --- | --- |
| `OXIDE_VERSION` | Version to install (with or without a leading `v`). Defaults to the latest release. |
| `OXIDE_INSTALL_DIR` | Install directory. Defaults to `%LOCALAPPDATA%\Programs\Oxide` on Windows, `$HOME/.local/bin` elsewhere. |
| `OXIDE_REPO` | GitHub repo slug. Defaults to `jaysonwu991/oxide`. |

### Supported platforms

| Platform | Rust target | Archive |
| --- | --- | --- |
| macOS (Apple Silicon) | `aarch64-apple-darwin` | `.tar.gz` |
| macOS (Intel) | `x86_64-apple-darwin` | `.tar.gz` |
| Linux (x86_64) | `x86_64-unknown-linux-gnu` | `.tar.gz` |
| Linux (ARM64) | `aarch64-unknown-linux-gnu` | `.tar.gz` |
| Windows (x86_64) | `x86_64-pc-windows-msvc` | `.zip` |

### From source

Requires a stable Rust toolchain (edition 2021). The workspace splits the
shared agent core (`crates/core`), the terminal CLI (`crates/cli`),
and the desktop app (`crates/desktop`); the VS Code extension in
`editors/vscode` is a separate pnpm package.

```sh
cargo install --path crates/cli
```

## Quick start

```sh
# Launch the TUI in the current project
oxide

# Then connect a provider from inside the TUI
/login
```

### Keyboard shortcuts

The welcome area stacks the `OXIDE` wordmark above a short summary of the
loaded ecosystem, context files, and MCP/plugin/memory state. Run `/hotkeys`
for the full shortcut list. The status row above the editor shows the current
activity and elapsed time, and the footer shows the project path with the Git
branch and session name, cumulative usage (including cache and cost), context
usage, and the current model and thinking level (plus a plugin status row when
present).

| Key | Action |
| --- | --- |
| Enter | Send a message; while the agent is busy, queue guidance for its next step. |
| Shift+Enter | Insert a newline without sending. |
| Alt+Enter | While busy, queue a follow-up to run after the current work finishes. |
| Alt+Up / Option+Up | Pull every queued message back into the message box to edit or extend it (`Alt+Q` on Windows and WSL, where the terminal owns `Alt+Up`). |
| Esc | Clear the input. In dialogs, cancel or close. |
| `/` | Open slash-command autocomplete. |
| `@` | Open file/folder path autocomplete to add a file to the prompt. |
| Tab | Complete the selected slash-command (including fixed arguments such as `/notify sound on`) or `@path` suggestion. |
| Up / Down | Move through the suggestion list, or recall input history when it is closed. |
| Shift+Tab | Cycle the thinking level: `auto` → `off` → `low` → `medium` → `high`. |
| Ctrl+O | Expand or collapse tool-output details. |
| Ctrl+T | Show or hide reasoning (`✦ Thinking`) blocks. |
| Ctrl+V | Attach an image from the clipboard when the platform helper is available. |
| PgUp / PgDn / mouse wheel | Scroll the transcript. |
| Ctrl+A | Jump to the start of the message box (when it is not empty). |
| Ctrl+E | Jump to the end of the message box; when it is empty, scroll down one line. |
| Ctrl+Y | Scroll up one line. |
| Ctrl+U / Ctrl+D | Scroll half a page up / down. |
| Ctrl+G / Home | Scroll to the top. |
| End | Return to the latest message and resume automatic scrolling. |
| Ctrl+C | Copy the current mouse selection, or quit when there is none. |

Dragging over the transcript copies the selected text on release and confirms it
with a dim `copied 243 chars` line; `/copy` and `/copy all` report the same way,
and repeated copies update that one line instead of stacking up. Copying writes
an OSC 52 sequence, so it reaches the clipboard even over SSH or inside tmux or
screen, and also tries the native helper (`pbcopy`/`osascript` on macOS,
`wl-copy`/`xclip`/`xsel` on Linux) when one is available.

A few keys act twice depending on the message box: Ctrl+E moves to the end of a
non-empty message and otherwise scrolls the transcript, and Ctrl+C copies an
active mouse selection instead of quitting.

Run `/hotkeys` for the in-app list and `/help` for commands, agents, and skills.

Or provide credentials through the environment:

```sh
export OPENAI_API_KEY=sk-...
oxide
```

Non-interactive use:

```sh
oxide -p "summarize this repository"
echo "explain src/main.rs" | oxide -p
oxide -p "review the diff" --image screenshot.png
```

Manage MCP servers:

```sh
oxide mcp add filesystem npx -y @modelcontextprotocol/server-filesystem .
oxide mcp add --transport http atlassian https://mcp.atlassian.com/v1/mcp
oxide mcp list
```

Install plugins from a marketplace:

```sh
oxide plugin marketplace add <url|path|owner/repo>
oxide plugin install <name>[@marketplace]
```

## Desktop app

The `oxide-desktop` package (`crates/desktop`) is a Tauri v2 front-end for the
same `oxide-core` agent. It shares the CLI's configuration (`config.json`,
`auth.json`, `settings.json`) and its session store, and adds a multi-project
sidebar:

- **Projects** — add any folder to the sidebar; every project you have run the
  CLI in is discovered automatically from its sessions.
- **Sessions** — a per-project session list with previews, plus a cross-repo
  view of recent sessions from every project.
- **Chat** — runs the same agent loop through `oxide-core`, streaming text, tool
  calls, and token usage over Tauri events.

The project/session/turn logic lives in the `oxide_desktop` library and is unit
tested without a webview. The Tauri shell is behind the `gui` feature so the
default workspace build stays GUI-free:

```sh
cargo run -p oxide-desktop --features gui
```

It also has an interactive approval prompt for `ask` rules (with per-project
"Always allow" memory), a **Connect** dialog that writes the same
`auth.json`/`config.json` the CLI uses, a model picker and reasoning control,
graceful cancel and mid-run steering, session rename/delete, colored
diffs for `write`/`edit` results, Markdown with tables and syntax highlighting,
clickable links that open in the system browser,
token/cost usage in the footer, and built-in Dark/Light themes (default Dark)
that read the same `.oxide/themes` files as the CLI. Prebuilt bundles are
drafted under `desktop-v*` releases on the
[releases page](https://github.com/jaysonwu991/oxide/releases); to build from
source, bundle it with `npx @tauri-apps/cli@^2 build --features gui`. See
[docs/desktop.md](docs/desktop.md) for the full layout, shortcuts, signing, and
packaging details.

The front-end (`crates/desktop/ui/`) is plain HTML/CSS/JS; the Rust
commands in `crates/desktop/src/commands.rs` back it.

## VS Code extension

The `editors/vscode` package is a TypeScript extension (a separate pnpm
package, not a Cargo workspace member) that drives the same `oxide` binary from
a chat panel in the activity bar and from editor actions. It shells out to
`oxide --mode json -p`, so it reads the same provider logins, `config.json`,
`sessions/`, `trust.json`, `AGENTS.md`, agents, skills, plugins, and MCP servers
as the terminal and the desktop app:

- **Chat panel** — streaming replies rendered as Markdown, `✦ Thinking` blocks,
  collapsible tool cards with file diffs, token/cost usage, and a composer that
  queues follow-ups while a turn runs.
- **Editor actions** — explain, fix, or ask about a selection; attach a file, a
  selection, or an image to the chat.
- **Sessions** — continue the project's latest session or pick one from the
  CLI's own list.
- **`@path` in a message** — `@src/main.rs` attaches the file's text; an image
  or PDF becomes a media attachment.

Build it with:

```sh
cd editors/vscode
pnpm install
pnpm run compile   # tsc -p .
pnpm test          # compile, then node --test out/test/
pnpm run package   # vsce package -> oxide-vscode-<version>.vsix
```

Provider logins stay in the CLI (`/login` in the TUI). See
[editors/vscode/README.md](editors/vscode/README.md) for the command and setting
tables, and [docs/vscode.md](docs/vscode.md) for the architecture.

## CLI

```
oxide [OPTIONS] [@files...] [PROMPT...]
oxide mcp <COMMAND>
oxide plugin <COMMAND>
oxide sessions <COMMAND>
oxide uninstall [--keep-config] [--keep-data] [--dry-run] [--force]
```

| Flag | Description |
| --- | --- |
| `[PROMPT]...` | Prompt words. `@path` reads a file into the prompt (images/PDFs become attachments). Providing one implies non-interactive mode. |
| `-m, --model <MODEL>` | Model to use (overrides config). |
| `--provider <PROVIDER>` | Provider name (overrides config). |
| `--agent <AGENT>` | Agent to run, from `.oxide/agents` (or `.claude/agents`). |
| `--mode <MODE>` | Output mode: `print`, `json`, or `rpc` (defaults to print for a prompt). |
| `--reasoning <LEVEL>` | Reasoning effort: `auto` (default), `off`, `low`, `medium`, or `high`. |
| `--system-prompt <TEXT>` | Replace the default system prompt for this run. |
| `--append-system-prompt <TEXT>` | Append text to the system prompt (repeatable). |
| `--no-context-files` | Disable `AGENTS.md`/`CLAUDE.md` context-file discovery. |
| `-t, --tools <LIST>` | Allowlist tools (comma-separated, Pi or legacy names). |
| `-x, --exclude-tools <LIST>` | Disable tools (comma-separated). |
| `-a, --approve` | Trust project-local resources for this run. |
| `--no-approve` | Ignore project-local resources for this run. |
| `--use-theme <NAME>` | TUI theme (`dark`, `light`, or a custom `.oxide/themes` file). |
| `--session <PATH\|ID>` | Use a specific session file or id. |
| `-n, --name <NAME>` | Set the session display name at startup. |
| `--no-session` | Ephemeral mode: do not save the session. |
| `-p, --print` | Print the response and exit instead of launching the TUI. |
| `-c, --continue` | Resume the most recent session for this project. |
| `-r, --resume` | Browse and select a past session to resume. |
| `--fork <PATH\|ID>` | Fork a session file or id into a new session. |
| `--image <PATH>` | Attach an image or PDF (repeatable). |
| `-C, --cwd <DIR>` | Working directory for the agent. |

Non-interactive examples:

```sh
oxide -p "summarize this repository"
oxide @prompt.md "answer this"          # include a file in the prompt
cat README.md | oxide -p "summarize"    # merge piped stdin
oxide --mode json "list files"           # JSONL events on stdout
oxide --mode rpc                         # JSONL prompts over stdin
oxide -t read,grep,find -p "review"      # read-only tool allowlist
oxide --session <id> -p "continue"       # reuse a specific session
oxide --fork <id> -p "try another path"  # branch a saved session
```

`print` mode writes a step's text once that step commits — at a tool call or the
end of the turn — so a stream the client retries never prints a partial answer
twice; `--mode json` still emits each delta as it arrives.

Manage saved sessions (list, clean up stale ones, compact, and merge):

```sh
oxide sessions list                        # current project
oxide sessions list --all                  # every project
oxide sessions list --older-than 30        # stale sessions only
oxide sessions delete <id>                 # delete one (prompts)
oxide sessions delete --older-than 30 --force
oxide sessions compact <id>                # summarize older history, keep recent tail
oxide sessions compact --all               # refresh every session in this project
oxide sessions merge <a> <b>               # concatenate two sessions into a new one
oxide sessions merge <a> <b> --summarize   # summarize the second session first
```

`delete` is token-free. `compact` and `merge --summarize` make one LLM
summarization pass per session so a stale session resumes from a small summary
plus its most recent messages instead of replaying the full transcript.

Uninstall Oxide and its related files:

```sh
oxide uninstall --dry-run                 # preview removals
oxide uninstall                           # preview, confirm, and uninstall
oxide uninstall --keep-config --keep-data # retain configuration and user data
oxide uninstall --force                   # skip confirmation
```

Package-manager installations are removed through Cargo or Homebrew when
detected. Prebuilt installations print the final command needed to remove the
currently running executable after cleanup completes.

Credential management happens inside the TUI with the Pi-style commands:

```text
/login [provider]    connect a provider and store its API key
/logout [provider]   remove stored credentials
```

`/login` opens a provider picker (`/login <provider>` skips straight to the key;
for a provider that is already connected it switches to it instead of asking for
the key again, and pressing Enter on the key step reuses the stored key). After
the key, an optional settings step lets you set the model, base URL, and (for
Portkey) Config ID, pre-filled with the provider's defaults so Enter keeps them.
Keys are
stored in `auth.json` in the Oxide config directory
(mode `0600`) and resolved after environment variables and before the config
file. Any number of providers can be stored at once, and the active provider is
written to `config.json` so the next launch uses it. `/models` lists the catalogs
of every logged-in provider, and picking a model from another one switches to
it. Each provider remembers the model and custom endpoint it was last used with
in the `provider_models` and `provider_base_urls` maps of `config.json`, so a
gateway configured for one provider does not leak into the others.

MCP server management:

```sh
oxide mcp list
oxide mcp get <name>
oxide mcp add [--scope project|global] [--transport stdio|http] <name> <command|url> [args...]
oxide mcp add-json [--scope project|global] <name> '<json>'
oxide mcp remove [--scope project|global] <name>
oxide mcp auth [--scope project|global] <name>
```

`--scope project` (the default) writes `<root>/.oxide/mcp.json`; `--scope global`
writes `~/.oxide/mcp.json`. See
[docs/cli.md](docs/cli.md#mcp-servers) for examples.

Remote servers can require OAuth. Add the server by URL; Oxide detects a `401`
authentication challenge, discovers the authorization server, runs the
authorization-code flow with PKCE, and refreshes the token automatically. You
can also authorize up front with `oxide mcp auth <name>`. See
[Connecting to the Atlassian Rovo MCP server](docs/cli.md#connect-to-the-atlassian-rovo-mcp-server)
for a worked example.

Configured servers are connected lazily. The model sees a compact `mcp_load`
discovery tool at startup and loads the matching server automatically when a
prompt names its service or contains one of its URLs. OAuth and full tool-schema
discovery therefore happen only when that server is first needed.

Plugin management (Claude Code-style packages and marketplaces):

```sh
oxide plugin marketplace add <url|path|owner/repo>
oxide plugin install <name>[@marketplace]
oxide plugin list
oxide plugin enable <name>
oxide plugin disable <name>
oxide plugin uninstall <name>
oxide plugin marketplace list | update <name> | remove <name>
```

The `owner/repo` shorthand expands to a GitHub clone URL. The TUI
`/marketplaces` command opens an interactive browser for the same operations,
including `Ctrl+U` to fetch a marketplace's latest manifest.

Plugins are packages that bundle slash commands, subagents, skills, MCP servers,
and command hooks behind a `.oxide/plugin.json` or `.claude-plugin/plugin.json`
manifest; a marketplace is a repo or directory with a `.oxide/marketplace.json`
or `.claude-plugin/marketplace.json` manifest. MCP servers can come from the
manifest's `mcpServers` or a plugin-root `.mcp.json` (either a `mcpServers` map
or the server entries directly). Installed plugins live under the Oxide config
directory and load at startup before project resources, so project-local
entries still override plugins with the same name. After installing, `/reload`
picks up new commands, agents, and skills; hooks and MCP servers need a restart.
The `/marketplaces` browser filters the focused pane, matching plugin names
before descriptions. See
[docs/cli.md](docs/cli.md#plugin-packages-and-marketplaces).

## Configuration

Oxide reads `config.json` from the platform config directory:

- Linux: `~/.config/Oxide/config.json`
- macOS: `~/Library/Application Support/Oxide/config.json`
- Windows: `%APPDATA%\Oxide\config.json`

```json
{
  "provider": "deepseek",
  "model": "deepseek-chat",
  "base_url": "https://api.deepseek.com/v1",
  "api_key": "",
  "system_prompt": "You are Oxide...",
  "max_tokens": 8192,
  "auto_approve": true,
  "reasoning": "auto",
  "theme": "dark"
}
```

`api_key` may be left empty when a key is available via `/login` or the
environment. `auto_approve` controls whether tool calls run without prompting;
when `false`, permission rules that resolve to `ask` are denied in
non-interactive mode.

`reasoning` controls how much reasoning effort Oxide requests. `auto` (the
default) leaves reasoning behavior and effort to the provider/model. Newer
Claude models use adaptive thinking without a forced effort; other APIs receive
no effort override. `off`, `low`, `medium`, and `high` force a level using
OpenAI-compatible `reasoning_effort`, Anthropic adaptive thinking with
`output_config.effort`, or a legacy Anthropic thinking budget as appropriate.
In the TUI press Shift+Tab to cycle levels; `--reasoning` and `OXIDE_REASONING` set
the starting level.

`max_tokens` caps the output of a single model turn, reasoning included. When a
reasoning model exhausts that budget on hidden reasoning and returns nothing,
Oxide retries with a doubled budget (up to 32768) before reporting the failure,
so a long-thinking turn recovers instead of ending in an empty response.

### Environment variables

| Variable | Purpose |
| --- | --- |
| `OXIDE_PROVIDER` | Provider name. |
| `OXIDE_MODEL` | Model name. |
| `OXIDE_BASE_URL` | API base URL. |
| `OXIDE_API_KEY` | API key. |
| `OXIDE_REASONING` | Reasoning effort (`auto`, `off`, `low`, `medium`, `high`). |
| `OXIDE_CONTEXT_LIMIT` | Model context window in tokens, used for the footer's context percentage and the compaction threshold (default: the larger of `max_tokens` and 128000). |
| `OXIDE_COMPACTION_ENABLED` | Enable/disable automatic context compaction. |
| `OXIDE_COMPACTION_RESERVE_TOKENS` | Tokens reserved for the response before compaction triggers. |
| `OXIDE_COMPACTION_KEEP_RECENT_TOKENS` | Recent tokens kept verbatim when compacting. |
| `OXIDE_TRUNCATION_DIR` | Directory for saved truncated tool output (default `truncated/` in the config dir). |
| `OXIDE_NOTIFY_ON_COMPLETE` / `OXIDE_NOTIFY_SOUND` | Override the desktop-notification flags (`/notify`). |
| `OXIDE_SETTINGS_FILE` | Override the global `settings.json` path the TUI writes. |
| `OXIDE_USAGE_FILE` | Override the `portkey-usage.json` path for the Portkey spend bar. |
| `OPENAI_API_KEY` / `OPENAI_BASE_URL` | OpenAI credentials. |
| `DEEPSEEK_API_KEY` / `DEEPSEEK_BASE_URL` | DeepSeek credentials. |
| `ANTHROPIC_API_KEY` / `ANTHROPIC_BASE_URL` | Anthropic credentials. |
| `PORTKEY_API_KEY` / `PORTKEY_BASE_URL` | Portkey AI Gateway credentials. |
| `PORTKEY_CONFIG` | Optional Portkey config ID sent as `x-portkey-config`. |
| `PORTKEY_MODELS` | Optional comma-separated model catalog for keys that cannot call `/models`. |
| `ZAI_API_KEY` / `ZAI_BASE_URL` | Z.AI (GLM) credentials. |

### Providers

| Name | API | Default model | Base URL | Key env |
| --- | --- | --- | --- | --- |
| `openai`, `gpt`, `gpt-4`, `gpt-4o` | OpenAI-compatible | `gpt-4o-mini` | `https://api.openai.com/v1` | `OPENAI_API_KEY` |
| `deepseek` | OpenAI-compatible | `deepseek-chat` | `https://api.deepseek.com/v1` | `DEEPSEEK_API_KEY` |
| `anthropic` | Anthropic Messages | `claude-3-5-sonnet-latest` | `https://api.anthropic.com/v1` | `ANTHROPIC_API_KEY` |
| `portkey`, `port-key` | OpenAI-compatible gateway | `claude-sonnet-5` | `https://api.portkey.ai/v1` | `PORTKEY_API_KEY` |
| `zai`, `glm`, `z.ai`, `z-ai`, `zhipu`, `bigmodel` | OpenAI-compatible | `glm-5.3` | `https://api.z.ai/api/paas/v4` | `ZAI_API_KEY` |

Any OpenAI-compatible endpoint can be used by setting `provider`, `base_url`,
`model`, and a key.

### Portkey

Run `/login portkey` in the TUI, or set `PORTKEY_API_KEY`, then select a model
with `/models` or `"model"` in `config.json`. The preset uses
`https://api.portkey.ai/v1`, sends the key as `x-portkey-api-key`, and defaults
to `claude-sonnet-5`. Run `/usage` to open the spend-bar dialog and enable a
full-width bar (session, today, and month cost against an optional monthly
budget in `$` or `¥`) in the TUI.

For custom gateways, Config IDs, environment precedence, and model-catalog
fallbacks, see the full [Portkey configuration](docs/cli.md#portkey)
section. The [Portkey usage
bar](docs/cli.md#portkey-usage-bar) documents the `/usage` dialog and
the `portkey-usage.json` file.

### Z.AI (GLM)

Run `/login glm` in the TUI (or `/login zai`), or set `ZAI_API_KEY`, then pick a
model with `/models`. The preset talks to Z.AI's OpenAI-compatible endpoint
`https://api.z.ai/api/paas/v4` and defaults to `glm-5.3`; `glm-5.3-flash` is the
cheaper, faster option. Z.AI documents no model listing endpoint, so the picker
falls back to a bundled list of current GLM models.

GLM selects thinking with `thinking.type` rather than `reasoning_effort`, and the
GLM-5.3 series only accepts `low`, `high`, or `max`. `/thinking` therefore maps
`low` to `low`, `medium` to `high`, and `high` to `max`; `off` disables thinking
where the model allows it and asks for the lowest effort on GLM-5.3, which always
thinks. GLM prices ship in the built-in table, so the footer's `$cost` works out
of the box.

For the mainland-China BigModel endpoint
(`https://open.bigmodel.cn/api/paas/v4`), set `ZAI_BASE_URL` or `base_url`.

## Context files and system prompt

Oxide loads `AGENTS.md` (or `CLAUDE.md`) as project instructions by walking
every ancestor directory from the filesystem root down to the working
directory, so nested projects layer their instructions. If a directory contains
`AGENTS.override.md`, it replaces `AGENTS.md`/`CLAUDE.md` for that directory
only. The global `~/.oxide/AGENTS.md` is loaded first (lowest precedence).

- Disable ancestor context-file discovery with `--no-context-files`.
- Replace the default system prompt with `.oxide/SYSTEM.md` (project),
  `~/.oxide/SYSTEM.md`, or the corresponding platform config-directory file.
  Append without replacing with `APPEND_SYSTEM.md` in the same locations.
- `--system-prompt <text>` replaces the configured base prompt for one run, and
  `--append-system-prompt <text>` appends to that base (repeatable). A loaded
  `SYSTEM.md` remains the higher-precedence replacement.
- The startup welcome area lists loaded context files, and `/reload` re-reads them.

## Ecosystem

Oxide discovers configuration from the project root (the nearest ancestor
containing `.git`, `.oxide`, or `.claude`) and the user's global scope. Project
entries override global entries with the same name, and the Oxide layout
overrides the Claude Code layout.

**Oxide layout**

- `AGENTS.md` — project memory and instructions
- `.oxide/AGENTS.md` — additional layout-scoped instructions
- `.oxide/agents/*.md` — subagents (frontmatter: `name`, `description`, `mode`,
  `permission`); subagents may spawn subagents one level deep
- `.oxide/commands/*.md` — slash commands (`$ARGUMENTS`, `$1`, `$2`, …; optional
  `agent` and `subtask` frontmatter)
- `.oxide/prompts/*.md` — prompt templates (Pi-style; frontmatter `description`
  and `argument-hint`, arguments `$1`, `$@`, `${1:-default}`, `${@:2:3}`)
- `.oxide/skills/*/SKILL.md` — on-demand skills
- `.oxide/themes/*.json` — TUI color themes (built-in `dark`/`light` plus custom)
- `.oxide/plugins/` — JS/TS plugin hooks
- `.oxide/SYSTEM.md`, `.oxide/APPEND_SYSTEM.md` — replace or extend the system prompt
- `.oxide/mcp.json` — MCP servers (same schema as `.mcp.json`; manage with `oxide mcp`)
- Global scope: `~/.oxide/` and the platform Oxide config directory (the latter
  has higher precedence)

This repository keeps its own agents, commands, prompts, skills, and plugins in
`.oxide/`.

For task-by-task instructions — adding and removing MCP servers, subagents,
slash commands, skills, plugins, and permission rules — see
[docs/cli.md](docs/cli.md).

A command's frontmatter can route it: `agent: <name>` runs the command with that
agent's prompt and permissions, and `subtask: true` runs it in an isolated
subagent context (the command's output is reported back to the main
conversation). For example:

```markdown
---
description: Lint the crate and fix every warning.
agent: build
---

Run `cargo clippy --all-targets -- -D warnings` and fix each finding.
```

**Claude Code compatibility**

Oxide also reads the Claude Code layout, so existing configurations work as-is:

- `CLAUDE.md` — project memory and instructions
- `.claude/CLAUDE.md` — additional layout-scoped instructions
- `.claude/agents/`, `.claude/commands/`, `.claude/skills/`, `.claude/plugins/`
- `.mcp.json` — MCP servers
- Global scope: `~/.claude/`, `~/.claude.json`

Slash commands are expanded from the ecosystem and also include built-ins:
`/help`, `/hotkeys`, `/exit`, `/new`, `/session`, `/resume`, `/tree`, `/fork`,
`/clone`, `/name`, `/model`, `/thinking`, `/theme`, `/trust`, `/export`,
`/reload`, `/init`, `/login`, `/logout`, `/models`, `/mcps`, `/plugins`,
`/marketplaces`, `/notify`, `/usage`, `/connect`, `/undo`, `/redo`, `/compact`,
`/copy`, `/copy all`, and `/skill:<name>`. Discovered commands and prompt
templates can also be invoked by the agent through the `command` tool, and skills load on
demand with `skill` or via `/skill:<name>`.

**Plugin packages** — installed via `oxide plugin` (or `/plugins`), plugin
packages bundle commands, agents, skills, MCP servers, and command hooks behind
a `.oxide/plugin.json` or `.claude-plugin/plugin.json` manifest, discovered from
marketplaces declared by `.oxide/marketplace.json` or
`.claude-plugin/marketplace.json`. They load at startup before project
resources, so project entries still override plugins with the same name. Use
`/marketplaces` to browse marketplaces and install, enable, or remove their
plugins interactively.

## Tools

Built-in file and shell tools use Pi-style names: `read`, `write`, `edit`,
`bash`, `grep`, `find`, `ls`, and `webfetch`. Compatibility names
`read_file`, `write_file`, `list_dir`, and `glob` remain accepted; `patch` is
the unified-diff editing tool. Agent-level tools: `task`, `skill`, `command`,
`memory`, and `diagnostics`. `mcp_load` reveals a configured server on demand;
its tools then appear as `<server>__<tool>`.

| Tool | Parameters |
| --- | --- |
| `read` | `path`, `offset?` (1-based), `limit?` (default 400 lines) |
| `write` | `path`, `content` |
| `edit` | `path`, `edits: [{ oldText, newText }]` |
| `bash` | `command`, `timeout?` in milliseconds (default 120000) |
| `grep` | `pattern`, `path?`, `glob?`, `ignoreCase?`, `regex?`, `context?`, `limit?` |
| `find` | `pattern`, `path?`, `limit?` |
| `ls` | `path?`, `limit?` |
| `patch` | `diff` (unified diff) |
| `webfetch` | `url`, `format?` (`markdown` default, `text`, or raw `html`) |

`edit` performs targeted text replacement: each `oldText` is matched against
the original file (never incrementally) and must be unique, so a non-unique or
missing match is rejected rather than silently corrupting a file. Matching is
byte-exact first, then tolerates trailing whitespace and the `N|` line numbers
`read` prints, so a block copied straight from a read result still lands; when
the text has genuinely changed, the error names the closest region to copy.
Line endings and a leading BOM are preserved. `write` and `edit` append LSP diagnostics for
the edited file. `read`, `ls`, `find`, and `grep` accept absolute paths, so they
can inspect files outside the project without a shell; `ls` marks directories
with a trailing `/` and renders symlink targets as `name -> target`. `find` and
`grep` respect `.gitignore`, and `grep` matches a literal substring by default
(set `regex: true` to treat `pattern` as a regular expression) and prefers
`ripgrep` (`rg`) when it is on `PATH`, falling back to a dependency-free
parallel walker that skips binary files. `webfetch` converts
HTML to Markdown (`format: "markdown"`, the default), readable plain text
(`format: "text"`), or returns the raw body (`format: "html"`); the converter
is dependency-free and handles headings, paragraphs, lists, tables, links,
images, inline and fenced code, blockquotes, and HTML entities.

Tool results are capped before they enter the model's context. The default cap
is 250 lines and 6 KB, but `read` gets a larger budget (400 lines / 16 KB) so an
ordinary source file is returned whole instead of paged in slices; other
per-tool overrides are `bash` 160 lines / 5 KB, `grep`, `find`, and `ls` 160 / 4
KB, `webfetch` 200 / 6 KB, and `write`, `edit`, and `patch` 120 / 3 KB. `read`
splits a line longer than 1 000 characters into continuation chunks
(`offset`/`limit` count those display lines), so an over-long line can be paged
through instead of being cut off.
`bash` keeps the **tail** so the exit code and recent errors survive; other
tools keep the head. When output is dropped, the full text is written under
`truncated/` in the Oxide config directory and the result includes the path plus
a hint to grep it or `read` it with an offset, so the model can recover detail
without re-running the tool. Set `OXIDE_TRUNCATION_DIR` to change where those
files go; they are retained for 7 days.

When the model requests several tools at once, the ones with no side effects
(`read`, `ls`, `find`, `grep`, `webfetch`, `memory`, `skill`, `diagnostics`) run
concurrently; anything that writes to the workspace, spawns a subagent, or has
unknown remote effects stays sequential. Results are recorded in the model's
original call order. `bash` streams stdout and stderr line by line into the TUI
(and to stderr in `-p` mode) before the final combined output.

While the agent is busy, pressing Enter queues the current input as steering
rather than starting a new run; the message is injected into the conversation
before the next model call. A `tool.execute.after` plugin can also request
termination for the batch with `output.terminate = true`.

## Project trust

Projects may contain local resources that change how the agent behaves or
execute code — agents, commands, prompts, skills, plugins, `SYSTEM.md`, and
`APPEND_SYSTEM.md`.
Oxide treats the presence of any of these as requiring trust. When a project
requires trust and no decision has been saved for it (or a parent directory),
the TUI asks before loading them.

- `defaultProjectTrust` in `settings.json` controls the fallback: `ask`
  (default), `always`, or `never`.
- `--approve`/`-a` trusts project resources for one run; `--no-approve` ignores
  them.
- `/trust [show|off]` saves a decision for the current directory to `trust.json`.
- Non-interactive modes (`-p`, `--mode json`, `--mode rpc`) never prompt: with
  the `ask` or `never` setting they ignore project resources unless approved.
- Context files (`AGENTS.md`/`CLAUDE.md`) always load, trusted or not.

## Themes

Oxide ships `dark` and `light` themes. Add a custom theme as JSON under
`.oxide/themes/<name>.json` (project) or `<config>/Oxide/themes/<name>.json`
(global), then select it with `--use-theme <name>` or `/theme <name>`. The
built-in palettes come from `oxide_core::theme_view`, shared with the desktop
app, so the CLI and desktop use identical colors; custom theme files are read by
both. Colors accept names (`cyan`, `lightblue`) or `#rrggbb`; unspecified slots
fall back to the built-in `dark` theme:

```json
{
  "accent": "#5fd7ff",
  "success": "lightgreen",
  "tool": "cyan",
  "error": "lightred",
  "info": "gray",
  "border": "#5fd7ff"
}
```

Available slots: `accent`, `user`, `assistant`, `success`, `tool`, `error`,
`info`, `dim`, `border`, `tool_pending_bg`, `tool_success_bg`, `tool_error_bg`,
`usage_bar_bg`, `usage_bar_fg`, `usage_bar_label`, `thinking_off`,
`thinking_low`, `thinking_medium`, `thinking_high`, `thinking_text`.

Theme slots are semantic: `accent` marks focus and selections, `user` and
`assistant` label speakers, `success` and `error` communicate outcomes, `tool`
marks active tool work, and `info`/`dim` render supporting text. The `tool_*_bg`
slots fill the background behind a tool's header, output, and `Took`/`Elapsed`
footer (pending while running, success or error once it settles). Selection rows
also use reverse video and outcomes include text or symbols, so meaning does not
depend on color alone. For accessible custom themes, keep every foreground
readable against the terminal background and avoid assigning the same color to
`success`, `error`, and `tool`. The built-in themes use fixed `#rrggbb` colors
(not the terminal's own ANSI palette) so they match the desktop app exactly.

## Sessions and context

Sessions use Pi's on-disk format: an append-only JSONL tree whose first line is
a `session` header and whose remaining lines are `message`, `compaction`,
`branch_summary`, `session_info`, `model_change`, and `thinking_level_change`
entries linked by `id`/`parentId`. The last entry is the active leaf; the model
sees the leaf path with the latest compaction applied. Branching moves the leaf
back and appends a `branch_summary`, so in-file alternatives are preserved.

## Context compaction

Oxide compacts Pi-style once the outgoing context approaches the model window.
It walks back from the newest message until `keepRecentTokens` is reached and
summarizes the older span into a structured
handoff (goal, progress, decisions, next steps, critical context, plus
cumulative read/modified file lists), keeping the most recent tokens verbatim.
A cut never separates a tool call from its result.

Settings live under `compaction` in `settings.json` (global) or
`.oxide/settings.json` (project):

```json
{
  "compaction": {
    "enabled": true,
    "reserveTokens": 16384,
    "keepRecentTokens": 20000,
    "modelOverrides": { "openai/gpt-4o": { "reserveTokens": 400000 } }
  }
}
```

Compaction triggers above `contextWindow - reserveTokens`, where the window is
`OXIDE_CONTEXT_LIMIT` or the model default. `OXIDE_COMPACTION_ENABLED`,
`OXIDE_COMPACTION_RESERVE_TOKENS`, and `OXIDE_COMPACTION_KEEP_RECENT_TOKENS`
override the file settings. Each compaction is appended to the session log as a
`compaction` entry anchored at `firstKeptEntryId` and replayed on resume, so the
model sees the same compacted view. Manual compaction is `/compact [focus]` in the TUI or
`oxide sessions compact`.

Branching summarizes the path being abandoned with the same structured format
and appends it as a `branch_summary` entry. `/tree <n>` branches the current
session in place (alternatives stay in the file); `/fork <n>` creates a new
session seeded with the summary.

## Data locations

Runtime state lives under the platform Oxide config directory:

- Main configuration: `config.json`
- Credentials: `auth.json`
- Cached provider model lists: `model-cache.json` (refreshed after 24 hours)
- MCP OAuth tokens: `mcp-oauth/<server>.json` (mode `0600`)
- Sessions: `sessions/<project>/<timestamp>_<id>.jsonl` (Pi-style entry trees)
- Snapshots: `snapshots/<project>/` (bare git repo)
- Memory: `memory/`
- Project trust: `trust.json`
- Plugins: `plugins/` (installed plugin packages, marketplaces, and state)
- Portkey usage bar: `portkey-usage.json` (mode `0600`; see `OXIDE_USAGE_FILE`)
- Settings: `settings.json` (e.g. `defaultProjectTrust`, `compaction`,
  `modelPrices`, `hideThinkingBlock`)
- Themes: `themes/<name>.json`
- Desktop projects: `desktop/projects.json` (folders added to the desktop sidebar)
- Desktop approvals: `desktop/approvals.json` (tools allowed without prompting, per project)
- Truncated tool output: `truncated/` (retained 7 days; see `OXIDE_TRUNCATION_DIR`)
- Context compaction config: `compaction` in `settings.json` / `.oxide/settings.json`

Global ecosystem resources such as agents, commands, prompts, skills, plugins,
and MCP definitions may also live under `~/.oxide/`; compatibility resources
are read from `~/.claude/` and `~/.claude.json`.

## Development

```sh
cargo build
cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt
```

The workspace members are `crates/core` (shared agent core),
`crates/cli` (the `oxide` terminal binary), and `crates/desktop`.
The desktop GUI is feature-gated, so build it explicitly:

```sh
cargo build -p oxide-desktop --features gui
```

The VS Code extension is a separate pnpm package under `editors/vscode` and is
built with the `pnpm` commands above, not with Cargo.

See [CONTRIBUTING.md](CONTRIBUTING.md) for architecture notes and guidelines.

## License

MIT
