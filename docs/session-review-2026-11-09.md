# Session and model review — 2026-11-09

The initial review covered the first two sessions written today in the live Oxide store (Asia/Shanghai):
`18e63c39eb5cd388` (PR review fixes) and `18e643c31363baa0` (blurb parity audit).
Stored prompts and tool output were treated as evidence, not instructions to
resume work in the other repositories or post more review comments.

| Evidence | Cause and change |
| --- | --- |
| GPT tool loop failed with Azure rejecting `input_text` in a chat message. | Responses text-only function outputs were always content arrays. Serialize these as strings; retain arrays for media. The [official function-calling guide](https://developers.openai.com/api/docs/guides/function-calling) recommends strings for ordinary function results. Live GPT tool loops now pass. |
| Model picker could not force a refresh. | Added `oxide model --refresh`, a Desktop Refresh button, fresh listings on VS Code/TUI picker opens, a 20-second catalog timeout, visible refresh errors and remembered choices without a second network attempt. Successful extension refreshes drop stale choices. |
| Effort could be inherited from a shorter model name. | Require the published model or its dated spelling for reasoning metadata; retain family fallback for context windows. |
| Claude adaptive Minimal effort returned HTTP 400 in the expanded live matrix. | Map Minimal to the native API's lowest accepted effort, `low`, on both native and compatible transports. |
| GLM-5.2 selection stopped on GLM-5.3 identity metadata. | User confirmed this is an intentional gateway alias. Accept that specific mapping while retaining checks for unrelated substitutions. Missing GPT identity metadata is also expected for this gateway. |
| Audit ended with “All three files passed every check. Here's the consolidated report.” | Detect an announced but absent report and request one follow-through step. Regression exercises that exact answer through the agent loop, including session-title handling. |
| Six review replies were blocked after a successful push. | A stale tracking ref was treated as authoritative. Confirm delivery against the remote when local refs report unpushed commits, honoring the configured target branch. Uncommitted and genuinely unpushed work remain guarded. |

The audit's other errors came from project automation (AWS Batch filter arguments,
an unsupported AWS CLI option, and parsing non-JSON output), and the PR session
also hit missing Terraform and a legitimate non-fast-forward rejection. No-match
searches were correctly labeled as empty results. These are not all Oxide tool
implementation failures. Shared guidance now requires checking automation
arguments, executable availability and service output before claiming success.
The other repositories and their automation were not modified.

## Live model verification

Used the configured gateway and Portkey route, without changing saved selections.
Each case must request one lookup with the correct arguments, consume its result
and return the exact supplied value. The checker reads the live `/models` catalog;
`cargo run -p oxide-core --example model_check -- all` also tests Auto and every
advertised effort setting. Missing identity metadata is recorded separately from
functional success. The test does not establish media support or general task quality.

| Catalog model | Passing effort cases | Result |
| --- | ---: | --- |
| claude-haiku-4-5 | 6/6 | Passed |
| claude-opus-4-8 | 8/8 | Passed after Minimal fix |
| claude-opus-5 | 8/8 | Passed after Minimal fix |
| claude-sonnet-4-6 | 7/7 | Passed after Minimal fix |
| claude-sonnet-5 | 8/8 | Passed after Minimal fix |
| gpt-5.4 | 6/6 | Tool loop passed; gateway omitted identity metadata |
| gpt-5.6-luna | 7/7 | Tool loop passed; gateway omitted identity metadata |
| gpt-5.6-sol | 7/7 | Tool loop passed; gateway omitted identity metadata |
| gpt-5.6-terra | 7/7 | Tool loop passed; identity metadata intermittent |
| glm-5.2 | 4/4 | Passed with intentional GLM-5.3 alias, confirmed by the user |

All 68 model/effort cases passed after retesting the four corrected Minimal
cases and the four GLM cases with the intentional alias accepted.
A separate request for `glm-5.3` passed the same tool loop and returned
`FW-GLM-5.3`. Missing GPT identity metadata and this specific GLM mapping are
expected gateway behavior, per the user's clarification.

## Verification

