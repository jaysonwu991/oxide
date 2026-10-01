# Desktop app

The `oxide-desktop` package (`crates/desktop`) is an Electron front-end for the
same agent the terminal CLI runs. The goal is one configuration and one session
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
`settings.json`, and `sessions/` tree. The VS Code extension (`editors/vscode`,
see [docs/vscode.md](vscode.md)) is a separate pnpm package that drives the
`oxide` binary, so it shares the same files without linking `oxide-core`.

## Desktop layout

```
crates/desktop/
  src/
    lib.rs          re-exports the Rust host library
    manager.rs      project registry + session aggregation (shared, tested)
    turn.rs         starts an agent turn against a project (shared, tested)
    approval.rs     interactive approve/deny broker (uses `oxide_core::approvals`)
    ask.rs          a skill's question broker (uses `oxide_core::ask`)
    bridge.rs       JSON-lines events and responses
    commands.rs     stable renderer command dispatcher
    main.rs         Rust sidecar entry point
  electron/
    main.cjs        secure BrowserWindow, native dialogs, host lifecycle
    preload.cjs     allowlisted command/event bridge
    host-client.cjs JSON-lines sidecar client
  ui/               front-end: index.html, app.js, style.css
  forge.config.cjs  cross-platform packaging configuration
  package.json      Electron/Forge scripts and pinned dependencies
  icons/            app icons (PNG, .icns, .ico)
```

