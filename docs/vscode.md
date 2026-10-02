# VS Code extension

The `editors/vscode` package is a TypeScript VS Code extension that drives the
same `oxide` binary the terminal runs. It is **not** part of the Cargo
workspace: it shells out to `oxide --mode rpc`, so it needs no Rust changes
and no `oxide-core` link, and it inherits the CLI's provider logins,
`config.json`, `settings.json`, project trust, sessions, `AGENTS.md`, agents,
skills, plugins, and MCP servers exactly as they are.

## Workspace

```
editors/vscode/
  package.json        manifest: view, commands, keybindings, settings
  tsconfig.json       strict TypeScript (commonjs, ES2022)
  pnpm-workspace.yaml build-script allowlist
  src/
    extension.ts      entry: commands, status bar, editor actions
    chat.ts           ChatController: transcript + running turn + queue
    chatView.ts       WebviewViewProvider: HTML shell, CSP, message bridge
    cli.ts            process layer: binary lookup, startTurn, runCapture
    attachments.ts    thumbnails and the temp files pasted blobs are written to
    core/             pure, webview-free logic (unit tested under node)
      protocol.ts     wire events -> transcript state machine -> view messages
      views.ts        view ids shared by the manifest and the provider
      approvals.ts    an approval request, its tool titles and its answers
      questions.ts    a skill's question request, its answers and its label
      args.ts         VS Code settings -> `oxide` argv
      prompt.ts       prompt assembly, @path expansion, attachments
      attachments.ts  attachment types, extensions, naming and data URLs
      preview.ts      write/edit/patch diff previews
      config.ts       shared config-dir resolution (read-only)
      settings.ts     settings.json / .oxide/settings.json reads (read-only)
      trust.ts        trust.json resolution and the access decision
      git.ts          the branch, read from .git/HEAD
      agents.ts       agent names for `--agent`
      plugins.ts      installed plugins, whose agents `--agent` also resolves
      json.ts         a tolerant JSON object reader
      project.ts      the on-disk state the footer reports
      footer.ts       footer chips, usage line and context gauge
      sessions.ts     `oxide sessions list` / `--version` parsing
    test/             node:test suites for src/core and the manifest
  media/
    main.js           dependency-free webview renderer (Markdown, diffs, …)
    style.css         themed styles (VS Code CSS variables)
    oxide.svg         container and view icon: the desktop app's mark
    icon.png          extension icon: a copy of the desktop app's app icon
```

The extension host owns all the state; the webview is a dumb renderer that
applies the view messages produced in `core/protocol.ts`. That module imports
nothing from `vscode`, so the whole event-to-DOM decision surface is unit tested
with `node --test` and no webview.

## Where the chat lives

The chat is one controller behind two contributed webview views, so it can sit
where the user keeps chat:

- `oxide.chat` — the `Chat` view in the `oxide` activity-bar container.
- `oxide.chatSecondary` — the same view in the `oxide-secondary`
  `secondarySidebar` container, the strip GitHub Copilot Chat uses.

Both ids live in `src/core/views.ts` and must match `package.json`; a view VS
Code contributes without a matching `registerWebviewViewProvider` is an empty
panel, and `test/views.test.ts` asserts the manifest, the activation events, the
view-title menus and the icons agree with those constants. `ChatController`
holds the transcript, the running turn and the queue, and broadcasts every view
message to all attached views, so both panes follow one conversation; on an
older VS Code that ignores `viewsContainers.secondarySidebar`, the container and
its view never appear and the activity-bar pane is the only one.

`oxide.openChat` raises the pane already on screen (`WebviewView.visible` decides
the order), falling back to the secondary side bar's and then to the
activity-bar one when that focus command does not exist.

The panel's own chrome is icon-first, the way Claude Code's is. The header shows
the thread's summarized title — the resumed session's name when the dialog knew
one, else the first message sent condensed to one line (Markdown stripped, cut
at a word boundary), else **New chat** — next to icon buttons for a new chat
and for resuming one, whose tooltips — and `aria-label`s, the names a screen
reader reads out — say **New chat** and **Resume a session**; the model is the
footer's first chip rather than a second line under the title, and the
composer's **Attach**, **Stop** and **Send** are icons too, so the only text in
the chrome is the phase and the numbers. The phase
appears while a turn runs (with the elapsed timer) and goes when it does, rather
than sitting in the toolbar as an idle dot. A turn that finishes while the panel
is hidden raises a toast naming the thread by that same title; it takes the
panel's own `oxide.notifyOnFinish` and the shared `notifyOnComplete` the
terminal's `/notify` writes, so turning notifications off in one silences the
other (a VS Code notification has no alert sound of its own, so `notifySound` is
the terminal's and the desktop app's). The view-title actions (`oxide.newSession`,
`oxide.resumeSession`) carry the codicon `$(add)` and `$(history)` for the same
reason, so VS Code draws them as icons instead of inline text.

`oxide.openChat` also sits on the editor's own toolbar: the manifest contributes
it to `editor/title` (`resourceScheme == file`, since an output or diff tab has
nothing to go with it), so the mark at the top right of a file's tab brings the
panel forward without leaving the editor. Bringing it forward leaves the caret in
the composer — focusing a webview view does not focus its own DOM — through the
`focusComposer` message the pane applies. A webview is built asynchronously, so
anything meant for the composer goes only to a pane that has asked for state;
what arrives while the panel is still being built waits, and goes to the first
pane that says it is ready. A shortcut that opens the chat on a cold window
therefore still leaves the caret in the box, and an inserted reference still
lands rather than being posted into a webview that had no listener yet.

`oxide.focusInput` is the caret toggle that goes with it (`Cmd+Esc`, `Ctrl+Esc`
elsewhere): from the editor it raises the panel and puts the caret in the message
box, and from the composer it hands the caret back to the editor group. Which
side it is on is the controller's to know, not something a command can read off
the screen: each pane reports its own window `focus`/`blur` (`noteViewFocus`),
because each holds its own composer and only one of them is being typed in. That
is also why a message about the composer goes to the pane with the caret — else
the pane on screen — while the transcript is broadcast to both.

An Escape is VS Code's when it carries a modifier: `Cmd+Esc`/`Ctrl+Esc` reaches
the composer as an Escape on its way to the keybinding, so only a plain one
closes the completion list or stops a turn.

## Brand assets

Both icons are the desktop app's: `media/oxide.svg` redraws the mark inside
`crates/desktop/icons/icon.png` — a cyan diamond (`#5fd7ff`) with a dark rim
(`#2d2d3a`), at the same proportions relative to its box (the cyan diamond
spans 62.5% of the canvas, the rim reaches 71.9%) — and `media/icon.png` is the
desktop's `128x128.png` byte for byte, which is what the Extensions view and the
Marketplace listing show.

The SVG carries those colours instead of `currentColor` on purpose: VS Code
draws a contributed icon as a plain background image, and `currentColor` in a
standalone SVG document resolves to black, so a tinted mark would vanish on a
dark side bar. `test/brand.test.ts` keeps both files honest — it reads the
manifest's `icon`, checks the PNG header, and compares the file against the
desktop's, so the two can only drift together.

## Sharing configuration with the CLI

`core/config.ts` resolves the same `<platform config dir>/Oxide` directory the
CLI uses (falling back to the pre-migration lowercase directory), so a provider
connected with the terminal's `/login` is immediately usable in the panel. The
extension never writes that directory: the model, agent, reasoning, tools, and
trust are VS Code settings (`oxide.*`) that become `--model` / `--agent` / …
flags, and the CLI applies them to its own config.

