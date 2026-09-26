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
      args.ts         VS Code settings -> `oxide` argv
      prompt.ts       prompt assembly, @path expansion, attachments
      preview.ts      write/edit/patch diff previews
      config.ts       shared config-dir resolution (read-only)
      sessions.ts     `oxide sessions list` / `--version` parsing
    test/             node:test suites for src/core
  media/
    main.js           dependency-free webview renderer (Markdown, diffs, …)
    style.css         themed styles (VS Code CSS variables)
    oxide.svg         activity-bar icon
```

The extension host owns all the state; the webview is a dumb renderer that
applies the view messages produced in `core/protocol.ts`. That module imports
nothing from `vscode`, so the whole event-to-DOM decision surface is unit tested
with `node --test` and no webview.

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
  the footer; the context gauge uses the most recent step's prompt tokens and
  `OXIDE_CONTEXT_LIMIT` when it is set (the extension cannot read a model's
  window out of the CLI's config).

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
settles, and `status` / `usage` / `context` for the footer. Reasoning and text
stream into separate items, and a thinking block is created by its first delta,
so a turn that only starts one never leaves an empty block in the transcript.
When a stream drops and the CLI retries, `auto_retry_start` drops the item the
failed attempt was streaming into, so the retry's fresh output does not extend
the partial reply.

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

The manifest defines the activity-bar view, the commands and keybindings, and
the `oxide.*` settings. See the
[extension README](../editors/vscode/README.md) for the user-facing tables.

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
resolution, binary lookup, and the transcript state machine. CI runs the same
checks on every pull request (the `vscode` job in
`.github/workflows/ci.yml`): `pnpm run check`, `pnpm test`, and
`pnpm run package`, so a type error or a broken manifest fails the PR.

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
`v*` tags and its drafter). The draft already carries the tag to push; build the
VSIX with `pnpm run package` and attach `oxide-vscode-<version>.vsix` to the
release.