pnpm is the desktop's package manager: it is a package of its own, so neither the
Rust workspace nor the VS Code extension shares its dependencies. CI installs it
with `pnpm install --frozen-lockfile` from `pnpm-lock.yaml`, and
`pnpm-workspace.yaml` records which dependencies pnpm may run build scripts for —
the Windows installer maker, and the two native modules the macOS DMG maker loads,
which pnpm skips by default and the maker would then find missing. It also
carries pnpm's own settings: the hoisted `node_modules` layout Electron Forge
expects (it loads its makers and the Electron binary from a flat tree, and refuses
to start on pnpm's isolated one), and the exemptions from pnpm's minimum release
age that let CI install this lockfile at all. `.npmrc` pins the public registry so
the lockfile never resolves against a mirror only its author can reach.

The Rust package builds `oxide-desktop-host`; it has no GUI framework dependency.
Electron starts that host and exchanges newline-delimited JSON over private
stdin/stdout pipes. The renderer receives only an allowlisted API from the
sandboxed preload.

The window and the host share one lifetime. A run can only be watched, answered
or stopped from the window that started it — the host's events carry no state a
fresh renderer could be rebuilt from — so closing the last window quits the app
on every platform, including macOS, and `before-quit` stops the host, which ends
the run with it. That is deliberately not the usual macOS "stay resident"
behavior: leaving the app running with no window would leave a turn streaming
into nothing, with its approval and question requests unanswerable.

The other direction is the `host-error` event, which says whether it is fatal.
The host exiting or failing to spawn is (`fatal: true`): every pending call is
rejected, and the renderer lets go of the run — the busy state and **Stop**, the
approval dialog and the question dialog — because no `agent-end` can arrive from
a process that is gone. A frame the bridge could not parse is not (`fatal:
false`): it is one lost line, the host is still running, and the run on screen
keeps going with the reason shown in the status line.

## Interface

The window follows a Codex-style layout:

- **Sidebar** — the `Oxide` brand, a **New task** button, the **Projects**
  tree, and a footer pinned to the bottom with **Connect**, project trust,
  theme, tool approvals, and help. The tree groups each project's sessions
  under it, and every project and stored session row carries a `✕` that removes
  it (see [Multiple projects](#multiple-projects-cross-repo)); hovering a session
  shows its `⌘1`…`⌘9` shortcut. The active project and the active thread both
  carry the accent bar, so which one is on screen reads the same in either
  list, and a thread is listed — under the summarized title of its first
  message — as soon as its turn starts rather than once it ends: the thread the
  window is in stands in for itself in the tree, in that project's session
  count and in the `/sessions` list until the store has written it, keyed by
  the same id so it is never listed twice. Its row is the window's own while no
  file stands behind it, so selecting it leaves the thread on screen as it is
  and it is offered no `✕` — there is nothing stored to delete. A window with
  no thread on screen is
  starting one, so its next message opens a thread of its own instead of being
  appended to whichever thread was used last. The **+ New**
  button in the Projects header
  opens the **Create project** dialog: pick one or more source folders and the
  **Project name** defaults to the first folder's basename (still editable), so
  creating a project never requires typing a name.
- **Top bar** — the open thread's title: the same label its sidebar row shows,
  and a running turn's own summarized title (sent with `agent-start`) before
  the listing has it. A new task carries no placeholder, and the provider is
  not repeated here because the composer's model chip already names it; the
  right side says only what has to be acted on (`no API key`, `project
  resources off`). The Electron window uses `acceptFirstMouse`; no AppKit event
  monitor, Objective-C hook, or platform-specific input path is installed.
- **Conversation** — a centered 760px column. User messages are right-aligned
  bubbles; assistant replies render Markdown and links open in the system
  browser (see [Rendering](#rendering)). Tool calls are compact cards
  showing the call (e.g. `bash cargo test --all`); they expand automatically for
  diffs and errors and can be clicked open/closed. `write`/`edit` results get a
  colored diff.
- **Composer** — a floating rounded box with the attach, model, and reasoning
  chips on the left and one action on the right, which swaps rather than
  sitting beside a second button: **Stop** while a turn runs and there is
  nothing to say, **Send** beside **Queue**/**Steer** the moment there is. Every
  control is wired to a plain `click` — each button, native radio/checkbox,
  sidebar/list row, change card, and attachment thumbnail. Chromium owns focus,
  pointer, keyboard, and activation semantics; the page neither synthesizes nor
  suppresses control clicks. A
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
  downscaled to a 1568px long edge in the renderer before they are sent, and an
  image a paste handed over at full resolution is downscaled again by
  `oxide_core::media::optimize_image` when the turn is built — the one place a
  data URL can be — so it is not embedded at full size in the request, the
  session and the renderer's own message at once. A file past the core's 20 MB
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

The desktop package is a pnpm project of its own, so install it with `pnpm`:

```sh
cd crates/desktop
pnpm install
pnpm start
```

`pnpm start` builds `oxide-desktop-host` and launches Electron Forge in development
mode. Renderer files are loaded from `ui/`; restart Electron after changing the
main process or preload.

To refresh the app you launch from `/Applications` (or any installed bundle),
build a bundle, replace it, and ad-hoc sign it if macOS complains:

```sh
# quickest test
pnpm start

# to refresh the installed .app
pnpm run package
# quit Oxide, then:
cp -R out/Oxide-darwin-*/Oxide.app /Applications/Oxide.app
codesign --force --deep --sign - /Applications/Oxide.app   # only if macOS complains
```

The desktop check validates the secure Electron shell, talks to the real Rust
host, and loads `ui/app.js` against a stubbed DOM to drive the dialogs no Rust
test can reach:

```sh
cd crates/desktop
pnpm run check
```

It covers the `/mcps` listing (including the state colors, a failed probe and a
toggle), the `/sessions` dialog (this project's threads only, the row that
resumes one, the empty case, and a store that could not be read), the project it
opens on (the sidebar's first row, an existing selection, and no project at
all), the **Create project** dialog, a host that stopped under a run (the turn
and its dialogs let go for a fatal error, and kept for a recoverable protocol
warning), and every client command in the catalog —
a command the app does not perform has to be answered here rather than sent to
the model as a prompt. It also reads `ui/index.html` to check what no stub can:
that both listings are attached to the composer (inside `.composer-wrap`, above
`.composer`) instead of floating over the window, and that each header button is
an icon with a title. The shell check additionally pins the window/host lifetime
above, reads `pnpm-lock.yaml` to confirm every tarball resolves to the public npm
registry — since CI and every clean contributor install from it — and holds
`pnpm-workspace.yaml` to the dependency build scripts the Forge makers need.

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
ecosystem; declining saves `false` and leaves project resources out. The ☑
button in the sidebar footer shows and reviews the current decision, so the
harness loads exactly as it would in the CLI instead of being silently dropped.

The **Connect** button stores credentials through `oxide_core::auth`: a new key
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

The window opens on the first row rather than on nothing: `app.js` selects the
project the sidebar would show first (a registered folder, most recently opened
first, else one discovered from a session), so the composer is usable at launch.
The composer belongs to a project — with none selected the box stays disabled,
and the path behind it would resolve against the directory the app was launched
in, which is `$HOME` on one platform and `/` on another, not a folder the user
picked. With no project at all the empty state stays, since there is nothing to
run in.

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
the run's `Steering` handles and its cooperative `Cancel` flag. The Rust host serializes events with
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
is reported as a failure rather than as an unchanged file. The arrow keys walk
the files and <kbd>Esc</kbd> closes it.
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
- **Reasoning** — cycles `auto → off → low → medium → high`.

Reasoning is sent per turn as a `--reasoning`-equivalent override; `start_turn`
passes it to `Config::load`, so it doesn't rewrite the stored config.

## Usage

Opening a session restores its cumulative `usage_totals()` (input/output tokens
and cost); live turns update the footer from each `usage` event, including a
rough context percentage using `config.context_window()`.

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
| `Shift+Tab` / `Ctrl+R` | Cycle reasoning |
| `Ctrl+K` | Model picker |
| `Ctrl+/` | Shortcut help |
| `⌘1`…`⌘9` / `Ctrl+1`…`9` | Open the session with that number in the Projects tree |
| `Escape` | Close any dialog; a question is dismissed, which the agent is told rather than left waiting |

## Slash commands

The composer answers a leading `/` with the client's own commands. Typing `/`
opens a palette of them — the built-in list plus the commands, prompt templates
and skills the project or its plugins load, from `oxide_core::commands`, the
catalog `oxide commands --json` prints and the terminal's own autocomplete
mirrors. A name the app itself owns is performed here: `/mcps` (`/mcp`) opens
the **MCP servers** dialog, `/sessions` (`/session`) the **Sessions** dialog,
`/model`, `/theme`, `/approvals`, `/trust`,
`/connect`, `/new`, `/usage` and `/help` open or run what their sidebar entries
do. A project command, a prompt template and a skill are sent on as a normal
message, so the CLI's own resolution handles them — as is a client command with
an argument, so `/mcp list` and `/session <id>` reach the agent instead of being
performed as the bare command would be. A skill is listed under its own name
(`/rust-conventions`), with the row marked `skill` and its description beside
it, so taking the row completes the name and sending it is what loads the skill:
the CLI resolves the same name the menu lists, and `/skill:<name>` is the
terminal's other spelling of it. A name the app cannot
perform — today `/agent`, whose palette of subagents the app does not have yet —
says so in the transcript rather than reaching the model as the literal text
`/agent`. With no project selected the project-scoped commands say that first,
so a listing or a toggle cannot land in the app's own directory.

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
clicking any link opens it in the system browser — navigation is denied inside
the application window. Tool results render as panels; `write`/`edit`
results include a colored diff.

## Packaging

Icons are checked in (`icons/`). Build a bundle or native installer with
Electron Forge:

```sh
pnpm run package   # unpacked app for the current platform
pnpm run make      # native installers for the current platform
```

Forge produces `.app`/`.dmg`/`.zip` on macOS, Squirrel `.exe`/`.zip` on
Windows, and `.deb`/`.rpm` on Linux. Signing and notarization are automatic when
the Developer ID variables are set:

- **macOS**: `APPLE_CERTIFICATE`, `APPLE_CERTIFICATE_PASSWORD`,
  `APPLE_SIGNING_IDENTITY`, `APPLE_ID`, `APPLE_PASSWORD`, `APPLE_TEAM_ID`.
- **Windows**: unsigned by default; Squirrel accepts signing options in
  `forge.config.cjs` when a certificate is configured.
- **Linux**: no signing; `.deb`/`.rpm` as-is.

Without a Developer ID, `.github/workflows/desktop.yml` — which builds macOS
(arm64 + x64), Linux, and Windows on a `desktop-v*` tag push and drafts a
release — sets `APPLE_SIGNING_IDENTITY=-`, so Electron Forge **ad-hoc signs** the macOS
bundle. The signature is valid, but the app is not notarized and macOS
quarantines the download, so the first launch must be approved in **System
Settings → Privacy & Security → Open Anyway**, or the app moved to
`/Applications` and the quarantine cleared with
`xattr -dr com.apple.quarantine /Applications/Oxide.app`. An unsigned bundle is
instead rejected outright as *damaged* on Apple Silicon, so the fallback
matters. Auto-update artifacts are not enabled yet (they need a signing key).

## Signing secrets

`desktop.yml` reads the signing material from repository secrets
(**Settings → Secrets and variables → Actions → New repository secret**).
Nothing is required to build the bundles; without a certificate the macOS app is
only ad-hoc signed (see above).

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

**Apple ID** (Electron Forge notarizes and staples automatically):

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
`APPLE_API_KEY_PATH`; the `.p8` can only be downloaded once, so store it
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

Each `desktop-v*` release carries prebuilt bundles for every platform. Pick the
asset whose platform matches the machine (`<version>` is the release tag without
the `desktop-v` prefix, e.g. `0.1.0`). The desktop app is versioned separately
from the CLI, so a CLI release never rebuilds these bundles:

| Asset | Platform |
| --- | --- |
| `Oxide_<version>_aarch64.dmg` | macOS, Apple Silicon (`uname -m` → `arm64`) |
| `Oxide_<version>_x64.dmg` | macOS, Intel (`uname -m` → `x86_64`) |
| `Oxide_<version>_amd64.deb` | Linux x64 (Debian/Ubuntu) |
| `Oxide_<version>_amd64.AppImage` | Linux x64 (portable) |
| `Oxide-<version>-1.x86_64.rpm` | Linux x64 (Fedora/RHEL) |
| `Oxide_<version>_x64-setup.exe` | Windows x64 (NSIS installer) |
| `Oxide_<version>_x64_en-US.msi` | Windows x64 (MSI) |

The CLI archives (`Oxide-v<version>-<platform>.tar.gz` and
`Oxide-v<version>-win32-x64.zip`) plus `install.sh`/`install.ps1` live in the
separate `v*` CLI releases, not here; see [install.md](install.md).
