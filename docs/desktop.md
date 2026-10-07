# Desktop app

The `oxide-desktop` package (`crates/desktop`) is an
[Electron](https://www.electronjs.org) front-end for the same agent the terminal
CLI runs: a **TypeScript window** (`electron/`) around a **Rust engine** (`src/`,
the `oxide-desktop` binary) written in the same language as the rest of Oxide.
They are one app in one bundle and two processes — the window is the app's
screen, and the engine beside it is started as a child and spoken to over one
JSON packet per line — and the goal is one configuration and one session store
shared by every front-end (the CLI, the desktop app, and the VS Code extension),
with a GUI that can manage **multiple projects** (cross-repo) and show each
project's session list.

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
`settings.json`, and `sessions/` tree. The Electron window only owns the screen
and the transport: the command layer, the project registry, the turn loop and the
brokers are ordinary Rust and are tested without a browser. `crates/desktop` is
excluded from the Cargo workspace (`exclude = ["crates/desktop"]` in the root
`Cargo.toml`), because the app's own build owns the binary's layout and version —
it is a project of its own, built from its own directory. The VS Code
extension (`editors/vscode`,
see [docs/vscode.md](vscode.md)) is a separate pnpm package that drives the
`oxide` binary, so it shares the same files without linking `oxide-core`.

## Desktop layout

```
crates/desktop/
  src/              the engine (Rust): the harness the window starts
    lib.rs          the library it is built from: at, manager, turn
    manager.rs      project registry + session aggregation
    turn.rs         starts an agent turn against a project
    at.rs           the `@path` walk the composer completes from
    commands.rs     the `oxide_invoke` dispatcher, and the update path
    approval.rs     interactive approve/deny broker
    ask.rs          a skill's question broker
    bridge.rs       the packet channel: one JSON packet per line
    update.rs       this app's own release train
    main.rs         harness entry point: read stdin, answer each request
  electron/         the window (TypeScript)
    main.ts         one BrowserWindow, the menu, the engine it starts
    preload.ts      the bridge the page is handed
    dist/           `tsc` output (generated; this is what Electron runs)
  ui/               the page: index.html, app.js, style.css
  electron-builder.yml  packaging: identifier, product name, targets, artifact
                        names, the engine copied in as a resource, signing
  build/
    entitlements.mac.plist  the hardened runtime's own entitlements
  icons/            app icons (`icon.icns` for macOS, `icon.ico`, `icon.png`)
  package.json      the app's own package: the pnpm scripts and Electron itself
  pnpm-workspace.yaml  pnpm's settings for this package (the install scripts it
                        runs, and the dependency sources it allows)
  pnpm-lock.yaml    the app's own lockfile (generated; what CI installs from)
  tsconfig.json     the window's TypeScript config (`electron/` → `electron/dist`)
  check-app.mjs     front-end check (stubbed DOM and bridge)
  check-shell.mjs   shell check (command/event contract, window, menu, bundle)
```

The engine has no window dependency at all: it links `oxide-core` and the crates
around it, and nothing of Electron's. `src/lib.rs` (the `at`, `manager` and
`turn` modules) is what its unit tests cover, while `commands.rs`, `update.rs`,
`approval.rs`, `ask.rs`, `bridge.rs` and `main.rs` are declared by `main.rs`, so
their tests run with `cargo test` from `crates/desktop` — which is what
`.github/workflows/ci.yml` runs on each platform, beside the window's own type
check.

The app is two programs in one bundle. The window is Electron's own files —
`electron/dist/**`, `ui/**` and `package.json` — and the engine is carried beside
them as a resource and started as a child process: `Resources/harness/oxide-desktop`
(`Resources/harness/oxide-desktop.exe` on Windows). What travels between them is
one JSON packet per line: the page's own requests out on the engine's stdin, its
answers and announcements back on the engine's stdout. The window is a relay — it
forwards a request and a packet unread, because a rule about a run belongs on the
half that can enforce it — and only the one packet the operating system makes
its own (`restart`, below) is read there at all, so the page and the engine are
the only two parties to the protocol.

The page is this crate's own `ui/` directory, loaded with `loadFile` (in a
checkout and inside the packaged app's archive alike): one plain script with no
imports to resolve, so there is nothing for a bundler to do. The window's preload
hands it the three globals its script has always read — `__electrobunHostBridge`
(what it sends through), `__electrobun.receiveMessageFromHost` (what the window
calls to hand it a packet) and the `__electrobunPendingHostMessages` queue in
between, for a packet that arrives before the page's own script has run — and
those names are kept as they are on purpose, because the page is what reads them.
A request is
`{type: "request", id, method: "oxide_invoke", params: {command, args}}`,
answered with `{type: "response", id, success, payload|error}`, and the page
imports nothing from a framework API — the engine performs the work, and the page
asks for it by command name — so the window needs no capability grant and no
allow-list beyond that bridge. Only the calls that are the operating system's
belong to the window rather than to the engine: the folder chooser
(`pick_folder`, answered by Electron's own panel, which no child process can
draw) and a link to open (`open_url`, answered by the window once the platform's
handler has started or refused to — a browser that would not run is reported to
the page rather than written to a console nobody is reading), and the relaunch
that runs an installed release, which the engine asks for and the window performs
since the engine is the half that knows whether a turn is running. The menu
bar is the window's own for the same reason (see below).
The page's own `Content-Security-Policy` in `ui/index.html` is the window's
(`default-src 'none'`, `script-src 'self'`, `style-src 'self' 'unsafe-inline'`,
`img-src 'self' data: blob:` for attachment thumbnails and previews,
`font-src 'self'`, `connect-src 'none'` — nothing in the page fetches anything —
and `form-action`/`base-uri`/`frame-ancestors` `'none'`).