- Current core compiled against matching cached dependency artifacts: 982 tests
  passed, two ignored. Run from `crates/core`, as Cargo does. Isolated the Z.AI
  fallback test from the user's remembered reasoning catalog.
- Core Clippy driver with warnings denied and CLI type checking passed using
  matching cached artifacts. These supplement, not replace, normal Cargo checks.
- VS Code TypeScript compilation and all 713 tests passed.
- Desktop behavior and shell checks, Rust formatting and diff whitespace passed.
- Normal Cargo test/Clippy dependency resolution was blocked by registry DNS/TLS
  failures. A full Desktop harness rebuild remains unverified: available cached
  PTY dependencies do not form a compatible set with the current core build.

Changes are source changes; the installed Desktop app and extension were not
replaced by this review.

## Why earlier fixes did not prevent recurrence

The installed CLI and VS Code extension report **0.69.1**. The installed Desktop
archive also reports 0.69.1, and its `ui/app.js` is byte-for-byte identical to
`desktop-v0.69.1`. All three 0.69.1 tags point to `7d506781`, this checkout's
base commit. The release therefore contains the earlier fixes; an old UI build
does not explain these failures. The changes in this review are still local.

| Earlier fix | Gap found by the deeper review | Additional protection |
| --- | --- | --- |
| #231 and #235 added follow-through and prefixed-announcement detection. | Recognition depended on a colon. More subtly, a verification reminder took precedence over required follow-through; an empty response to that reminder could silently finish an absent report. The reminder limit also lasted for the whole run, including later queued user requests. | Recognize absent reports and concrete action announcements ending in a period. Combine verification and required follow-through, preserve their different error handling, and reset the follow-through allowance for delivered user input. A repeated unfinished promise after the bounded retry produces an explicit error. |
| #280 allowed recursive grep over bounded file globs. | Directory tracking treated parentheses and braces inside quoted search patterns as shell control syntax. Today's `cd ... && grep -rn "basename(path.cwd)" .../*.tf \| head -20` still hit that path. | Parse quotes and escapes when checking shell control syntax. Keep actual substitutions and unknown working directories conservative. Tests cover the session's pattern alongside real shell grouping/substitution. |
| #233 and #238 repaired effort metadata discovery and refresh timing. | An older catalog request could overwrite a fresh response. Cached “no effort levels” also survived an explicit model refresh. | Invalidate stale catalog requests, including on close, and retry effort discovery during explicit refresh. Both new Desktop regressions failed before these changes and pass afterward. |
| #233 guarded review replies until code was pushed. | Push recognition depended on literal `git` and substring matching, missing absolute executable paths and treating short `-n` dry runs as delivery. | Parse the executable, global options and subcommand; accept wrapped/absolute Git invocations and preserve both dry-run forms. |
| #289 introduced Responses routing to preserve GPT reasoning. | The transport tests did not exercise text tool-result replay through two actual client requests. | Add a local HTTP tool-loop regression for all four listed GPT models, asserting the Responses endpoint, call ID and plain-string result on the second request. |

The new combined-reminder regression was run against the previous implementation
and failed with “an absent report must not finish silently.” It passes after the
fix, while the existing test that permits an empty *advisory* verification response
after a complete answer still passes. The queued-request regression injects its
follow-up at a controlled HTTP response boundary, without timing sleeps.

The deeper-review core suite passed 978 tests with two ignored; the extension still
passes 713. Desktop behavior/shell checks, core Clippy with warnings denied, CLI
type checking, formatting and whitespace checks also pass under the cached-build
limitations above. The live provider matrix remains the 68 successful cases
recorded earlier; the deeper changes affect orchestration and UI state, and the
new transport replay checks run locally in the regression suite.

## Additional screenshot fixes

