# Tools

The tools the agent can call, their parameters, and the limits their results are
subject to.

Built-in file and shell tools use Pi-style names: `read`, `write`, `edit`,
`bash`, `grep`, `find`, `ls`, and `webfetch`. Compatibility names `read_file`,
`write_file`, `list_dir`, and `glob` remain accepted; `patch` is the unified-diff
editing tool. Agent-level tools: `task`, `skill`, `command`, `memory`,
`diagnostics`, and `ask`. `mcp_load` reveals a configured server on demand; its
tools then appear as `<server>__<tool>`.

| Tool | Parameters |
| --- | --- |
| `read` | `path`, `offset?` (1-based), `limit?` (default 400 lines) |
| `write` | `path`, `content` |
| `edit` | `path`, `edits: [{ oldText, newText }]` |
| `bash` | `command`, `timeout?` in milliseconds (default 120000) |
| `grep` | `pattern`, `path?`, `glob?`, `ignoreCase?`, `regex?`, `context?`, `limit?` |
| `find` | `pattern`, `path?`, `limit?` |
| `ls` | `path?`, `limit?` |
| `patch` | `diff` (unified diff) |
| `webfetch` | `url`, `format?` (`markdown` default, `text`, or raw `html`) |

## Behavior

`edit` performs targeted text replacement: each `oldText` is matched against the
original file (never incrementally) and must be unique, so a non-unique or
missing match is rejected rather than silently corrupting a file. Matching is
byte-exact first, then tolerates trailing whitespace and the `N|` line numbers
`read` prints, so a block copied straight from a read result still lands; when the
text has genuinely changed, the error names the closest region to copy. Line
endings and a leading BOM are preserved. `write` and `edit` append LSP diagnostics
for the edited file.

`read`, `ls`, `find`, and `grep` accept absolute paths, so they can inspect files
outside the project without a shell; `ls` marks directories with a trailing `/`
and renders symlink targets as `name -> target`. `find` and `grep` respect
`.gitignore`, and `grep` matches a literal substring by default (set
`regex: true` to treat `pattern` as a regular expression) and prefers `ripgrep`
(`rg`) when it is on `PATH`, falling back to a dependency-free parallel walker
that skips binary files. A `read` the platform refuses — macOS keeps the
Desktop, Documents and Downloads folders behind a per-app grant — is reported
with the reason and the grant to give, naming the app the run was started from
and the name that app's own bundle carries (so VS Code, whose bundle is
`Visual Studio Code.app`, is named `Code`), since that is the app the settings
pane lists and not `oxide` itself.

`webfetch` converts HTML to Markdown (`format: "markdown"`, the default),
readable plain text (`format: "text"`), or returns the raw body
(`format: "html"`); the converter is dependency-free and handles headings,
paragraphs, lists, tables, links, images, inline and fenced code, blockquotes,
and HTML entities.

`bash` runs the command through a shell with stdin closed and its output
captured, in a terminal of its own rather than the front-end's — a session on
Unix, a console without a window of its own on Windows — so a command that asks
the reader's terminal for an answer of its own — zsh's `compinit` asking whether
to keep an insecure directory, a credential or passphrase prompt, a pager — fails
with the reason in its own output (`compinit: initialization aborted`) instead of
painting its prompt over the interface that started it and reading the keys typed
there. The children that are not commands get the same treatment: an LSP server,
an MCP server and the plugin host.

## Output limits

Tool results are capped before they enter the model's context. The default cap is
250 lines and 6 KB, but `read` gets a larger budget (400 lines / 16 KB) so an
ordinary source file is returned whole instead of paged in slices; other per-tool
overrides are `bash` 160 lines / 5 KB, `grep`, `find`, and `ls` 160 / 4 KB,
`webfetch` 200 / 6 KB, and `write`, `edit`, and `patch` 120 / 3 KB. A connected
MCP tool (`<server>__<tool>`) gets the largest budget of all, 500 lines / 20 KB:
its result is structured data whose shape the server chose, so unlike a file read
it cannot be narrowed with `offset`/`limit` and unlike a search it cannot be
re-asked for less.

`read` splits a line longer than 1 000 characters into continuation chunks
(`offset`/`limit` count those display lines), so an over-long line can be paged
through instead of being cut off.

`bash` keeps the **tail** so the exit code and recent errors survive; an MCP
result keeps **both ends** — half the lines from each end, with the byte budget
split three-to-one in the head's favour — and a `[truncated: N lines, M bytes;
full: <path>]` marker between them, because a server's answer puts its summary
at the top and its totals (a `nextPageToken`, a count) at the bottom — keeping
one end alone discards the half the next call needs. Every other tool keeps the
head.

When output is dropped, the full text is written under `truncated/` in the Oxide
config directory and the result names that file, so the model can recover the
dropped detail without re-running the tool. Set `OXIDE_TRUNCATION_DIR` to change
where those files go; they are retained for 7 days.

## Concurrency and steering

When the model requests several tools at once, the ones with no side effects
(`read`, `ls`, `find`, `grep`, `webfetch`, `memory`, `skill`, `diagnostics`) run
concurrently; anything that writes to the workspace, spawns a subagent, or has
unknown remote effects stays sequential. Results are recorded in the model's
original call order. `bash` streams stdout and stderr line by line into the TUI
(and to stderr in `-p` mode) before the final combined output.

While the agent is busy, pressing Enter steers the response being written,
injecting the message before the agent's next model step. Alt+Enter queues the
current input as the next turn after the current response finishes instead. A
`tool.execute.after` plugin can also request termination for the batch
with `output.terminate = true`.
