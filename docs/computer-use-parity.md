# Computer Use parity review — 2026-11-08

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
| Preview beside chat | Last returned screenshot in the details dock, plus a dialog available in compact windows. Text-only calls preserve the image and its original app label. | Snapshots are explicitly labeled. Continuous capture/streaming is absent. |
| Independent chats | Per-thread activity, snapshot, stop state; background updates cannot paint into the visible chat. Covered by Desktop behavior checks. | Native runtime uses the shared foreground desktop. Separate helper instances do not isolate input. |
| Stop / takeover | Desktop cancels the visible turn through existing `cancel_run`; helper menu terminates its MCP runtime. EOF and proxy-death cleanup tested. | No cursor takeover arbitration. Already completed clicks/writes cannot be undone. |
| Separate native app | `dev.oxide.computer-use`, native menu-bar cursor, LaunchServices launch, private local IPC, bundled MCP runtime. Native handshake and observed helper ancestry tested. | Opt-in development build, not installed automatically or included in the release installer. |
| OS permissions | Native permission diagnostics and user-initiated request menu. MCP reports the helper as its host with both grants missing, while the existing Oxide session reported its own grants. | User must grant helper access. Actual System Settings entries, granted capture and input need live validation. |
| Per-app approvals and settings | Existing shared Oxide tool approvals plus the server's allow/deny policy remain in force. | Codex-style per-app Allow / Always allow / revoke settings and app identity validation are not implemented. |
| Background macOS use | Existing AXPress/AXValue paths can avoid foreground input for supported controls. | Keyboard/pointer still activate apps. True background input requires native runtime work; packaging cannot provide it. |
| Signed distribution | Build script can sign the helper and explicit runtime with a supplied identity. Ad-hoc bundle built and signature verified locally. | Notarization, pinned runtime provenance/version, CI architectures, installer/update integration, stable identity through updates. |
| Browser, app selection and lock behavior | Existing MCP discovery and native app tools. | Built-in browser, native `@App` selection/settings, automatic unlock/cover displays/local-input relock are not implemented. |
| Platform coverage | This helper is macOS-only; Desktop controls are platform-neutral. | Windows native computer-use runtime and foreground behavior are not validated or supplied by this helper. |

The documented Codex policy also restricts terminal/self automation and OS
security approvals. The current server's configurable policy is different;
these policies must be specified and tested rather than described as equivalent.
OS permissions, app approvals, and action-specific approval are distinct layers.

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
