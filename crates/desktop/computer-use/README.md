# Oxide Computer Use helper (macOS development build)

This companion app gives the existing `computer-use` MCP runtime a separate
macOS app identity. The native menu-bar cursor provides permission diagnostics,
user-initiated permission requests, and **Stop computer use and quit**. Desktop
provides the per-call activity indicator, last screenshot, and **Stop task**.
These are separate controls: quitting the helper disconnects that MCP session;
Desktop's Stop task cancels the conversation's running turn.

## Build and connect

Requires Xcode Command Line Tools and a built macOS `computer-use` executable.
The runtime is supplied explicitly, copied into the bundle, and signed with the
helper. It is not downloaded or silently replaced. Build for the host CPU:

```sh
python3 crates/desktop/computer-use/build.py \
  --server "$HOME/.local/bin/computer-use" \
  --output "$PWD/crates/desktop/artifacts/Oxide Computer Use.app"
```

The destination must not exist. Default signing is ad-hoc for development.
`--identity 'Developer ID Application: …'` signs both executables with hardened
runtime and a timestamp; notarization and release/installer integration remain
separate work. The helper executable targets macOS 13; the app declares macOS 14 or newer to
match the runtime's documented requirement. A supplied runtime can require a
newer OS. Only the current host OS
and architecture have been exercised.

Move the app to its intended stable location before granting OS permissions.
Register its **proxy**, rather than launching its runtime directly:

```sh
oxide mcp add oxide-computer-use "/absolute/path/Oxide Computer Use.app/Contents/MacOS/oxide-computer-use" --mcp
```

Desktop's app-managed plugin already contributes an `oxide-computer-use` server
that runs the executable from `PATH`. This explicit global entry overrides that
plugin server with the helper proxy. Review the existing MCP configuration before
replacing it; use another `oxide-computer-use-*` server name for a side-by-side
trial. Reconnect the server or restart Oxide after changing configuration.
Existing permission and approval choices are not migrated or reset by the build.

Open the helper normally to use its permission menu. Only the user can grant
app control and screen capture. On macOS 27 and later these are under System
Settings → Privacy & Security → Device Control & Data Access and Screen &
System Audio Recording respectively; older systems call them Accessibility and
Screen Recording. Query `permissions` through the actual MCP connection
afterward. OS grants to Oxide.app do not grant this helper access. Rebuilding an
ad-hoc bundle can invalidate grants; release builds need a stable signing
identity. Setup remains manual in this development workflow.

## Transport and lifecycle

`--mcp` starts a stdio proxy, creates a private mode-0700 IPC directory, and
launches a fresh app instance through LaunchServices (`open -n -g`). The helper
connects through a Unix domain socket; both ends check the peer's Unix user.
The bundled runtime's stdin/stdout use that socket, preserving MCP JSON packets
and image data without modifying tool names or bypassing the shared approvals.
There is no network listener. Each MCP connection owns its helper and runtime.
The proxy transfers only `HOME`, `COMPUTER_USE_CONFIG` and the working directory
through a private context file so a custom/relative app policy is preserved; it
does not forward the caller's entire environment through LaunchServices.

The helper monitors its proxy every 250 ms. Proxy death stops the runtime;
normal stdin EOF lets the runtime shut down; the menu's stop command terminates
it, escalating to SIGKILL after one second. Already completed OS actions cannot
be undone by cancellation. The native icon describes a connected session;
Desktop's indicator describes individual active tool calls. A connected server
alone does not imply that a GUI action is in progress.

## Verification and limits

Run the native smoke check outside a sandbox that prohibits local sockets or
LaunchServices:

```sh
python3 crates/desktop/computer-use/check.py "/absolute/path/Oxide Computer Use.app"
```

It checks two independent MCP sessions, discovery, the runtime's observed app
identity, preservation of a custom relative deny policy, graceful EOF, abrupt
proxy death and process cleanup. It requires a running windowed app for the read-only policy guard check. It
never requests OS grants or sends input to another app. Close other instances of the tested
bundle first. The Desktop regressions are in `check-app.mjs` and `check-shell.mjs`.

This is not complete Codex parity. Pointer input still moves the real cursor;
keyboard input can use `background: true` to post to one process without
activating it. `get_app_state` returns the current app-window screenshot by
default, while `computer-use://screen` is pulled by the client rather than
streamed on a timer. Oxide Desktop's Computer Use dialog provides per-app
approval controls and the last captured screenshot; a system-wide cursor
overlay, automatic unlock/display covering, and a built-in browser remain
client features. Native permission prompts, actual capture/input,
helper-crash recovery, install/update paths and notarization still need
end-to-end validation. See [the parity review](../../../docs/oxide-reins-parity.md).

## Electron Desktop live view

While a Computer Use tool is running, Electron Desktop reads
`computer-use://screen` from the turn's own MCP session every 1.5 seconds and
shows it as the live preview, and draws a click-through overlay (an edge frame
on every display and a ring on the pointer) that is excluded from screen
capture. Once a turn starts using Computer Use, both stay active through the
waits between actions and end when that turn ends. Escape cancels the active
Computer Use turn, matching Codex's task-level shortcut. Browser integration
and unlock/relock remain outside Desktop; they belong to the runtime.
