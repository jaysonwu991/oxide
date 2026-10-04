# Desktop app

The `oxide-desktop` package (`crates/desktop`) is an
[Electrobun](https://electrobun.dev) front-end for the
same agent the terminal CLI runs: one native window whose **main process is
Rust**, the same language the rest of Oxide is written in. The goal is one
configuration and one session
store shared by every front-end (the CLI, the desktop app, and the VS Code
extension), with a GUI that can manage **multiple projects** (cross-repo) and
show each project's session list.

## Workspace

The repository is a Cargo workspace:

```
crates/
  core/       shared agent core (config, providers, tools, MCP, sessions,
              snapshots, plugins, agent loop, runner)
  cli/        the `oxide` terminal binary (TUI + print/json/rpc modes)
  desktop/    the `oxide-desktop` app
```

The Cargo package names are `oxide-core`, `oxide`, and `oxide-desktop`.
`oxide-core` owns everything the agent needs; the CLI and desktop are thin
front-ends over it, and `oxide-desktop` reuses `oxide_core::config` and
`oxide_core::session` directly, so they read the same `config.json`, `auth.json`,
`settings.json`, and `sessions/` tree. The Electrobun shell only owns the window
and the transport: the command layer, the project registry, the turn loop and the
brokers are ordinary Rust and are tested without a webview. `crates/desktop` is
excluded from the Cargo workspace (`exclude = ["crates/desktop"]` in the root
`Cargo.toml`), because Electrobun's own build owns the binary's layout and
version — it is a project of its own, built from its own directory. The VS Code
extension (`editors/vscode`,
see [docs/vscode.md](vscode.md)) is a separate pnpm package that drives the
`oxide` binary, so it shares the same files without linking `oxide-core`.

## Desktop layout

```
crates/desktop/
  src/
    lib.rs          the library the window drives: at, manager, turn
    manager.rs      project registry + session aggregation
    turn.rs         starts an agent turn against a project
    at.rs           the `@path` walk the composer completes from
    commands.rs     the window's command dispatcher (needs `gui`)
    approval.rs     interactive approve/deny broker (needs `gui`)
    ask.rs          a skill's question broker (needs `gui`)
    bridge.rs       the window, and where an event leaves it (needs `gui`)
    update.rs       this app's own release train (needs `gui`)
    main.rs         Electrobun entry point: the window, the menu, the loop (needs
                    `gui`)
  ui/               front-end: index.html, app.js, style.css
  electrobun.config.ts  app config: name, identifier, version, window, the copy
                        of `ui/` into `views/main/`, icons, entitlements, and the
                        signing flags the release pipeline's env decides
  hutch.config.ts   the Electrobun release whose devkit this package builds against
  .hutch/devkit/    the Rust SDK `hutch electrobun prepare` projects here (generated,
                    not checked in; `Cargo.toml` points its path dependency at it)
  icons/            app icons (`icon.iconset` for macOS, `icon.ico`, `icon.png`)
  check-app.mjs     front-end check (stubbed DOM and bridge)
  check-shell.mjs   shell check (command/event contract, config, bundle)
```

`electrobun` is an ordinary dependency of this package rather than one behind a
feature, because Hutch's own build is what produces the app: it compiles the
manifest and binary `electrobun.config.ts` names (`Cargo.toml` /
`oxide-desktop`) with no feature to turn on, so a binary gated behind one is a
binary that build would not produce. `src/lib.rs` (the `at`, `manager` and
`turn` modules) builds without a window at all — it is what the unit tests
cover — while `commands.rs`, `update.rs`, `approval.rs`, `ask.rs`,
`bridge.rs` and `main.rs` are declared by `main.rs`, so their tests run with
`cargo test` from `crates/desktop` — which is what `.github/workflows/ci.yml`
runs on each platform.

Electrobun runs a build in `crates/desktop`, which compiles the Rust main
process and copies `ui/` into `views/main/` verbatim: the
page is one plain script with no imports to resolve, so there is nothing for a
bundler to do and no `package.json` to install. The window loads the page from
Electrobun's own `views://` protocol, and it talks to the main process over the
bridge Electrobun's preload installs: a request is
`{type: "request", id, method: "oxide_invoke", params: {command, args}}`
posted on the user bridge, answered with
`{type: "response", id, success, payload|error}`. The page imports nothing from
a framework API — `bridge.rs` performs the folder picker and `open_url` in Rust,
and the page asks for them by command name — so the window needs no capability
grant and no allow-list beyond the bridge itself; the page's own
`Content-Security-Policy` in `ui/index.html` is the window's
(`default-src 'none'`, `script-src 'self'`, `style-src 'self' 'unsafe-inline'`,
`img-src 'self' data: blob:` for attachment thumbnails and previews,
`font-src 'self'`, `connect-src 'none'` — nothing in the page fetches anything —
and `form-action`/`base-uri`/`frame-ancestors` `'none'`).

Events travel the other way on the window's own channel: `bridge.rs` holds the
window and emits `agent-start`, `agent-event`, `agent-end`, `approval-request`,
`question-request` and `question-closed`, which `app.js` listens for, plus
`check-updates`, which the macOS menu item emits so the window performs the
check and paints one dialog (see [Check for updates](#check-for-updates)). A run can
only be watched, answered or stopped from the window that started it — the
events carry no state a fresh window could be rebuilt from — so closing it ends
the app and the turn with it (`runtime.exitOnLastWindowClosed` in
`electrobun.config.ts`, and the window's own `close` callback, which stops the
event loop), which is deliberately not the usual macOS "stay resident" behavior:
a resident app with no window would leave a turn streaming into nothing, with its
approval and question requests unanswerable.

## Interface

The window follows a Codex-style layout. Every glyph it draws for itself is
inline SVG — the tree's folder, a document, a caret and a check, the mark that
signs a reply, `+`, and the remove `✕` — stroked with `currentColor` in the
panel's own two boxes and weights (24 units at 1.8 for a glyph that fills its
button, 16 at 1.5–1.6 for the marks that sit on a text baseline), so nothing on
screen is a character whose shape and size would come from the machine's fonts,
and every control's words live in its tooltip and its `aria-label`, as the
extension's do. The characters the window still types are text in a line rather
than a control's drawing: a tool card's `✔`/`✖` and the `☐`/`☑` of an assistant's
task list, which the panel's own renderer types too, the `✦` that opens a
thinking block, which the terminal's does, the `⌘` naming a shortcut in words,
the `·` a line of derived facts is joined with, and the image preview's head-row
`✕`. Drawing one here and typing it there would make the two front-ends read the
same state two ways, so `check-app.mjs` pins the set the window's own strings
carry — a character a control could wear cannot appear under cover of them. It
also holds the paths the two front-ends share beside the extension's own
sources, so chrome drawn one way in the panel and another in this window fails
the check.

The regions, top to bottom:

- **Sidebar** — **+ New Chat** at the top, opening a thread in the project the
  window is in and, when nothing is open yet, the folder picker that asks which
  project to start in — a folder is never picked for the reader, and with no
  project at all the same button offers the Add-project dialog; the **Projects**
  tree, and a footer pinned
  to the bottom with the two controls that belong to the window rather than to a
  message: the theme and the check for updates, an icon button each (a
  half-filled circle, and the same refresh arrow the
  extension's own check carries). The tree groups each project's sessions
  under it, and every project and stored session row carries a `✕` that removes
  it (see [Multiple projects](#multiple-projects-cross-repo)), drawn on the row
  itself rather than behind a pointer, beside the project's own **New task**
  `+`. A row carries nothing else: the key a thread answers to is nowhere on
  the row, since the shortcuts dialog is where that list is written down and a
  badge beside the `✕` was a second thing the title had to make room for. A
  control that exists only while the row is pointed at is one the reader has to
  hover before they can click it, and a tree whose buttons appear under the
  cursor changes what it says as the reader moves; the room each one takes
  belongs to the row's own padding and nothing moves when a pointer crosses it,
  and they are quiet at rest and take the accent or the error colour while
  pointed at, which is a highlight rather than the reason they are there. That
  padding is also what keeps them off the text: each row reserves the room the
  control at its end takes (`padding-right` against that control's own width and
  offset), so a press meant for a name is a press on the name and never on the
  button drawn beside it. The active project and the active thread both
  carry the accent bar, so which one is on screen reads the same in either
  list, and a thread is listed — under the summarized title of its first
  message — as soon as its turn starts rather than once it ends: the thread the
  window is in stands in for itself in the tree, in that project's session
  count and in the `/sessions` list until the store has written it, keyed by
  the same id so it is never listed twice. It is named by the title the window
  kept beside that id rather than on the turn that gave it, since the thread
  outlives the turn: a run whose thread was parked (see
  [Agent turns](#agent-turns)) and opened again is that same row under the same
  name. Its row is the window's own while no
  file stands behind it, so selecting it leaves the thread on screen as it is
  and it is offered no `✕` — there is nothing stored to delete. A turn's own
  thread is marked on its row wherever the reader is — a spinner beside the
  title, and a tooltip saying a turn is running in it — since the transcript may
  be another conversation's (see [Agent turns](#agent-turns)). A window with
  no thread on screen is starting one, so its next message opens a thread of its
  own instead of being appended to whichever thread was used last. The `+` in
  the Projects header
  opens the **Create project** dialog: pick one or more source folders and the
  **Project name** defaults to the first folder's basename (still editable), so
  creating a project never requires typing a name.
- **Home** — what the transcript shows before a thread is in it. It follows
  Codex's own habit of opening on something to do rather than on an empty box:
  with no project open it lists the newest threads across **every** project the
  sidebar knows, each under its summarized title and the folder it is in, and
  one click resumes it in its own project; with a project open it is the
  invitation, the folder and the suggestion buttons it always was.
- **Top bar** — the open thread's title: the same label its sidebar row shows,
  and a running turn's own summarized title (sent with `agent-start`) before
  the listing has it. Beside it, while the reader is in another thread, a strip
  names the turn still running (`A turn is running in “<title>”`) with a spinner
  and an **Open** affordance, and the click opens that thread again — the way
  back to the Queue or Steer it takes. A new task carries no placeholder, and the provider is
  not repeated here because the composer's model chip already names it; the
  right side says only what has to be acted on (`no API key`, `project
  resources off`). The window is Electrobun's own (`WindowOptions` in
  `src/main.rs`): no AppKit event monitor, Objective-C hook, preload script or
  platform-specific input path is installed, and every control is wired to a
  plain `onclick`.
- **Conversation** — a centered 780px column, the width the composer and the
  popovers above it share, so the window's edges line up rather than each row
  measuring itself. User messages are right-aligned
  bubbles; assistant replies render Markdown and links open in the system
  browser (see [Rendering](#rendering)). Tool calls are compact cards
  showing the call (e.g. `bash cargo test --all`) — a finished call is a line of
  the transcript rather than a box around it, so a long run reads as a log, and
  only the card still being written and the one that failed keep a surface, each
  carrying its state on its left edge (accent, error) with the reply's own text
  above the dimmer output below it. They expand automatically for
  diffs and errors and can be clicked open/closed — a card wears the fold caret
  only once it has output behind it, so a card showing everything it printed
  stays the text it looks like instead of offering a fold that does nothing, and
  `write`/`edit` results get a colored diff.
- **Composer** — a floating rounded box whose project chip names the folder the
  message will run in and whose whole rest is icon-first, the
  way the VS Code panel's is: the attach paperclip, the model, the thinking
  level and the trust shield lead, then — after a rule of its own — the app's own
  dialogs as icon buttons in the extension's own style: a plug that opens
  **Connect**, a padlock for the saved tool approvals, and a
  circled `?` for the shortcut help, each with the words in its tooltip and its
  `aria-label`. The project chip is the one control there that keeps its words
  on the face of it — **Choose a project** while none is open, else the folder's
  name, accented in the first case the way the model picker marks the row in use
  — because with nothing open it is how a first thread starts rather than a
  value to look up (see
  [Multiple projects](#multiple-projects-cross-repo)).
  The controls the extension's own row carries come first and in
  its order, and the glyphs the two front-ends share — the paperclip, the `+`,
  the send arrow, the stop square, the refresh arrow, the close `✕`, the MCP
  power switch, the model's cube, the thinking sparkles and the trust shield —
  are the same paths in both, which `check-app.mjs` holds beside the
  extension's own sources so a control drawn one way here and another there
  fails the check. The box itself says whether it is holding something: its
  border lifts while a message or a chip is in it. No input box paints on
  focus — the caret moving into a box leaves its border alone — so every box
  looks the same whether the caret is in it or not, and WebKit's own focus ring
  is suppressed with `outline: none`. One
  action sits on the right, which swaps rather than sitting beside a second
  button: **Stop** while a turn runs and there is nothing to say, **Send**
  beside **Queue**/**Steer** the moment there is — and only **Stop** while that
  turn is running in a thread this composer is not showing, since a message
  typed here would be steered into a run whose reply belongs to the
  conversation being read. Every
  control is wired to a plain `click` — each button, native radio/checkbox,
  sidebar/list row, change card, and attachment thumbnail — and no control is
  revealed by hovering it: a `:hover` rule in this window is a highlight (a
  background, a colour, a brightness), never the reason a control is there —
  and a hovered rule changes only how a control looks, never the box it sits in,
  since one that moves under the pointer moves out from under the press. Nor is
  anything invisible left where a press lands: nothing in this sheet is drawn
  at `opacity: 0`, no rule lets a press through what is drawn with
  `pointer-events`, and the `[hidden]` an overlay or popover carries keeps its
  `!important`, so a dialog the app has put away cannot be painted back over
  the window by a later rule. `crates/desktop/check-app.mjs` reads each of those
  out of the sheet. The
  webview owns focus, pointer, keyboard, and activation semantics, and
  page-control activation stays native: while an editor owns focus — including
  one WebKit ended just before the press, remembered from its `focusout` — a
  primary `mousedown` on any control the page wires a click to — a button, a
  link, a sidebar/list row, a change card or a question choice — prevents only
  the focus-changing default, so WebKit delivers that press's click instead of
  spending it on moving focus — the press that otherwise read as one needing a
  second click — and the box the reader has moved on from gives the caret up
  with that press, so the next thing typed goes where they clicked rather than
  into a box they have left. The rows of the two completion lists are the
  exception, since taking one finishes the text in the box the caret is in
  rather than leaving it; a press that lands on no control at all is left to
  WebKit, which ends the editing session on its own, and one into another text
  field places that field's caret without the page's help. Keyboard focus and
  activation are unchanged, and the native click stays authoritative: the page
  supplies one on the next task only when the webview withheld it, so a control
  answers exactly once. A
  control inside another stops its click from reaching the row around it, so a
  thread's ✕ removes the thread rather than selecting the row and a chip's ✕
  removes the chip rather than opening the picture — and a thumbnail's picture
  is undraggable (`-webkit-user-drag: none` in the stylesheet as well), so the
  gesture on it stays the click that opens the preview instead of starting the
  drag WebKit withholds it for. The
  status and
  token/cost usage sit just below it. The 📎 button (or a pasted clipboard
  image) attaches images/PDFs, shown above the input as thumbnails that open a
  full preview when clicked (or focused and opened with Enter/Space) — the
  preview is closed by the ✕ icon button its siblings carry, which sits in a
  head row above the picture rather than on it, so what closes the overlay is
  never painted over the image it shows, and carries the error color — and can be
  removed before sending; a message queued while busy carries the same
  attachments, and reopening a stored thread restores their thumbnails. Pasted and picked images are
  downscaled to a 1568px long edge in the page before they are sent, and an
  image a paste handed over at full resolution is downscaled again by
  `oxide_core::media::optimize_image` when the turn is built — the one place a
  data URL can be — so it is not embedded at full size in the request, the
  session and the page's own message at once. A file past the core's 20 MB
  attachment limit, or of an unsupported type no browser can paint, is
  refused with a status line instead of being read. A message can also name a
  file or folder with `@path`, which the composer completes: typing `@` offers
  the project's own paths in the same box the `/` palette uses, `↑`/`↓` walk the
  rows, `Enter`/`Tab` takes one, `Escape` closes the list, a folder keeps the
  reference open so the query goes on narrowing inside it (and a folder the
  reference already spells is left out, so the row taken next walks into it)
  while a file closes it with a space. The rules are the terminal's own —
  `oxide_core::at`, the module both composers complete from — and the walk of the
  project behind them is kept until the project changes. A reference to an image
  or a PDF is attached to the turn the way the terminal attaches one, with the
  message text left exactly as it was typed.

## Running

```sh
# from crates/desktop: build the main process, lay out the app, and run it
hutch electrobun dev
hutch electrobun dev --watch   # rebuilds and relaunches on an edit

# the package is its own workspace, so it can also be built by hand — the Rust
# SDK has to be projected into `.hutch/devkit` first (`hutch electrobun prepare`)
cargo run
```

`crates/desktop` is not a member of the root workspace, so a plain `cargo
build`/`cargo test` at the repository root neither builds nor tests the window.
Run the package's own commands from `crates/desktop` — `hutch electrobun prepare`
then `cargo test` for the shell's own tests.

The front-end (`ui/`) is not bundled: Electrobun's build copies `ui/index.html`,
`ui/app.js` and `ui/style.css` into the app beside the compiled main process (the
`build.copy` map in `electrobun.config.ts`), and the window loads them from the
`views://` protocol. A plain `cargo run` re-compiles the main process and copies
nothing, so an edit to `ui/` reaches the window through `hutch electrobun dev
--watch` (or another build) rather than on its own — a running window is never
hot-reloaded — and a new asset has to be added to that copy map before it can be
loaded at all.

To refresh the app you launch from `/Applications` (or any installed copy),
build an installer and put the app in place:

```sh
# an unsigned installer of this platform, written to crates/desktop/artifacts/
cd crates/desktop && hutch electrobun build --env=stable
# quit Oxide, then install from the artifact this produced
```

A released build is signed and notarized by the pipeline (see
[Packaging](#packaging)); a local one is left unsigned, and macOS quarantine on a
downloaded copy is what the release notes advise a reader about rather than a
failure of the app itself.

The two checks are plain Node scripts — the page is JavaScript and the shell's
config is TypeScript, which `cargo test` cannot reach:

```sh
cargo build -p oxide              # the catalog and MCP state are read from the CLI
node crates/desktop/check-app.mjs
node crates/desktop/check-shell.mjs
```

`check-app.mjs` loads `ui/app.js` against a stubbed DOM and a stubbed bridge. It covers the `/mcps` listing (including the state colors, a failed
probe and a toggle), the `/sessions` dialog (this project's threads only, the
row that resumes one, the empty case, and a store that could not be read), the
project it opens on (no project at all, the picker's rows, and the folder a
picked row opens), the **Create project** dialog, and every client command in the
catalog — a command the app does not perform has to be answered here rather
than sent to the model as a prompt. It also reads `ui/index.html` to check what
no stub can: that both listings are attached to the composer (inside
`.composer-wrap`, above `.composer`) instead of floating over the window, and
that each header button is an icon with a title.

`check-shell.mjs` joins the two halves the stubs separate, reading the sources
rather than a hand-kept list: every `invoke("…")` in `app.js` has an arm in
`src/commands.rs` (and every arm is one the page performs, or a listed one kept
for a client that calls the app rather than the page), the events `commands.rs`,
`approval.rs`, `ask.rs` and `turn.rs` emit are exactly the events `app.js`
listens for, the page reaches the app through `oxide_invoke` and no private
bridge, `main.rs` reads the packets the core queues for this process and hands
them to the command layer, the answers and announcements go back over the
channel the page's preload gives it, the window's options are the app's own (its
state carried in, the page loaded out of the folder the build copies it into,
the process ending with the window), the CSP is the page's own rather than one
injected over it, `electrobun.config.ts` copies everything the page loads and
names icons that are on disk, `set-version.sh` writes the version both the
config and the Cargo manifest report, the `gui` feature is what builds the
binary and what links the SDK the devkit projects (kept out of the workspace's
own build), and the migration's own end: the only JavaScript in the crate is the
front-end plus these two files and the build config, and nothing of the shell it
replaced is left behind (`tauri.conf.json`, `capabilities/`, `build.rs`, `gen/`,
`package.json`, `bun.lockb`, `node_modules`).

## Sharing configuration with the CLI

`oxide-desktop` does not have its own settings. It calls
`oxide_core::config::Config::load(project, ...)` for whichever project is
selected, which reads the global `config.json` and `auth.json` and loads that
project's ecosystem — exactly what `oxide` does when launched in that folder.
Credentials added with the CLI's `/login` are therefore available to the
desktop, and vice versa. Project trust (`trust.json`) is resolved the same way
the CLI resolves it before a run.

When a project has resources that can execute or reshape the agent
(`.oxide/agents`, `.oxide/commands`, `.oxide/skills`, `.oxide/plugins`,
`SYSTEM.md`/`APPEND_SYSTEM.md`, or the Claude Code equivalents) and no decision
is saved, the desktop asks **Trust this project?** the way the CLI's trust
prompt does. Trusting it saves a `true` decision to `trust.json` and reloads the
ecosystem; declining saves `false` and leaves project resources out. The trust
button in the composer — a shield, tinted with the accent once the project is
trusted — shows and reviews the current decision, so the harness loads exactly
as it would in the CLI instead of being silently dropped.

The **Connect** button in the composer stores credentials through
`oxide_core::auth`: a new key
via `auth::connect` (which also makes that provider active), or an existing
stored provider via `auth::select_stored`. Model and base URL are persisted with
`Config::persist_selection_at`, the same writer the CLI uses.

## Multiple projects (cross-repo)

`manager::DesktopManager` keeps a project registry at
`<config>/Oxide/desktop/projects.json` (same config directory as the CLI). The
sidebar shows two kinds of project:

- **Added** — folders the user added in the desktop, persisted in the registry.
- **Discovered** — projects seen in the shared session store that were never
  added (e.g. opened only from the terminal). They are derived by grouping
  `SessionLog::list_all()` by the session's `cwd`.

Each row shows its session count. Clicking a project selects it and reveals its
sessions nested underneath; clicking a session opens that thread (switching to
its project first when the selection differs). The transcript is loaded with
`session_messages` (`SessionLog::open_id`).

The window does not select a project for the reader: nothing is open at launch,
and the transcript shows the app's **home state** — the composer ready to type
in, the project chip under it, and, when the sidebar already lists threads, the
newest few across **every** project (up to `MAX_RECENT_THREADS`, in the order
`all_sessions` answers with, which is newest first) with the folder each one is
in, so a click resumes a thread. The home state is painted before either listing
answers and repainted as each one arrives, a thread listing that failed
included: the threads are what it lists, but the folders are the sidebar's and
they are already known, so a store that cannot be read still leaves the reader
with what is on offer rather than with the note to add a folder. The composer
belongs to a project all the same:
the path behind it would otherwise resolve against the directory the app was
launched in, which is `$HOME` on one platform and `/` on another, not a folder
the user picked. So the chip in the composer's own row carries the open folder's
name — **Choose a project** while there is none — and opens the folder picker
(`openProjects`): the rows the sidebar draws, plus a `+` that opens the
Add-project dialog. Picking one (`pickProject`) selects it, which is also how a
thread starts there, since selecting a project clears the transcript for the next
message; picking the project already open only puts the picker away. The chip can
be clicked before `list_projects` has answered, when there is nothing to offer
yet, so a picker already on screen is repainted with the folders that arrive
rather than left saying there are none.

Everything that needs a folder asks for one rather than guessing. A message sent
with none open is not sent at all: it stays in the box and the picker opens with
`Select a project first.` in the status, as do the model chip (whose catalog is
read from a project's own config) and `/new`. All three ask through the same
helper (`askForProject`), which offers the picker, or the Add-project dialog when
there is no folder to pick at all — as the sidebar's **+ New Chat** does when
nothing is open, which otherwise just starts a thread in the open project. With
no project at all the home state says to add one with the project chip.

A folder is never switched out from under a running turn. The turn belongs to
this window's process and its run id is the thread it is in, so the next Queue or
Steer would be sent to the run of a project the chip no longer names.
`selectProject` refuses while one runs (`busyRefusal`, the answer starting a new
thread already gives) and reports the refusal, and every caller that would go on
to open something honors it: the picker's rows, the sidebar's project rows, and
resuming a thread that lives in another project — which would otherwise open a
session in the folder the window is not in.

A draft survives the folder it was waiting for: text typed and files attached
before anything was open are still in the composer once a row of the picker is
taken, since that pick is the last step of the message rather than a move
somewhere else (`selectProject` only clears the attachments when a project is
already open, where the chips were meant for the folder being left).

The `✕` on a project row removes an **Added** project from the registry
(`remove_project`), keeping its sessions; for a **Discovered** project it
deletes that project's sessions, since those sessions are the only reason the
row exists. The `✕` on a session row deletes just that thread
(`SessionLog::delete`). Both ask for confirmation, and removing a deleted
project's last session drops its row on the next refresh. A stray discovered row
such as `oxide-old` simply means a session was recorded while the CLI ran in
that directory; it is not a project you added.

A run is told about those projects, not just the one it starts in: the shared
core reads the same `desktop/projects.json` into the `# Workspaces` section of
the system prompt (`oxide_core::workspaces`), which names the project the turn
is in and, beside it, every other folder added to Oxide with its path — the
sibling of the current project first, then the most recently opened, capped at
24. When the user asks whether the agent can reach another repository, the
answer is in the prompt instead of a `find ~` that reads every unrelated file on
the machine until the command times out, and the section says the file tools
take absolute paths, so a file in a sibling project can be read, searched and
edited from the turn. The section is composed in `oxide-core`, so a turn started
from the terminal is told the same thing.

## Agent turns

`turn::start_turn(project, prompt, session, approve, ask, reasoning, inline)` loads the
project's config, resolves the session (`new` / `latest` / an id), appends the
user message through `oxide_core::runner::begin_session`, and spawns the shared
agent loop with `runner::spawn_agent`, returning a stream of `AgentEvent`s plus
the run's `Steering` handles and its cooperative `Cancel` flag. The window's
event channel serializes events with
`oxide_core::cli::event_json` (the same Pi-shaped JSON the CLI emits in
`--mode json`), attaches any `DiffPreview`, and forwards them over
`agent-event` tagged with a run id.

- **Approvals** — `approval.rs` implements `Approver`. A rule resolves to `ask`
  or `deny` while `auto_approve` is off in the shared `config.json` (it defaults
  to on, so nothing is asked): the broker then emits `approval-request` and
  awaits the UI's `resolve_approval` (`deny`, `once`, or
  `always`); `always` records a per-project rule in
  `oxide_core::approvals::ApprovalStore`
  (`<config>/Oxide/approvals.json`, migrating the older
  `<config>/Oxide/desktop/approvals.json`) so the prompt does not repeat for
  that tool — and so the terminal and the VS Code extension honour the same
  rule. The 🔒 dialog lists and clears those rules. An unanswered request denies
  after a 5-minute timeout so a turn cannot hang.
- **Questions** — `ask.rs` implements the `ask` tool's `Asker`, which the turn
  always wires up: when a skill needs a decision the model calls `ask`, the
  broker emits `question-request`, and the UI answers with `resolve_question`.
  The dialog asks one question at a time, the way the dialog it is modelled on
  reads: `N of M questions` with a dash per question beside it, the question
  itself under its own header, and its options as rows — a radio group (the first
  option preselected) or checkboxes when several labels may be picked — with a
  description under each label and a row asking for an answer in the user's own
  words with its field under it, so a question with no options is still
  answerable. **Next** walks to the question after this one (**Back** returns to
  it, keeping what was already answered), and the last step's **Submit** sends
  the whole set — a submission with nothing filled in is sent as the same
  dismissal **Dismiss** is, so the agent hears one thing — while **Dismiss** (or
  <kbd>Esc</kbd>) sends nothing at all, which the agent reports to the model as a
  question nobody answered. A request
  that is never answered gives up after the same 5-minute timeout, and the
  broker then emits `question-closed` so the dialog goes away even though the
  turn it belongs to is still running; a turn that ends (or is stopped) takes
  its own requests with it (`AskBroker::clear_run`), and the window closes the
  dialog with it. Nothing is remembered between questions: an answer is about
  the turn that asked it.
- **Cancel / queue / steer** — `send_prompt` returns a run id immediately and runs the
  turn in the background. `cancel_run` sets the run's cooperative `Cancel` flag
  (`oxide_core::agent::Cancel`): the loop finishes the current step — recording
  a result for any planned tool calls so the session stays a valid
  call/result sequence — and ends cleanly, with a 5-second force-abort fallback
  if it is stuck. While the turn is active, typing new context replaces Stop
  with Send and an explicit **Queue** / **Steer** choice. Queue is the safe
  default: it waits until the current response finishes, then becomes the next
  turn. Steer injects a course correction before the agent's next model step. `steer_run` pushes into the
  selected follow-up or interleaved steering queue, then the composer returns to
  Queue so a later message cannot redirect work accidentally. `Alt+Enter`
  remains a direct Queue shortcut.
- **Reading another thread while a turn runs** — a turn belongs to the thread
  it started in and to no other, and that is kept apart from the thread on
  screen: the run's id and title live in `state.runSession` / `state.runTitle`
  while `state.session` follows the transcript, so a stored thread opened from
  the sidebar tree or the `/sessions` list while a turn is going — the review a
  running turn used to block — can be read without taking the turn over. What is
  left behind is parked rather than thrown away: the thread's transcript goes
  off-screen whole, with its change cards, its totals and the folder it belongs
  to, and coming back is a swap rather than a re-read of a store that is a step
  behind. A message that has only just been sent is parked under the thread it
  was composed in while the turn is still learning which thread that is — and
  under no thread at all when it is starting a new one, which is what the park's
  `pending` flag says `agent-start` finishes by re-keying it to the id the run
  reports — so a reader who opens another conversation in that window loses
  neither the bubble they sent nor the history they were reading. The header then
  carries a strip naming the running thread (`A turn is running in
  “<title>”`) and opening it again on the click, the sidebar's row for that
  thread is marked with a spinner and is a way back to it on its own (a row
  standing in for a thread the store has not written yet makes the window switch
  to it rather than read a file that is not there, and is named by the title the
  turn gave that thread — `state.heldThread`, kept with the thread's id rather
  than on the run, which is what lets a parked thread opened again keep its
  name), and the composer's corner
  offers **Stop**
  alone, since a message typed into another thread's transcript would be steered
  into a run whose reply has nowhere here to land (Send says where the turn is
  and keeps what was typed). Transcript events paint only while the thread on
  screen is the run's own — a reply written into the transcript it was not
  started in is the thing this split exists to prevent — while the window's own
  status line is not the thread's and shows wherever the reader is. Which thread
  the run takes over is the reader's place to decide and not the run's: a turn's
  thread is adopted only while both the thread and the folder the message was
  sent from are still the ones on screen, so a prompt that started a new thread
  in one folder while the reader opened another — where there is no thread
  either — is not adopted there, and the reader's own view is left alone. A turn
  that
  ends while another thread is on screen files its change card under the thread
  it changed, which paints it when that thread is opened, rather than dropping a
  listing about files that really are on disk; the thread a turn is running in
  is refused the ✕ wherever the reader is, since the run appends to its file as
  it works.
- **Notification** — a finished turn raises the same desktop toast the TUI does
  (`oxide_core::notify`, gated by the shared `notifyOnComplete` and
  `notifySound` settings). `turn::notify_finished` names the thread by its
  summarized title — the session's name, else the first message sent, the label
  the sidebar shows it under — and stays quiet for a turn the user stopped
  themselves, which has no outcome to announce.

## What a turn changed

A finished turn leaves a card in the transcript listing the files it changed.
The listing is not the run's tool calls: the project is recorded in its shadow
snapshot when the prompt is sent (`mark_baseline`), the state that run started
from travels out with `agent-end` (`{runId, sessionId, baseline, changes}`), and
the files are what a diff of the whole work tree against that baseline finds —
so a file a formatter, a shell command or an MCP server wrote is listed beside
the ones a tool call named. `oxide_core::changes` builds each entry (status, the
added/removed line counts, a binary file named rather than counted, and the same
compact preview the tool cards paint), and a turn that changed nothing paints no
card at all.

- **The card** — the header says `Edited N files` with the turn's `+`/`−`
totals; each row carries the `A`/`M`/`D` badge, the path and its own counts. Five
rows are shown with a `+N more files` button for the rest, and the header's
caret collapses the card.
- **Review** — opens the turn over the whole window: its files on the left, and
on the right the selected file's two sides — the state the run found, read out
of the snapshot at the card's own baseline, against what is on disk now, aligned
by `oxide_core::diff::lines` with the number each line holds on the side it sits
on and long unchanged stretches folded behind their count. A file that is not
text on either side reads as **Binary file — no text diff.**, the same verdict
the card's row carries (a NUL makes a file binary even when its bytes are valid
UTF-8), a side that is not there reads as empty, and one that could not be read
is reported as a failure rather than as an unchanged file. Its head carries the
file count and the arrows' own hint beside two chevron buttons that walk the
files and a close button — the same glyph the extension's own review head
carries — while the arrow keys do the same and <kbd>Esc</kbd> closes it.
- **Undo** — asks to confirm and then calls `undo_turn(project, baseline)`,
which puts the project back to the run's own baseline
(`oxide_core::snapshots::Snapshots::restore`): files the run created are
removed, the ones it edited are put back, and the ones it deleted return. It
reaches exactly what the card lists rather than the repository's last commit.
The card then reads **Undone** instead of offering it again.

A project does not have to be a git clone for any of this: the snapshot is
oxide's own bare repository (`snapshots/<project>/` in the config directory), so
a plain folder is recorded too. `snapshot_scope_is_safe` refuses only what must
not be walked — the volume root, the home directory or any ancestor of it,
anything holding the config directory, and a directory that is neither inside a
git work tree nor project-sized (over 20,000 files or 512 MB, counted without
the excluded build directories). Where the snapshot is refused, `mark_baseline`
returns `None`, the run goes ahead, `agent-end` carries no `changes`, and the
turn simply leaves no card.

Because the call's own diff is already in that listing, a tool card that changed
a file (`write`, `edit`, `patch`) reads as one line — its header carries the path
and its state the `+`/`−` counts — and keeps its own diff for the reader who
clicks it, so the same change is not painted twice.

## Models and reasoning

The top bar exposes the same controls the CLI has:

- **Model** — opens a picker backed by `list_models`, which uses
  `oxide_core::config::provider_configs` (shared with the CLI's `/models`) to
  fetch every logged-in provider's catalog. Choosing one calls `set_model`,
  which applies it through `apply_provider` / `apply_login_options` and
  persists with `Config::persist_selection_at`.
- **Reasoning** — opens a picker listing `auto`, `off`, `low`, `medium` and
  `high` with the level in use marked, the way the panel's own thinking chip does.
  It is a `role="dialog"` / `aria-modal` panel named by its own title: the
  keyboard goes to the level in use when it opens and comes back to the chip
  whatever closes it, so a reader who picked a level is not left behind the
  overlay. `Shift+Tab` and `Ctrl+R` still walk the levels, the bare `/reasoning`
  opens the same picker, and `/reasoning <level>` applies one directly.

Reasoning is sent per turn as a `--reasoning`-equivalent override; `start_turn`
passes it to `Config::load`, so it doesn't rewrite the stored config — which is
why the picker is a choice about the turns this window sends rather than a
setting it saves. `off` is a request rather than a promise: a model that always
thinks — GLM 5.3 and later, which the client asks for its lowest effort instead
of for none at all — still reasons at that level, which is why the row asks for
no thinking rather than offering an answer without any.

## Usage

Opening a session restores its cumulative `usage_totals()` (input/output tokens
and cost); live turns update the footer from each `usage` event, including a
rough context percentage using `config.context_window()`. The percentage is
worked out where the totals are painted rather than where the event arrived: the
run's `prompt` count travels with its thread and the window is the one in force on
screen, so a turn in another folder counts its own tokens while the reader is
looking at a project with a window of its own, and its gauge is its own again when
the strip brings its thread back.

## Themes

The desktop ships built-in **Dark** and **Light** themes (default Dark) and
reads the same `.oxide/themes/<name>.json` (project) and
`<config>/Oxide/themes/<name>.json` (global) files the CLI uses. Each theme
resolves every slot to a `#rrggbb` string; the desktop maps them onto CSS
variables. Slots cover the surfaces (`background`, `sidebar`, `panel`,
`panel_2`, `panel_3`, `border`, `text`, `dim`, `faint`) as well as the semantic
colors (`accent`, `user`, `assistant`, `success`, `tool`, `error`, `info`,
`tool_pending_bg`, `thinking_*`). A custom theme file overrides only the slots it
sets, on top of Dark. `set_theme` writes the `theme` key in `config.json`, which
the CLI already reads as its startup default, and the choice is re-applied on
launch. The **Theme** dialog lists each theme with a swatch strip of its key
colors, the theme name, and a ✓ on the active one; selecting a row applies it
immediately and persists it.

## Keyboard shortcuts

| Key | Action |
| --- | --- |
| `Enter` | Send with the selected Queue or Steer behavior |
| `Shift+Enter` | Newline |
| `Alt+Enter` | Queue a follow-up while busy |
| `Shift+Tab` / `Ctrl+R` | Cycle reasoning (the thinking chip opens the level picker) |
| `Ctrl+K` | Model picker |
| `Ctrl+/` | Shortcut help |
| `⌘1`…`⌘9` / `Ctrl+1`…`9` | Open the thread standing at that place in the Projects tree |
| `Escape` | Close any dialog; a question is dismissed, which the agent is told rather than left waiting |

## Slash commands

The composer answers a leading `/` with the client's own commands. Typing `/`
opens a palette of them — the built-in list plus the commands, prompt templates
and skills the project or its plugins load, from `oxide_core::commands`, the
catalog `oxide commands --json` prints and the terminal's own autocomplete
mirrors. A name the app itself owns is performed here: `/mcps` (`/mcp`) opens
the **MCP servers** dialog, `/sessions` (`/session`) the **Sessions** dialog,
`/model`, `/theme`, `/approvals`, `/trust`,
`/connect`, `/new`, `/usage` and `/help` open or run what the composer's own
buttons do. A project command, a prompt template and a skill are sent on as a
normal message, so the CLI's own resolution handles them — as is a client
command with an argument, so `/mcp list` and `/session <id>` reach the agent
instead of being performed as the bare command would be. A skill is listed
under its own name
(`/rust-conventions`), with the row marked `skill` and its description beside
it, so taking the row completes the name and sending it is what loads the skill:
the CLI resolves the same name the menu lists, and `/skill:<name>` is the
terminal's other spelling of it. A name the app cannot
perform — today `/agent`, whose palette of subagents the app does not have yet —
says so in the transcript rather than reaching the model as the literal text
`/agent`. With no project selected the project-scoped commands say that first,
so a listing or a toggle cannot land in the app's own directory, and the palette
itself is the built-in list alone: a command, a prompt template and a skill are
read from a folder, while the built-ins are what the app performs itself, so
`list_commands` answers a projectless ask with `builtin_entries` and `/new` and
`/help` are offered on the home state rather than left behind a
`No matching command.` row.

The **MCP servers** listing (`/mcps`, alias `/mcp`) and the **Sessions** listing
(`/sessions`, alias `/session`) open out of the composer rather than over the
app: each is a panel of the composer's own column, growing upward from its top
edge and staying attached to it, with its own scrollbar once the list is longer
than the space it takes. Nothing is dimmed behind them, so the transcript stays
readable, and their buttons are icons — a power switch per server, a plus and a
close in the headers — whose tooltips carry the words. **Escape** closes either.

The **MCP servers** dialog lists every server the project loads, with the state
the core probed (`Connected`, `Needs Auth`, `Needs Trust`, `Disabled`, or the
connection error) and a line naming its transport, its endpoint or command line,
and the file it was defined in. The power switch beside a row writes `enabled` into
that file — and Claude Code's `disabled`, kept in step, since either harness may
be the one reading it — without deleting the configuration, **Recheck** probes
again, and the listing comes from
`oxide_core::mcp_config::server_views`, so it matches `oxide mcp list --json`
and the VS Code panel's listing. A project's own servers report **Needs Trust** rather
than being started until the project is trusted.

The **Sessions** dialog (`/sessions`, alias `/session`) lists this project's
threads, newest first, from `all_sessions` — the same rows the tree groups under
the project — each named by its session name or, unnamed, by the summarized
preview the terminal's picker and the VS Code panel's listing show it under, with how long
ago it was written and how many messages it holds, and a row's tooltip is that
same text rather than the nodes it was written into. Picking one opens that thread
and closes the listing, and the plus in its header starts a fresh one; the argument form
`/session <id>` is left to the agent, so the dialog is the bare command's own.
A store that cannot be read is not a project with no threads: `loadSessions`
reports why it failed, and the listing says so in place of the empty message the
same call would otherwise paint — the way the MCP listing already reports a probe
that could not reach a server.

## Rendering

Assistant replies render as Markdown: headings, ordered/unordered lists
(including `- [ ]` tasks), blockquotes, rules, pipe tables, fenced code with
lightweight syntax highlighting (Rust, JS/TS, Python, Go, Bash, JSON), and
inline emphasis/code/links. Bare `http(s)://` URLs are auto-linked too, and
clicking any link opens it in the system browser through `open_url` — one call
into Electrobun's `Core::open_external`, which hands the URL to whatever the
machine uses as its handler rather than a program this source picks per target —
while navigation is denied inside the application window. Tool results render as
panels;
`write`/`edit` results include a colored diff.

## Check for updates

The check-for-updates button in the sidebar footer — or **Check for Updates…**
in the macOS app menu,
directly under **About**, which asks the open window to run the same check —
reports the newest **Oxide Desktop** release and installs it. The app's own
release train is `desktop-v*` and nothing else: the window runs the same check
the terminal's `oxide update --check --json --component desktop` performs,
served by the shared resolution in `oxide_core::updates`, so the release it
offers is the desktop release and not the oxide command line's own newest tag.
The dialog shows the version the app runs now, the tag it would install
(`desktop-v0.34.0`), and the installer it would fetch —
`macos-arm64-Oxide.dmg` on this Mac, the `-Setup.tar.gz` on Linux, the
`-Setup.zip` on Windows — then **Install**, **Release notes** (the release page,
in the system browser) and **Close**. It does not update the `oxide` command
line, and the terminal, the panel and the desktop app each update their own
installation.

**Install** downloads that artifact — re-resolved at the click, so what is
installed is the release that is newest then — verifies it against the SHA-256
GitHub reports for the asset, and puts it in this installation's place the way
this copy was installed: on macOS the `.dmg` is mounted read-only with
`hdiutil`, the `.app` inside it is copied out beside the installed bundle and
only then renamed over it (the copy that can fail happens before anything is
moved, and the copy that was there is put back if the rename cannot land), and
the image is detached again; elsewhere the setup archive is unpacked with the
machine's own `tar` and **its installer is run**, which owns the installation
from there — it writes over the files and waits for the running app to be closed
— so that install is reported as started rather than finished. The running app
keeps running either way — the dialog ends with the path the release landed at
and the one thing left to do, quitting and opening the app again. A release
that carries no build for this platform (macOS Intel included), and an install
that cannot be written to from here (a distribution's own package, a checkout's
build, a copy an administrator put in place for every user), is reported with
the file to install by hand rather than replaced.

A download that does not match its checksum is refused before anything is
replaced, and one install runs at a time. A check asked for again — the menu
item and the sidebar's check button are the same command — is the one that owns the
dialog, so an answer a newer check has already replaced is dropped rather than
repainting the dialog with an older release. A check asks GitHub for the newest
release, so a machine with no network reports what went wrong instead of
pretending to be up to date.

### The update a launch performs itself

A launch does not wait to be asked. `main.rs`'s `setup` spawns
`commands::auto_update` off the window's path, which resolves the newest release
of this app's own train through the shared `oxide_core::update_notice`: the
answer the last launch found is read at once and looked up again only once that
answer is older than six hours, so a launch costs at most one request and never
waits on the network for it. `checkForUpdates` in a `settings.json` decides
whether a launch looks at all (`OXIDE_CHECK_FOR_UPDATES` overrides it); a launch
is about the app rather than the folder open in it, so the global file answers
when nothing is open (`enabled_in(None)`), which is why the flag turns the
shared notice, this install and the terminal's off together.

Only a copy the app owns installs itself: `update::launch_installs` is true when
the release is newer than the build running here *and* this installation is one
the app replaces in place — a macOS `.app` bundle the reader installed. A
checkout's build, a distribution's package, a copy an administrator put in
place for every user, and an installation an Electrobun installer made (which
takes over the files and waits for the app to be closed) all leave the launch
alone: nothing is written over behind the reader's back, and the dialog's own
**Install** stays the way in.
What does run is the dialog's own install — the same download, the same
checksum, the same swap — with one install at a time, so a second launch cannot
race the first.

The window reports it as a row above the sidebar's foot rather than a dialog,
since nobody asked for this one: the stages arrive as `update-progress` events
and the row follows them (`Looking for a new release…`, `Downloading Oxide
0.36.0…`, `Verifying…`, `Installing…`), ending at `Oxide 0.36.0 is installed.`
with **Restart** beside it. The install starts from `setup`, before the page has
loaded and subscribed to the event channel, so each step is kept beside the app's
state as well as emitted and the page asks for the newest one as it starts
(`launch_update`): the report the restart hangs on is not one to have missed,
and a window that opened mid-install paints exactly what it would have heard.
The process running is still the build that started,
so the reload is the only thing that runs the release: **Restart** is the
`restart_app` command, which relaunches this copy and stops the event loop — the
loop that owns the window being this process — and starts the new copy as its
own process (`open -n` on a macOS bundle, the executable again elsewhere), so
the release just put in place is the one that runs. A turn is work this process
owns — its tools write
files and its stream is read here — so a restart mid-turn is refused the way
replacing the thread on screen is refused, and the row says so. The ✕ puts the
row away without stopping the install, and it puts it away for the rest of the
launch: what the row says goes on arriving, and a row that came back with the
next stage is one the reader cannot dismiss. Because the window remembers the
release it installed, a later **Check for Updates…** reports that install rather
than offering to repeat it — including a dialog that was already open when the
launch's install landed, which is repaired with the install rather than left
offering a release that is by then on disk. A launch's install that could not
finish reports itself in a line under the composer instead of a dialog, and the
dialog's own button is the one to ask again with.

## Packaging

Icons are checked in (`icons/`: a 10-entry `icon.iconset` for macOS, plus
`icon.ico` and `icon.png`). Hutch builds the installer for the machine it runs
on:

```sh
cd crates/desktop
hutch electrobun prepare            # project the Rust SDK, resolve the toolchain
hutch electrobun build --env=stable # an unsigned installer for this platform
hutch electrobun dev                # a dev build, launched
```

Each build writes to `crates/desktop/artifacts/`, flat and prefixed the way
Electrobun names them: a `.dmg` on macOS, a `-Setup.zip` on Windows, a
`-Setup.tar.gz` on Linux, each beside the `*.tar.zst` update archive and the
`*-update.json` metadata of Electrobun's own updater. The release uploads only
the installer, because this app's updater installs the **installer** (see [Check
for updates](#check-for-updates)) rather than applying Electrobun's update
payload: the archive, the delta patches and metadata naming an archive the
release does not carry are all left behind.

Builds are per-platform: a macOS build produces only the macOS artifacts, so a
release is four builds (macOS arm64, Linux x64, Linux arm64, Windows x64), each
on a runner of its own platform. macOS is the only platform Hutch signs and
notarizes; the build's entitlements come from `electrobun.config.ts` (the JIT
and unsigned-executable-memory the runtime needs, and outbound network for the
model and MCP endpoints), and Electrobun adds its own runtime entitlements under
them. An **AppImage is not among them** — the packaging formats are the DMG, the
Windows setup zip and the Linux setup tar, which is what `oxide` resolves a
release against.

A build with no signing credentials in the environment is unsigned, which is
what a local build is. `.github/workflows/desktop.yml` — which builds macOS
arm64, Linux x64 and arm64, and Windows x64 on a `desktop-v*` tag push — hands
Hutch the signing variables, renamed from the repository's secrets, and only on
the macOS job:

| Electrobun variable | Secret | Value |
| --- | --- | --- |
| `ELECTROBUN_DEVELOPER_ID` | `APPLE_SIGNING_IDENTITY` | the identity `security find-identity -v -p codesigning` prints |
| `ELECTROBUN_APPLEID` | `APPLE_ID` | the enrolled Apple ID |
| `ELECTROBUN_APPLEIDPASS` | `APPLE_PASSWORD` | an app-specific password |
| `ELECTROBUN_TEAMID` | `APPLE_TEAM_ID` | the 10-character team ID |
| `ELECTROBUN_APPLEAPIKEY` | `APPLE_API_KEY` | an App Store Connect key ID |
| `ELECTROBUN_APPLEAPIISSUER` | `APPLE_API_ISSUER` | that key's issuer UUID |
| `ELECTROBUN_APPLEAPIKEYPATH` | `APPLE_API_KEY_P8` | the `.p8` file, written out of the base64 secret |

The certificate travels as `APPLE_CERTIFICATE`/`APPLE_CERTIFICATE_PASSWORD` and
is imported into a temporary keychain of the run's own, which is put first in
the search list, since `codesign` can only use an identity in a keychain. Only
variables with a value are exported, so a repository without secrets still
builds a bundle — unsigned rather than half-signed. `electrobun.config.ts` reads
those variables itself and turns `codesign`/`notarize` on only when it finds
them, so the decision belongs to the pipeline and a checkout never signs by
accident.

An unsigned app downloaded from the internet is quarantined by the browser and
Gatekeeper may refuse it; a reader can clear it with
`xattr -cr /Applications/Oxide.app`, which is what the release notes say. A
signed and notarized release needs none of that, and Electrobun's own updater
payload (`*-update.json` and the `*.tar.zst` it names) is not published at all:
this app updates itself from the plain installers the release carries (see
[Check for updates](#check-for-updates)) rather than from a manifest beside
them.

## Signing secrets

`desktop.yml` reads the signing material from repository secrets
(**Settings → Secrets and variables → Actions → New repository secret**).
Nothing is required to build the installers; without a certificate the macOS app
is built unsigned (see above).

### Enroll first

A **Developer ID Application** certificate can only be created by an Apple ID
enrolled in the [Apple Developer Program](https://developer.apple.com/programs/enroll/)
($99/year). An unenrolled Apple ID that opens
`developer.apple.com/account/resources` gets *Access Unavailable*: enroll as an
**Individual** (usually minutes to ~48 h; needs your legal name/address and an
identity check) or as an **Organization** (needs a D-U-N-S number and can take
days to weeks). Enable two-factor authentication on the account. Until then
there is nothing to sign with and the ad-hoc fallback is the only option.

### macOS certificate

1. Create a **Developer ID Application** certificate:
   - Xcode — *Settings → Accounts → (team) → Manage Certificates → **+** →
     Developer ID Application*, or
   - [developer.apple.com](https://developer.apple.com/account/resources/certificates)
     → *Certificates → **+** → Developer ID Application* (make the CSR with
     Keychain Access → *Certificate Assistant → Request a Certificate From a
     Certificate Authority*).
2. Export it — *Keychain Access → login → My Certificates*, right-click the
   certificate → *Export*, save as `.p12` with a password.
3. Read the certificate common name on the same Mac:
   `security find-identity -v -p codesigning`.

Then either use your **Apple ID**, or an **App Store Connect API key**, for
notarization — not both.

**Apple ID** (Hutch signs, notarizes and staples the app and the DMG):

| Secret | Value |
| --- | --- |
| `APPLE_CERTIFICATE` | base64 of the `.p12` (`base64 -i cert.p12 \| pbcopy`; Linux `base64 -w0 cert.p12`) |
| `APPLE_CERTIFICATE_PASSWORD` | the `.p12` export password |
| `APPLE_SIGNING_IDENTITY` | e.g. `Developer ID Application: Jayson Wu (ABCDE12345)` |
| `APPLE_ID` | the enrolled Apple ID email |
| `APPLE_PASSWORD` | an **app-specific password** (account.apple.com → *Sign-In and Security → App-Specific Passwords*) |
| `APPLE_TEAM_ID` | 10-character Team ID (developer.apple.com → *Membership details*) |

**App Store Connect API key** (create under *Users and Access → Integrations →
App Store Connect API*, role *Admin* or *App Manager*):

| Secret | Value |
| --- | --- |
| `APPLE_API_ISSUER` | the issuer UUID shown above the key list |
| `APPLE_API_KEY` | the key ID (the `AuthKey_<id>.p8` file name) |
| `APPLE_API_KEY_P8` | base64 of the `.p8` (`base64 -i AuthKey_XXXX.p8 \| pbcopy`) |

The workflow decodes `APPLE_API_KEY_P8` to `$RUNNER_TEMP/AuthKey.p8` and sets
`ELECTROBUN_APPLEAPIKEYPATH`; the `.p8` can only be downloaded once, so store it
somewhere safe.

### From the CLI

On the machine that holds the `.p12`:

```sh
base64 -i cert.p12 | gh secret set APPLE_CERTIFICATE
gh secret set APPLE_CERTIFICATE_PASSWORD
gh secret set APPLE_SIGNING_IDENTITY --body 'Developer ID Application: Your Name (ABCDE12345)'
gh secret set APPLE_ID --body 'you@example.com'
gh secret set APPLE_PASSWORD              # paste the app-specific password
gh secret set APPLE_TEAM_ID --body 'ABCDE12345'
```

`gh secret set NAME` without `--body` prompts, so the value never lands in your
shell history.

## Release assets

Each `desktop-v*` release carries installers for the four platforms the app is
built for. Pick the asset whose platform matches the machine — Electrobun's
installer names carry the app name and the platform rather than the version, so
they read the same in every release. The desktop app is versioned separately
from the CLI, so a CLI release never rebuilds these:

| Asset | Platform |
| --- | --- |
| `macos-arm64-Oxide.dmg` | macOS, Apple Silicon (`uname -m` → `arm64`) |
| `linux-x64-Oxide-Setup.tar.gz` | Linux x64 |
| `linux-arm64-Oxide-Setup.tar.gz` | Linux arm64 |
| `win-x64-Oxide-Setup.zip` | Windows x64 |

There is no **macOS Intel** asset: Electrobun publishes no x64 core for macOS,
so the app ships for Apple Silicon alone and `oxide` resolves no desktop release
for `darwin-x64`. The Linux archive unpacks an executable named `installer`
beside a README; the Windows zip carries `Oxide-Setup.exe` and hides the payload
it unpacks in a `.installer/` directory of its own. Electrobun's own updater
payload — the `*.tar.zst` archive and the `*-update.json` metadata naming it —
is not uploaded: this app's updater ignores it and installs the installer
itself.

The CLI archives (`Oxide-v<version>-<platform>.tar.gz` and
`Oxide-v<version>-win32-x64.zip`) plus `install.sh`/`install.ps1` live in the
separate `v*` CLI releases, not here; see [install.md](install.md).

The workflow builds every platform in its own job and hands the installers to
the release job: each build job uploads what it produced as a workflow artifact
and the release job — the only job granted `contents: write`, and the one that
runs for a `desktop-v*` tag alone — drafts the release and uploads them, so a
`workflow_dispatch` build publishes nothing and needs no more than a read-only
token.
