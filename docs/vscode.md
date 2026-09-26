# VS Code extension

The `editors/vscode` package is a TypeScript VS Code extension that drives the
same `oxide` binary the terminal runs. It is **not** part of the Cargo
workspace: it shells out to `oxide --mode json -p`, so it needs no Rust changes
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
    core/             pure, webview-free logic (unit tested under node)
      protocol.ts     wire events -> transcript state machine -> view messages
      views.ts        view ids shared by the manifest and the provider
      args.ts         VS Code settings -> `oxide` argv
      prompt.ts       prompt assembly, @path expansion, attachments
      preview.ts      write/edit/patch diff previews
      config.ts       shared config-dir resolution (read-only)
      settings.ts     settings.json / .oxide/settings.json reads (read-only)
      trust.ts        trust.json resolution and the access decision
      git.ts          the branch, read from .git/HEAD
      agents.ts       agent names for `--agent`
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

## Agent turns

Each turn is one process: `oxide --mode json -p` with the prompt written to
stdin (so the text never goes through argv or the CLI's positional `@file`
expansion), started by `cli.ts::startTurn`. Its stdout is framed by `drainLines`
(compacted once per chunk) and each line parsed by `parseEvent`; the `session`
header supplies the id reused for the next message with `--session <id>`, and
`--continue` resumes the newest session.

- **Queue / stop** — a message sent while a turn runs is queued and started
  after it finishes; **Stop** kills the process (SIGTERM, then SIGKILL after 3
  s). The session on disk is intact, so the next message continues the thread.
- **Per folder** — sessions are per project, so switching to a different
  workspace folder resets the transcript and starts its own thread.
- **Usage** — `usage` events accumulate input/output/cache tokens and cost for
  the usage line, and the latest one sets the context gauge (its prompt tokens
  over the window), which is `OXIDE_CONTEXT_LIMIT` when it is set else the
  config's `max_tokens` floored at 128k.

## The footer

Under the transcript sits the footer, which mirrors the terminal's. Every value
is composed in the extension host — `core/project.ts` gathers the on-disk state
and `core/footer.ts` turns it plus the transcript's usage into a `FooterState` —
and the webview only paints it, so the footer reads the same in both panes and
is unit tested without a webview.

- **Chips** — `model: <model> · <window>`, `thinking: <level>`, `agent: <name>`,
  `access: trusted|untrusted` and `session: <short id>`, each with a tooltip
  saying which setting or file behind it. A click posts a `control` message that
  the controller routes to the same action the matching command runs:
  `thinking` cycles the level (like <kbd>Shift+Tab</kbd> in the terminal), the
  others open the model, agent, trust and session pickers.
- **Branch** — the repository the folder sits in, read from `.git/HEAD` rather
  than through the Git extension, so it needs no other extension installed; a
  worktree's or submodule's `gitdir:` pointer is followed to the real HEAD.