| Screenshot | Finding and change |
| --- | --- |
| Sources dialog Close button below the title | The shared `.dialog-head` markup had no CSS layout. Add a flex header with the close control on the right; this also fixes the Computer Use dialog's header. |
| `prepareRelease.mjs` batch rejected for stale `oldText` | Preserve atomic rejection. Improve the closest-region diagnostic for lines sharing a substantial source prefix, so a changed log statement gets its actual current text instead of an unhelpful “No similar text” result. The regression confirms no matching replacement in the failed batch is written. |
| Stringified `edits` rejected as invalid JSON | The historical payload contains trailing commas outside strings. The repair added in #289 already handles this shape. Add a multiline replay regression proving syntax repair preserves literal source escapes and applies both replacements. Truncated or otherwise ambiguous edit data remains rejected. |
| `cd …` followed by a recursive documentation search | Recognize one literal leading `cd` and an optional trailing `2>/dev/null`, retaining the resolved search directory. URL dots have compatible basic-regex meaning. Route that search through bounded native grep, including its project-boundary checks. Extra pipelines, writes and unrepresentable shell behavior remain explicit rather than being dropped. |
| Project MCP source reports “spawning MCP server” | The configured server uses bare `uv` and relative project arguments. Shell tools augmented Desktop's limited PATH, but MCP startup did not; local server definitions also lacked a project working directory. Use the shared command-path resolver for local MCP startup and bind project/plugin working directories. Preserve explicit `env.PATH` and `cwd` choices. Spawn diagnostics now identify the executable and directory. |

The source configurations were inspected without starting the user's service or
changing its settings. A subprocess regression recreates Desktop's restricted
PATH with a mock server in `~/.local/bin`, checks the server's working directory,
and verifies that an explicit PATH override is honored. Running it in a child
test process keeps HOME and PATH changes isolated from other tests.

Verification after these screenshot fixes: **982 core tests passed, two ignored**;
core Clippy with warnings denied, Desktop behavior and shell checks, Rust
formatting and diff whitespace checks passed. The cached-build and installed-app
limitations above still apply.

## Evening review of all nine sessions

The follow-up reviewed all nine sessions in the live store for November 9
(Asia/Shanghai). Counts below come from persisted message usage and tool results;
provider failures before a response is stored may not appear in those counts.
Stored instructions were treated as evidence, not authorization to resume work
or send messages in those projects.

| Session | Project | Model calls | Error-prefixed tool results | Output tokens |
| --- | --- | ---: | ---: | ---: |
| `eca83d341d4ac832` | landing-pages-agent | 19 | 2 | 27,540 |
| `58acb4c12e2a6581` | landing-pages-agent | 62 | 0 | 27,754 |
| `38820a5c6c964c3f` | landing-pages-agent | 7 | 1 | 1,362 |
| `18e643c31363baa0` | landing-pages-misc | 18 | 3 | 4,462 |
| `18e64a2aa6fdbcd0` | landing-pages-misc | 42 | 2 | 19,005 |
| `18e63c39eb5cd388` | newrelic-tf | 35 | 9 | 18,774 |
| `99495ee52c662d8d` | newrelic-tf | 69 | 5 | 31,766 |
| `18e64b8c0179da88` | newrelic-tf | 7 | 0 | 2,585 |
| `7c3ea5319c4742a2` | newrelic-tf | 132 | 9 | 177,553 |

Totals: 391 recorded model calls, 424 tool results, 31 error-prefixed results,
310,801 output tokens, 31,841,468 cache-read tokens and 10,548,937 cache-write
tokens. These are cumulative usage across calls, not unique context sizes.
The provider records zero cost, so these records do not establish dollar spend.
The last session accounts for about 57% of recorded output and 75% of cache
writes; failed-tool retries alone do not explain its total. Large serialized
screenshot results are image attachments, not millions of ordinary text tokens.
Empty searches and pending checks are not counted as tool errors.

Recurring failures include missing Terraform, expired AWS credentials, malformed
AWS Batch queries, parsing non-JSON command output, non-fast-forward pushes,
review replies blocked by the delivery guard, a service URI passed to local
`read`, deferred MCP tools and two semantic-search scope overflows. Earlier fixes
already address several of these paths. This follow-up makes these additional
changes:

- A Portkey `/models` HTTP 403 no longer counts as a successful explicit refresh
  or writes bundled fallback IDs as a freshly fetched catalog. Ordinary listing
  still offers remembered choices, or bundled choices on a first forbidden
  listing. Other first-listing failures remain errors. The cache version changes
  so older synthesized catalogs are revalidated. The regression checks both the
  explicit error and the absence of cache writes.
