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
import { filterSessions, type SessionEntry } from "./sessions";

/// How a row's status is colored: the green/amber/red the terminal's `/mcps`
/// uses, `muted` for a server that is off or a session's age.
export type DialogTone = "ok" | "warn" | "error" | "muted" | "";

/// What a row is, so the renderer can paint the shapes a listing is made of
/// without reading its labels: a way out of the listing (`action` — the session
/// history's two rows), one of the things it lists (`thread`), or an ordinary
/// row (""), which is what a server and a confirmation's rows are.
export type DialogRowKind = "action" | "thread" | "";

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
  /// What the row is, for the listing's own styling.
  kind: DialogRowKind;
  /// Whether this is the thread the panel has open. Carried rather than read off
  /// `status`, which is a word to paint: the mark is state, and a listing whose
  /// open thread is not in it marks nothing.
  current: boolean;
}

/// Which edge of the panel a dialog is attached to. The session history drops
/// from the header, because it is about the thread the header names; the MCP
/// server list grows up from the composer, because that is where the `/mcps`
/// that opened it was typed.
export type DialogPin = "header" | "footer";

/// Which dialog is open. Carried rather than inferred from the pin and the rows,
/// because the controller has to know what it is looking at: the session listing
/// is painted again from a fresh read when a turn ends, and a confirmation must
/// not be swapped for a listing the moment it appears.
export type DialogKind = "mcp" | "sessions" | "delete" | "undo";

