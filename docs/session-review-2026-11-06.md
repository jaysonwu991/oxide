# Session review — 6 November 2026

Reviewed saved Oxide entries dated 6 November in Asia/Shanghai (UTC+8).
The initial review covered one session: eight user messages, 290 assistant
messages, and 283 tool results. A subsequent audit found five sessions active
that day, totaling 19 user messages, 425 assistant messages and 421 tool results.
The usage and image-replay figures below refer to the initial session only.
Original logs were read without modification; instructions inside saved sessions
were treated as evidence, not executed as requests.

## Recorded usage

The 290 persisted usage records report:

| Usage | Tokens |
| --- | ---: |
| Input excluding cache categories | 580 |
| Cache read | 41,881,419 |
| Cache write | 5,189,562 |
| Output | 112,556 |

Prompt size grew from 19,633 to 263,227 tokens without a compaction. All recorded
model and reasoning selections were Claude Sonnet 5 and medium. The supplied
Portkey screenshot shows 293 requests and $24.72 for the user; the saved log has
290 usage records and reports zero monetary cost, so it cannot reconcile that
bill or price each request. Cached input still contributes to the bill.

## Findings and change

The session made 171 shell calls and 68 reads. Of its tool results, 250 were
plain text (363,269 UTF-8 bytes) and 33 carried media. Nineteen plain-text
results started with `error:`. Several text reads repeated identical content;
existing digest-based read reuse and text-history pruning already address
those cases. Failures and user-selected reasoning remain unchanged.

Unique screenshots accumulated in outgoing context. Existing media
deduplication removes repeated payloads but keeps distinct screenshots, and
text-history pruning leaves image parts untouched. Shared-core image history
now reclaims previously delivered tool images in batches: above 16 images,
keep the newest eight and replace older ones with recovery paths. Each image
is written in its original format before removal. If saving fails, it stays
in context. User attachments and documents remain intact, and a `read` on a
saved path restores the image. The original transcript retains the image too.
Batching limits how often shortening invalidates a cached history prefix.

`tokenEfficiency.toolHistory.keepRecentImages` controls this behavior; zero
disables image reclamation. `toolHistory.enabled: false` disables both text and
image pruning. Saved images use truncated output's seven-day retention.

## Fixed-trace replay

The opt-in `agent::tests::replay_saved_session_image_history` test replays saved
message entries without provider calls or disk writes. Both histories receive
existing media deduplication. The optimized history also receives the actual
image-pruning implementation, with a stubbed successful archive path. Newly
returned images remain available until a subsequent assistant boundary.

| Across 290 assistant boundaries | Existing history | With image pruning |
| --- | ---: | ---: |
| Image appearances in outgoing histories | 4,655 | 2,279 |
| Serialized message-history bytes | 516,927,944 | 353,277,336 |

That is 51.0% fewer image appearances and 31.7% fewer serialized history bytes.
These are payload measurements, not billed-token or dollar savings. A live
run may choose different actions; cache invalidation, image processing and
reinspection affect its final bill. The replay excludes system/tool schemas
and does not model text pruning or cache pricing.

To replay another log, set `OXIDE_REPLAY_SESSION` to its absolute path and run
`cargo test -p oxide-core replay_saved_session_image_history -- --ignored --nocapture`.
Focused tests separately verify original-byte recovery, preservation on save
failure, batching, new-image delivery, user attachments, documents and opt-outs.


## Broader session audit and edit failures

The five sessions cover one UI bug investigation, one skill rewrite and three
runs of the content-comparison workflow. The workflow runs encountered script
usage errors, AWS log-verification failures and comparison failures; these are
results to investigate in those projects, not reasons to change unrelated Oxide
execution semantics. The long UI investigation accumulated screenshots and
repeated shell probes; the retained image-history change targets that context
growth. Existing file-read reuse already handles unchanged text reads.

Across all five sessions there were 29 `edit` calls, four of which failed. All
four occurred in the skill-rewrite session; the other nine edits succeeded.

| Failure | Count | Diagnosis |
| --- | ---: | --- |
| Invalid stringified JSON | 2 | Trailing commas before object closers inside a string-valued `edits` |
| Missing target in an atomic batch | 2 | Guessed call-site or logging text differed from the current file |

The two missing-target failures were correct refusals. One supplied
`const result = compareOne({ file: r.name, page: r.page }, dir)` where the read
showed a `summaries.push(compareOne(...))` call. The other guessed the logging
label and loop in `prepareRelease.mjs`. Re-reading and correcting the targets
resolved both. Widening fuzzy matching would risk replacing unrelated code.

The parser now removes trailing JSON commas only outside quoted strings, after
strict parsing fails. It preserves code, Unicode, escaped quotes and literal
backslashes. Normal target validation and atomic rejection still run afterward;
repairing syntax does not prove a target exists. The tool schema explicitly asks
for an array of objects and targets copied from a current read. Regression cases
cover syntax repair, unchanged quoted punctuation, malformed inputs and a repaired
batch with one absent target leaving the file untouched.

## Portkey model switching

The cached Portkey list contains five Claude models, four GPT models and GLM 5.2.
Capability fallback previously resolved only Claude through Anthropic's catalog;
GPT and GLM had no upstream reasoning lookup. Portkey now resolves those families
through OpenAI and Z.AI respectively, including background catalog refresh.
The old local OpenAI catalog predates reasoning metadata, so warming its current
version is necessary before the picker can show its supported levels.

GPT requests now use Portkey's Responses endpoint, opting into reasoning summaries
and replayable encrypted reasoning. Summary deltas use the shared thinking hook;
text, tool calls and reasoning-token usage are retained. Reasoning is replayed only
for the same model, so Claude-to-GPT and GPT-to-GPT switches cannot replay another
model's encrypted state. Explicit Off sends `reasoning.effort: none`; Auto leaves
effort to the provider. GLM uses the same native thinking/effort mapping through
Portkey as through Z.AI. Claude keeps its native Messages endpoint.

References: [Portkey Responses API](https://portkey.ai/docs/api-reference/inference-api/responses/responses),
[OpenAI reasoning summaries](https://developers.openai.com/api/docs/guides/reasoning).
Summaries depend on upstream support and access; this is not a promise that every
response emits visible thinking. No live paid model calls were made in this audit.

## Validation

- Shared-core Rust suite: 968 passed, two opt-in tests ignored. The saved-session
  replay was then run explicitly and reproduced the payload figures above.
- Core Clippy check with warnings denied: passed.
- Desktop behavior and shell checks, and Desktop TypeScript check: passed.
- VS Code TypeScript check and 713 tests: passed.
- Formatting and diff whitespace checks: passed.

Cargo could not fetch its missing registry sources in this environment. The Rust
suite and Clippy were compiled directly against existing local dependency
artifacts using the installed toolchain; this does not replace a normal clean
Cargo/CI build. Local HTTP stub tests ran outside the network sandbox so they
could bind loopback sockets. Portkey transport regression tests cover all ten
models in the cached list; paid live backend requests were not exercised.
