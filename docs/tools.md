# Tools

The tools the agent can call, their parameters, and the limits their results are
subject to.

Built-in file and shell tools are `read`, `write`, `edit`, `bash`, `grep`, `code_search`,
`find`, `ls`, and `webfetch`. Agent-level tools: `task`, `skill`, `command`,
`memory`, `diagnostics`, `ask`, and `tool_search`. Enabled MCP servers connect
in the background; `tool_search` discovers capabilities and declares only the
matching `<server>__<tool>` schemas on the next model call. `mcp_load` connects or
retries a particular server while keeping its catalog deferred.

A remote MCP request rejected with HTTP 404 while carrying a session ID clears
that ID, initializes a new session, and retries the request once. A second
rejection is reported. An ordinary 404 without a session ID, a timeout, or a
server error is not replayed: the outcome of a writing tool may be unknown.

| Tool | Parameters |
| --- | --- |
| `read` | `path`, `offset?` (1-based), `limit?` (default 2000 lines) |
| `write` | `path`, `content` |
| `edit` | `path`, `edits: [{ oldText, newText }]` |
| `bash` | `command`, `timeout?` in milliseconds (default 120000) |
| `grep` | `pattern` (regex), `path?`, `include?` (`glob?` alias), `limit?`, `output_mode?` (`files_with_matches` default or `content`), `ignoreCase?`, `literal?`, `context?` (content only) |
| `code_search` | `query`, `path?` (local directory), `limit?` (default 5, 1–20), `content?` (`code` default, `docs`, `config`, `all`), `max_snippet_lines?` (default 80, 0–200) |
| `find` | `pattern`, `path?`, `limit?` |
| `ls` | `path?`, `limit?` |
| `webfetch` | `url`, `format?` (`markdown` default, `text`, or raw `html`) |
| `tool_search` | `query`, `limit?` (default 8, maximum 64) |
| `mcp_load` | `server` |

## Behavior

`code_search` discovers implementations from natural-language questions, returning
ranked paths, line ranges and numbered snippets. It complements `grep` for exact
or exhaustive matches and `read` for surrounding context. An empty ranked result
is a successful `No matches found`; model, download and filesystem failures are
reported as errors.

