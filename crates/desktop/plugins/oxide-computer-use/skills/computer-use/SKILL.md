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

The user controls these grants in System Settings. On macOS 27 and later, app
control is under **Privacy & Security → Device Control & Data Access**, and
screen capture is under **Privacy & Security → Screen & System Audio Recording**.
Older macOS releases call these **Accessibility** and **Screen Recording**.
Report a missing permission from the actual MCP call and ask the user to enable
the hosting app. Do not try to approve operating-system permission prompts on
their behalf.

`type_text` and `press_key` accept `background: true`, which posts keyboard
events to one process without activating it; verify the app accepted them.
Prefer accessibility operations (`set_value`, `select_text`, `perform_secondary_action`
and a plain element `click`, which uses `AXPress` when available) because they
can leave the person's foreground app and keyboard focus alone. When a keyboard
action is required and the task should not take over the desktop, pass
`background: true` with the target `app`. Do not describe pointer input as
background: fallback clicks, coordinate clicks, scrolling and dragging move the
real cursor and bring the target forward.

The `computer-use://screen` MCP resource captures on read and announces when the
screen may have changed; it is not a continuous live stream.
