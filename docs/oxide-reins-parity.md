# Computer Use parity review — the window, the helper and the runtime

Target: the user-visible behavior in the supplied screenshots and the
[official Computer Use documentation](https://learn.chatgpt.com/docs/computer-use),
checked on 2026-11-08. Codex's private implementation is not available here.
A matching app name or icon is not evidence of matching control behavior.

## Today's sessions (Asia/Shanghai)

Reviewed the root session dump and all sessions whose header date is November 8
in `~/Library/Application Support/Oxide/sessions`:

| Time | Session | Observed outcome |
| --- | --- | --- |
| 10:28 | `18e5f0e5fc7d68e8` (root dump) | Permission diagnosis and preview request; previously reviewed in `session-review-2026-11-08.md`. |
| 11:59 | `18e5f5da885dc8f0` | Tool searches returned unrelated MCP integrations; the assistant reported GUI control unavailable. Why the server was absent from these results is not established. |
| 12:00 | `18e5f5f01d158f18` | Actual MCP permissions report both grants for Oxide.app. AX calls locate the Create Publisher form. Four results retain image parts (two state calls and two explicit screenshots). Later AX-only results contain no fresh screenshot. |

The later session contains literal backslash-n text passed
to `type_text` while trying to navigate, retries, a truncated accessibility tree,
and a dismissed question. These are additional control/agent reliability gaps;
a persistent preview does not fix them. Do not equate an assistant's statement
that it found a form with verified form completion. The user's instruction to
leave submission to them remains part of the task.

The earlier review's statement that its single supplied dump contains no
successful screenshot remains true for that dump. It is not true of the two
later sessions now included in this review.

## Behavior and evidence

| Behavior | Current implementation / verification | Remaining work |
| --- | --- | --- |
| Distinct computer-use activity | Mouse glyph in tool cards and topbar; active styling only while computer tools run. | No global screen cursor overlay. |
| Preview beside chat | Last returned screenshot in the details dock, plus a dialog available in compact windows. Text-only calls preserve the image and its original app label. The runtime publishes `computer-use://screen`, which captures on read and reports `notifications/resources/updated` after any call that could have changed the screen, so a pane can stay current without a tool call. | Snapshots are explicitly labeled, the resource is pulled rather than pushed on a timer, and the window does not use it yet. |
| Independent chats | Per-thread activity, snapshot, stop state; background updates cannot paint into the visible chat. Covered by Desktop behavior checks. | Separate helper instances do not isolate input: they share one desktop, and only `background` keyboard delivery avoids taking focus. |
| Stop / takeover | Desktop cancels the visible turn through existing `cancel_run`; helper menu terminates its MCP runtime. EOF and proxy-death cleanup tested. The runtime now yields to a person's keyboard or pointer input during a paced drag or a long `type_text`, releases what it holds and reports `user takeover`. | A synchronous AX call or a single click is not arbitrated, and the window has no separate control for it. Already completed clicks/writes cannot be undone. |
| Separate native app | `dev.oxide.computer-use`, native menu-bar cursor, LaunchServices launch, private local IPC, bundled MCP runtime. Native handshake and observed helper ancestry tested. | Opt-in development build, not installed automatically or included in the release installer. |
| OS permissions | Native permission diagnostics and user-initiated request menu. MCP reports the helper as its host with both grants missing, while the existing Oxide session reported its own grants. | User must grant helper access. Actual System Settings entries, granted capture and input need live validation. |
| Per-app approvals and settings | The runtime decides per app — allow, deny, approve one call, or a default for an app nobody decided about — and pins an approval to the app's bundle identity, which a changed identity fails. Decisions are re-read on every call, so one made in a terminal reaches a running server. The hosting bundle is refused by default (`apps self-automation allow` lifts it). Shared Oxide tool approvals remain a separate gate. | No settings UI in the window: decisions are made with `oxide-reins apps allow\|grant-once\|deny\|revoke\|policy\|self-automation`. |
| Background macOS use | Existing AXPress/AXValue paths avoid foreground input for supported controls, and `type_text`/`press_key` with `background: true` deliver keys to one process through `CGEventPostToPid` without activating it. | Pointer events still move the real cursor, and an app that keeps its own key focus can ignore delivered keys — the reply asks for verification. Whether each app honours this is unverified. |
| Signed distribution | Build script can sign the helper and explicit runtime with a supplied identity. Ad-hoc bundle built and signature verified locally. The runtime writes an install marker beside its binary and `doctor --json` reports that marker, its build, its target and whether it is a debug or release binary; its CI builds both Mac architectures. | Notarization, an installer/update path, and a stable identity through updates. |
| Browser, app selection and lock behavior | Existing MCP discovery and native app tools. | Built-in browser, native `@App` selection/settings, automatic unlock/cover displays/local-input relock are not implemented. |
| Platform coverage | This helper is macOS-only; Desktop controls are platform-neutral. | Windows native computer-use runtime and foreground behavior are not validated or supplied by this helper. |

The documented Codex policy also restricts terminal/self automation and OS
security approvals. The runtime now refuses both by default — a terminal by
bundle identity over nine known emulators, and the hosting bundle — each with the
command that lifts it (`apps terminals allow`, `apps self-automation allow`).
OS security approvals are untouched: this runtime never answers a permission
dialog. OS permissions, app approvals, and action-specific approval remain
distinct layers, and Oxide's own tool approvals are a further, separate gate.

## Verification

Passed: Desktop behavior checks (including snapshot restoration, background
isolation, stop state, errors and keyboard dialog access), Desktop shell checks,
JavaScript syntax, Python syntax, Rust formatting, diff whitespace, native Swift
compilation for arm64/macOS 13+, ad-hoc code-signature verification, two concurrent
MCP connections, tool discovery, helper-host reporting, custom relative deny-policy
preservation, normal EOF and SIGKILL
proxy cleanup. Native smoke checks requested no grants and sent no GUI input.

Cargo tests and clippy could not resolve the uncached `anyhow` package offline.
Desktop TypeScript checks could not run: window dependencies are absent and the
pnpm bootstrap could not complete with the configured registry unavailable.
Native input/capture, permission dialogs, UI visual QA, other CPU/OS versions,
and release paths remain unverified. No claim of complete Codex parity is made.

Build/connect instructions and the repeatable read-only smoke check live in
[the helper guide](../crates/desktop/computer-use/README.md).

## Where the runtime side is recorded

The server's half of this list is tracked against the runtime that implements it,
in oxide-reins PRs [#3](https://github.com/jaysonwu991/oxide-reins/pull/3) and
[#4](https://github.com/jaysonwu991/oxide-reins/pull/4) and the `docs/parity.md`
they add: per follow-up, what the runtime now does, what belongs to the client,
and what is still unverified. The rows above that name the runtime follow that
record.

The runtime also gained `oxide-reins verify`, which exercises each native path
behind these rows — the accessibility tree, a capture, a bundle identity, the
takeover tap, background typing — and reports `ok`, `MISSING` or `skipped` with
the evidence, exiting nonzero on a gap. Every "unverified" above is one command
away from a report instead of a reading exercise.

That work is verified in its own repository: `cargo test`, `cargo fmt --check`,
`cargo clippy --all-targets -- -D warnings` and a build of the non-macOS stub.
A granted run is still not part of it — the machine that implemented it has
neither Accessibility nor Screen Recording, so a live bundle identity, granted
capture and input, the takeover callback and visual QA are compiled and
unit-tested only. The same gaps listed under Verification above apply to the
window, and none of this is a claim of complete Codex parity.