The implementation is native Rust, inspired by
[Semble](https://github.com/MinishLab/semble): the official `model2vec-rs` library
loads the same `minishlab/potion-code-16M-v2` static embedding model; BM25 lexical
scores and embedding similarities are combined with reciprocal rank fusion.
Identifier-like queries receive more lexical weight. This is not a full port of
Semble's language coverage or reranking heuristics, and its benchmark claims do
not establish oxide's retrieval quality or token savings.

Rust, JavaScript, TypeScript/TSX and Python use tree-sitter boundaries. Other
recognised code, documentation and configuration extensions use text chunks.
Chunks are capped at 60 lines / 6000 bytes, with five-line overlap inside long
regions; longer individual lines are skipped. Overlapping results are collapsed.
The walker honours `.gitignore` and `.sembleignore` exclusions, including outside
Git repositories, skips dependency/build directories, and does not follow symlinks.
Files above 1 MiB are skipped with a note; scopes above 64 MiB of source or 50,000
chunks require a narrower `path`.

The first call downloads about 34 MB of public model assets from Hugging Face at
a pinned revision. Source code and queries stay local. Set
`OXIDE_CODE_SEARCH_MODEL` to a local directory containing `config.json`,
`tokenizer.json` and `model.safetensors` to run without downloads. Models and
indexes live in the OS cache directory under `oxide/code-search/`; indexes
contain source snippets and can be removed there. Content hashes refresh added,
changed, deleted and newly ignored files on every search, reusing embeddings for
unchanged files. Model changes invalidate cached vectors. Cache write failures
are reported as notes while the search result remains usable.

The shared tool registry, permission rules and filesystem scope checks apply in
CLI, Desktop and VS Code. `code_search` defaults to allowed and honours explicit
ask/deny rules, like other read tools. Calls run sequentially because they refresh
the index cache. No Python installation, Semble CLI or subprocess is used.

**When to upgrade or replace the code-search model**

The model has no scheduled expiry. Keep the pinned revision while retrieval
quality, resource usage and availability meet the project's needs. A newer
upstream release is a reason to evaluate a candidate, not to update automatically.

Review the model when:

- Representative natural-language queries repeatedly miss known implementations
  or rank irrelevant snippets above them, especially for languages or domains
  newly important to the project.
- Cold indexing time, warm search latency or memory usage exceeds the product's
  budget on representative repositories and supported hardware.
- Upstream publishes a relevant retrieval correction, or the pinned assets
  become unavailable or incompatible with the supported Rust loader.
- Licensing or distribution requirements change and the current model no longer
  meets them.

Before attributing a missed result to the model, check ignore rules, supported
extensions, file/chunk limits and chunk boundaries. Changing the embedding model
cannot recover source that never entered the index. Use `grep` and `read` to
establish the expected location.

Prefer a revision upgrade when the same model family resolves the observed
problem. Consider replacing it when another compatible Model2Vec model provides
better measured retrieval or resource usage. A transformer or hosted embedding
service requires a separate loader and execution design; it cannot be selected
by changing the model name alone.

Evaluate candidates through `OXIDE_CODE_SEARCH_MODEL` using separate local model
directories. Compare the current model and candidate on the same repository
snapshots and labelled queries, covering natural-language discovery, exact
identifiers and important languages. Record top-5 retrieval of expected locations,
cold indexing time, warm latency and peak memory, including regressions. Set
acceptance budgets before comparing and retain the current pin if the candidate
does not justify the change. Confirm the candidate's license and distribution
terms as part of that decision.

To promote an accepted candidate, update `MODEL` and its immutable `REVISION` in
`crates/core/src/tools/code_search.rs`, check its three asset names and download
size limits, and update the documented model name and size. Run the native model
smoke test explicitly (`cargo test -p oxide-core native_model_smoke -- --ignored
--nocapture`) alongside normal tests and Clippy. The smoke test verifies loading
and basic retrieval; the labelled comparison establishes whether to upgrade.

Model asset hashes automatically invalidate persisted vectors; the next search
rebuilds the selected index. If chunking, embedding input preparation or index
format changes, also increment `INDEX_VERSION`. Ranking-only changes do not
require re-embedding. Restart running clients after changing a local model to
ensure cached weights are reloaded, including when file timestamps were preserved.
Keep the previous model/revision and evaluation results in the PR so a regression
can be rolled back by restoring the pin; indexes rebuild on the next search.

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
and renders symlink targets as `name -> target`.

By default `grep` follows the last public Codex `grep_files` contract:
[`openai/codex` at `32c4993`](https://github.com/openai/codex/blob/32c4993c8a17cb0608d747f268e3d543745de962/codex-rs/core/src/tools/handlers/grep_files.rs).
It invokes `rg --files-with-matches --sortr=modified --regexp <pattern>
--no-messages`, returning one filename per matching file, newest modified first.
`path` can name a file or directory (default: the working directory), and `include`
restricts files by glob; surrounding whitespace in `pattern` and `include` is
trimmed. The default limit is 100 files, capped at 2000; zero and invalid limits
are rejected. Searches time out after 30 seconds, and cancellation kills the child.
Invalid regexes, missing paths, missing ripgrep, and unsuccessful searches report
errors; exit 1 is an ordinary empty result (`No matches found`). Filename mode uses
ripgrep's own hidden-file and ignore rules, including its requirement for a Git
repository when applying `.gitignore`. It does not impose content mode's extra
dependency-directory exclusions. Oxide keeps its ordinary success classification
for empty results and accepts `glob`, `ignoreCase`, and `literal` as extensions.
Codex removed its dedicated handler from main; this compatibility target is pinned.

Use `output_mode: "content"` for the existing `path:line: text` matches and context
lines. This mode, like `find`, respects `.gitignore` even outside Git repositories
and skips `.git`, `target`, `node_modules`, and `.venv`. It prefers ripgrep and
falls back to a parallel built-in walker that skips binary files. Patterns are
regexes by default (`literal: true` searches plain text); invalid regex syntax is
searched literally with a note. Content mode's default limit is 100 output rows,
with the shared 2000-line / 50 KB output budget. Shell searches redirected to
`grep` explicitly select content mode to preserve their output.

A `read` the platform refuses — macOS keeps the
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
-q TODO notes.md` is an empty search rather than a failed command, and so is
`node check.mjs >/tmp/s.log 2>&1; echo shell=$?; grep -E '^  fail' /tmp/s.log |
head` — an expansion in an argument and a `&&` half of an earlier command only
decide that command's own word, not which command the status belongs to. An
unsuccessful producer, invalid regex, a failed leading `cd`, or an earlier
command that could hide the last one (a comment, a heredoc, a background `&`, a
`&&`/`||` half that ends the shell with `exit`/`exec`/`set -e`, or a command
word that is itself an expansion) still fails.

The same script reader decides the pending `gh` exits below, so a wait written as
`sleep 270; cd . && gh pr checks 260 2>&1 | tail -12` is still read through the
`gh` command that produced the status.

A command's two streams are shown in order under one `[stderr]` marker, and only
when both have content: the marker is the boundary between them, so a command
that wrote nothing to stdout — a failing script whose whole result is its
traceback — is shown without one.

`gh pr checks` exits 8 while its checks are still running, which is a documented
state rather than a failure: no check has reported a failure yet (that is its
exit 1). A `gh pr checks` run — on its own, piped through text filters, or the
last command of a `sleep …; cd … &&` script — that
exits 8 keeps `[exit: 8]`, adds `pull request checks are still pending, not
failed`, and leaves the tool's error flag clear, so the transcript shows a result
instead of a red tool call while CI runs. `gh run view` asked for the logs of a
run that has not finished exits 1 with `is still in progress; logs will be
available`; that is a result too, marked `the run is still in progress, not
failed`. On Unix the pipeline's own stage
statuses have to agree, so an unrelated failing command or a filter that exited 8
is still a failure; Windows has neither `pipefail` nor `PIPESTATUS`, so only an
unpiped run is read this way. Because such a read confirms no outcome, it does
not clear the pull-request entry in the run's Definition-of-Done tracking: the
reminder still asks for the checks to be read again until they report.

Corepack download failures point to installed package managers such as Volta or
local test executables before another download, a version-pin change, or a hook
bypass. Obsolete Jest flags, missing Nx targets and unsupported JSON fields
carry recovery hints that ask for the installed command or project configuration.
The agent saves verified, reusable environment fixes and test commands in memory
with their project and version context.

Shell failures caused by missing `nvm` or `timeout`, absent Git tracking refs,
unsupported grep repetition counts, unclosed quotes,
or selecting a package outside the current Cargo workspace carry a specific
recovery hint, as does a command whose request never completed over the network
— `gh`, `curl` and `git` report a TLS handshake timeout, a reset or a DNS
failure that way, and the hint asks for the same command again rather than for a
repository, authentication or syntax fix. A command that only reads over the
network is also retried once on its own before that hint is returned: `gh` read
commands (`api` without a method or field, `pr view`, `search`, and the like), a
GET-shaped `curl`, and `git ls-remote`, optionally piped through a text filter,
so a transport failure does not cost the run a step, while anything that could
change something is left alone. A standalone literal recursive `grep` that can
be represented by the search tool runs through its bounded, ignore-aware search
without a failed call and model retry. The result names the substitution, and
the requested path still passes the front-end's project boundary check. Shell
pipelines, redirections, expansions, multiple paths and unsupported options are
never dropped to make a call fit. The recursive-search guard refuses any other
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
as written. Simple unquoted globs such as `docs/*.md` and
`apps/*/package.json` also run when at least one file matches and every final
match is a regular file. A matched directory (including a symlink to one) keeps
the guard active. Relative globs are checked against the directory reached by
literal preceding `cd` commands; an unknown working directory keeps the guard.
Verification visits at most 4096 directory entries. Recursive `**` globs,
unsupported glob syntax and shell expansions remain guarded. Wildcard matching
uses an iterative scan to avoid exponential recursion on repeated `*` patterns.
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

`read` can return a reference to an unchanged earlier read instead of repeating
its contents. Set `fresh: true` to force the full result. Content hashes, read
ranges and retained context determine whether reuse is safe; mutable remote
queries are never cached. See [token efficiency settings](configuration.md#token-efficiency-and-run-budgets)
for selective stale-output shortening and active MCP schema limits.

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