Events travel the other way as packets of their own: the engine announces
`agent-start`, `agent-event`, `agent-end`, `approval-request`, `question-request`
and `question-closed`, which `app.js` listens for, plus `update-available`,
`update-progress`, `update-ready` and `update-failed` from the install this
process is running — the launch's own, or the one the window asked for (see
[The update a launch performs itself](#the-update-a-launch-performs-itself)), and
the window raises `check-updates` from its menu item so the page performs the
check and paints one dialog (see [Check for updates](#check-for-updates)). One
more is the window's own to carry out rather than the page's — `restart` — and it
is read off the packet channel there; a link to open is not, since opening one is
the window's own command, answered on the bridge before the engine sees it. A run
can only be
watched, answered or stopped from the window that started it — the events carry
no state a fresh window could be rebuilt from — so closing it ends the app and
the turn with it (`window-all-closed` quits the app, quitting kills the engine's
child process, and the engine's own loop ends when its stdin does), which is
deliberately not the usual macOS "stay resident" behavior: a resident app with no
window would leave a turn streaming into nothing, with its approval and question
requests unanswerable.

The menu bar is the window's own: `installMenu` builds it with
`Menu.buildFromTemplate` from the platform's own roles (`about`, `quit`,
`hideOthers`, `undo`/`copy`/`paste`, `minimize`/`zoom`/`close`) plus the one item
the app answers itself — **Check for Updates…**, which sends the page the
`check-updates` event and so ends at the same dialog the sidebar's own button
opens. macOS is the only platform that gets one; everywhere else the window is
the app, as it was before. `check-shell.mjs` reads the menu back out of
`electron/main.ts` and holds its shape: the roles, the one `click` being the
update check, and that item sitting after **About**.

The window is 1200×800 and opens centered, with 820×560 as its floor (`MIN_SIZE`
in `electron/main.ts`, kept as the window's own `minWidth`/`minHeight`) — the size
below which a transcript, its tool cards and the composer stop being usable. No
resize handler measures anything for it: Electron clamps the window itself. The
window is shown once its first frame is there (`show: false` until
`ready-to-show`) rather than as a white rectangle while the transcript is built,
and it takes the press that activates it as a press in the window
(`acceptFirstMouse: true`), so the first click on a control in a window that is
not yet key is not spent on making it key.

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

Lists do not wear a bar that is always there. A scrollable box keeps its
scrollbar's lane clear of its rows, and the bar is drawn only while the box is
being scrolled: `ui/app.js` marks the element the `scroll` came from and the
sheet paints the thumb under that mark, taking it away again a moment after the
scrolling stops, so nothing stands in the lane while a list sits still. The
thumb is inset inside its lane (a transparent border with
`background-clip: padding-box`) and painted in the theme's own text colour
rather than in the border colour, which sits at the same weight as the rules it
runs beside; it is painted with `background-color`, since the `background`
shorthand would reset that clip and bring the bar back the full width of its
lane. `check-app.mjs` holds that pairing — a resting thumb painted, a mark that
paints nothing, a mark never taken away or landing on the body, and a painted
bar that has lost its inset are each a failed check.

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
  count and in the `/session` list until the store has written it, keyed by
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
  resources off`). The window is Electron's own (`createWindow` in
  `electron/main.ts`): no AppKit event monitor, Objective-C hook or
  platform-specific input path is installed — the page's own preload is the one
  thing it adds, and it installs the packet bridge and nothing else — and every
  control is wired to a plain `onclick`.
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
- **Composer** — a floating rounded box holding the message and what goes with it:
  the attachment strip, the message box itself and the icon row of chips.
  Where the turn will run is the status line's business, under the box, and it
  names it the way Codex's own composer does: the folder's chip, the branch that
  folder is on, and the machine the turn runs on — this app
  has one environment, a folder on this computer, and it says so rather than
  leaving the reader to assume a cloud, shown with a folder open rather than
  before one, since there is no turn to place until then. The branch is read out of the
  repository's own `.git/HEAD` (`oxide_desktop::git::view`) rather than by
  running git, so a worktree or a submodule is answered through the `gitdir:`
  file that points at its real `HEAD`, a detached `HEAD` is named by the commit
  it is on, and a folder that is in no repository has no branch to name at all,
  which the row draws by leaving that fact out rather than by saying nothing. It
  is a fact about the folder rather than a control, so it is a plain readout with
  its words in a tooltip (`On branch main` and the repository's own path), it is
  read again when the folder changes, when a turn ends — a run may switch branch
  — and when the reader comes back to the window, since a branch switched in a
  terminal is theirs to see here. The branch is the one fact that gives way when
  the row runs out of room: it is the name that can be arbitrarily long, so it is
  cut with an ellipsis while the folder, the machine, the window's own status and
  the totals keep the room they need, and the row stays one line whatever the
  folder's own names are. The right end of that row is the context reading: a ring
  filled to the percent of the window the thread's last request used, with that
  percent spelled out beside the arc — it is one reading rather than two, which is
  why the totals line does not spell out `ctx` as well — and the tokens the
  request took and the window itself in its tooltip and accessible name. The arc
  escalates through the terminal's own thresholds — its dim tone on its own, the
  theme's amber past 70% of the window and its error colour past 90% — and an
  empty ring means a thread that has spent nothing yet, since the numbers behind
  it are this conversation's rather than the window's ([Usage](#usage)). The
  composer's own row is icon-first, the
  way the VS Code panel's is: the attach paperclip, the model, the thinking
  level and the trust shield lead, then — after a rule of its own — the app's own
  dialogs as icon buttons in the extension's own style: a plug that opens
  **Connect**, a padlock for the saved tool approvals, and a
  circled `?` for the shortcut help, each with the words in its tooltip and its
  `aria-label`. The project chip is the one control on the status line that keeps
  its words on the face of it — **Choose a project** while none is open, else the
  folder's name, accented in the first case the way the model picker marks the row
  in use — because with nothing open it is how a first thread starts rather than a
  value to look up (see
  [Multiple projects](#multiple-projects-cross-repo)); with nothing open it is
  also the only control on the row, and it wears a marker of its own (`unset`)
  rather than the sheet's `.empty`, whose `margin: auto` centers the placeholders
  it belongs to.
  The controls the extension's own row carries come first and in
  its order, and the glyphs the two front-ends share — the paperclip, the `+`,
  the send arrow, the stop square, the refresh arrow, the close `✕`, the MCP
  power switch, the model's cube, the thinking sparkles and the trust shield —
  are the same paths in both, which `check-app.mjs` holds beside the
  extension's own sources so a control drawn one way here and another there
  fails the check. The box itself says whether it is holding something: its
  border lifts while a message or a chip is in it. No input box paints on
  focus — the caret moving into a box leaves its border alone — so every box
  looks the same whether the caret is in it or not, and the engine's own focus
  ring is suppressed with `outline: none`. One
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
  window's engine owns focus, pointer, keyboard, and activation semantics, and
  page-control activation stays native: while an editor owns focus — including
  one Chromium ended just before the press, remembered from its `focusout` — a
  primary `mousedown` on any control the page wires a click to — a button, a
  link, a sidebar/list row, a change card or a question choice — prevents only
  the focus-changing default, so the engine delivers that press's click instead
  of spending it on moving focus — the press that otherwise read as one needing a
  second click — and the box the reader has moved on from gives the caret up
  with that press, so the next thing typed goes where they clicked rather than
  into a box they have left. The rows of the two completion lists are the
  exception, since taking one finishes the text in the box the caret is in
  rather than leaving it; a press that lands on no control at all is left to the
  engine, which ends the editing session on its own, and one into another text
  field places that field's caret without the page's help. Keyboard focus and
  activation are unchanged, and the native click stays authoritative: the page
  supplies one on the next task only when the window withheld it, so a control
  answers exactly once. A
  control inside another stops its click from reaching the row around it, so a
  thread's ✕ removes the thread rather than selecting the row and a chip's ✕
  removes the chip rather than opening the picture — and a thumbnail's picture
  is undraggable (`-webkit-user-drag: none` in the stylesheet as well), so the
  gesture on it stays the click that opens the preview instead of starting the
  drag the engine withholds it for. The
  status and
  token/cost usage sit just below it. The 📎 button (or a pasted clipboard
  image) attaches images, PDFs and text files, shown above the input as
  thumbnails that open a
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
  session and the page's own message at once — on Windows the page's own
  downscale is the only one, since the platform ships no image tool to resize
  with. A file past the core's 20 MB
  attachment limit is refused with a status line instead of being read, and
  nothing else is turned away at the door: a format no provider takes but the
  tools the machine has can convert (a TIFF, a HEIC) is converted by the core —
  `sips` on macOS, ImageMagick on Linux, and the imaging stack Windows carries —
  and anything that is neither an image nor a PDF travels as its own text — a
  payload that is binary in a way neither reads (`application/octet-stream`, a
  video) is refused by the core with the file named, rather than sent as
  mojibake. A message can also name a
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
cd crates/desktop
pnpm install     # once: Electron, TypeScript and electron-builder

pnpm start       # `cargo build`, then `tsc`, then the window
pnpm run build   # compile the window only (`tsc -p tsconfig.json`)
pnpm run check   # type check the window, writing nothing
pnpm run dist    # both halves in release, packaged into the platform's installer
```

`pnpm start` compiles the engine with `cargo build` first, compiles the window,
and runs Electron on this crate. The window finds the engine it starts in this
order: `OXIDE_DESKTOP_HARNESS` when a developer points it at one, the copy
carried in the bundle (`Resources/harness/oxide-desktop`, or
`Resources/harness/oxide-desktop.exe` on Windows) when the app is packaged, and
otherwise `target/debug/oxide-desktop` (or `target/release/oxide-desktop` when
`OXIDE_DESKTOP_PROFILE=release`). A window whose engine is missing reports the
path it looked for and exits, rather than painting a UI whose every click would
fail.

`crates/desktop` is not a member of the root workspace, so a plain `cargo
build`/`cargo test` at the repository root neither builds nor tests it. Run the
package's own commands from `crates/desktop` — `pnpm run check` for the window's
TypeScript, `cargo test` for the engine's own tests.

The window's packages are pnpm's, the way the VS Code extension's are: the tree
is installed from `pnpm-lock.yaml` (which CI installs with `--frozen-lockfile`),
`package.json` names the pnpm version to use, and pnpm's own settings live in
`pnpm-workspace.yaml` rather than an `.npmrc`. Two of them are there because the
build needs them: Electron's install script is what downloads the browser the
window runs in — pnpm runs no dependency's scripts unless that file allows it —
and electron-builder names its `node-gyp` as a git URL at a pinned commit, which
pnpm refuses under a dependency unless the file says otherwise. pnpm also
installs the tree the way the manifest declares it rather than flattening every
dependency into one directory, which is why `@types/node` is a dependency of
this package: `tsconfig.json` names it in `types`, so a copy that only arrived
under something else would type-check on a machine with a flat tree and not
here.

The compiler is TypeScript 7, whose configuration is `tsconfig.json`'s: it emits
CommonJS for Electron's own loader, and the resolution mode it uses is
`nodenext` — the older `node`/`node10` mode is one 7 removed.

The front-end (`ui/`) is not compiled: `electron-builder.yml` copies the
`ui/` directory into the app, and the window reads the page out of it by
`loadFile` — this crate's own directory in a checkout, the app's archive in a
packaged build — so an edit to `ui/` needs no build step and is in the window
the next time the page is loaded. An edit to `electron/*.ts` is not, since
Electron runs `electron/dist/main.js`: `pnpm run build` compiles it.

To refresh the app you launch from `/Applications` (or any installed copy),
build an installer and put the app in place:

```sh
cd crates/desktop
pnpm run dist   # this platform's installer, written to artifacts/
# quit Oxide, then install from the artifact this produced
```

A released build is signed and notarized by the pipeline when its secrets are
configured (see [Packaging](#packaging)); a local one is left unsigned, and
macOS quarantine on a downloaded copy is what the release notes advise a reader
about rather than a failure of the app itself.

The two checks are plain Node scripts — the page is JavaScript and the window's
own sources are TypeScript, which `cargo test` cannot reach:

```sh
cargo build -p oxide              # the catalog and MCP state are read from the CLI
node crates/desktop/check-app.mjs
node crates/desktop/check-shell.mjs
```

`check-app.mjs` loads `ui/app.js` against a stubbed DOM and a stubbed bridge. It covers the `/mcp` listing (including the state colors, a failed
probe and a toggle), the `/session` dialog (this project's threads only, the
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
listens for (plus the two the window takes off the packet channel itself), the
page reaches the engine through `oxide_invoke` over the bridge its preload
installs (`__electrobunHostBridge`, `__electrobun.receiveMessageFromHost` and
the pending-messages queue — the names the page has always read, and no name it
does not), the window and its preload spell the two channel names the same way,
`electron/main.ts` starts the engine as a child and speaks to it one packet per
line, `src/main.rs` reads those packets off its own stdin and hands them to the
command layer, the answer and the announcement keep the two shapes `bridge.rs`
writes, the window's own rules (its own page and nothing else — no popup, no
navigation, no attached webview — Electron's profile kept out of the config
directory the CLI reads, and the layout's smallest window kept as the option
Electron clamps with), the package's own `main` and pnpm scripts being the
window's build, the tree installed and compiled the way the extension's is (a
pnpm lockfile and no npm one, the settings `pnpm-workspace.yaml` has to carry for
this package's dependencies, `@types/node` declared here rather than borrowed,
and the `nodenext` resolution TypeScript 7 asks for) and the two workflows that
build the app installing that same tree with the pnpm the package names, the menu
bar and its one
click, the artifact names in
`electron-builder.yml` against the ones
`oxide_core::updates::Component::Desktop` resolves and the engine carried into
every bundle the build makes, the installer (each kind of installation replaced
the way it was installed, and the stages a launch reports), the CSP as the page's
own rather than one injected over it, `set-version.sh` writing the version both
the Cargo manifest and `package.json` report, and the migration's own end:
nothing of the shell the app replaced is left in the crate (no Electrobun or
Hutch config, no devkit, no bun lockfile), and the generated parts of the crate
are the ones its `.gitignore` keeps out.

## Sharing configuration with the CLI

`oxide-desktop` does not have its own settings. It calls
`oxide_core::config::Config::load(project, ...)` for whichever project is
selected, which reads the global `config.json` and `auth.json` and loads that
project's ecosystem — exactly what `oxide` does when launched in that folder.
Credentials added with the CLI's `/connect` are therefore available to the
desktop, and vice versa. Project trust (`trust.json`) is resolved the same way
the CLI resolves it before a run.

When a project has resources that can execute or reshape the agent
(`.oxide/agents`, `.oxide/commands`, `.oxide/skills`, `.oxide/plugins`,
`SYSTEM.md`/`APPEND_SYSTEM.md`, or the Claude Code equivalents) and no decision
is saved, the desktop asks **Trust this project?** the way the CLI's trust
prompt does. Trusting it saves a `true` decision to `trust.json` and reloads the
ecosystem; declining saves `false` and leaves project resources out. The trust
button in the composer — a shield, tinted with the accent once the project is
trusted — shows and reviews the current decision, so the project's resources load
exactly as they would in the CLI instead of being silently dropped.

The **Connect** button in the composer stores credentials through
`oxide_core::auth`: a new key
via `auth::connect` (which also makes that provider active), or an existing
stored provider via `auth::select_stored`. Model and base URL are persisted with
`Config::persist_selection_at`, the same writer the CLI uses.

The dialog is the provider table itself, read from `auth::provider_views()` — the
one listing the CLI's `/connect` picker and the VS Code panel draw — with a search
box over it, since the table holds every provider a client can connect. A row
names the provider the way a login takes it, and says what state it is in (`In
use` for the active one, `No key needed` for a server on this machine). Each row
also carries where its credential comes from (`credential`: `key`, `external` or
`none`), which is what decides whether the dialog asks for one at all. Opening it
selects the provider in use, else the first stored one, else the first row, and a
search that hides the selection moves it to the first visible row rather than
leaving nothing selected. The selection is held by provider name, not by position,
because the list is filtered while the reader types. A provider that needs no key
— a server on this machine, or one that authorizes with a credential this machine
already holds — is connected as it is: the key field goes and a sentence saying
why takes its place. A provider whose own credential is missing is asked for a
key, and `login` reports what to set when the machine has neither.

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
the engine's process and its run id is the thread it is in, so the next Queue or
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
the run's `Steering` handles and its cooperative `Cancel` flag. The engine's
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
  the sidebar tree or the `/session` list while a turn is going — the review a
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
the strip brings its thread back. The same percentage is drawn at the end of the
status line, under the composer's box: an arc filled to it with the percent
spelled out beside it (the totals line beside them draws what the thread spent,
and the fraction of the window that took is this one reading rather than a second
word on that line), in the colour of the level (the theme's dim, amber past 70%,
error past 90%), with the tokens and the window in its tooltip.

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
mirrors. A name the app itself owns is performed here — the built-ins the catalog
names the desktop app for, each with the one spelling the catalog declares: `/mcp`
opens the **MCP servers** dialog, `/session` the **Sessions** dialog, and
`/model`, `/theme`, `/permissions`, `/trust`, `/connect`, `/new`, `/reasoning`,
`/usage` and `/help` open or run what the
composer's own buttons do, while a built-in another front-end owns (`/agent` is
the VS Code panel's) is not a row at all. A project command, a prompt template
and a skill are sent on as a normal message, so the CLI's own resolution handles
them — as is a client command with an argument, so `/mcp list` and `/session
<id>` reach the agent instead of being performed as the bare command would be. A
skill is listed under its own name (`/rust-conventions`), with the row marked
`skill` and its description beside it, so taking the row completes the name and
sending it is what loads the skill: the CLI resolves the same name the menu
lists, and `/skill:<name>` is the terminal's other spelling of it. Typing a
built-in out in full names the front-end the catalog hands it to (`/agent is the
VS Code panel's command.`) rather than sending the literal text `/agent` to the
model, and a client command no front-end performs yet is named as not being in
the app yet, in the same status line — which is what keeps every name the
catalog declares a client command from being sent on as a prompt. With no
project selected the project-scoped commands say that first,
so a listing or a toggle cannot land in the app's own directory, and the palette
itself is the built-in list alone: a command, a prompt template and a skill are
read from a folder, while the built-ins are what the app performs itself, so
`list_commands` answers a projectless ask with `builtin_entries` and `/new` and
`/help` are offered on the home state rather than left behind a
`No matching command.` row.

The **MCP servers** listing (`/mcp`) and the **Sessions**
listing (`/session`) open out of the
composer rather than over the app: each is a panel of the composer's own column, growing upward from its top
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

The **Sessions** dialog (`/session`) lists this project's
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
clicking any link opens it in the system browser through `open_url`: the window
is the half that has the machine's handler, so it answers the page's request
directly — checking the scheme first (http(s) alone), so a `file:` path or a
custom scheme reaches nothing at all rather than the platform's handler, calling
`shell.openExternal`, and answering with whether the browser started, which is
what a click the machine could not carry out is reported by instead of a console
line nobody reads. Neither side reads the URL as a command line, and no branch of
it is one platform's. Navigation is denied inside the
window for the same reason: it shows the page it was built with and nowhere
else, so a link in a reply is a browser tab rather than a remote document
painted in the app (`setWindowOpenHandler` denies every popup, and
`will-navigate`/`will-attach-webview` are prevented). Tool results render as
panels;
`write`/`edit` results include a colored diff.

## Check for updates

The check-for-updates button in the sidebar footer — or **Check for Updates…**
in the macOS app menu,
directly under **About**, which asks the open window to run the same check —
reports the newest **Oxide Desktop** release and installs it. The app's own
release train is `desktop-v*` and nothing else: the engine resolves it with the
same check the terminal's `oxide update --check --json --component desktop`
performs,
served by the shared resolution in `oxide_core::updates`, so the release it
offers is the desktop release and not the oxide command line's own newest tag.
It does not update the `oxide` command line, and the terminal, the panel and the
desktop app each update their own installation.

One dialog holds the three steps of one install, and only the step's own actions
are on screen, so nothing offered belongs to a step the reader has left:

- **Update Available (v0.34.0)** — what the release changed, drawn from the
  release's own notes (`Check::notes`, the body GitHub holds) under the version
  and the day it went out (`released_at`), with the facts under them: the version
  this app runs, the version it would install (`0.34.0`), the installation the
  app found itself in and its path, and the installer it would fetch —
  `macos-arm64-Oxide.dmg` on this Mac, the `-Setup.AppImage` on Linux, the
  `-Setup.exe` on Windows. **Update** appears only where this copy may be written
  over from inside the app; **Release notes** — the release page, in the system
  browser — appears where it may not, and where the release carried no notes to
  draw; **Dismiss** closes it.
- **Downloading update** — a bar that fills from the stages the engine reports
  (`update-progress`, carrying the bytes written and the size the response
  announced; a release that announced no size shows the bar working and the bytes
  instead of a fraction, and the stages on either side of the transfer carry no
  bytes and so no bar), with **Download in background**, which puts the dialog
  away without stopping the download, and **Cancel**, which stops it. Cancel is
  offered while the install can still be stopped — looking the release up,
  downloading it, checking its checksum — and not once the release is being put
  in place, where stopping halfway is an installation lost; the step's own title
  names the stage it has reached. It names the install
  it is about (`cancel_update` with the token the window minted, or the one the
  engine reported for an install nobody in the window asked for), so a cancel
  asked for before that install had begun is still its own and one asked for a
  release ago is not the next install's. Nothing is put in place either way, and
  the next check offers the release again.
- **Restart and install update** — `Update downloaded. You need to restart Oxide
  to install the update.` The process running is still the build that started, so
  **Restart and install update** is the `restart_app` command and **Later** puts
  the dialog away with the install in place: the next check reports what is left
  to do rather than offering it again. A Windows setup, which this app starts and
  cannot wait on, is reported as running instead, with **Close** alone.

**Update** downloads that artifact — re-resolved at the click, so what is
installed is the release that is newest then — verifies it against the SHA-256
GitHub reports for the asset, and puts it in this installation's place the way
this copy was installed. A macOS `.app` bundle is replaced from the release's disk
image: the `.dmg` is mounted read-only with `hdiutil` (and `-nobrowse`, so a
check puts no volume in the Finder's sidebar), the `.app` inside it is copied out
beside the installed bundle with `ditto`, and only then renamed over it — the
copy that can fail happens before anything is moved, and the copy that was there
is put back if the rename cannot land — and the image is detached again. An
AppImage is a file this process is running from, so the release's own file is
copied beside it, made executable, and renamed over the path — one step, since a
Linux process keeps the file it started with — which is the whole installation.
A Windows copy is replaced by **running the release's setup**, which owns the
installation from there: it writes over the files it put there and waits for the
running app to be closed, so that install is reported as started (pending) rather
than finished. That setup is copied into `<config>/Oxide/desktop/installer`
before it is started, since the download itself lands in the run's own scratch
directory, which is removed as the call returns — a program started from there
would be reading a file an unrelated cleanup is about to delete, and on Windows a
program that is still running is exactly what the cleanup cannot remove. The
running app keeps running either way — the dialog ends with
the path the release landed at and the one thing left to do, quitting and opening
the app again. A release this copy cannot be replaced with is reported with the
file to install by hand instead: a checkout's build, a distribution's own package
or a copy an administrator put in place for every user cannot be written to from
here at all, and an artifact of another kind than this installation's (a Windows
setup for an app bundle) is not the one that replaces it.

A download that does not match its checksum is refused before anything is
replaced, and one install runs at a time. A check asked for again — the menu
item and the sidebar's check button are the same command — is the one that owns the
dialog, so an answer a newer check has already replaced is dropped rather than
repainting the dialog with an older release. A check asks GitHub for the newest
release, so a machine with no network reports what went wrong instead of
pretending to be up to date.

### The update a launch performs itself

A launch does not wait to be asked. The engine's `main.rs` spawns
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
the app replaces in place — a macOS `.app` bundle the reader installed, or an
AppImage. A checkout's build, a distribution's package, a copy an administrator
put in place for every user, and a Windows installation a setup made (which
takes over the files and waits for the app to be closed) all leave the launch
alone: nothing is written over behind the reader's back, and the dialog's own
**Install** stays the way in.
What does run is the dialog's own install — the same download, the same
checksum, the same swap — with one install at a time, so a second launch cannot
race the first.

The window is told rather than asking: `update-available` names the release the
launch found and opens the dialog on it, and the install's own steps follow as
`update-progress` events — so the reader watches the download in the same dialog
the button opens, and is left with the restart that runs what landed. The install
starts with the engine, before the page has loaded and subscribed to the event
channel, so each event is kept beside the app's
state as well as emitted and the page asks for the newest one as it starts
(`launch_update`): the report the restart hangs on is not one to have missed,
and a window that opened mid-install paints exactly what it would have heard.
The process running is still the build that started,
so the reload is the only thing that runs the release: **Restart** is the
`restart_app` command, which the engine refuses while a turn runs and otherwise
asks the window for, since a process cannot replace the app it was started from —
the window relaunches this copy and exits (`app.relaunch()`, naming the
AppImage's own file from `$APPIMAGE` when that is what is running), so the release
just put in place is the one that runs. A turn is work this process
owns — its tools write
files and its stream is read here — so a restart mid-turn is refused the way
replacing the thread on screen is refused, and the dialog says so.
**Later** and **Download in background** put the dialog away without stopping
anything, and the install goes on: what is left when it lands opens the dialog
again on the restart, since the window keeps no row of its own for an install
nobody in the window asked for. An install that could not finish is repainted
into the dialog when that is what the reader is looking at — the step it was on
is not left standing over an install that has ended — and is the line under the
composer when it is not. Because the window remembers the
release it installed, a later **Check for Updates…** reports that install rather
than offering to repeat it. The dialog's own button is the one to ask again with.

## Packaging

Icons are checked in (`icons/`: `icon.icns` for macOS, `icon.ico` for Windows,
`icon.png` for Linux, and the `.png` sizes and `icon.iconset` they were made
from). One command produces this platform's installer:

```sh
cd crates/desktop
pnpm run dist   # tsc, then cargo build --release, then electron-builder
```

That script is three steps: `tsc -p tsconfig.json` compiles the window,
`cargo build --release` compiles the engine, and
`electron-builder --config electron-builder.yml` packages the two into one app —
Electron's own files (`electron/dist/**`, `ui/**`, `package.json`) with the engine
carried beside them as `Resources/harness/oxide-desktop`
(`Resources/harness/oxide-desktop.exe` on Windows), named in the macOS build as a
binary so it is signed along with the bundle that runs it.

Each build writes to `crates/desktop/artifacts/`, named the way
`oxide_core::updates::Component::Desktop` resolves a release's assets: a
`macos-arm64-Oxide.dmg`, a `linux-<arch>-Oxide-Setup.AppImage` and a
`linux-<arch>-Oxide-Setup.tar.gz` for each Linux architecture, and a
`win-x64-Oxide-Setup.exe`. electron-builder also writes its own `latest*.yml`
metadata and a blockmap beside the installer; this app resolves its release
through GitHub itself, so metadata describing a feed it does not read is
deliberately not published.

Builds are per-platform: a macOS build produces only the macOS artifact, so a
release is four builds (macOS arm64, Linux x64, Linux arm64, Windows x64), each
on a runner of its own platform — the Linux arm64 artifact on an arm64 runner
rather than cross-compiled, so the engine and the AppImage are both native.
macOS is the only platform that is signed and notarized; the hardened runtime it
runs under needs the entitlements in `build/entitlements.mac.plist` (the JIT and
unsigned-executable-memory Electron's own JavaScript engine needs, and the third
that lets the app run the engine it ships, which is signed as part of the
bundle), which `electron-builder.yml` points both `entitlements` and
`entitlementsInherit` at. macOS Intel is not built at all: the app ships for
Apple Silicon alone. The Windows installer is a one-click, per-user install
(`oneClick: true`, `perMachine: false`), which is what lets the app offer it as
an install rather than as a file to unpack.

A build with no signing credentials in the environment is unsigned, which is what
a local build is. `.github/workflows/desktop.yml` — which builds macOS arm64,
Linux x64 and arm64, and Windows x64 on a `desktop-v*` tag push — sets those
variables, and only on the macOS job:

| Variable | Secret | Value |
| --- | --- | --- |
| `CSC_LINK` | `APPLE_CERTIFICATE` | the `.p12` the base64 secret is decoded to (`$RUNNER_TEMP/certificate.p12`) |
| `CSC_KEY_PASSWORD` | `APPLE_CERTIFICATE_PASSWORD` | that certificate's password |
| `APPLE_API_KEY` | `APPLE_API_KEY_P8` | the `.p8` file, written out of the base64 secret as `$RUNNER_TEMP/AuthKey.p8` |
| `APPLE_API_KEY_ID` | `APPLE_API_KEY` | an App Store Connect key ID |
| `APPLE_API_ISSUER` | `APPLE_API_ISSUER` | that key's issuer UUID |
| `APPLE_TEAM_ID` | `APPLE_TEAM_ID` | the 10-character team ID |

The certificate is decoded from the secret into a `.p12` and imported into a
temporary keychain of the run's own, which is put first in the search list, since
`codesign` can only use an identity in a keychain; electron-builder is handed that
`.p12` as `CSC_LINK` and its password as `CSC_KEY_PASSWORD`. A build with no
certificate is left to produce an unsigned bundle rather than a half-signed one —
the import step exits without setting anything when `APPLE_CERTIFICATE` is empty
— and the App Store Connect step sets nothing when there is no key, so a
repository without secrets still builds.

An unsigned app downloaded from the internet is quarantined by the browser and
Gatekeeper may refuse it; a reader can clear it with
`xattr -cr /Applications/Oxide.app`, which is what the release notes say. A
signed and notarized release needs none of that, and electron-builder's own
update metadata (`latest*.yml` and the blockmap it names) is not published at
all: this app updates itself from the installers the release carries (see
[Check for updates](#check-for-updates)) rather than from a feed beside them.

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

There is no identity name to record: the workflow imports the `.p12` into a
keychain of its own and hands electron-builder that file, so the certificate it
carries is the identity that signs.

Notarization needs your **App Store Connect API key**, which is what
`desktop.yml` is written for — not both it and an Apple ID. Create the key under
*Users and Access → Integrations → App Store Connect API*, role *Admin* or *App
Manager*:

| Secret | Value |
| --- | --- |
| `APPLE_CERTIFICATE` | base64 of the `.p12` (`base64 -i cert.p12 \| pbcopy`; Linux `base64 -w0 cert.p12`) |
| `APPLE_CERTIFICATE_PASSWORD` | the `.p12` export password |
| `APPLE_API_KEY_P8` | base64 of the `.p8` (`base64 -i AuthKey_XXXX.p8 \| pbcopy`) |
| `APPLE_API_KEY` | the key ID (the `AuthKey_<id>.p8` file name) |
| `APPLE_API_ISSUER` | the issuer UUID shown above the key list |
| `APPLE_TEAM_ID` | 10-character Team ID (developer.apple.com → *Membership details*) |

The workflow decodes `APPLE_API_KEY_P8` to `$RUNNER_TEMP/AuthKey.p8` and hands
electron-builder that path as `APPLE_API_KEY`, with the key's id as
`APPLE_API_KEY_ID`, the issuer as `APPLE_API_ISSUER` and the team as
`APPLE_TEAM_ID`; the `.p8` can only be downloaded once, so store it somewhere
safe.

### From the CLI

On the machine that holds the `.p12` and the `.p8`:

```sh
base64 -i cert.p12 | gh secret set APPLE_CERTIFICATE
gh secret set APPLE_CERTIFICATE_PASSWORD
base64 -i AuthKey_XXXX.p8 | gh secret set APPLE_API_KEY_P8
gh secret set APPLE_API_KEY --body 'ABCDE12345'      # the key ID, not the file
gh secret set APPLE_API_ISSUER --body 'your-issuer-uuid'
gh secret set APPLE_TEAM_ID --body 'ABCDE12345'
```

`gh secret set NAME` without `--body` prompts, so the value never lands in your
shell history.

## Release assets

Each `desktop-v*` release carries installers for the four platforms the app is
built for. Pick the asset whose platform matches the machine — the names carry
the app name and the platform rather than the version, so they read the same in
every release. The desktop app is versioned separately from the CLI, so a CLI
release never rebuilds these:

| Asset | Platform |
| --- | --- |
| `macos-arm64-Oxide.dmg` | macOS, Apple Silicon (`uname -m` → `arm64`) |
| `linux-x64-Oxide-Setup.AppImage` | Linux x64 (the file the app replaces itself with) |
| `linux-x64-Oxide-Setup.tar.gz` | Linux x64, an archive to unpack by hand |
| `linux-arm64-Oxide-Setup.AppImage` | Linux arm64 |
| `linux-arm64-Oxide-Setup.tar.gz` | Linux arm64, an archive to unpack by hand |
| `win-x64-Oxide-Setup.exe` | Windows x64 (a one-click, per-user install) |

There is no **macOS Intel** asset: the app is built for Apple Silicon alone, and
`Component::Desktop` resolves no asset for `darwin-x64`, so a check on an Intel
Mac is offered no desktop release. electron-builder's own update metadata
(`latest*.yml` and the blockmap naming an archive) is not uploaded: this app
resolves its release through GitHub itself and installs the artifact
`Component::Desktop::assets()` names for this platform — macOS the disk image,
Linux the AppImage, Windows the setup — rather than a feed beside it.

The CLI archives (`Oxide-v<version>-<platform>.tar.gz` and
`Oxide-v<version>-win32-x64.zip`) plus `install.sh`/`install.ps1` live in the
separate `v*` CLI releases, not here; see [install.md](install.md).

The workflow builds every platform in its own job and hands the installers to
the release job: each build job uploads what it produced — `*.dmg`, `*.AppImage`,
`*-Setup.tar.gz` and `*-Setup.exe` — as a workflow artifact, and the release job —
the only job granted `contents: write`, and the one that
runs for a `desktop-v*` tag alone — drafts the release and uploads them, so a
`workflow_dispatch` build publishes nothing and needs no more than a read-only
token.