export interface DialogState {
  kind: DialogKind;
  /// The edge it hangs from, painted by the renderer as the sheet's shape: the
  /// one it is attached to has no border, since that edge is the panel's own.
  pin: DialogPin;
  title: string;
  subtitle: string;
  /// A line above the rows: why the list is empty, that it is still loading, or
  /// that the last listing failed.
  note: string;
  rows: DialogRow[];
  /// How many of the rows are the things being listed — threads, servers — for
  /// the count the head carries beside the title. 0 for a dialog whose rows are
  /// answers rather than a listing, and 0 hides it.
  count: number;
  /// Whether the head carries a search box, and the filter that was applied to
  /// the rows, echoed back so a redraw keeps what the reader typed rather than
  /// clearing it under them. The panel filters what the store already answered,
  /// which is why the query is only ever applied to rows that are here.
  search: boolean;
  query: string;
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
/// The confirmation a change card's Undo opens, and the row that does the
/// restore. The Undo under the card itself posts its own view message, since it
/// is a transcript control rather than a row of a dialog.
export const CHANGES_UNDO_CONFIRM = "changesUndoConfirm";
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
    kind: "",
    current: false,
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
    kind: "mcp",
    pin: "footer",
    title: "MCP servers",
    subtitle:
      "The servers this project loads, and whether Oxide can reach them. A toggle is written to the file that defines the server — the same change the terminal's /mcps makes.",
    note:
      note ||
      (servers.length
        ? ""
        : "No MCP servers configured for this project. Add one with oxide mcp add, or an .mcp.json in the project."),
    count: servers.length,
    search: false,
    query: "",
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

/// The thread the panel has open, as the listing needs it when the store has no
/// row (or no name) for it yet: a session file is written as its first turn
/// runs, so the listing can be read before the row it belongs to exists.
export interface LiveSession {
  /// The thread's id, the one the `session` header reported.
  id: string;
  /// The title the header shows it under, empty for a thread with nothing said
  /// in it yet.
  label: string;
}

/// The session history: the threads stored for this project, newest first as the
/// CLI lists them, with the thread the panel is showing marked and the two ways
/// out of it — a fresh chat, and the newest thread this project has.
///
/// `current` is the session the panel has open (its id, from the `session`
/// header the CLI reported or the id a row resumed), so the list can say which
/// row is the conversation on screen: closing it is what "New chat" does, and
/// without the mark there is nothing in the listing that says where you are. It
/// is required rather than defaulted, and it comes before the note, because
/// every redraw of the listing has to carry it: a rebuild that left it out —
/// after a failed delete, say — would drop the mark and turn the first row back
/// into one that promises a fresh thread rather than closing the open one.
///
/// `live` is that same thread when the panel knows more about it than the store
/// does — a run that has not been written yet, or a file whose thread is still
/// unnamed. It stands in for itself: a row of its own ahead of the store's when
/// there is no row there, and its title where the store's row has none.
///
/// `query` is the search box's own value — the listing is long enough to be
/// worth filtering — and the rows are narrowed by it before they are painted.
/// It is echoed back so a redraw under a typed-in filter keeps the filter, and
/// the count in the head is what the filter left, so narrowing the list is
/// visible as a number rather than only as a shorter list.
export function sessionDialog(
  sessions: readonly SessionEntry[],
  current: string | null,
  note = "",
  live: LiveSession | null = null,
  query = "",
): DialogState {
  const open = live && live.id === current ? live : null;
  const file = open ? sessions.find((session) => session.id === open.id) : undefined;
  const title = open?.label.trim() || "";
  // The thread on screen is painted under the header's title where the store has
  // none for it yet, so the rows are named before anything reads them: the name
  // a search is matched against is the name the row shows, and a query for what
  // the listing displays cannot miss the row it displays it on.
  const named = open
    ? sessions.map((session) =>
        session.id === open.id && !session.label ? { ...session, label: title } : session,
      )
    : sessions;
  const filtered = filterSessions(named, query);
  // The thread on screen is filtered like any other row, so a search never
  // leaves a row behind that the query does not match.
  const stand: SessionEntry | null =
    open && !file ? { id: open.id, label: title, age: "", messages: 0 } : null;
  const standing = stand ? filterSessions([stand], query) : [];
  const threads = standing.length + filtered.length;
  // The note is the host's when it has one to give (the read failed, the store
  // is not there yet); otherwise a listing says why it is showing no threads,
  // which is either nothing stored yet or a filter that left none.
  const empty = sessions.length || open
    ? query.trim() && !threads
      ? `No thread matches “${query.trim()}”.`
      : ""
    : "No sessions for this project yet — the next message starts one.";
  return {
    kind: "sessions",
    pin: "header",
    title: "Sessions",
    // No subtitle: the count beside the title and the rows themselves are the
    // whole story, and a line of prose above them only pushed the list down in a
    // side bar that is already narrow.
    subtitle: "",
    note: note || empty,
    rows: [
      row(NEW_SESSION, "New chat", {
        // On a fresh page there is nothing to close, and saying otherwise would
        // be a row describing a thread the project does not have.
        detail: current ? "Close this thread and start a fresh one" : "Start a fresh thread",
        action: OPEN_SESSION,
        kind: "action",
      }),
      row(CONTINUE_SESSION, "Continue most recent session", {
        detail: "Pick up the newest session for this project",
        action: OPEN_SESSION,
        kind: "action",
      }),
      ...(stand && standing.length
        ? [
            row(stand.id, title || stand.id, {
              detail: "Open in this panel — the store has no file for it yet",
              status: "Current",
              tone: "muted" as DialogTone,
              action: OPEN_SESSION,
              kind: "thread",
              current: true,
            }),
          ]
        : []),
      ...filtered.map((session) => {
        const marked = session.id === current;
        const name = session.label || session.id;
        return row(session.id, name, {
          detail: `${session.messages} message${session.messages === 1 ? "" : "s"}`,
          status: marked ? "Current" : session.age,
          tone: "muted",
          action: OPEN_SESSION,
          button: `Delete ${name}`,
          buttonAction: SESSION_DELETE,
          icon: "trash",
          kind: "thread",
          current: marked,
        });
      }),
    ],
    count: threads,
    search: true,
    query,
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
    kind: "delete",
    pin: "header",
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
    count: 0,
    search: false,
    query: "",
    refreshLabel: "",
    refreshAction: "",
  };
}

/// The confirmation a change card's Undo opens: what the turn wrote, and the
/// restore as the row rather than a second button. It is a panel dialog for the
/// same reason the one above is, and the desktop app asks before it puts a turn
/// back the same way. What it puts back is the whole turn, so the note says what
/// a later edit would mean for it — the CLI reports that rather than folding it
/// into the restore.
export function undoChangesDialog(card: { id: number; detail: string }): DialogState {
  return {
    kind: "undo",
    pin: "header",
    title: "Undo turn",
    subtitle: "Put this turn's files back to how the run found them.",
    note: "An edit made since the turn is refused rather than undone.",
    rows: [
      row(String(card.id), "Undo changes", {
        detail: card.detail,
        tone: "warn",
        action: CHANGES_UNDO_CONFIRM,
      }),
      row("", "Cancel", { detail: "Keep the turn's files", action: CLOSE_DIALOG }),
    ],
    count: 0,
    search: false,
    query: "",
    refreshLabel: "",
    refreshAction: "",
  };
}