Provider logins stay in the CLI: run **Oxide: Open Terminal (TUI)** and `/login`
there. The status bar reads `config.json` only to label the active
provider/model.

## MCP servers

The composer answers `/mcps` (alias `/mcp`) itself, the way the terminal's
`/mcps` and Claude Code's `/mcp` do: the message is not sent to the model but
opens the panel's own **MCP servers** listing — not a `showQuickPick`, which
takes over the window, hides the transcript the listing is about, and cannot be
answered while a turn streams. It is the listing that belongs near the composer
where it was typed: the same standalone card used for session history, moved to
the other end of the panel, with its own scrollbar once a project has more
servers than fit.
Every server is a row carrying the state the core reported (`Connected`, `Needs
auth`, `Needs trust`, `Disabled`, `Error`) and a line naming its transport, the
file that defined it and its state, plus an icon-only power switch that turns it
off, or back on, in the file that defines it — `oxide mcp disable|enable <name>
--scope project|global` — after which the listing is repainted from the CLI's
fresh answer, so a toggle is answered in the same place it was asked. Its tooltip
names the server it would change (**Disable context7**), since the switch itself
is one glyph. A **Recheck** icon and a **Close** icon sit in the listing's
header, and <kbd>Esc</kbd> dismisses it too. **Oxide: MCP Servers…** opens the
same listing from the palette.
Every server has to be started or reached to learn its state, so the wait is
announced in the status bar and the listing says **Checking servers…** rather
than looking like nothing happened; a CLI that does not answer within the
command timeout is reported as such instead of leaving the listing unopened. A
**Recheck** clicked while one is already running supersedes it — the listing is
the controller's, and each probe claims a generation before it runs, so an
answer that arrives after a newer probe has already been sent is dropped instead
of repainting the list with the state it replaced.

The listing comes from `oxide mcp list --json` (`core/mcps.ts`), so the
extension never reads `mcp.json` itself and cannot disagree with the CLI about
which servers a project loads, which scope's definition wins a name, or what
their state is. A server the project defines is only probed while the project
is trusted: an untrusted one reports **Needs trust** rather than being
connected. `/mcps` is one of the composer's own commands — *The `/` palette*
below covers the rest, and how a project command, a prompt template or a skill is
left to the CLI to resolve, while the footer chips and the VS Code command
palette remain the way to change the model, the agent, the reasoning level and
the project's trust from here.

## Sessions

The header's history icon is the one control for it, and it is a toggle: the
click that opens the listing is the click that closes it, which is what the
button's own `aria-expanded` says — the panel is part of the column rather than a
window over it, so it is opened and put away the same way. Reading the store is
all opening does: the rows that switch threads refuse while a turn owns the
session file, so the listing itself can be read mid-turn. **Oxide: Session
History** and `/session` (alias `/sessions`) in the composer reach the same
listing — the listing `oxide sessions list` prints, painted in the panel rather
than in a QuickPick. The command is the button's own control under another name,
while `/session` typed in the composer opens the listing (or repaints it if it is
already up) rather than closing it: a command that names the history should not
answer by taking it away. It hangs from the header rather than from the composer
the way the MCP servers listing does: it is about the thread the header names, and
its first row is about leaving it.

Its first two rows are the way out of the thread the panel has open. **New
chat** closes it and puts the panel back on the new-chat page it starts on —
the transcript is cleared and the thread's id dropped, so the next message
starts a session of its own instead of appending to the one that was open —
and, with nothing open to close, the row settles for "start a fresh thread".
**Continue most recent session** picks up the newest thread this project has.

Under them is every thread stored for the folder, newest first, each named by
its session name or, unnamed, by the summarized preview `oxide_core::title`
gives it in the terminal's own picker and the desktop app's sidebar, with how
long ago it was written and how many messages it holds; the title carries the
number of threads listed beside it, and the row the panel is showing carries
**Current** where the others carry their age (on the selection background a
selected row gets, so it is marked without being moved out of the order it is
being read in), since "close this thread" would otherwise name nothing. Each row
carries a trash button that appears on the row the pointer is on — the same
bargain VS Code makes for a tab's close — asks to confirm, and then deletes the
thread through `oxide sessions delete <id>`, the way the desktop app's sidebar
does; deleting the thread the panel is showing starts a new one rather than
leaving a thread that is gone from the list still on screen. A deletion is
refused while a turn is running — the turn's CLI appends to that session file as
it works, so removing it would pull the file out from under the process — and the
confirmation says so.

A project can hold hundreds of threads, so the listing carries a search box
above the rows. It filters what the store already answered rather than reading it
again — typing never spawns the CLI — and it is the host that filters: the view
posts the box's text as `dialogSearch`, the host narrows the rows, the count and
the note together (`filterSessions` matches the title and the id,
case-insensitively), and the narrowed listing is painted back with the query
echoed in it, so a repaint under a reader keeps their filter (`syncSessions`
while a turn runs) and never takes the box away from the middle of a word. A
query nothing matches leaves the two rows that are ways out of the listing and
says so; the box's own ✕ clears it.

