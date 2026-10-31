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

Or provide credentials through the environment: a provider reads the variable
its own listing names (`OPENAI_API_KEY`, `DEEPSEEK_API_KEY`,
`ANTHROPIC_API_KEY`, `GEMINI_API_KEY`, …) or `OXIDE_API_KEY`, which every
provider reads, so the run has to be on the provider that key belongs to.

```sh
export OPENAI_API_KEY=sk-...   # or DEEPSEEK_API_KEY, ANTHROPIC_API_KEY, …
oxide --provider openai        # the provider whose key that is
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
from (`• /tmp/shot.png`) — the parts a paste attached and the ones a queued
message brought back when it was pulled into the box (`/attach` lists and edits
them) — with the ones past four folded into a `• N more` row so a long list
cannot push the box off the screen, and a part with no file of its own named by
what it is (`• image (png)`). An `@path` reference is not one of those rows: it
is resolved when the message is sent, and named on the transcript line the
message becomes. `Ctrl+V` is the other way in: it inserts the
clipboard's own text — a copied file's path, a clipboard image's scratch path, or
plain text — and the model reads that path. The transcript line the message
becomes names the attachments the same way.

The composer highlights file references with an accent, bold underline and a
background. A spaced filename such as `@Packages to Country - Reviewed.xlsx` is
highlighted in full; quoted paths stay together too. The decoration keeps the
original text and terminal cursor positions unchanged.

An `@path` in the message attaches a file the same way, and a reference to one
this app may not read is still an attachment: the refusal is reported with the
grant to give rather than being sent to the model as the text it was typed as.

`/language` draws the interface in British English (the default) or Simplified
Chinese: `/language zh-CN` switches it for every front-end — the same
`settings.json` key the desktop app's globe and the VS Code panel's command write
— and `/language` alone reports the choice. `--language <tag>` sets it for one
run without writing the setting. The transcript's own labels (a thinking block,
the queued-message rows, the approval prompt) are part of the interface and
follow the language; what the model writes and what a tool prints do not.

A context compaction renders as Pi's `[compaction]` block and a branch summary
as `[branch]`, each folded to a one-line note with `Ctrl+O to expand` until
`Ctrl+O` shows its text — the same key that unfolds tool output.

A URL in a reply is clickable: written bare or as an `<autolink>` the URL is
the link itself, and a labelled link is drawn as its label alone — the target
is not printed, as in the desktop app and the panel — with the whole label
answering. A left click opens it in the system browser (and the click is not
also a text selection). When you have scrolled away from the newest output, a
`↓ Jump to latest message · End` row floats over the bottom of the transcript;
click it or press `End` to return to the newest line, and it hides once the end
is on screen.

A message typed while the agent is busy waits above the message box as a dim
`Steering: …` or `Follow-up: …` row, with a `↳ <key> to edit all queued messages`
hint, rather than appearing in the transcript — it becomes a turn there when the
run delivers it. `Alt+Up` (`Alt+Q` on Windows and WSL, `Option+Up` on macOS)
pulls them all back into the box. How many are handed over at once is set by
`steeringMode`/`followUpMode` (see
[configuration](configuration.md#queued-messages)).

Run `/hotkeys` for the in-app shortcut list and `/help` for commands, agents, and
skills. A `/word` that names no command at all — a retired spelling such as
`/mcps`, a typo such as `/hlep` — is answered in place rather than sent to the
agent, so a mistyped command costs nothing: the transcript shows
`unknown command `/mcps`. Did you mean `/mcp`?` and the message box is cleared
for the next attempt. The same rule runs before a turn starts in the desktop
app, the VS Code panel and `-p`/`--mode rpc`, so the sentence is the same
wherever the word was typed.

## Keyboard shortcuts

| Key | Action |
| --- | --- |
| Enter | Send a message; while the agent is busy, steer the active response before its next model step. |
| Shift+Enter | Insert a newline without sending. |
| Alt+Enter | While busy, queue a follow-up for after the current response. |
| Alt+Up / Option+Up | Pull every queued message back into the message box to edit or extend it (`Alt+Q` on Windows and WSL, where the terminal owns `Alt+Up`). |
| Esc | Clear the input, or refuse a tool waiting for an approval. In dialogs, cancel or close. |
| `/` | Open slash-command autocomplete. |
| `@` | Open file/folder path autocomplete to add a file to the prompt. |
| Tab | Complete the selected slash-command (including fixed arguments such as `/notify sound on`) or `@path` suggestion. |
| Up / Down | Move through the suggestion list, or recall input history when it is closed. |
| Shift+Tab / Ctrl+R | Cycle the thinking level through the levels the active model advertises: `auto`, then those levels, wrapping around. A model whose provider listing and remembered pi.dev entry give it none has no selector, so the key does nothing. |
| Ctrl+O | Expand or collapse tool-output details, and compaction/branch summaries. |
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

Neither does the pane's own decoration reach the continuation rows: the indent
a wrapped reasoning body hangs under is drawn on every row it wraps and a
quote's `│ ` bar runs down all of them, but the copy leads with the bar (or
with the indent) once and rejoins the rest onto that line. The whitespace a
code line holds — two spaces in a string literal, say — is kept wherever the
pane had to split it, since it belongs to the line and not to this pane.
