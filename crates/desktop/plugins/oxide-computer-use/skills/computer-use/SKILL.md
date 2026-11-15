---
name: computer-use
description: Inspect and operate desktop applications through Oxide Computer Use.
---

# Computer Use

Use this skill when a task depends on a graphical interface that files, shell
commands, or a structured integration cannot establish.

Discover the configured `oxide-computer-use` tools with `tool_search`. Check
`permissions` when an action fails, then inspect the target app with
`get_app_state` before acting. It returns an accessibility tree and the
app-window screenshot by default; set
`include_screenshot: false` for a text-only observation. Re-observe after every
action that changes the interface.

Keep the task scoped to the app and flow the user named. Use `list_apps` and
`app_policy` (with `action: "list"`) to respect application access decisions.
Observe the interface after actions; a successful tool call alone does not
establish that the desired state was reached.

The user controls Accessibility and Screen Recording in System Settings.
Report a missing permission from the actual MCP call and ask the user to grant
it to the hosting app. Do not try to approve operating-system permission
prompts on their behalf.

`type_text` and `press_key` accept `background: true`, which posts keyboard
events to one process without activating it; verify the app accepted them.
Pointer input still moves the real cursor. The `computer-use://screen` MCP
resource captures on read and announces when the screen may have changed; it
is not a continuous live stream.
