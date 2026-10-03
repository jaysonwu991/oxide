// The dialogs the panel paints itself: settings, the MCP server list and the
// session history.
//
// They are composed here as data — a title, a note and a list of rows — so the
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
import { updateVsix, type UpdateCheck } from "./updates";

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

/// Which end of the panel a dialog card sits near. Session history belongs near
/// the header it describes; settings and MCP servers belong near the composer
/// where they are opened.
export type DialogPin = "header" | "footer";

/// Which dialog is open. Carried rather than inferred from the pin and the rows,
/// because the controller has to know what it is looking at: the session listing
/// is painted again from a fresh read when a turn ends, and a confirmation must
/// not be swapped for a listing the moment it appears.
export type DialogKind =
  | "update"
  | "model"
  | "agent"
  | "reasoning"
  | "trust"
  | "mcp"
  | "sessions"
  | "delete"
  | "undo";

export interface DialogState {
  kind: DialogKind;
  /// The end of the panel that positions the complete standalone card.
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
  /// The prompt inside a searchable/custom-value listing.
  searchPlaceholder?: string;
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
export const APPLY_MODEL = "applyModel";
export const APPLY_AGENT = "applyAgent";
export const APPLY_REASONING = "applyReasoning";
export const APPLY_TRUST = "applyTrust";
/// The update dialog's own two rows: installing the release the check resolved,
/// and opening its release page. The check itself is run by the command, not by
/// a row, so a dialog that is only reporting has nothing to press.
export const UPDATE_INSTALL = "updateInstall";
export const UPDATE_NOTES = "updateNotes";
/// Restarts the window, which is what puts the freshly installed extension in
/// charge: the code in this window is the one that was running when it was
/// replaced.
export const UPDATE_RELOAD = "updateReload";

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

export interface ModelChoice {
  model: string;
  provider: string;
}

/// Model selection stays attached to the composer. The search field doubles as
/// custom input: a value not in the provider catalog becomes a row the user
/// can deliberately choose.
export function modelDialog(
  configured: string,
  provider: string,
  current: string,
  models: readonly ModelChoice[],
  query = "",
  note = "",
): DialogState {
  const needle = query.trim().toLowerCase();
  const choices =
    current && !models.some((entry) => entry.model === current)
      ? [...models, { provider, model: current }]
      : [...models];
  const known = choices.filter((entry) => !needle || entry.model.toLowerCase().includes(needle));
  const exact = choices.some((entry) => entry.model.toLowerCase() === needle);
  const configLabel = "Oxide config default";
  const configMatches =
    !needle ||
    configLabel.toLowerCase().includes(needle) ||
    configured.toLowerCase().includes(needle);
  return {
    kind: "model",
    pin: "footer",
    title: "Model",
    subtitle: "Choose a model available from the active provider, or enter any model ID.",
    note,
    rows: [
      ...(configMatches
        ? [
            row("", configLabel, {
              detail: configured
                ? `Use ${configured}, the model stored in the Oxide config`
                : "Use the model stored in the Oxide config",
              status: current ? "" : "Current",
              tone: "muted",
              action: APPLY_MODEL,
            }),
          ]
        : []),
      ...known.map((entry) =>
        row(entry.model, entry.model, {
          detail: entry.provider ? `Available from ${entry.provider}` : "Current model override",
          status: entry.model === current ? "Current" : entry.provider || provider,
          tone: "muted",
          action: APPLY_MODEL,
        }),
      ),
      ...(needle && !exact
        ? [
            row(query.trim(), `Use “${query.trim()}”`, {
              detail: "Pass this model ID with --model",
              action: APPLY_MODEL,
              kind: "action",
            }),
          ]
        : []),
    ],
    count: known.length,
    search: true,
    query,
    searchPlaceholder: "Filter or enter a model ID…",
    refreshLabel: "",
    refreshAction: "",
  };
}

export interface AgentChoice {
  name: string;
  description: string;
}

export function agentDialog(
  agents: readonly AgentChoice[],
  current: string,
  query = "",
): DialogState {
  const needle = query.trim().toLowerCase();
  const known = agents.filter(
    (agent) =>
      !needle ||
      agent.name.toLowerCase().includes(needle) ||
      agent.description.toLowerCase().includes(needle),
  );
  const exact = agents.some((agent) => agent.name.toLowerCase() === needle);
  const defaultMatches = !needle || "default no agent".includes(needle);
  return {
    kind: "agent",
    pin: "footer",
    title: "Agent",
    subtitle: "Choose a discovered agent, or type an agent name below.",
    note: agents.length ? "" : "No configured agents were discovered; you can still enter a name.",
    rows: [
      ...(defaultMatches
        ? [
            row("", "Default agent", {
              detail: "Run without a subagent prompt",
              status: current ? "" : "Current",
              tone: "muted",
              action: APPLY_AGENT,
            }),
          ]
        : []),
      ...known.map((agent) =>
        row(agent.name, agent.name, {
          detail: agent.description,
          status: agent.name === current ? "Current" : "",
          tone: "muted",
          action: APPLY_AGENT,
        }),
      ),
      ...(needle && !exact
        ? [
            row(query.trim(), `Use “${query.trim()}”`, {
              detail: "Pass this name with --agent",
              action: APPLY_AGENT,
              kind: "action",
            }),
          ]
        : []),
    ],
    count: 0,
    search: true,
    query,
    searchPlaceholder: "Filter or enter an agent name…",
    refreshLabel: "",
    refreshAction: "",
  };
}

export function reasoningDialog(current: string, levels: readonly string[]): DialogState {
  return choiceDialog(
    "reasoning",
    "Reasoning effort",
    "The effort passed to the next turn with --reasoning.",
    APPLY_REASONING,
    levels.map((level) => ({ value: level, label: level, detail: "" })),
    current,
  );
}

export function trustDialog(current: string): DialogState {
  return choiceDialog(
    "trust",
    "Project access",
    "Choose whether this workspace's .oxide agents, commands, skills and plugins may load.",
    APPLY_TRUST,
    [
      {
        value: "default",
        label: "Use saved decision",
        detail: "Follow trust.json or defaultProjectTrust",
      },
      { value: "always", label: "Always trust", detail: "Pass --approve for this workspace" },
      { value: "never", label: "Never trust", detail: "Pass --no-approve for this workspace" },
    ],
    current,
  );
}

function choiceDialog(
  kind: "reasoning" | "trust",
  title: string,
  subtitle: string,
  action: string,
  choices: readonly { value: string; label: string; detail: string }[],
  current: string,
): DialogState {
  return {
    kind,
    pin: "footer",
    title,
    subtitle,
    note: "",
    rows: choices.map((choice) =>
      row(choice.value, choice.label, {
        detail: choice.detail,
        status: choice.value === current ? "Current" : "",
        tone: "muted",
        action,
      }),
    ),
    count: 0,
    search: false,
    query: "",
    refreshLabel: "",
    refreshAction: "",
  };
}

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
///
/// `running` is the thread a turn is writing into when that thread is not the
/// one on screen — the reader moved to another thread while it ran. Its row says
/// so rather than reading as any other past conversation, because a turn is
/// still adding to it: the row is a way back to the reply being written, not
/// only to what it already said. The open thread's own mark is `current`, and a
/// row is never both.
export function sessionDialog(
  sessions: readonly SessionEntry[],
  current: string | null,
  note = "",
  live: LiveSession | null = null,
  query = "",
  running: LiveSession | null = null,
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
  // The turn's own thread, when the reader is not looking at it: its row says a
  // turn is in it rather than reading as a past conversation.
  const runningId = running && running.id !== current ? running.id : null;
  // The threads the store has no row for yet: the one on screen — a fresh thread
  // whose file it is still catching up to — and, when the reader is elsewhere,
  // the one a turn is writing into. A session file is written as its turn
  // starts, so the second is only the moment before the read lands; without a
  // stand-in the listing would omit the very thread it has the most to say
  // about.
  const stands: SessionEntry[] = [];
  if (open && !file) stands.push({ id: open.id, label: title, age: "", messages: 0 });
  if (runningId && !sessions.some((session) => session.id === runningId)) {
    stands.push({ id: runningId, label: running!.label.trim(), age: "", messages: 0 });
  }
  // The thread on screen is filtered like any other row, so a search never
  // leaves a row behind that the query does not match.
  const standing = filterSessions(stands, query);
  const threads = standing.length + filtered.length;
  // The note is the host's when it has one to give (the read failed, the store
  // is not there yet); otherwise a listing says why it is showing no threads,
  // which is either nothing stored yet or a filter that left none.
  const empty = sessions.length || stands.length
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
      ...standing.map((entry) =>
        row(entry.id, entry.label || entry.id, {
          // The store has no file to delete yet: a trash on this row would ask
          // the CLI to remove a session it cannot find.
          detail: "Open in this panel — the store has no file for it yet",
          status: entry.id === runningId ? "Running" : "Current",
          tone: (entry.id === runningId ? "ok" : "muted") as DialogTone,
          action: OPEN_SESSION,
          kind: "thread",
          current: entry.id === current,
        }),
      ),
      ...filtered.map((session) => {
        const marked = session.id === current;
        const live = session.id === runningId;
        const name = session.label || session.id;
        return row(session.id, name, {
          detail: `${session.messages} message${session.messages === 1 ? "" : "s"}`,
          status: marked ? "Current" : live ? "Running" : session.age,
          tone: live ? "ok" : "muted",
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

/// What the update dialog is showing. The check runs through the CLI and takes
/// a round trip to GitHub, so the panel has something to say before there is an
/// answer, and the install takes minutes; each state carries the words rather
/// than the call sites composing a dialog of their own.
export type UpdateState =
  /// Asking the installed CLI, which is asking GitHub.
  | { k: "checking" }
  /// The install is running. `what` names what is being installed, phrased for
  /// the title: a CLI too old to be checked has no version to name, so it is
  /// `the newest CLI` there and `Oxide 0.34.0` when the check resolved one.
  | { k: "installing"; what: string }
  /// The install landed. `check` is the check that resolved it, so the note
  /// can name the release and the file that carries it, and VS Code holds the
  /// new version from the next window on.
  | { k: "installed"; check: UpdateCheck }
  /// The check or the install did not answer with a result. `stage` says which
  /// one, because the same failure means different things at each: a check that
  /// could not reach GitHub leaves the installation alone, while a failed
  /// install may have left a half-downloaded release behind.
  | { k: "failed"; stage: "check" | "install"; message: string }
  /// The check resolved a release.
  | { k: "ready"; check: UpdateCheck }
  /// The installed CLI is older than this panel: it does not know `--json`, so
  /// there is no release to report — but `oxide update` still updates it, which
  /// is what `text` (the CLI's own refusal) is shown under. `headline` replaces
  /// the title for the same state read after an install ran, when what it holds
  /// is that install's report rather than a refusal, and `subtitle` says what
  /// that report is instead of describing the refusal it is not.
  | { k: "legacy"; text: string; path: string; headline?: string; subtitle?: string };

/// The update dialog: the release the extension's own train resolved, what this
/// window runs, and the install — the `.vsix` — as a row.
///
/// It is the panel's own update and deliberately not the CLI's: the check is
/// asked about the `extension-v*` releases and the panel installs what it
/// resolved, so the row cannot offer a release this editor would not run. The
/// one exception is the row a CLI too old to answer the check is offered, which
/// updates that binary — the thing standing between the user and a check at
/// all — and says so in as many words.
export function updateDialog(state: UpdateState): DialogState {
  const empty = {
    kind: "update" as const,
    pin: "footer" as const,
    count: 0,
    search: false,
    query: "",
    refreshLabel: "",
    refreshAction: "",
  };
  if (state.k === "checking") {
    return {
      ...empty,
      title: "Checking for updates",
      subtitle:
        "Oxide asks the installed CLI which release of this extension is newest — the same check the panel runs as `oxide update --check --component extension`.",
      note: "Asking GitHub for the newest extension release…",
      rows: [],
    };
  }
  if (state.k === "installing") {
    return {
      ...empty,
      title: `Installing ${state.what}`,
      subtitle:
        "The release's .vsix is downloaded, checked against the checksum its release published, and handed to VS Code to install.",
      note: "This takes as long as the download; the dialog reports what happened when it is done.",
      rows: [],
    };
  }
  if (state.k === "installed") {
    const { check } = state;
    return {
      ...empty,
      title: `Oxide ${check.latest} is installed`,
      subtitle:
        "VS Code holds the new version; the extension running in this window is the one that was there when it was installed.",
      note: [
        check.current ? `Was ${check.current}` : "",
        `Installed ${check.tag}`,
        check.asset?.name ?? "",
        "Restart this window to run it",
      ]
        .filter(Boolean)
        .join(" · "),
      rows: [
        row("", "Restart Window", {
          detail: "Run the version that was just installed",
          action: UPDATE_RELOAD,
        }),
        row("", "Close", {
          detail: "Restart later from the command palette",
          action: CLOSE_DIALOG,
        }),
      ],
    };
  }
  if (state.k === "failed") {
    return {
      ...empty,
      title: state.stage === "check" ? "Could not check for updates" : "Could not install the update",
      subtitle:
        state.stage === "check"
          ? "The check asks GitHub which release of this extension is newest, so a machine with no network — or a request GitHub refuses — reports it here."
          : "The install downloads the release's .vsix, checks it against the checksum its release published and hands it to VS Code.",
      note: state.message,
      rows: [row("", "Close", { detail: "Leave this extension as it is", action: CLOSE_DIALOG })],
    };
  }

  if (state.k === "legacy") {
    // The release is unknown here — the CLI that would have resolved it cannot
    // be asked — so the dialog offers the one command that works on any version
    // rather than a version to install. That command updates the CLI, which is
    // what this panel reads its own releases through; the sentence says so
    // rather than leaving the row looking like the extension's own install.
    return {
      ...empty,
      title: state.headline ?? "The oxide CLI is older than this panel",
      subtitle:
        state.subtitle ??
        "Check for Updates reads the extension's own releases through the installed oxide, and this CLI predates the report it reads — so updating the oxide command line is what makes both the check and the extension's next update reachable from here. It works on any version.",
      note: state.text,
      rows: [
        row("", "Install the newest CLI", {
          detail: state.path ? `Replaces ${state.path}` : "Runs oxide update",
          action: UPDATE_INSTALL,
        }),
        row("", "Close", {
          detail: "Leave this installation as it is",
          action: CLOSE_DIALOG,
        }),
      ],
    };
  }

  const { check } = state;
  const vsix = updateVsix(check);
  const notes = [
    check.current ? `Current ${check.current}` : "",
    `Latest ${check.tag}`,
    vsix?.name ?? "",
    check.pinned ? "pinned" : "",
  ].filter(Boolean);
  // A release this panel cannot install itself — one with no build for this
  // platform, or an artifact that is not a VSIX — is reported with the check's
  // own sentence, which names what to install by hand: the row it would belong
  // to is the one thing missing.
  if (check.updateAvailable && !vsix && check.advice) notes.push(check.advice);
  const rows: DialogRow[] = [];
  if (vsix) {
    rows.push(
      row(vsix.name, `Install ${check.latest}`, {
        detail: `Downloads and installs ${vsix.name}`,
        action: UPDATE_INSTALL,
      }),
    );
  }
  if (check.releaseUrl) {
    rows.push(
      row(check.releaseUrl, "Release notes", {
        detail: "Open the release page in your browser",
        action: UPDATE_NOTES,
      }),
    );
  }
  rows.push(
    row("", "Close", {
      detail: check.updateAvailable ? "Keep the version installed now" : "Nothing to install",
      action: CLOSE_DIALOG,
    }),
  );
  return {
    ...empty,
    title: check.updateAvailable
      ? `Oxide ${check.latest} is available`
      : `Oxide ${check.current || check.latest} is up to date`,
    subtitle: check.updateAvailable
      ? "Installing downloads this release of the extension — the .vsix published on its own release train — and hands it to VS Code, which runs it from the next window on."
      : `Oxide ${check.current || check.latest} is the newest released version of this extension.`,
    note: notes.join(" · "),
    rows,
  };
}