Picking a row resumes that thread here: the CLI is launched with `--session
<id>`, and the title in the header follows. Resuming also fills the transcript
with what the thread already holds — `oxide sessions show <id> --tail 60 --json`
is read back through `core/history.ts` and its turns are painted followed by a
note saying the thread was resumed — so a resumed thread reads like the thread it
is instead of an empty panel that answers nothing. A turn is what was said and
what was called, and a stored thread is mostly the second: an assistant step that
only dispatched tools paints no bubble, each call it made paints the same tool
card the live turn painted, and the result the CLI stores as a message of its own
is folded into the card it answers rather than shown beside it. The tail is
counted in those stored messages, so reopening a long thread shows several of its
turns rather than its last reply only. A replayed card carries the change its own
arguments describe — an `edit`'s replacements, a `patch`'s diff — rather than one
built against the file as it stands, which is no longer the state the call found;
an `edit`'s replacements are previewed one block each, separated by the same `⋯`
the preview draws between hunks, since joining them into one pair of sides could
align a line of one replacement with a line of the next, or cancel the two out;
and a `write` names only what it wrote, so its state is nowhere on disk and its
card shows the call and its result instead. A replayed card claims no state at
all: the store does not record whether the call applied, so a call that failed is
stored exactly like one that landed, and the card marks it as unrecorded (`•`,
with its own tooltip) instead of the green `✔` of a landed call or the red `✖` of
a failed one — the result text it replayed is what says what happened. The
footer's usage line and context gauge come from the same answer's usage totals.
`/session <id>` is left alone — an argument is the agent's, matching `/mcp
list`.

The listing is unchanged from the native picker it replaces (`core/sessions.ts`
parses `oxide sessions list`), so the panel and the terminal agree on which
threads exist and what they are called. Sessions are per project: the dialog
lists the folder that is open.

The thread the panel is in stands in for itself while the store has not written
it yet: `sessionDialog` is handed the open thread as a `LiveSession` (its id and
the title the header shows), so a thread that is running — or one whose first
message has not been flushed — is listed under the name the header carries
instead of missing from the list, and the store catching up replaces the row
rather than adding a second one. For the same reason the listing is read again
while it is open, at the two moments a session is written: when the `session`
header names the thread, and when the turn ends. Only a session listing is
re-read — `chat.ts::syncSessions` returns early for a confirmation or the MCP
list, and again for a listing closed while the read was in flight — and the
repaint goes through `showSessions`, which is the one place the dialog is
composed. The two reads overlap (the header's starts before the store has the
file, the exit's after it) and either can answer first, so each takes the next
`sessionsSync` token and only the newest one, for the folder it was read in, is
applied: a listing that started earlier and lands later is dropped instead of
putting the just-created row back out of the list.

Both dialogs are composed in the extension host as data (`core/dialogs.ts`) — a
kind, a title, the panel edge it hangs from (`pin`), the number of rows being
listed and whether the head carries a search box (`count`, `search`, and the
`query` that composed them), a note for an empty or failed listing, and rows
where each row carries the action it posts back (`mcpToggle` with the server's
name, `mcpRefresh`, `openSession` with `new`, `continue` or a session id,
`sessionDelete` and `sessionDeleteConfirm` for a thread's trash button,
`dialogClose`) — and the controller holds the open one, so both panes paint the
same dialog and a pane that attaches afterwards is sent it again. A row also
carries what it is (`kind`: `action` for the listing's own ways out, `thread` for
one of the threads it lists, empty for an ordinary row), which is the only thing
the renderer needs to paint a thread apart from a way out of the listing, and
`current` for the thread the panel has open — the mark the listing puts on the
conversation on screen, sent rather than read off the row's status word, which
is a word to paint. `kind: "sessions"` is also the only state the header's own
button has to agree with, since it is that listing which puts the button's
`aria-expanded` up.

Nothing about where it opens is left to the renderer either: the MCP list is
pinned to the footer, above the composer it was asked for in, the session
history to the header that names the thread it lists, and the webview only
carries that as a class. A
row's button carries the glyph it is painted as (`icon: "power"`,
`icon: "trash"`) and the words it would otherwise show, which become its
tooltip and its name for a screen reader. The webview only builds the rows and
posts the clicked one's action back as a `dialogAction`; it decides nothing about
what a click means, the same way a footer chip posts the control id it carries.
Neither dialog covers the panel with a backdrop, so a click inside one keeps it
open: it is dismissed by its Close icon, by <kbd>Esc</kbd>, or by picking a row.

## Image previews

A thumbnail in the attachment strip is a button: clicking it opens the full-size
image the host sent, in an overlay on the panel, and the overlay's ✕,
<kbd>Esc</kbd> or a click on the backdrop closes it. A chip has room for a 96px
copy, so without this the only look at what is being sent was too small to check.
The way out is the desktop app's: an icon in a head row above the picture —
outside the image it closes — in the error color, rather than a text button under
it.

## Agent turns

Each turn is one process: `oxide --mode rpc` with the prompt written to stdin
as a request frame (`core/rpc.ts`), so the text never goes through argv or the
CLI's positional `@file` expansion, and the pipe stays open for the one request
the CLI has to send back mid-turn — a tool approval. Its stdout is framed by
`drainLines` (compacted once per chunk) and each line parsed by `parseEvent`;
the `session` header supplies the id reused for the next message with
`--session <id>`, and `--continue` resumes the newest session. The turn ends
with a `quit` frame when `agent_end` arrives, so the process exits on its own.

- **Queue / stop** — a message sent while a turn runs is queued and started
  after it finishes; **Stop** kills the process (SIGTERM, then SIGKILL after 3
  s). The session on disk is intact, so the next message continues the thread.
- **Approvals** — with `oxide.askApprovals` on (the default) a turn starts with
  `--ask-approvals`, so a tool a permission rule holds comes back as an
  `approval_request` event instead of running. The turn waits: a card appears in
  the transcript naming the tool, what it would do (`Run a shell command`) and
  the command or path it would touch, with **Deny** / **Allow once** / **Always
  allow**. The answer travels back over the same pipe as an `approval` frame
  (`Approve` in `core/approvals.ts`; `chat.ts::approve` → `cli.ts`), and
  **Always allow** is remembered by the CLI's own broker in
  `<config>/Oxide/approvals.json` — the file the terminal and the desktop app
  read — so the question does not come back for that tool in that project. An
  unanswered card is settled when the run ends or is stopped, since the request
  it belonged to went with the process (the CLI itself denies after 5 minutes).
  The flag is passed explicitly in either direction, so `oxide.askApprovals`
  decides for a panel run; `askApprovals` in the shared `settings.json` still
  decides for the terminal and the desktop app.
- **Questions** — a turn always starts with `--ask-questions`, so a skill that
  needs a decision reaches the panel instead of the model guessing. The `ask`
  tool's request arrives as a `question_request` event and becomes a card in the
  transcript, asked one question at a time: `N of M questions` with a dash per
  question beside it (neither on a card that asks only one), the question under
  its own header, and its options as rows — a radio group (the first option
  preselected) or, for a list where several labels may be picked, checkboxes —
  with a description under each label and, for a single choice, a row asking for
  an answer in the user's own words with its field under it, so a question with
  no options is still answerable and the typed text answers it instead of riding
  beside a picked label. **Next** walks to the question after this one (**Back**
  returns to it, keeping what was already answered), and the last step's
  **Submit** posts the whole set as a `question` frame (`core/questions.ts`;
  `chat.ts::answerQuestion` → `cli.ts`), while **Dismiss** answers with nothing
  at all, which the agent reports to the model as a question nobody answered, and
  the card then shows the answer it was
  answered with — a submission with nothing filled in is sent as that same
  dismissal rather than as a set of blank answers, so both reach the model
  alike. A card is settled when the CLI gives up on the request (a
  `question_closed` event, which the CLI sends after its 5-minute timeout while
  the turn it belongs to keeps running) or when the run ends or is stopped. An
  answer that would otherwise have ridden on it is never sent. Nothing is
  remembered between them: unlike an approval, a question is about this
  conversation only.
- **Per folder** — sessions are per project, so switching to a different
  workspace folder resets the transcript and starts its own thread.
- **Usage** — `usage` events accumulate input/output/cache tokens and cost for
  the usage line, and the latest one sets the context gauge (its prompt tokens
  over the window), which is `OXIDE_CONTEXT_LIMIT` when it is set else the
  config's `context_window`, else the model's known window (1M when unknown).

## The footer

Under the transcript sits the footer, which mirrors the terminal's. Every value
is composed in the extension host — `core/project.ts` gathers the on-disk state
and `core/footer.ts` turns it plus the transcript's usage into a `FooterState` —
and the webview only paints it, so the footer reads the same in both panes and
is unit tested without a webview.

It is arranged the way the composer is used, from the top down: the chips that
describe the next turn, the composer itself, and the dim line of numbers under
it.

- **Controls** — four icon-only buttons for model, reasoning, agent and project
  access. Each keeps its complete current value and action in its tooltip and
  accessible name. A click posts a `control` message that the controller routes
  to the same in-panel picker as the matching command. Session history remains
  the history button in the header instead of taking a second footer slot.
- **Composer** — the message box: the attachment strip, the textarea and the
  toolbar inside one bordered block. It starts two rows tall (`rows="2"`) and
  grows with the message up to 200px, where it scrolls instead.
- **Toolbar** — the **Attach** icon (the file picker), the live phase with an
  elapsed timer while a turn runs, and one action in the corner that swaps
  rather than sitting beside a second button: **Stop** while a turn runs with
  nothing to say, **Send** — which reads as **Queue** — the moment the box holds
  something. The desktop app swaps the same two the same way, so the button the
  reader is aiming at does not move as the box is typed into; an attachment is
  something to send too, while a context chip on its own is not.
- **Branch** — the repository the folder sits in, read from `.git/HEAD` rather
  than through the Git extension, so it needs no other extension installed; a
  worktree's or submodule's `gitdir:` pointer is followed to the real HEAD.
- **Gauge** — the last request's prompt tokens over the context window, amber
  past 70% and red past 90% (the terminal's thresholds).
- **Usage line** — `↑input · ↓output · RcacheRead · WcacheWrite · CHhit% · $cost
  · ctx %/window (auto)`, matching the terminal's footer segments. `CH` is the
  latest request's cache hit rate (`cache_read / prompt`), the same number
  `UsageTotals::cache_hit_rate` reports, and a step that reads no cache leaves
  the previous rate in place; `(auto)` marks auto-compaction as on, and the
  window is shown dimmed when nothing has run yet.

## Attachments

A message can carry images and PDFs, which the selected LLM reads as media. They are
added by pasting an image into the composer, dropping files onto it, or from the
**Attach** button or **Oxide: Add File or Selection to Chat**; on an image or PDF
in the explorer, `addToChat` attaches it as media rather than trying to inline
it as text. Each one becomes a chip above the textarea: a thumbnail for an image
the host could render small enough to send, a glyph and the file size otherwise,
with a ✕ to drop it and a **Clear** to drop them all. A chip's thumbnail is
drawn on a canvas once per attachment and cached at chip size, so the strip
being rebuilt with every state message paints the small copy rather than
decoding the photo-sized preview again.

The CLI takes attachment *paths*, so:

- a file already on disk is passed through as its absolute path, and its
  thumbnail is read by `src/attachments.ts` only when the file is small enough to
  be worth sending to the webview, and only once per length and mtime, so the
  chips that ride along with every state message never re-read and re-encode it;
- a pasted or dropped blob exists only as a `data:` URL, so the host writes it
  into one private temporary directory per window (removed when the window is
  disposed) and passes that path instead.

The webview downscales an image's longest edge to 1568px on a canvas before it
sends it on (`oxide_core::media::optimize_image` does the same on the CLI side),
so a retina screenshot does not travel as a data URL at full resolution. `src/core/attachments.ts`
is pure: it reads a data URL's type, refuses anything but the image types the
CLI sniffs (`png`, `jpg`/`jpeg`, `gif`, `webp`, `bmp`, and PDF), maps a type onto
an extension, names a written file so it cannot escape its directory, and
content-addresses the bytes so pasting the same screenshot twice stays one chip.
At most eight attach to a message, and a duplicate is refused with a notice.

An attachment is bounded so it cannot be multiplied through the chat: a paste
past the core's 20 MB limit (`oxide_core::media::MAX_ATTACHMENT_BYTES`) is
refused before the blob is read into a string, the host writes and echoes
nothing past it, and a thumbnail over `MAX_PREVIEW_CHARS` falls back to the
glyph and the file size.

The chips and the text attachments share one id space and travel in one
`context` message, so a single `removeChip` addresses either list. A queued
follow-up keeps the attachments it was queued with, and a turn that never started
hands them back to the composer instead of losing them.

The reads are best effort: a missing or malformed file blanks the value it
feeds — the model chip falls back to `config.json`, the branch and agent names
to empty — and never throws in the middle of a turn. `trust.ts` resolves
`trust.json` by closest ancestor like `oxide_core::trust`, folds in
`oxide.projectTrust` (a saved decision beats `defaultProjectTrust`, and `ask`
reads as untrusted because a non-interactive run cannot prompt), and
`settings.ts` reads `compaction.enabled` from the global and the project
`settings.json` with the project winning per key.

Two of those reads are gated on the numbers they feed rather than taken at face
value. The agent names come from the project's `.oxide/agents` and
`.claude/agents` only while the project is trusted: an untrusted run reloads the
ecosystem without project resources, but `Config::load` activates `--agent`
before that reload, so offering a project agent would run its prompt and
permissions inside a project the user did not trust. Installed plugins are read
too, from the CLI's own plugin state (`plugins/config.json`, enabled entries
with a live directory and manifest), because `ecosystem::load_enabled_plugins`
loads their `agents/` ahead of project resources — the order is project, then
plugins, then the global directories. The model picker asks `oxide models
--json --active` for the active provider's complete normalized catalog, the same source
as the TUI and desktop pickers. It keeps remembered models as a fallback when a
catalog refresh fails and accepts a custom ID; models from another provider are
not mixed in because a per-turn model override runs against the active provider.

## The file you are editing

The composer tracks the active editor, so the file being worked in rides along
with the next message without having to be attached by hand. It appears as the
last chip in the strip, dashed rather than solid and with a ✎, and
`oxide.autoContext` (on by default) decides whether it is tracked at all.

- It is a *path*, not a copy: the controller remembers the file and reads it when
  the message goes, so an unsaved edit made while the chip sat in the composer is
  what the run receives. A file the message already carries — attached by hand,
  or named with `@path` — is not sent twice.
- Its ✕ takes it out of the message while leaving the file tracked, so the chip
  is painted again the moment another file is opened; **Clear** removes it along
  with everything else pending. A new or resumed thread keeps it, since it is not
  a chip attached to the conversation on screen.
- It is not context the reader asked for, so it does not make an empty message
  sendable: the box still cannot send, and the host refuses a message with no text
  and no context of its own.
- Only a file open in a text editor is tracked — an image, a settings tab or an
  untitled buffer leaves the strip alone — and a file past the block limit is
  trimmed at the same cap as any other block, silently, since nothing was
  attached by hand to report on.
- A selection in that file is what the chip holds: it names the lines
  (`src/app.ts:12-15`) and sends those lines under the range's own header instead
  of the whole file, so a question about four lines does not carry four hundred.
  The lines are the 1-based ones a reader would count, and a drag that stopped at
  the start of a line does not take that line with it. Letting the selection go —
  or selecting elsewhere — returns the chip to the whole file, and its tooltip
  says in words what it is holding (`4 lines selected — sent with the next
  message`), since a hyphenated range is not what a screen reader reads out. The
  text is sliced out of the buffer when the message goes, like the whole file was,
  so an unsaved edit inside the selection is what the run receives; a file that no
  longer holds those lines sends nothing rather than something other than what the
  chip says.

## The `@` completion

Typing `@` in the composer offers the project's files and folders: `core/at.ts`
reads the token at the caret and matches it against the list the terminal's own
completion filters — `oxide_core::tools::workspace_paths`, one entry per file and
per folder with a trailing `/` — by a case-insensitive substring, capped at 200
rows. The rows are ordered by how well each path answers the query (a name that
starts with it, then a whole path that does, then one that mentions it anywhere),
the same ranking the terminal asks `oxide_core::at` for: the panel cannot link
the crate, so it mirrors the module's rules rather than its code.

- The token starts at a word boundary and runs to the next whitespace: an
  address (`mail me at a@b.com`) names no file, and a caret parked inside a
  half-typed `@src/ma|in.rs` completes the whole token rather than the part
  behind it.
- A file is taken with a trailing space, so the next word can be typed; a folder
  keeps the token open (`@src/`) so the query goes on narrowing inside it. A
  folder the reference already spells is left out of the rows — taking it would
  complete the token to what is already typed — so the row taken next walks
  *into* that folder.
- The paths are the workspace's own. The host reads them through the search
  provider (`vscode.workspace.findFiles`, so the exclude settings leave build
  output out), once per folder rather than per keystroke — and again when a turn
  ends, since that is where files appear, or when the exclude settings change —
  adds each folder above every file with the trailing `/`, and answers out of
  that list.
- Only the pane that asked is answered, since the rows replace a token in the box
  whose caret was read. Every question carries a sequence number and every answer
  echoes it, so a list for a value the reader has typed past — or dismissed with
  Escape, or sent — is dropped instead of painted under a moved caret or popping
  a list back over an empty composer.
- The arrows walk the rows, Tab and Enter take the highlighted one (a reference
  midway through a message is not the message), Escape closes the list and leaves
  the next Escape to a running turn. Taking a row splices in what the host said
  it stands for — the view never reads a token itself — and asks again, so a
  folder's own list is already up.

The list is a flex child of the composer card, above the message and the
attachment strip, so it stays attached to the box it completes and takes its room
from the transcript; it scrolls its own rows past `30vh`.

The same reference can be written from the editor instead of typed:
`oxide.insertReference` (`Option+K` / `Alt+K`, the key Claude Code's extension
uses) reads the file the editor has open — or the selection in it — and splices
`@src/app.ts`, or `@src/app.ts#5-10` for a selection, into the box at the caret,
with a one-line selection written as the one number it is (`fileReference` in
`core/prompt.ts`, the module that resolves it back into context). The lines are
the ones `selectionLines` reads, the same rule the tracked chip follows, so a
drag that stopped where a line starts does not name that line in either. The
host reads the editor, the renderer only splices what it is handed, and the
reference is kept off the words around it with the caret left after it, so it
can be followed by a question. It is offered wherever a file is open, since
what it writes is about that file.

