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
editor shows the current activity and elapsed time, and the footer shows the
project path with the Git branch and session name, cumulative usage (including
cache and cost), context usage, and the current model and thinking level (plus a
plugin status row when present, and the Portkey spend bar when it is enabled).

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
| Ctrl+V | Attach the clipboard's image, or the file a copy of one names — the Finder's own copy of a file attaches that file rather than its icon. |
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
