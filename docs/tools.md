# Tools

The tools the agent can call, their parameters, and the limits their results are
subject to.

Built-in file and shell tools are `read`, `write`, `edit`, `bash`, `grep`,
`find`, `ls`, and `webfetch`. Agent-level tools: `task`, `skill`, `command`,
`memory`, `diagnostics`, `ask`, and `tool_search`. Enabled MCP servers connect
in the background; `tool_search` discovers capabilities and declares only the
matching `<server>__<tool>` schemas on the next model call. `mcp_load` connects or
retries a particular server while keeping its catalog deferred.

| Tool | Parameters |
| --- | --- |
| `read` | `path`, `offset?` (1-based), `limit?` (default 2000 lines) |
| `write` | `path`, `content` |
| `edit` | `path`, `edits: [{ oldText, newText }]` |
| `bash` | `command`, `timeout?` in milliseconds (default 120000) |
| `grep` | `pattern` (regex), `path?`, `glob?`, `ignoreCase?`, `literal?`, `context?`, `limit?` |
| `find` | `pattern`, `path?`, `limit?` |
| `ls` | `path?`, `limit?` |
| `webfetch` | `url`, `format?` (`markdown` default, `text`, or raw `html`) |
| `tool_search` | `query`, `limit?` (default 8, maximum 64) |
| `mcp_load` | `server` |

## Behavior

`edit` performs targeted text replacement: each `oldText` is matched against the
original file (never incrementally) and must be unique, so a non-unique or
missing match is rejected rather than silently corrupting a file. Matching is
byte-exact first, then tolerates trailing whitespace and the `N|` / `N+|` line numbers
`read` prints. Continuation chunks are joined back into their original line in both
the target and replacement. When text has genuinely changed, a closest-region hint
is offered only when it shares meaningful content; blank lines and punctuation
alone do not identify a region. Read the current target before retrying. Line
endings and a leading BOM are preserved. `write` and `edit` append LSP diagnostics
for the edited file when its language server is already running; the first edit
starts the server in the background rather than waiting for it to index the
project, so the edit is never blocked on a cold `rust-analyzer`.

Each `edit` call changes one file. Send separate calls for different files, omit
replacements whose `oldText` equals `newText`, and merge overlapping changes. A
no-op that is supplied is still validated. If any replacement fails, the entire
batch is rejected without writing anything, including the replacements that
matched. The error names the failures and tells the agent to re-read the correct
file and resend the corrected batch. CLI, Extension and Desktop share these rules.

A `write` or `edit` that increases the count of U+FFFD replacement characters
is refused as likely encoding corruption. A write compares the full file; an
edit compares each replaced block. Existing characters may be preserved or
removed. Validation happens before creating directories or writing files, and
one refused edit rejects the entire batch. Resend the real characters; deliberate
U+FFFD content can be written with `bash`.

`read`, `ls`, `find`, and `grep` accept absolute paths, so they can inspect files
outside the project without a shell; `ls` marks directories with a trailing `/`
and renders symlink targets as `name -> target`. `find` and `grep` respect
`.gitignore`, and `grep` treats `pattern` as a regular expression by default (set
`literal: true` to search for plain text, and a pattern that is not valid regex
syntax is searched literally with a note instead of failing the call) and prefers `ripgrep`
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
and HTML entities. A 401 reports that authentication is required; a 403 reports
access denied, which may reflect authentication, permissions or automated-fetch
restrictions. Both point at the service's authenticated CLI or a configured MCP
server. A 429 reports rate limiting and asks the caller to wait before retrying.

`bash` runs the command through a shell with stdin closed and its output
captured, in a terminal of its own rather than the front-end's — a session on
Unix, a console without a window of its own on Windows — so a command that asks
the reader's terminal for an answer of its own — zsh's `compinit` asking whether
to keep an insecure directory, a credential or passphrase prompt, a pager — fails
with the reason in its own output (`compinit: initialization aborted`) instead of
painting its prompt over the interface that started it and reading the keys typed
there. The children that are not commands get the same treatment: an LSP server,
an MCP server and the plugin host.