## The `/` palette

Typing `/` at the start of the composer offers what the CLI itself offers: the
catalog `oxide commands --json` prints — `oxide_core::commands::palette`, the
same listing the terminal's `/` menu and the desktop app's palette draw. The host
reads it through the installed binary rather than walking `.oxide/commands`,
`.oxide/prompts` and `.oxide/skills` itself, so the panel cannot disagree with
the CLI about what a project holds, which scope wins a name, or which spelling
runs what. It is read once per folder and kept with the folder it came from, so
moving the active editor to another project cannot answer with the one before it,
and a read that fails is remembered as an empty catalog rather than respawned on
every keystroke. `core/palette.ts` holds the rules — a row per built-in, per
project command, per prompt template and per skill, matched by name
or alias with a name that starts with the query ranked ahead of one that merely
mentions it, capped at `MAX_COMMAND_ROWS` (200, the `@` list's own cap) — and the
webview only draws the rows it is handed, in the same list element as the `@`
completion, labelled *Commands and skills*.

- A skill is listed under its own name (`/rust-conventions`) with its description
  and a `skill` badge, because it is loaded as instructions rather than run as a
  command; taking the row completes the name and leaves it in the box, and
  sending it is what loads the skill, since the CLI resolves the name the menu
  lists. `/skill:<name>` is the terminal's other spelling of the same skill, and
  a command or a template with a skill's name stays the row — the CLI resolves it
  that way too, so the menu and the run agree.
- A row is completed rather than run: a command and a skill both take arguments
  (`/build src`), and the message that goes is what the CLI expands. A client
  command with no arguments is the exception — the panel performs it on the spot,
  through the same switch the matching footer chip uses, so a chip and its
  command cannot drift apart. `/model`, `/reasoning` (`/thinking`), `/agent`,
  `/trust` (`/access`), `/mcps` (`/mcp`), `/session` (`/sessions`), `/new`,
  `/attach`, `/usage` (`/cost`) and `/help`.
- A client command the panel has no action for — `/permissions`, the desktop
  app's `/theme`, `/connect`, `/logout` — is answered in the transcript rather
  than sent to the model as the text `/permissions`. Its row is left out of the
  palette, and the desktop-only names are dropped with it, so a row is only ever
  offered for something the panel or the CLI can actually do.
- Arguments after the name are the message, as they are in the terminal:
  `/session auth is broken` is a prompt the agent has something to say about, not
  the session listing.

## Wire protocol

`core/protocol.ts` turns the CLI's Pi-shaped events into view updates. Only
`type` is guaranteed, so every field is read defensively.

Events consumed: `session`, `thinking`, `thinking_done`, `message_update`
(`thinking_delta` / `text_delta`), `tool_call`, `tool_execution_update`,
`tool_execution_end`, `usage`, `auto_retry_start`, `compaction`, `error`,
`approval_request`, `question_request`, `question_closed`, `turn_changes`, and
`agent_end`. `thinking_done` marks the end of a model step (its `ThoughtDone`
counterpart), so a later step's output does not merge into, and a retry cannot
discard, a previous step's committed reply. `turn_changes` carries the folder
that run was in, the files it changed and the revision it started from; the CLI
writes it behind that run's `agent_end` on the same channel — the listing can
neither overtake the turn it belongs to nor the events of a prompt sent after it
— so the card lands where the turn ended.

The transcript is a list of items (`user`, `assistant`, `thinking`, `tool`,
`notice`). The view applies small deltas: `push` a new item, `remove` a
discarded one, `append` a text/output fragment, `patch` a tool card when it
settles, and `status` / `usage` / `context` for the footer. The `state`,
`status` and `usage` messages carry the whole `FooterState` — the controller
attaches it, since it is the only place that knows the context window and the
resolved settings. Reasoning and text stream into separate items, and a
thinking block is created by its first delta, so a turn that only starts one
never leaves an empty block in the transcript. When a stream drops and the CLI
retries, `auto_retry_start` drops the item the failed attempt was streaming
into, so the retry's fresh output does not extend the partial reply.

`context` carries the composer's pending context *and* attachments — the tracked
file's chip travels in the same list, marked `auto`, so the view can paint it as
tracked and leave it out of what counts as something to send — and the
messages back are `send`, `stop`, `newSession`, `resumeSession`, `dialogSearch`
(the session listing's search box, which the host answers with the narrowed
listing), `attach` (a
pasted blob as a `data:` URL), `attachFiles` (dropped paths), `pickFiles`,
`removeChip` (by chip id, either list), `clearChips`, `completeAt` (what the
composer holds and where its caret is, numbered, answered with `atSuggestions`:
the rows and the range of the value they replace, or no rows to close the list),
`notice` (something the
view could not do, such as a paste it could not read) and `control`. A
chip-shaped `control` message is the one that carries the model, agent or trust
picker's choice back into the same action the matching command runs;
`test/commands.test.ts` asserts the two sides agree, because a mistyped message
kind would otherwise fail silently on either side of the bridge. A `dialog`
message paints the listing on the edge it was composed with — under the header,
or above the footer (or clears it with `null`) — and `dialogAction` is the
answer to one: the action a clicked row or button carried, in the same shape a
chip's control id travels.

## Prompt assembly

`core/prompt.ts` builds the prompt the same way the CLI's own `@file` expansion
reads: each attached block is a `--- path[:range] ---` header plus its text,
then the message. The tracked file's block is built here rather than painted
into its chip, which is what carries an unsaved edit into the run. Images and PDFs (`png`, `jpg`/`jpeg`, `gif`, `webp`, `bmp`)
are not inlined; they are attached as media, named in the `prompt` request the
CLI reads (`images` in `core/rpc.ts`).

Because the prompt is sent on stdin, a message's own `@path` references are
resolved by the extension instead of the CLI: `@src/main.rs` becomes a context
block, an image/PDF becomes an attachment, and a reference that does not resolve
stays in the message. Duplicate references are collapsed, and trailing
punctuation is not taken as part of the path. The composer's completion offers
the paths that will resolve this way — a folder is only a step into one, and a
reference that resolves to nothing is left for the model to read as text.

A reference may also name a line range, which is what the editor's insert
shortcut writes for a selection: `@src/app.ts#5-10` becomes a block holding those
lines under the range's own header (`--- src/app.ts:5-10 ---`), so a selection
arrives as the lines that were selected rather than as the whole file. A range
that starts past the end of the file is left in the message as typed instead of
being sent as a block with nothing in it — a file edited between the shortcut and
the send would otherwise read as an empty file — a range that runs past the last
line stops there, two ranges of one file are two blocks, and a range written
after an image or PDF is ignored, since an attachment travels whole rather than
being read as text — which also makes two ranges of one image one attachment,
not the same bytes riding on the message twice.

## Diff previews

The event stream does not carry `AgentEvent::ToolResult`'s
`DiffPreview`, so `core/preview.ts` rebuilds one from the tool's arguments
against the current file. It uses the same LCS line diff and the same compact
line-numbered layout as `oxide_core::diff`, and applies `edit` calls the way the
tool does — a byte-exact match first, then the same tolerance for trailing
whitespace and the `N|` line numbers a `read` result prints — while still
tolerating the argument shapes the tool itself accepts. A preview is only built
for a file inside the workspace, so the panel cannot be used to read outside it.

## Changes a turn made

VS Code's own diff editor draws a finished turn's changes; the panel renders no
diff of its own. The CLI emits `turn_changes` when a run ends — the folder it
ran in, the revision it started from, the state it left behind and the files it
changed, the same listing the desktop app's change card shows and `oxide changes
show` reads, built from the project's shadow snapshot so a file a shell command
or a formatter wrote is listed beside the ones a tool call named. A run that
changed nothing sends no frame.

`core/changes.ts` reads that frame and composes the card as data: the title and
the turn's `+`/`−` totals, the folder the run was in, and a row per file with its
`A`/`M`/`D` badge, path and detail (`+12 −3`, `−4`, `no line changes`, `binary`).
`media/main.js` only paints the rows — a click posts back the card's id and the
row's index, and the header's icon opens the turn as a whole — so the webview
decides nothing about what a change is. The whole turn goes to `vscode.changes`,
which VS Code draws as one multi-file diff whose rows are the listing the card
just showed; a row goes to `vscode.diff`, on that one file.

The left side of a diff is the file as the run found it, which is nowhere on
disk: it is read out of the shadow snapshot by the CLI's own read —
`oxide changes show <path> --baseline <rev> --project <root>` — and served to
VS Code by a content provider registered for the `oxide-changes` scheme, with
the project and the revision in the URI's query (`snapshotQuery`, since the
provider is handed the URI alone). Both sides therefore come from the card's own
run rather than from whatever folder is active when a row is clicked: a
multi-root window can move the active editor elsewhere while the card stays in
the transcript. A file the run added has no such revision, so its left side is
empty; one it removed has no right side, and the diff shows the deletion. Both
are best effort: a read that fails leaves that side empty rather than failing the
open, and a card whose turn has already left the transcript opens nothing at all.

The triple each file is passed to `vscode.changes` as is the shape that command
takes — `[label, original, modified]`, where the label is the file's own URI —
and its last two entries are the `[left, right]` pair `vscode.diff` is given off
the same triple for a single row.

A card also carries **Review**, which opens the turn's files over the panel
without leaving the chat. It is the card's own rows again — the badge, path and
`+`/`−` detail — with the arrows walking them with wraparound, and <kbd>Esc</kbd>
or the ✕ closing it, so the review and the listing behind it cannot disagree
about what a turn changed. The diff is not painted there: the row it lands on
opens that file in VS Code's own diff editor, which already draws both sides with
per-side line numbers and word-level marks. The panel's rows are therefore not a
second listing to keep in step, and nothing in the webview renders a diff of its
own.

A review opens its file as a *preview* and without taking the keyboard
(`preview` and `preserveFocus` on the `vscode.diff` options), since walking on
replaces the one tab rather than leaving a turn's worth of them behind, and the
arrows keep working while the editor holds it. A click on a row of the card
itself is neither: it opens a tab of its own and focuses it, because the reader
asked for that one file. A rebuilt transcript closes a review left open, since
the card it was opened from is gone with it.

A listing longer than five files folds the rest away behind one row (`+N more
files`, and `Show less` back), as the desktop app's card does. The count and both
sets of words are composed by `core/changes.ts`; the webview only hides the rows
past `visible`, so a fold cannot disagree with the listing it folds.

A card also carries **Undo**, which puts the turn's files back to how the run
found them. It asks first, in the panel's own dialog: `undoChangesDialog`
composes a row naming the card and a row that keeps it, and only the first of
them restores. The restore is the CLI's — `oxide changes undo --baseline <rev>
--after <after> --project <root>` — which is the same `Snapshots::restore` the
desktop app's Undo and the terminal's `/undo` perform, so the panel needs no
snapshot code of its own. `--after` is the state the turn left, which the CLI
checks the work tree still holds before it puts anything back: an older card's
restore cannot take a newer turn's work with it, and what it refused is reported
in the panel rather than failing silently. So only the newest turn's card offers
the Undo — a card a later turn came after has it taken away, by the `{k:
"changes"}` message that newer card's own push returns — and a card whose turn
was put back says `Undone` and offers nothing further, since the listing is still
what that turn did. An Undo clicked while a turn is running is refused the way a
thread delete is, because the run owns the files a restore would move under it.

A project does not have to be a git clone for any of that: the snapshot is
oxide's own bare repository under the config directory, so a plain folder is
recorded the same way. What is refused is a directory that must not be walked —
the home directory or an ancestor of it, anything holding the config directory,
and one that is neither inside a git work tree nor project-sized. Where the
snapshot is refused there is no `baseline`, so `turn_changes` is never emitted,
the run is unaffected, and the panel simply has no card to show.

Because the call's own diff is already in that listing, a tool card that changed
a file (`write`, `edit`, `patch`) reads as one line — its header carries the path
and its state the `+`/`−` counts — and keeps its own diff for the reader who
clicks it, so the same change is not painted twice. `media/main.js` counts the
markers itself, the way `oxide_core::changes` does, so a card's numbers and a
change row's agree. The host still composes that diff (`core/preview.ts`); what
changed is when the webview paints it.

## Check for updates

**Oxide: Check for Updates...** (in the command palette, and on the panel's
title where the other pane-level commands sit) reports the newest release of
**this extension** and installs it. The extension releases from its own train
(`extension-v*`) carrying `oxide-vscode-<version>.vsix`, and that is what the
panel offers: the check is `oxide update --check --json --component extension
--current <the version loaded here>` (`core/updates.ts` composes the arguments
and parses the answer, `core/dialogs.ts` composes the dialog, so the webview
only paints rows), so the resolution — which tag belongs to the extension,
which file that release publishes for it, whether it is newer than this
window's — is the shared `oxide_core::updates` rules the terminal's own update
reads, while what is installed is this editor's extension rather than the
command line the panel, the terminal and the desktop app all run.

A release resolved as a VSIX is a row — **Install 0.34.0**, naming
`oxide-vscode-0.34.0.vsix` — and a release with nothing this panel can install
(a train that published no `.vsix`) is reported with the check's own sentence
naming the file to install by hand, with no row to press. **Release notes**
opens the release page in the system browser through `openUrl`. The row's
detail says what the click does: `src/updates.ts` fetches that URL — following
the redirect a release download answers with — into a private temp directory
only this user can read, and verifies the file against the SHA-256 the release
reports for it before anything else happens; a body that does not match is
refused, and a release that published no checksum is installed unverified with
that said in the dialog. The `.vsix` is then handed to VS Code itself —
`workbench.extensions.installExtension` — because the editor owns what
installing an extension means (where it goes, and whether the package is this
extension at all), and the dialog reports what it put in place: `Oxide 0.34.0
is installed`, `Was 0.33.0 · Installed extension-v0.34.0 ·
oxide-vscode-0.34.0.vsix`, with **Restart Window** (which reloads the window,
since the extension running here is the one that was there when it was
replaced) and **Close**. The temporary directory does not outlive the install,
whether it worked or not, and one install runs at a time.

A CLI released before this panel cannot answer the check at all: it says
`unexpected argument '--json'`. That is not a check that failed, so the dialog
names it and offers the one command that works on any version — **Install the
newest CLI**, which runs the plain `oxide update` and replaces that binary,
exactly the thing standing between the user and a check at all — with the
report it printed shown under the headline; an installation already current
answers `Already up to date`, and that is repeated as the CLI's own line rather
than reported as an install. A check that could not reach GitHub reports what
went wrong instead, rather than an empty listing that would read as up to date.
The command is a palette entry, so it can be run again while the first request
is waiting on GitHub: the newest check owns the dialog, and an answer that a
newer one has replaced is dropped rather than painting an older release over it
— as is an install a newer check has overtaken, since what it would report is a
state a newer answer already describes. The extension is updated from the release
train it was built from rather than from the Marketplace, so a window that is
already new enough is told exactly that.

### The check a launch makes by itself

A window also looks on its own: `ChatController.checkForUpdatesInBackground`
runs once at activation (`extension.ts`), so a release published since the last
look is noticed without anyone asking for it — the way every other extension in
the editor is kept current. Nothing is painted over the panel and nothing waits
on the network: the launch goes on, and what the check finds is reported with a
VS Code notification rather than the panel's own dialog.

`checkForUpdates` in a `settings.json` decides whether a launch looks at all
(`core/settings.ts`; the project's `.oxide/settings.json` wins over the global
one and `OXIDE_CHECK_FOR_UPDATES` overrides both, the same resolution
`update_notice.rs` makes for the terminal's notice). The answer is remembered in
the window's global state — when it last asked, and the release an install left
here — so a launch asks GitHub at most once every six hours
(`BACKGROUND_CHECK_MS`, the interval `oxide_core::update_notice` refreshes on),
and a window already carrying a release it has not reloaded into asks nothing,
since every answer would be the version already on disk (`core/updates.ts`).

What happens with the answer is `backgroundAction`: a release this editor can
take — a `.vsix`, which is what the extension publishes — raises a notification
(`Oxide 0.36.0 is available (installed: 0.35.0).`) with an **Install** row and a
**Later** one, and pressing **Install** downloads that file, verifies it against
the checksum the release reports and hands it to VS Code — the dialog's own
machinery without its dialog, since the panel is not what the user was looking
at. The file is fetched when that row is pressed rather than written into the
editor at launch, which is how the marketplace's own extensions are kept
current: a check that reports, and a click that installs. The launch's own flow
is the notification's alone: its install runs the dialog's machinery with the
transcript and the dialog left out of it (`report = false`), so a release
noticed at activation never writes a line into whatever conversation happens to
be on screen. Once VS Code holds the
release the window is told what is left — `Oxide 0.36.0 was installed. Restart
the window to run it.` with a **Restart Window** row — since the code running is
the one it replaced. Which install happened is remembered by the install itself
rather than by the row that asked for it, so a release installed by hand is
also a release every later window knows about: the version on disk is what
answers, and a window still running the old code offers nothing while it is
newer. A release the panel has no file for (no build for this
platform, or a CLI that answered without one) is reported with a **Release
notes** row instead, which opens the release page where that file is, rather
than offering an install nothing could run. A window with nothing to do stays
silent: an extension already current, a check that could not reach GitHub, a
machine with no `oxide` on it, and a CLI too old to know the check's flags are
all reported by **Oxide: Check for Updates...** alone — running that command is
what says the installed CLI has to be replaced first. What a launch did find is
named in the extension's Output channel (`Oxide 0.36.0 is available (installed:
0.35.0).`), which is where a check that failed or an install that did not finish
says so too, so the offer is never the only thing the reader has to go on.

## Rendering

`media/main.js` renders the transcript in the webview; it is adapted from the
desktop app (`crates/desktop/ui/app.js`), so a reply reads the same in both.
Assistant replies are Markdown (headings, lists, tables, fenced code with
lightweight syntax highlighting, inline emphasis/code/links, and auto-linked
bare URLs). Tool calls are collapsible cards colored by state, with a spinner
and elapsed time while running, a short per-tool output preview that expands on
click, and a colored diff for `write` / `edit` / `patch`. A card whose output was
cut carries the words offering the rest — `Show 36 more lines` — as a second
handle under the preview it kept, which opens and closes the card the same way
the header does and keeps that count off the body it stands for. The header is
that handle only while something is behind it: a card with a rest to show is a
control — a caret pointing at the output, a tab stop, and the state a screen
reader asks a fold for — while a call that already shows everything it has is
plain text, with no caret, no tab stop and no button that reveals nothing when
it is pressed. Reasoning renders as a muted thinking block that
`oxide.showThinking` can hide.

Monospace output — a tool's body, code blocks, the diff — is set at
`--code-size`: the panel's own text size or the editor's `editor.fontSize`,
whichever is larger. The editor's font alone is often small enough that a build's
output is hard to read in a side bar, and the panel's body carries a 1.5
line-height so those lines are not left at the browser's `normal`.

The footer and composer are painted the same way — from `FooterState` data, not
from decisions taken in the browser — and the composer's own interactions (paste,
drag and drop, the canvas resize before a pasted image is sent on) are the only
logic in the webview. A dialog is handed over the same way: the rows and the
action each one posts are composed in the host (`core/dialogs.ts`), so the
webview is only a renderer and a click is reported rather than interpreted.
Its own layout is all in `media/style.css`: a listing is a flex child of the
panel's column rather than an overlay, which lets its row list scroll. One
complete, rounded card serves both positions — it sits under the header in the
markup and the `.pin-footer` class the host's `pin` becomes moves the same node
above the footer with `order`. The transcript is ordered between the two either
way, so the listing takes its room from the transcript. It is also
the one child of that column that does not shrink — the transcript is a scroll
container whose content is what its base size is measured from, so a dialog that
may shrink opens a couple of rows tall with the rest of the list scrolling
inside a sliver. A hidden icon button is turned off explicitly
(`button.icon[hidden] { display: none }`) rather than left to the attribute's
own rule, since the `display` an icon button is laid out with beats it — which
is what the composer's corner depends on to show Stop and Send one at a time,
and what keeps a dialog's **Recheck** from appearing before the host offers one.
Nothing there writes to disk: a pasted blob goes back to
the host as a `data:` URL, and the host is what decides which file to write and
which path to send.

Links in a reply open in the system browser (`chatView.ts::openUrl`), since a
webview cannot navigate to a remote page; a path in a tool card opens in the
editor, but only inside the workspace.

The transcript follows the newest line on its own, and stops doing so the moment
you scroll up to read something earlier — scrolling back to the bottom picks it
up again. Two details make that work: text deltas are painted on an animation
frame, so the scroll that follows one is taken after that paint rather than
before it, and the pane gets shorter whenever the composer grows (an attachment
chip, a taller message box, a usage line that wraps), so a resize puts the
newest line back on screen. A running command's own output box is capped at 40vh
and follows its last line the same way, since writing its text back would
otherwise reset it to the top.

## Commands and settings

The manifest defines the two view containers (activity bar and secondary side
bar), the editor toolbar entry, the commands and keybindings, and the `oxide.*`
settings. See the [extension README](../editors/vscode/README.md) for the
user-facing tables. The footer's chips are shortcuts into the same actions:
`setModel`, `setAgent`, `setReasoning` and `setProjectTrust` are reached from a
chip click and from the palette, so the two entry points never drift. Session
history stays in the header (and is also reachable through `/session` and the
command palette), rather than duplicating another control in the footer.

## Development and testing

```sh
cd editors/vscode
pnpm install
pnpm run compile   # tsc -p .
pnpm test          # compile, then node --test out/test/
pnpm run package   # vsce package -> oxide-vscode-<version>.vsix
```

Press <kbd>F5</kbd> with the folder open to launch an Extension Development
Host. The tests cover the pure modules only: argv building, prompt assembly and
`@path` expansion (a reference's line range among them, down to the lines its
block carries and the range a file that shrank leaves in the message), the `@`
completion's token and rows (`test/at.test.ts`),
attachment types and naming, diff and tool previews, the change listing, its rows
and the diff plan a click opens (`test/changes.test.ts`),
session-list parsing, config-dir resolution, binary lookup and the plan a
command is started from, and the transcript state machine — plus, in
`test/approvals.test.ts`, the approval request parsing,
the titles a card shows and the request frames the CLI reads, in
`test/questions.test.ts`, the same for a skill's question — the request a
`question_request` event becomes, the answers the webview posts back (a blank
form among them, which is the dismissal Dismiss posts, and the answers an earlier
step kept when a later one is submitted), the title and settled label a card
carries, and the frames the CLI reads — in
`test/views.test.ts`, that the chat view ids the host
registers match the views `package.json` contributes, in `test/brand.test.ts`,
that the two icons stay the desktop app's, in `test/commands.test.ts`, that
every contributed command has a handler, every footer chip has a click handler,
and every message the webview posts is handled by `chatView.ts` — and that an
answer is routed on to the running turn rather than only settling the card, and that the
editor's own chrome agrees with the host: the toolbar entry and the icon it
needs, the two keybindings, the caret toggle asking the controller which side it
is on rather than assuming, and the insert shortcut reading the editor in the
host and handing over a reference the renderer only splices — and that the
composer's own messages have a case in the renderer, and that the
header's new-chat button carries the command's own name in its tooltip and
`aria-label` rather than the name the command had before, and that the session
listing is composed in exactly one place, which supplies the id of the thread on
screen, so no redraw can quietly drop the `Current` mark, and that an open
session listing is read again when the `session` header names the thread and when
a turn ends — through `showSessions`, and only for a session listing — and that
the composer's
chip for the file being edited stays in step with the editor and is read when the
message goes, and that an `@` completion is answered from the shared core — the
token and the rows from `core/at.ts`, the paths from the workspace — since the
renderer never decides what a token is, and that the `/` palette is drawn from
the CLI's own catalog — the listing `core/palette.ts` parses, the rows it builds
and the routing that leaves a command or a skill to the CLI while the client
commands the panel owns are performed (checked against the real catalog in
`test/palette.test.ts`) — and in
`test/webview.test.ts`, that `media/main.js` — plain JavaScript with no type
checking — paints the footer, the chips, the attachment strip, the approval
card, a question card's steps, options and free-text fields and the answers a
click posts, splices the host's reference into the box at the caret and leaves
it there for the next thing typed, tells the host when the pane takes the
keyboard and when it gives it up, and leaves a modified Escape (the host's own
keybinding) to the editor,
the change card and the diff a row or its header opens (against the CLI's own
listing, which the host composed), and the review it opens — its rows, no diff of
its own, the file each row and each arrow names to the editor, the arrows walking
them with wraparound, and the ✕ or a rebuilt transcript closing it —
the output of a tool card folding from the row that offers the rest of it as
well as from its header, and a card that already shows everything it has
offering no fold and no caret —
the `/mcps` and `/sessions` listings (and the full-size image a thumbnail
opens, from the ✕ in its head row, the backdrop or <kbd>Esc</kbd>), the tracked file's dashed chip and the empty box it cannot send on its
own, and the rows of the `@` completion with the keys that walk, take and close
them — the `@` rows and the palette's side by side, since the two share one
list, and the palette row carries the name, the arguments hint, the description
and what the row is — and the disabled
state of Send from its messages when it runs against a DOM stub, and that the
composer's corner holds one action that swaps between Send and Stop rather than
two visible buttons, and that the
shell puts the listing under the header, ahead of the transcript, with
`flex: 0 0 auto` so it cannot be squeezed to a sliver of scrolling rows, turns
the same element around above the footer for the listing the host pins to that
edge, and carries icon-only buttons in its own header, and that the completion is
inside the composer card above the attachment strip, and that the welcome page
stays up while the page only carries a notice — which is what "New chat" leaves
behind, since a line about the thread that was closed is not a message in a new
one. The dialog composition itself — the rows, the status each server is in,
the switch it offers, the session rows — is covered in `test/dialogs.test.ts`.
The footer's own
readers are covered one file each: `test/settings.test.ts`, `test/trust.test.ts`,
`test/git.test.ts`, `test/agents.test.ts`, `test/plugins.test.ts`,
`test/project.test.ts` (the five together, against an injected file map, with the
untrusted and plugin-agent cases spelled out) and `test/footer.test.ts` (the
labels, the usage line and the gauge). The `/mcps` listing's own parsing — the
server listing, the per-state names, the toggle arguments and the bare slash
command — is covered in `test/mcps.test.ts`, which also holds the controller's
`send` to answering that command before a message is queued or prompted (a
source-level assertion, because `chat.ts` imports `vscode` and cannot be loaded
there). What each dialog's rows say — a server's state and the scope its toggle
writes to, a session's age and size, the row that closes the open thread and the
`Current` mark on it, the panel edge each listing opens on, the rows' own
actions — is covered in
`test/dialogs.test.ts`, which needs neither a webview nor a CLI. What a resumed
thread reads back — `oxide sessions show --json` parsed into the turns the panel
replays, a call paired with the result that answered it, and the totals the
footer shows, including the output of a CLI that answered with nothing — is
covered in `test/history.test.ts`.

`pnpm test` runs on Linux, macOS and Windows in CI (`.github/workflows/ci.yml`),
because the parts of the extension that touch the system have a per-platform
branch: the binary lookup across `PATH`, `~/.local/bin` and `~/.cargo/bin` with
`.exe`/`.cmd`/`.bat` candidates, the `cmd.exe` wrapper a batch shim needs, the
config directory, and the git branch read from the closest `.git/HEAD`.

## Packaging

`pnpm run package` runs `vsce package`. `.vscodeignore` keeps `src/`,
`out/test/`, source maps, `node_modules/`, `tsconfig.json`, `docs/`, and built
`.vsix` files out of the bundle, so the VSIX ships `out/` (without the tests),
`media/`, the manifest, the README, and the pnpm lockfiles.
`vscode:prepublish` compiles first, so a packaged bundle never contains stale
JavaScript.

`pnpm-workspace.yaml` disables the install scripts of the two `vsce` transitive
dependencies (`@vscode/vsce-sign`, `keytar`) that pnpm otherwise refuses to run.

Release notes are drafted on every push to `main` by
`.github/release-drafter.vscode.yml`, pinned to the `extension-v*` tag prefix so
it resolves versions from its own releases only (the prefix avoids the CLI's
`v*` tags and its drafter). The draft already carries the tag to push. Pushing
that tag runs `.github/workflows/vscode.yml`, which builds the VSIX with
`pnpm run package` and attaches `oxide-vscode-<version>.vsix` to the release.