- **Status** — the live phase and an elapsed timer while a turn runs.
- **Gauge** — the last request's prompt tokens over the context window, amber
  past 70% and red past 90% (the terminal's thresholds).
- **Usage line** — `↑input · ↓output · RcacheRead · WcacheWrite · CHhit% · $cost
  · ctx %/window (auto)`, matching the terminal's footer segments. `CH` is the
  latest request's cache hit rate (`cache_read / prompt`), the same number
  `UsageTotals::cache_hit_rate` reports, and a step that reads no cache leaves
  the previous rate in place; `(auto)` marks auto-compaction as on, and the
  window is shown dimmed when nothing has run yet.

The reads are best effort: a missing or malformed file blanks the value it
feeds — the model chip falls back to `config.json`, the branch and agent names
to empty — and never throws in the middle of a turn. `trust.ts` resolves
`trust.json` by closest ancestor like `oxide_core::trust`, folds in
`oxide.projectTrust` (a saved decision beats `defaultProjectTrust`, and `ask`
reads as untrusted because a non-interactive run cannot prompt), and
`settings.ts` reads `compaction.enabled` from the global and the project
`settings.json` with the project winning per key.

## Wire protocol

`core/protocol.ts` turns the CLI's Pi-shaped events into view updates. Only
`type` is guaranteed, so every field is read defensively.

Events consumed: `session`, `thinking`, `thinking_done`, `message_update`
(`thinking_delta` / `text_delta`), `tool_call`, `tool_execution_update`,
`tool_execution_end`, `usage`, `auto_retry_start`, `compaction`, `error`, and
`agent_end`. `thinking_done` marks the end of a model step (its `ThoughtDone`
counterpart), so a later step's output does not merge into, and a retry cannot
discard, a previous step's committed reply.

The transcript is a list of items (`user`, `assistant`, `thinking`, `tool`,
`notice`). The view applies small deltas: `push` a new item, `remove` a
discarded one, `append` a text/output fragment, `patch` a tool card when it
settles, and `status` / `usage` / `context` for the footer. The `state`,
`status` and `usage` messages carry the whole `FooterState` — the controller
attaches it, since it is the only place that knows the context window and the
resolved settings — and `control` is the one message that travels the other way,
carrying a chip's id. Reasoning and text stream into separate items, and a
thinking block is created by its first delta, so a turn that only starts one
never leaves an empty block in the transcript. When a stream drops and the CLI
retries, `auto_retry_start` drops the item the failed attempt was streaming
into, so the retry's fresh output does not extend the partial reply.

## Prompt assembly

`core/prompt.ts` builds the prompt the same way the CLI's own `@file` expansion
reads: each attached block is a `--- path[:range] ---` header plus its text,
then the message. Images and PDFs (`png`, `jpg`/`jpeg`, `gif`, `webp`, `bmp`)
are not inlined; they are passed as `--image` and the CLI attaches them as
media.

Because the prompt is sent on stdin, a message's own `@path` references are
resolved by the extension instead of the CLI: `@src/main.rs` becomes a context
block, an image/PDF becomes an attachment, and a reference that does not resolve
stays in the message. Duplicate references are collapsed, and trailing
punctuation is not taken as part of the path.

## Diff previews

The `--mode json` stream does not carry `AgentEvent::ToolResult`'s
`DiffPreview`, so `core/preview.ts` rebuilds one from the tool's arguments
against the current file. It uses the same LCS line diff and the same compact
line-numbered layout as `oxide_core::diff`, and applies `edit` calls the way the
tool does — a byte-exact match first, then the same tolerance for trailing
whitespace and the `N|` line numbers a `read` result prints — while still
tolerating the argument shapes the tool itself accepts. A preview is only built
for a file inside the workspace, so the panel cannot be used to read outside it.

## Rendering

`media/main.js` renders the transcript in the webview; it is adapted from the
desktop app (`crates/desktop/ui/app.js`), so a reply reads the same in both.
Assistant replies are Markdown (headings, lists, tables, fenced code with
lightweight syntax highlighting, inline emphasis/code/links, and auto-linked
bare URLs). Tool calls are collapsible cards colored by state, with a spinner
and elapsed time while running, a short per-tool output preview that expands on
click, and a colored diff for `write` / `edit` / `patch`. Reasoning renders as a
muted thinking block that `oxide.showThinking` can hide.

Links in a reply open in the system browser (`chatView.ts::openUrl`), since a
webview cannot navigate to a remote page; a path in a tool card opens in the
editor, but only inside the workspace.

## Commands and settings

The manifest defines the two view containers (activity bar and secondary side
bar), the commands and keybindings, and the `oxide.*` settings. See the
[extension README](../editors/vscode/README.md) for the user-facing tables. The
footer's chips are shortcuts into the same actions: `setModel`, `setAgent`,
`cycleReasoning`, `setProjectTrust` and `resumeSession` are reached from a chip
click and from the palette, so the two entry points never drift.

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
`@path` expansion, diff and tool previews, session-list parsing, config-dir
resolution, binary lookup, and the transcript state machine — plus, in
`test/views.test.ts`, that the chat view ids the host registers match the views
`package.json` contributes, in `test/brand.test.ts`, that the two icons stay
the desktop app's, and, in `test/commands.test.ts`, that every contributed
command has a handler and every footer chip has a click handler. The footer's own
readers are covered one file each: `test/settings.test.ts`, `test/trust.test.ts`,
`test/git.test.ts`, `test/agents.test.ts`, `test/project.test.ts` (the four
together, against an injected file map) and `test/footer.test.ts` (the labels,
the usage line and the gauge).

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
