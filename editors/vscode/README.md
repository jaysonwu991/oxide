# Oxide for VS Code

Drive the [Oxide](https://github.com/jaysonwu991/oxide) coding agent from inside
VS Code. The extension runs your installed `oxide` binary as a subprocess
(`oxide --mode rpc`), so it uses the same provider logins, `config.json`,
`settings.json`, project trust, sessions, `AGENTS.md`, agents, skills, plugins
and MCP servers as the terminal and the desktop app — nothing is reconfigured.

- **Chat panel**: streaming replies rendered as Markdown, `✦ Thinking`
  blocks, tool cards with file diffs, token/cost usage, and a composer that
  offers the same explicit **Queue** / **Steer** choice as the desktop while a
  turn runs. It is contributed twice — an **Oxide**
  icon in the activity bar and a **Chat** pane in the secondary side bar, the
  strip Copilot Chat lives in — and both panes show the same thread. Its icons
  are the desktop app's: the cyan diamond mark and, in the Extensions view, the
  desktop app icon.
- **Attachments**: paste an image into the composer, drop files onto it, or
  click **Attach** — images and PDFs become chips above the message and travel
  to the model as media, while a text file is inlined as context. Each chip
  shows a thumbnail (or its size), can be removed with ✕, and the message box
  starts two rows tall and grows as you type.
- **Footer** around the composer, matching the terminal's: icon controls for
  model, reasoning, agent and project access inside its toolbar, and under it
  the git branch, the context gauge (amber past 70%, red past
  90%) and the context percentage. Focus or hover that percentage for the full
  accessible usage description: `↑`/`↓` tokens, `R`/`W` cache tokens, `CH` hit
  rate, `$cost` and `ctx %/window`, with an `(auto)` marker when auto-compaction is on.
  A live elapsed timer appears in the composer's toolbar while a turn runs.
- **Editor actions**: explain, fix, or ask about a selection; attach a file,
  a selection, or an image to the chat.
- **Sessions**: continue the project's latest session, or pick one from the
  CLI's own list with **Oxide: Resume Session…** or `/session` in the message
  box — the list opens in the panel, hanging under the header, not a native
  picker, with **New chat** as its first row to close the thread on screen and
  go back to the new-chat page.
- **`@path` in a message**: `@src/main.rs` attaches the file's text; an image or
  PDF becomes a media attachment. Typing `@` completes the project's files and
  folders, the way the terminal's composer does: the list hangs inside the
  composer card, the arrows walk it, `Tab`/`Enter` take the highlighted row (a
  folder stays open so you can narrow inside it) and `Esc` closes it. The list is
  the workspace's, read once per folder, and only the pane you typed in is
  answered from it — an answer for a path you have typed past, sent or dismissed
  is dropped rather than painted under a caret that has moved.

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
   file changes; `Enter` sends and `Shift+Enter` adds a newline. While a response
   runs, **Queue** is the safe default and sends the prompt as the next turn
   after the current response; **Steer** deliberately redirects the active
   response before its next model step. Paste an image, drop files on
   the composer, or click **Attach** to add images, PDFs and text files to the
   message.
4. The four icons in the composer toolbar are the next turn's settings: model,
   reasoning level, subagent and project access. Their complete values remain
   in tooltips. Session history stays in the header's history button. Under the
   composer the branch and context gauge stay compact; focus or hover the
   context percentage for the terminal footer's complete usage details.
5. `/mcps` in the message box opens the MCP server list above the composer it
   was typed in, and `/session` the project's threads under the header — both
   inside the panel, with a server's power switch or
   the thread a row names acting where it was asked (the same lists **Oxide:
   MCP Servers…** and **Oxide: Resume Session…** open from the palette). The
   thread row you are in is marked **Current**, and **New chat** above the list
   closes it and starts you on a fresh page.
6. Click a thumbnail in the attachment strip to see the full-size image before
   it is sent.
7. The file you are editing is already attached: it shows as a dashed chip with
   a ✎ above the message box and goes with the next message as context, read
   when the message is sent so unsaved edits are included. Select part of it and
   the same chip narrows to those lines — `src/app.ts:12-15`, sent as the lines
   it names rather than the whole file — and letting the selection go widens it
   back. Its ✕ (or **Clear**) takes it out; opening another file brings the chip
   back. Turn off `oxide.autoContext` to attach only what you add by hand.
8. Type `@` in the message box to complete a path from the project: the rows
   appear in the composer, `↑`/`↓` walk them, `Tab` or `Enter` takes the
   highlighted one, and a folder keeps the `@` open so you can go on narrowing
   inside it. `Esc` closes the list without clearing the message. `Alt+K` in the
   editor writes the file you have open into the box instead — `@src/app.ts`, or
   `@src/app.ts#5-10` for a selection, which sends just those lines.
9. The mark on the editor's toolbar brings the panel forward and leaves the
   caret in the message box; `Cmd+Esc` (`Ctrl+Esc` elsewhere) toggles the caret
   between the editor and the box, so a question about the file you are looking
   at can be typed and sent without reaching for the mouse.

## Commands

| Command | What it does |
| --- | --- |
| **Oxide: Open Chat** | Focus the chat pane (`Ctrl+Alt+O` / `Cmd+Alt+O`): the one already on screen, otherwise the secondary side bar's. Also the mark on the editor toolbar, which brings the panel forward and puts the caret in the message box. |
| **Oxide: Focus Input** | Move the caret between the editor and the message box (`Ctrl+Esc` / `Cmd+Esc`). |
| **Oxide: Insert File Reference** | Write the file you have open — or the selection in it — into the message box as an `@path` reference (`Alt+K`). |
| **Oxide: New Chat** | Close the thread on screen and go back to the new-chat page; the next message starts a thread of its own. |
| **Oxide: Resume Session…** | Pick from this project's sessions (or continue the latest), in a list inside the panel. |
| **Oxide: Continue Last Session** | Continue the most recent session on the next message. |
| **Oxide: Stop** | Cancel the running turn (the session keeps what it has written). |
| **Oxide: Add File or Selection to Chat** | Attach the selection or the whole file as context (`Ctrl+Alt+A` / `Cmd+Alt+A`); on an image or PDF in the explorer, it attaches the file as media instead. |
| **Oxide: Ask About Selection** | Attach the selection and ask a question about it. |
| **Oxide: Explain Selection** | Attach the selection and ask for an explanation. |
| **Oxide: Fix Selection** | Attach the selection and ask for a minimal fix. |
| **Oxide: Review Working Tree Changes** | Review the uncommitted changes without modifying files. |
| **Oxide: Open Terminal (TUI)** | Run the interactive `oxide` TUI in a terminal, for `/login` and `/models`. |
| **Oxide: MCP Servers…** | List the MCP servers this project loads and connect or disconnect them; typing `/mcps` in the composer opens the same list above the composer. |
| **Oxide: Check for Updates...** | Report the newest oxide CLI release and offer to install it, by running the CLI's own `oxide update --check --json` and then `oxide update` if you take it. The extension itself updates from the Marketplace. |
| **Oxide: Show Output Channel** | The command line, prompt, stderr and event log of each turn. |
| **Oxide: Set Model…**, **Set Agent…**, **Set Reasoning Effort…**, **Set Project Trust…** | Write the matching workspace setting; the footer's chips are the same actions. |

## Settings

| Setting | Default | Meaning |
| --- | --- | --- |
| `oxide.binaryPath` | `oxide` | The binary to run. A bare name is looked up on `PATH`, then in `~/.local/bin` and `~/.cargo/bin`. |
| `oxide.model` | *(empty)* | `--model`. Empty uses the model from Oxide `config.json`; the in-panel picker loads the active provider's complete catalog through `oxide models --json --active`, falls back to remembered models if it cannot refresh, and accepts a custom ID. |
| `oxide.agent` | *(empty)* | `--agent`, loaded from the workspace's `.oxide/agents`. |
| `oxide.reasoning` | `auto` | `--reasoning`: `auto`, `off`, `low`, `medium`, `high`. |
| `oxide.projectTrust` | `default` | `always` passes `--approve`, `never` passes `--no-approve`; `default` follows `trust.json`/`defaultProjectTrust`. |
| `oxide.tools` | *(empty)* | `--tools` allowlist, e.g. `read,grep,find,ls` for a read-only run. |
| `oxide.excludeTools` | *(empty)* | `--exclude-tools` denylist. |
| `oxide.additionalArguments` | `[]` | Extra argv appended to every invocation. |
| `oxide.showThinking` | `true` | Show `✦ Thinking` blocks. |
| `oxide.autoContext` | `true` | Track the file you are editing as a chip in the composer, sent with the next message as context. The chip follows the file and the lines selected in it, and both are read when the message goes, so unsaved edits are included and a selection sends just the lines it names; its ✕ or **Clear** takes it out for as long as that file is the one being edited, and another file brings it back. |
| `oxide.askApprovals` | `true` | `--ask-approvals`: ask in the transcript before running a tool a permission rule gates. **Always allow** is remembered per project in the shared `approvals.json`. Off passes `--no-ask-approvals`. Either flag is passed explicitly, so this setting decides for panel runs (the shared `settings.json` key still decides for the terminal). |
| `oxide.notifyOnFinish` | `true` | Notify when a run finishes while the panel is hidden, naming the thread by its title. |

## Notes and limits

- One turn at a time per window; a message sent while a turn runs is either
  queued as the next turn after the current response or steers it, and is shown in the transcript.
  **Stop** terminates the process — the session on disk
  keeps everything up to that point, so the next message continues the thread.
- The two chat panes are the same conversation: either can be used, both stream
  a turn, and **Oxide: Open Chat** raises whichever one is on screen. VS Code can
  move either pane — drag its title, or right-click it and pick **Move View** — so
  the activity-bar copy can be dropped into the secondary side bar (or the panel)
  and the icon hidden with a right-click on the activity bar. Nothing is lost
  either way: sessions live in the CLI's own store.
- A tool the permission rules mark `ask` comes back as a card in the transcript
  rather than being run on trust: the panel forwards your answer over the CLI's
  own channel, and **Always allow** is remembered by the CLI's broker, so the
  terminal and the desktop app stop asking for that tool in that project too.
- The model can also ask *you* something mid-turn — typically while following a
  skill — and the panel answers it in the transcript: pick one option, tick
  several, or type an answer, then **Submit** to send the whole set. A call that
  asks several things is asked one at a time, with `1 of 2 questions` and a dash
  per question saying where you are; **Back** returns to one, and **Dismiss**
  lets the model continue with its own default. The panel always passes
  `--ask-questions`, so a question reaches you instead of the model guessing, and
  an unanswered card is settled when the run ends.
- An image or PDF attached from the clipboard is written to a private OS
  temporary directory (`os.tmpdir()`), because the CLI takes attachment *paths*
  (`--image`); the directory is removed when the window closes. Files picked or
  dropped from the explorer are passed where they already are, so nothing is
  copied for them. At most eight attachments ride on one message.
- The footer reads the same files the CLI does, read-only: `config.json` for the
  model and window, `trust.json` plus `defaultProjectTrust` for the access chip,
  `settings.json`/`.oxide/settings.json` for auto-compaction, the project's
  `.oxide/agents` and `.claude/agents` for the agent names (only while the project
  is trusted, since an untrusted run drops project resources), the CLI's own
  plugin state for the agents installed plugins bundle, and `.git/HEAD` for the
  branch. Nothing outside the folder you opened is written, and a missing file
  only blanks the value it feeds.
- Diff previews are rendered from the tool's arguments (the JSON stream carries
  no diff), and they read the file from disk — the panel is a preview, not a
  file viewer.
- Links in a reply open in your browser; a path in a tool card opens in the
  editor, but only inside the workspace.
- A message's `@path` references are the panel's own: the prompt goes to the CLI
  on stdin, so the extension resolves them itself, which is why `@src/app.ts#5-10`
  sends those lines. A reference it cannot resolve — a range past the end of the
  file, a path that is not there — is left in the message as typed, and the CLI's
  own `@file` arguments take a whole path.

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
