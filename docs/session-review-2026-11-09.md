# Session and model review — 2026-11-09

Reviewed both sessions written today in the live Oxide store (Asia/Shanghai):
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
