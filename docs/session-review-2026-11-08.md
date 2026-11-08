# Computer-Use session review — 2026-11-08

Reviewed the root `sessions/593979a6232cf3be/1794104897151_18e5f0e5fc7d68e8.jsonl`
(the sole JSONL file). It records a publisher-form task in Oxide Desktop with
`deepseek-flash`, followed by requests to use Computer-Use and see a preview.

| Evidence | Cause | Change |
| --- | --- | --- |
| The initial answer declared the signed-in form inaccessible before discovering GUI tools. | Service routing guidance omitted GUI fallback. | Shared prompt now discovers browser/Computer-Use tools for forms and native apps, while preserving user submission constraints. |
| `get_app_state` failed with missing Accessibility; `doctor` reported missing Screen Recording too. | Diagnostics told a Desktop user to grant a terminal. | Local server adds a read-only `permissions` tool in the MCP process, reports the observed ancestor app bundle, and explains which permission enables which capability. |
| User expected a preview. | Core MCP conversion discarded image payloads; tool-result events had no media; stored tool cards ignored attachments. | Preserve MCP images for the model and session, emit image attachments, and render clickable previews in live and stored Desktop cards. |
| Local code inspection found right-click on an AXPress element invoked its primary action. | The AXPress fast path ignored the requested button. | Restrict AXPress to left-click; right-click uses the context-button pointer path. |

The supplied session reached permission diagnosis, not successful form completion.
It contains no successful screenshot or browser interaction to replay as a native
end-to-end test. This review does not treat its assistant's claims about macOS
TCC or signing as authoritative OS documentation.

[Official OpenAI Computer Use documentation](https://learn.chatgpt.com/docs/computer-use)
describes GUI tasks, separate OS and app approvals, and macOS background use.
The local MCP server remains a different implementation: AX actions can work
without foreground input, but its keyboard and pointer tools activate apps.
Tool-result screenshot previews are not a live stream. Full product parity
would additionally require isolated input, a native setup/settings experience,
and app-scoped approval UX. The current generic MCP approval store and local
server policy remain in force.

Verification: Desktop behavior and shell checks, Rust formatting and diff
whitespace checks passed. The shared core and its test code type-checked using
matching cached compiler artifacts. A test binary built directly with those
artifacts passed five focused regressions: MCP image preservation, stdio MCP
image dispatch into ToolOutput, JSON event attachments, authenticated routing,
and structured tool errors. Cargo tests and clippy could not resolve uncached
registry dependencies; crates.io failed with DNS errors in the sandbox and TLS
errors on the escalated fetch. The sibling server's Rust tests and native input
paths remain unverified; a live smoke test requires user-granted OS permissions.

## Later sessions and native-helper follow-up

Two further sessions from the live Oxide store were reviewed later on November 8.
The 12:00 session reports both permissions granted to Oxide.app and retains four
screenshot-bearing tool results. The earlier conclusions above describe only
the original 10:28 root dump. The new Desktop controls keep the latest screenshot
beside the conversation and indicate computer-use calls with a mouse glyph.
An opt-in native helper provides a separate app bundle and permission setup menu.
See [the expanded review and parity matrix](computer-use-parity.md) for evidence,
verification and the substantial behaviors still needed for Codex parity.
