// The dialogs the panel paints itself: the MCP server list and the session
// history.
//
// Both are composed here as data — a title, a note and a list of rows — so the
// webview stays a dumb renderer and never decides what a click means: a row
// carries the action it posts back, exactly like a footer chip carries its
// control id. The listing itself comes from the CLI (`oxide mcp list --json`,
// `oxide sessions list`), so the rows here say what the terminal's `/mcps` and
// `/resume` say about the same project.
//
// Rendering them in the panel instead of a `showQuickPick` keeps the flow in
// Oxide's own UI: a native picker takes over the window, hides the transcript
// the listing is about, and cannot be answered while a turn streams.

import { mcpStateLabel, type McpServerView } from "./mcps";
import type { SessionEntry } from "./sessions";

/// How a row's status is colored: the green/amber/red the terminal's `/mcps`
/// uses, `muted` for a server that is off or a session's age.
export type DialogTone = "ok" | "warn" | "error" | "muted" | "";

/// The glyphs a row's own button can be painted with. The webview cannot load
/// VS Code's codicon font, so the few it needs are named here and inlined as
/// SVG by the renderer — the words the button carries stay in `button`.
export type DialogIcon = "power" | "trash" | "";

export interface DialogRow {
  /// What the row's action posts back — a server name, a session id.
  value: string;
  label: string;
  /// The dim second line: what the row is, and where it comes from.
  detail: string;
  /// A short trailing word (`Connected`, `2h ago`), painted in `tone`.
  status: string;
  tone: DialogTone;
  /// The action a click on the row itself posts, or `""` for a row that only
  /// carries a button.
  action: string;
  /// A button at the row's end: what it does, in words — its tooltip and its
  /// label for a screen reader, and its text when `icon` is empty.
  button: string;
  buttonAction: string;
  /// The glyph the button is painted as, so a narrow pane gets a switch rather
  /// than a word that would not fit.
  icon: DialogIcon;
}

export interface DialogState {
  title: string;
  subtitle: string;
  /// A line above the rows: why the list is empty, that it is still loading, or
  /// that the last listing failed.
  note: string;
  rows: DialogRow[];
  /// A trailing action beside Close (`Recheck`), and the action it posts.
  refreshLabel: string;
  refreshAction: string;
}

/// The actions a row posts back. The controller routes them (`dialogAction`),
/// so a rename here is a compile error there rather than a dead button.
export const MCP_TOGGLE = "mcpToggle";
export const MCP_REFRESH = "mcpRefresh";
export const OPEN_SESSION = "openSession";
/// The trash at the end of a session row: it opens the confirmation rather than
/// deleting, because a thread cannot be recovered once its file is gone.
export const SESSION_DELETE = "sessionDelete";
/// The confirmation's own row, which does the deletion.
export const SESSION_DELETE_CONFIRM = "sessionDeleteConfirm";
/// Close or Escape: the view dismisses it and tells the controller, so the next
/// pane to attach does not paint it again.
export const CLOSE_DIALOG = "dialogClose";

/// The values the session dialog's own two rows carry, so the controller can
/// tell a session id from "start over".
export const NEW_SESSION = "new";
export const CONTINUE_SESSION = "continue";

function row(
  value: string,
  label: string,
  rest: Partial<Omit<DialogRow, "value" | "label">> = {},
): DialogRow {
  return {
    value,
    label,
    detail: "",
    status: "",
    tone: "",
    action: "",
    button: "",
    buttonAction: "",
    icon: "",
    ...rest,
  };
}

const MCP_TONES: Record<string, DialogTone> = {
  connected: "ok",
  "needs-auth": "warn",
  "needs-trust": "warn",
  disabled: "muted",
  error: "error",
};

/// The `/mcps` dialog: every server this project loads, the state the core
/// probed, and a switch that turns one off or back on in the file that defines
/// it. `note` overrides the empty-list message, so a listing that failed can say
/// so in place where a QuickPick would just vanish.
export function mcpDialog(servers: readonly McpServerView[], note = ""): DialogState {
  return {
    title: "MCP servers",
    subtitle:
      "The servers this project loads, and whether Oxide can reach them. A toggle is written to the file that defines the server — the same change the terminal's /mcps makes.",
    note:
      note ||
      (servers.length
        ? ""
        : "No MCP servers configured for this project. Add one with oxide mcp add, or an .mcp.json in the project."),
    rows: servers.map((server) =>
      row(server.name, server.name, {
        detail: [server.transport, server.detail, `source: ${server.source}`]
          .filter(Boolean)
          .join(" · "),
        // A server that is switched off reports that rather than the state a
        // probe could not reach.
        status: server.enabled ? server.status || mcpStateLabel(server.state) : "Disabled",
        tone: MCP_TONES[server.enabled ? server.state : "disabled"] ?? "",
        button: server.enabled ? `Disable ${server.name}` : `Enable ${server.name}`,
        buttonAction: MCP_TOGGLE,
        icon: "power",
      }),
    ),
    refreshLabel: "Recheck",
    refreshAction: MCP_REFRESH,
  };
}

/// The session history: the threads stored for this project, newest first as the
/// CLI lists them, with the two ways to leave the current one.
export function sessionDialog(sessions: readonly SessionEntry[], note = ""): DialogState {
  return {
    title: "Sessions",
    subtitle:
      "Threads stored for this project. Resuming one continues from its stored context, the way --continue does in the terminal.",
    note:
      note ||
      (sessions.length ? "" : "No sessions for this project yet — the next message starts one."),
    rows: [
      row(NEW_SESSION, "New session", { detail: "Start a fresh thread", action: OPEN_SESSION }),
      row(CONTINUE_SESSION, "Continue most recent session", {
        detail: "Pick up the newest session for this project",
        action: OPEN_SESSION,
      }),
      ...sessions.map((session) =>
        row(session.id, session.label || session.id, {
          detail: `${session.messages} message${session.messages === 1 ? "" : "s"}`,
          status: session.age,
          tone: "muted",
          action: OPEN_SESSION,
          button: `Delete ${session.label || session.id}`,
          buttonAction: SESSION_DELETE,
          icon: "trash",
        }),
      ),
    ],
    refreshLabel: "",
    refreshAction: "",
  };
}

/// The confirmation a session row's trash opens: the thread is named, so the
/// dialog says what it would delete, and the deletion itself is the row rather
/// than a second button.
///
/// A panel dialog rather than `showWarningMessage({ modal: true }, …)`: the
/// listing stays readable behind it, and answering it does not hand the window
/// to VS Code's own chrome.
export function deleteSessionDialog(session: {
  id: string;
  label: string;
  detail: string;
}): DialogState {
  const label = session.label || session.id;
  return {
    title: "Delete thread",
    subtitle: `“${label}” and its stored conversation are removed from this project.`,
    note: "This cannot be undone. The file is deleted from Oxide's session store.",
    rows: [
      row(session.id, "Delete thread", {
        detail: session.detail,
        tone: "error",
        action: SESSION_DELETE_CONFIRM,
      }),
      row("", "Cancel", { detail: "Keep the thread", action: CLOSE_DIALOG }),
    ],
    refreshLabel: "",
    refreshAction: "",
  };
}
