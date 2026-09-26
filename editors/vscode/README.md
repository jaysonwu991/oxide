# Oxide for VS Code

Drive the [Oxide](https://github.com/jaysonwu991/oxide) coding agent from inside
VS Code. The extension runs your installed `oxide` binary as a subprocess
(`oxide --mode json -p`), so it uses the same provider logins, `config.json`,
`settings.json`, project trust, sessions, `AGENTS.md`, agents, skills, plugins
and MCP servers as the terminal and the desktop app — nothing is reconfigured.

- **Chat panel**: streaming replies rendered as Markdown, `✦ Thinking`
  blocks, tool cards with file diffs, token/cost usage, and a composer that
  queues follow-ups while a turn runs. It is contributed twice — an **Oxide**
  icon in the activity bar and a **Chat** pane in the secondary side bar, the
  strip Copilot Chat lives in — and both panes show the same thread. Its icons
  are the desktop app's: the cyan diamond mark and, in the Extensions view, the
  desktop app icon.
- **Footer** under the composer, matching the terminal's: clickable
  `model: …`, `thinking: …`, `agent: …`, `access: …` and `session: …` chips, the
  git branch, a live elapsed timer while a turn runs, the context gauge
  (amber past 70%, red past 90%), and a usage line of `↑`/`↓` tokens, `R`/`W`
  cache tokens, `CH` hit rate, `$cost` and `ctx %/window` with an `(auto)`
  marker when auto-compaction is on.
- **Editor actions**: explain, fix, or ask about a selection; attach a file,
  a selection, or an image to the chat.
- **Sessions**: continue the project's latest session, or pick one from the
  CLI's own list with **Oxide: Resume Session…**.
- **`@path` in a message**: `@src/main.rs` attaches the file's text; an image or
  PDF becomes a media attachment.

## Install

Download `oxide-vscode-<version>.vsix` from an `extension-v*`
[release](https://github.com/jaysonwu991/oxide/releases) and run **Extensions:
Install from VSIX…** in VS Code.

## Requirements

The `oxide` binary on your `PATH` (or set `oxide.binaryPath`). Install it with:

```sh
curl -fsSL https://raw.githubusercontent.com/jaysonwu991/oxide/main/install.sh | bash
# or, from a checkout
cargo install --path crates/cli
```

Provider logins live in the CLI: run **Oxide: Open Terminal (TUI)** and use
`/login` there. The extension reads the resulting `auth.json` and
`config.json` — it never asks for a key itself.

## Getting started

1. Open a folder in VS Code and reveal the chat: the **Oxide** icon in the
   activity bar, or the same icon in the secondary side bar (the strip Copilot
   Chat lives in), or **Oxide: Open Chat** (`Ctrl+Alt+O` / `Cmd+Alt+O`).
2. If the workspace has its own `.oxide/` resources, decide on project trust
   with **Oxide: Set Project Trust…** (untrusted runs simply skip them).
3. Type a message. Tool calls appear as coloured panels with a diff preview for
   file changes; `Enter` sends, `Shift+Enter` adds a newline, and `Alt+Enter`
   queues a follow-up for after the current turn.
4. The row under the transcript is the footer: click `model: …` to switch the
   model, `thinking: …` to cycle the reasoning level, `agent: …` to pick a
   subagent, `access: …` for project trust, or `session: …` to resume another
   session.

## Commands

| Command | What it does |
| --- | --- |
| **Oxide: Open Chat** | Focus the chat pane (`Ctrl+Alt+O` / `Cmd+Alt+O`): the one already on screen, otherwise the secondary side bar's. |
| **Oxide: New Session** | Start a fresh thread. |
| **Oxide: Resume Session…** | Pick from this project's sessions (or continue the latest). |
| **Oxide: Continue Last Session** | Continue the most recent session on the next message. |
| **Oxide: Stop** | Cancel the running turn (the session keeps what it has written). |
| **Oxide: Add File or Selection to Chat** | Attach the selection or the whole file as context (`Ctrl+Alt+A` / `Cmd+Alt+A`). |
| **Oxide: Ask About Selection** | Attach the selection and ask a question about it. |
| **Oxide: Explain Selection** | Attach the selection and ask for an explanation. |
| **Oxide: Fix Selection** | Attach the selection and ask for a minimal fix. |
| **Oxide: Review Working Tree Changes** | Review the uncommitted changes without modifying files. |
| **Oxide: Open Terminal (TUI)** | Run the interactive `oxide` TUI in a terminal, for `/login` and `/models`. |
| **Oxide: Show Output Channel** | The command line, prompt, stderr and event log of each turn. |
| **Oxide: Set Model…**, **Set Agent…**, **Set Reasoning Effort…**, **Set Project Trust…** | Write the matching workspace setting; the footer's chips are the same actions. |

## Settings

| Setting | Default | Meaning |
| --- | --- | --- |
| `oxide.binaryPath` | `oxide` | The binary to run. A bare name is looked up on `PATH`, then in `~/.local/bin` and `~/.cargo/bin`. |
| `oxide.model` | *(empty)* | `--model`. Empty uses the model from the Oxide `config.json`. |
| `oxide.agent` | *(empty)* | `--agent`, loaded from the workspace's `.oxide/agents`. |
| `oxide.reasoning` | `auto` | `--reasoning`: `auto`, `off`, `low`, `medium`, `high`. |
| `oxide.projectTrust` | `default` | `always` passes `--approve`, `never` passes `--no-approve`; `default` follows `trust.json`/`defaultProjectTrust`. |
| `oxide.tools` | *(empty)* | `--tools` allowlist, e.g. `read,grep,find,ls` for a read-only run. |
| `oxide.excludeTools` | *(empty)* | `--exclude-tools` denylist. |
| `oxide.additionalArguments` | `[]` | Extra argv appended to every invocation. |
| `oxide.showThinking` | `true` | Show `✦ Thinking` blocks. |
| `oxide.notifyOnFinish` | `true` | Notify when a run finishes while the panel is hidden. |

## Notes and limits

- One turn at a time per window; a message sent while a turn runs is queued and
  shown in the transcript. **Stop** terminates the process — the session on disk
  keeps everything up to that point, so the next message continues the thread.
- The two chat panes are the same conversation: either can be used, both stream
  a turn, and **Oxide: Open Chat** raises whichever one is on screen. VS Code can
  move either pane — drag its title, or right-click it and pick **Move View** — so
  the activity-bar copy can be dropped into the secondary side bar (or the panel)
  and the icon hidden with a right-click on the activity bar. Nothing is lost
  either way: sessions live in the CLI's own store.
- Runs are non-interactive, so there is no approval prompt: a tool the
  permission rules mark `ask` is decided by Oxide's own `auto_approve` setting.
- The footer reads the same files the CLI does, read-only: `config.json` for the
  model and window, `trust.json` plus `defaultProjectTrust` for the access chip,
  `settings.json`/`.oxide/settings.json` for auto-compaction, `.oxide/agents` for
  the agent names, and `.git/HEAD` for the branch. Nothing outside the folder you
  opened is written, and a missing file only blanks the value it feeds.
- Diff previews are rendered from the tool's arguments (the JSON stream carries
  no diff), and they read the file from disk — the panel is a preview, not a
  file viewer.
- Links in a reply open in your browser; a path in a tool card opens in the
  editor, but only inside the workspace.

## Development

```sh
pnpm install
pnpm run compile   # tsc -p .
pnpm test          # compile, then node --test out/test/
pnpm run package   # vsce package -> oxide-vscode-<version>.vsix
```

Press <kbd>F5</kbd> in VS Code with this folder open to launch an Extension
Development Host. The extension has no VS Code dependencies outside the
extension host: `src/core/` is plain TypeScript with unit tests, and
`media/main.js` is a dependency-free renderer shared in spirit with the desktop
front-end (`crates/desktop/ui/app.js`).

See [docs/vscode.md](../../docs/vscode.md) for the full architecture notes.