On Unix, a shell search whose only unsuccessful stages are
`grep`/`rg` returning 1 is a normal empty result. The output retains `[exit: 1]`
and adds `search returned no matches`, while the tool's error flag stays clear.
That holds when the search is the last command of a `;`- or newline-separated
script too, since a shell reports its last command's status: `git status; grep
-q TODO notes.md` is an empty search rather than a failed command. An
unsuccessful producer, invalid regex, a failed leading `cd`, or anything that
could hide which command ran last (a comment, a heredoc, a subshell, an
expansion, `&&`/`||`, or an earlier `exit`/`exec`/`set -e`) still fails.

`gh pr checks` exits 8 while its checks are still running, which is a documented
state rather than a failure: no check has reported a failure yet (that is its
exit 1). A `gh pr checks` run — on its own, or piped through text filters — that
exits 8 keeps `[exit: 8]`, adds `pull request checks are still pending, not
failed`, and leaves the tool's error flag clear, so the transcript shows a result
instead of a red tool call while CI runs. On Unix the pipeline's own stage
statuses have to agree, so an unrelated failing command or a filter that exited 8
is still a failure; Windows has neither `pipefail` nor `PIPESTATUS`, so only an
unpiped run is read this way. Because such a read confirms no outcome, it does
not clear the pull-request entry in the run's Definition-of-Done tracking: the
reminder still asks for the checks to be read again until they report.

Shell failures caused by unsupported grep repetition counts, unclosed quotes,
or selecting a package outside the current Cargo workspace carry a specific
recovery hint, as does a command whose request never completed over the network
— `gh`, `curl` and `git` report a TLS handshake timeout, a reset or a DNS
failure that way, and the hint asks for the same command again rather than for a
repository, authentication or syntax fix. A command that only reads over the
network is also retried once on its own before that hint is returned: `gh` read
commands (`api` without a method or field, `pr view`, `search`, and the like), a
GET-shaped `curl`, and `git ls-remote`, optionally piped through a text filter,
so a transport failure does not cost the run a step, while anything that could
change something is left alone. The recursive-search guard refuses a
recursive `grep` that could
walk a directory — no operand, a folder, an unresolved path, or an unverified glob —
and steers it to the `grep` tool, which applies the repository's ignore rules
and bounds its results; when the command spells out its pattern, path and
`--include` filter as literal shell arguments, the refusal suggests a `grep` call
with those values. Quoted spaces and glob filters are decoded, and `-F` maps to
`literal: true`. Multiple paths or patterns, unsupported matching options, shell
expansions, regex dialect differences and preceding commands keep the generic
guidance rather than suggesting a search with different meaning. A
recursive search scoped to files named one by one
(`grep -rn needle README.md docs/guide.md`) is not a directory sweep and runs
as written. Simple unquoted filename globs such as `docs/*.md` also run when
at least one file matches and every match is a regular file. A matched directory
(including a symlink to one) keeps the guard active. Relative globs are verified
only in the first shell segment, since an earlier command may change the working
directory; absolute globs can be verified in later segments too. Globs in parent
directories and shell expansions remain guarded. Wildcard matching uses an
iterative scan to avoid exponential recursion on repeated `*` patterns.
It also recognizes option arguments, so a pattern such as
`grep -e -retry file` is not mistaken for a recursive flag.
Repeated-build reminders are conditional: shell commands and other tools may
have changed the inputs since the previous check.

## Output limits

Tool results are capped before they enter the model's context, at Pi's own
budget: 2000 lines or 50 KB, whichever is hit first. `read`, `bash`, `grep`,
`find`, `ls` and `webfetch` all use it, so a whole source file, a failing build's
tail or a large match set arrives in one call instead of several paged round
trips. `write` and `edit` keep a smaller confirmation budget (120 lines / 3 KB),
and a connected MCP tool (`<server>__<tool>`) uses the same 2000 / 50 KB: its
result is structured data whose shape the server chose, so unlike a file read it
cannot be narrowed with `offset`/`limit` and unlike a search it cannot be
re-asked for less.

`read` splits a line longer than 1 000 characters into continuation chunks
(`offset`/`limit` count those display lines), so an over-long line can be paged
through instead of being cut off.

`bash` keeps the **tail** so the exit code and recent errors survive; an MCP
result keeps **both ends** — half the lines from each end when the line cap is
what bites, and an even split of the bytes when one long line (a server's JSON)
is what exceeds it — with `…N lines, M bytes truncated…` where the middle went,
because a server's answer puts its summary at the top and its totals (a
`nextPageToken`, a count) at the bottom — keeping one end alone discards the
half the next call needs.

A head or tail preview cuts on a **line boundary**, not at the byte offset the
budget lands on: a tail begins where a line begins and a head ends where a line
ends, so a result never opens or closes with a fragment whose other half the
model cannot see — a `git diff` cut mid-hunk otherwise reads as a stray
`t.mjs';` with no indication that the line it belongs to was severed. This
applies to a spooled `bash` tail as well as the central cap, since a long
command's output is previewed from its spool file. Snapping is skipped when it
would spend more than half the budget on alignment — a single line longer than
the budget has no useful boundary, and dropping it would answer with a marker
and nothing to read — in which case the cut falls back to a character boundary.
The leftover bytes are reported with the dropped lines, and the full text is
still saved.

A server's JSON answer is the one exception: instead of cutting it at a byte
offset — which leaves a fragment that starts and ends mid-token, parses as
nothing, and says nothing about how much of the answer was dropped — it is
elided at its largest array. The records that fit are kept from both ends
(three quarters from the front), a marker element names the rest
(`… 659 of 800 records omitted …`), and the result is headed `[truncated: 659 of
800 records omitted, 92812 bytes; full: <path>]`. What reaches the model is
still the same document, so it can see the record count, read the samples it was
given, and narrow the next query itself instead of paying for the same answer
twice. The elision only applies when a record actually has to go: an answer that
is over budget because a server pretty-printed it comes back whole and
compacted, with no marker at all. Every other tool keeps the head.

When output is dropped, the full text is written under `truncated/` in the Oxide
config directory and the result is headed `[truncated: N lines, M bytes; full:
<path>]` (`full:` is left out when nothing could be saved), so the model can
recover the dropped detail without re-running the tool. Set
`OXIDE_TRUNCATION_DIR` to change where those files go; they are retained for
7 days.

The agent applies these limits again after plugin hooks and agent-level tools
(such as `task` and `diagnostics`), preserving an existing full-output pointer
when the result is already bounded. This prevents those paths from bypassing
the context budget.

A typed MCP authorization failure blocks subsequent calls to every tool on that
connection before making another request. A newly stored, unexpired OAuth token
for the same resource releases the guard without restarting the app. Otherwise,
authorize out of band and reconnect after changing server configuration.

Within a run, three identical failures from the same tool and JSON arguments
stop the run with an error. Changed failures restart the count; successful
state-changing tools clear the history, and successful reads clear their own
failure. Successful polling (including pending CI and empty searches) is not a
failure. This guard does not replace transport retry/backoff for transient errors.

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

Cancellation interrupts asynchronous tool waits. A cancelled call reports that
its effects may be partial, so inspect the target before retrying; it does not
undo edits. Shell cancellation terminates the process tree. Preview alignment
searches newline bytes before slicing UTF-8, including when the budget falls
inside a Chinese character or emoji.
