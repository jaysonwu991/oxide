# Terminal UI

The TUI's layout, keyboard shortcuts, and copy behavior. Configuration lives in
[configuration.md](configuration.md); the task guides are in [cli.md](cli.md).

## Quick start

```sh
# Launch the TUI in the current project
oxide

# Then connect a provider from inside the TUI
/connect
```

Or provide credentials through the environment:

```sh
export OPENAI_API_KEY=sk-...
oxide
```

## Layout

The welcome area stacks the `OXIDE` wordmark above a short summary of the loaded
ecosystem, context files, and MCP/plugin/memory state. The status row above the
editor shows the current activity and elapsed time — what is pending travels in
the box itself rather than being counted there — and the footer shows the
project path with the Git branch and session name, cumulative usage (including
cache and cost — marked `(sub)` for a subscription-backed provider, whose number
is what the plan would have billed), context usage with Pi's one decimal
(colored above 70% and 90%), the current model and thinking level, and Pi's `xp`
marker when `OXIDE_EXPERIMENTAL=1`; the provider is named before the model only when several
providers are available and the row still holds it, and a plugin status row and
the Portkey spend bar appear when present and enabled.

The message box names what the message will carry: one row per pending
attachment above the text, indented one column and named by the file it came
from (`• /tmp/shot.png`) — the `@path` references and `/attach` entries the run
will read, with the ones past four folded into a `• N more` row so a long list
cannot push the box off the screen, and a part with no file of its own named by
what it is (`• image (png)`). `Ctrl+V` is the other way in: it inserts the
clipboard's own text — a copied file's path, a clipboard image's scratch path, or
plain text — and the model reads that path. The transcript line the message
becomes names the attachments the same way.

An `@path` in the message attaches a file the same way, and a reference to one
this app may not read is still an attachment: the refusal is reported with the
grant to give rather than being sent to the model as the text it was typed as.

Run `/hotkeys` for the in-app shortcut list and `/help` for commands, agents, and
skills.

## Keyboard shortcuts

| Key | Action |
| --- | --- |
| Enter | Send a message; while the agent is busy, queue a follow-up after the current response. |
| Shift+Enter | Insert a newline without sending. |
| Alt+Enter | While busy, steer the active response before its next model step. |
| Alt+Up / Option+Up | Pull every queued message back into the message box to edit or extend it (`Alt+Q` on Windows and WSL, where the terminal owns `Alt+Up`). |
| Esc | Clear the input, or refuse a tool waiting for an approval. In dialogs, cancel or close. |
| `/` | Open slash-command autocomplete. |
| `@` | Open file/folder path autocomplete to add a file to the prompt. |
| Tab | Complete the selected slash-command (including fixed arguments such as `/notify sound on`) or `@path` suggestion. |
| Up / Down | Move through the suggestion list, or recall input history when it is closed. |
| Shift+Tab | Cycle the thinking level: `auto` → `off` → `low` → `medium` → `high`. |
| Ctrl+O | Expand or collapse tool-output details. |
| Ctrl+T | Show or hide reasoning (`✦ Thinking`) blocks. |
| Ctrl+V | Paste the clipboard into the message box the way Pi does: the path a copied file names (one per line for a multi-select copy), the scratch path a clipboard image is written to, or plain clipboard text. The model reads the path, so a pasted image arrives through its file rather than as a pending attachment. A file this app may not read is written out as the pasteboard's own picture instead, so the inserted path still points at something the run can read; where the pasteboard carries none, macOS gates the Desktop, Documents and Downloads folders behind a per-app grant. |
| PgUp / PgDn / mouse wheel | Scroll the transcript. |
| Ctrl+A | Jump to the start of the message box (when it is not empty). |
| Ctrl+E | Jump to the end of the message box; when it is empty, scroll down one line. |
| Ctrl+Y | Scroll up one line. |
| Ctrl+U / Ctrl+D | Scroll half a page up / down. |
| Ctrl+G / Home | Scroll to the top. |
| End | Return to the latest message and resume automatic scrolling. |
| Ctrl+C | Copy the current mouse selection, or quit when there is none. |

A few keys act twice depending on the message box: Ctrl+E moves to the end of a
non-empty message and otherwise scrolls the transcript, and Ctrl+C copies an
active mouse selection instead of quitting.

## Copying

Dragging over the transcript copies the selected text on release and confirms it
with a dim `copied 243 chars` line; `/copy` and `/copy all` report the same way,
and repeated copies update that one line instead of stacking up. Copying writes an
OSC 52 sequence, so it reaches the clipboard even over SSH or inside tmux or
screen, and also tries the native helper (`pbcopy`/`osascript` on macOS,
`wl-copy`/`xclip`/`xsel` on Linux) when one is available.

What a copy carries is the text behind the wrap, not the rows it is drawn on:
a line the transcript had to break at a space comes back as one line, a long
path or word it broke inside itself is put back together, and only a line the
text itself starts begins a new line in the copy. Paste it into a terminal of
another width, an editor or a chat and it reflows there, instead of arriving
with this pane's breaks baked into it.