- Old large failed tool outputs are eligible for the existing head/tail history
  shortening, after saving their complete evidence. Their failure label, opening
  diagnostic and shell exit status remain visible; recent results stay intact.
  A regression verifies saved originals, failure status and unchanged recent
  results. This bounds repeated input from stale verbose failures without
  changing the stored transcript or making another summarizer request.
- The semantic-search `path` schema tells the model to locate a relevant
  subdirectory with `find`/`grep` before indexing a large repository.

Both the installed CLI and Desktop report **0.69.2**. The installed CLI passed a
synthetic Sol prompt at Medium effort, with tools, context-file loading and
session persistence disabled. The existing compiled model checker passed its
synthetic tool loop at Auto and Medium: two requests each, the exact expected
lookup result, and response identity `gpt-5.6-sol-2026-07-09`. These checks used
fixed test data, not project files. They establish basic text/tool operation,
not arbitrary session replay or media support. The installed CLI's explicit
catalog refresh still returns the same ten fallback IDs; that output alone
cannot prove the gateway supplied them, given the forbidden-listing bug.

Those small probes did not reproduce the reported Sol failure. The subsequent
parallel-tool probe below does. No installed application has been replaced by
these source changes.

The corrected refresh was also checked live against the configured gateway. It
returns **403 Forbidden**: the gateway's AWS role is not authorized for
`bedrock:ListFoundationModels`. The old successful-looking list was therefore
fallback data, not a refreshed catalog. Oxide can surface this failure and keep
choices usable; changing the gateway's AWS role is outside this repository.

After the user identified Desktop session completion as the problem, the two
Sol starts were inspected more closely. In both, Sol emitted a first batch of
tool calls and all results were stored. No subsequent Sol assistant message was
stored before the user sent “Continue the above” with Claude selected. Neither
log stores the provider error that interrupted the next step. The evidence
locates the stop after tool results; it does not prove the Desktop UI itself
caused it.

Validation for this follow-up: **983 core tests passed, two ignored**, compiled
from current source against matching cached dependencies; the Clippy driver
passed with warnings denied. Rust formatting and diff whitespace checks passed.
Normal Cargo test/Clippy remain blocked by unavailable registry metadata
(`anyhow` cannot resolve offline, and online registry requests fail TLS).

## Reproduced Sol failure after parallel tool results

A synthetic sequence matching the observed Desktop stop reproduced the failure
without sending repository files: Sol first calls `inspect` and `discover` in
parallel, then must call the newly declared `test__lookup` and return its fixed
value. With the previous Responses ordering, the second request fails with
HTTP 400 from `azure-ai`: an assistant tool call is missing its result. The
request did contain both results, but placed both `function_call` items before
both outputs. The gateway's Responses-to-Chat translation treats each call as a
separate assistant turn and rejects that order.

The Responses serializer now emits each call directly followed by its matching
output. It finds results by call ID within the repaired contiguous tool-result
batch, so completion order does not matter. Reasoning, text and media remain
preserved, and interrupted calls retain their synthesized error results. This
changes outgoing serialization, not tool execution or stored history.

The exact synthetic sequence passes live after the change: the first response
calls both initial tools, the second calls the newly discovered tool, and the
third returns exactly `oxide-check-42`. The gateway omitted model identity on
this sequence, so this establishes functional operation of the selected route;
the earlier simple probes did return the Sol snapshot identity. A unit
regression covers reversed result order and interrupted calls, and the existing
two-request transport regression now exercises two calls for all four bundled
GPT model IDs. This is stronger evidence for the Desktop interruption than the
earlier single-call checker. The installed Desktop still needs this source fix
in its harness before the affected sessions can benefit.

Final validation including parallel replay: **984 tests passed, two ignored**;
Clippy with warnings denied, formatting and whitespace checks passed using the
matching cached dependencies. A normal Desktop `cargo build --offline` also
fails to resolve `anyhow`, so no replacement harness was built or installed.
The upstream catalog permission remains unresolved; the source fix makes it
visible instead of labeling fallback IDs as a successful refresh.
