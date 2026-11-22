// Oxide desktop front-end. The same page is hosted by Electron and Tauri. It has
// no Node of its own: the project/session/config stores it reads are the CLI's,
// reached through the app's commands rather than the filesystem.
//
// A command is performed by name: one packet carries the name and the arguments,
// and its answer carries that command's own value or the reason it failed.
// Everything a run has to say while one is in flight arrives as a packet of its
// own, and a handler is given the event's own `payload`.
//
// Both directions use the names this page has always read. Electron's preload
// installs the bridge before the page runs; Tauri exposes its global API when
// requested in `tauri.conf.json`. A packet goes out on the user bridge, and the
// window answers through
// `window.__electrobun.receiveMessageFromHost`, which the preload leaves
// filling a queue until a page takes it over — this page does, since it speaks
// the wire itself. A window whose preload installs no user bridge still has the
// event one, which carries the packet as the `host-message` event it was written
// as; the page reads the same packet off either.
const nativeTauri = window.__TAURI__;
if (!window.__electrobun) window.__electrobun = {};
window.__electrobunPendingHostMessages = window.__electrobunPendingHostMessages || [];

let userBridge = window.__electrobunHostBridge;
const eventBridge = window.__electrobunSendToHost;

if (!userBridge && nativeTauri?.core?.invoke && !eventBridge) {
  userBridge = {
    postMessage(raw) {
      let request = null;
      try {
        request = JSON.parse(raw);
      } catch {}
      return nativeTauri.core.invoke("oxide_message", { message: raw }).catch((error) => {
        if (request?.id != null) {
          deliver({
            type: "response",
            id: request.id,
            success: false,
            error: error instanceof Error ? error.message : String(error),
          });
        }
      });
    },
  };
  window.__electrobunHostBridge = userBridge;
}

function hostSend(packet) {
  if (userBridge) return userBridge.postMessage(JSON.stringify(packet));
  if (eventBridge) return eventBridge(packet);
  throw new Error("This window has no channel to the app");
}

const pending = new Map();
const listeners = new Map();
let nextRequest = 1;

let pageEventsReady = false;
const tauriListener = nativeTauri?.event?.listen
  ? nativeTauri.event.listen("oxide:to-page", ({ payload }) => receiveMessageFromHost(payload))
  : Promise.resolve();

function deliver(packet) {
  if (packet.type === "response") {
    const waiting = pending.get(packet.id);
    if (!waiting) return;
    pending.delete(packet.id);
    if (packet.success) waiting.resolve(packet.payload);
    else waiting.reject(new Error(packet.error || "the command failed"));
  }
}

// A message is handed to every handler in turn and awaited, so a handler that
// paints a streamed reply has finished writing before the next one runs — and
// the host's own paint is as far as the last frame it sent.
async function receiveMessageFromHost(raw) {
  let packet = raw;
  if (typeof packet === "string") {
    try {
      packet = JSON.parse(packet);
    } catch {
      return;
    }
  }
  if (!packet || typeof packet !== "object") return;
  if (!pageEventsReady && packet.type === "message") {
    window.__electrobunPendingHostMessages.push(raw);
    return;
  }
  if (packet.type === "message") {
    for (const handler of listeners.get(packet.id) || []) {
      await handler({ event: packet.id, payload: packet.payload });
    }
    return;
  }
  deliver(packet);
}

window.__electrobun.receiveMessageFromHost = receiveMessageFromHost;
window.__electrobun.receiveMessageFromBun = receiveMessageFromHost;

// A message the host sends before this page has taken the channel over is kept
// by the preload rather than dropped, and `init()` below is the point every
// listener this file registers is in place.
function drainPendingHostMessages() {
  const queued = window.__electrobunPendingHostMessages;
  if (!Array.isArray(queued) || !queued.length) return;
  window.__electrobunPendingHostMessages = [];
  for (const packet of queued) void receiveMessageFromHost(packet);
}

function invoke(command, args = {}) {
  const id = nextRequest++;
  return new Promise((resolve, reject) => {
    pending.set(id, { resolve, reject });
    hostSend({ type: "request", id, method: "oxide_invoke", params: { command, args } });
  });
}

async function listen(name, handler) {
  const handlers = listeners.get(name) || [];
  handlers.push(handler);
  listeners.set(name, handlers);
}

const el = (id) => document.getElementById(id);


// What each level asks the model for, in the words the picker shows beside it.
// `off` is worded as a request rather than a promise: a model that always
// thinks — GLM 5.3 and later, which the client asks for its lowest effort rather
// than for no thinking at all (`llm::client::glm_forces_thinking`) — still
// reasons when this level is chosen, and the row must not say otherwise.
const REASONING_HINTS = {
  auto: "Let the provider decide",
  off: "Turn thinking off where the model allows it",
  minimal: "The least reasoning",
  low: "A little reasoning",
  medium: "Balanced reasoning",
  high: "A lot of reasoning",
  xhigh: "Extra-high reasoning",
  max: "The most reasoning",
};

const SUGGESTIONS = [
  "Explain this codebase and its architecture.",
  "Find and fix the highest-priority bug in this repository.",
  "Add tests for the most important untested code path.",
  "Review the working tree changes and summarize the risks.",
];

// The sidebar follows Codex's disclosure model for a project's chats: enough
// to be useful at a glance, with an explicit expansion instead of one
// unbounded wall of chats. Expansions live only for this window; the
// underlying project order and complete session list are unchanged. The
// project list itself is never capped: every registered project is shown.
const SIDEBAR_THREAD_LIMIT = 5;
const SIDEBAR_RECENT_LIMIT = 5;

// ---------- glyphs ----------

/// Every icon the window draws: inline SVG stroked with `currentColor`, so a
/// control's glyph is the same drawing on every machine. A character is not one
/// — a `✕`, a `📁` or a `＋` is whatever shape and size the machine's own font
/// gives it, which is how the composer's stroked paperclip ended up beside an
/// emoji folder.
function glyph(paths, box = 24) {
  return `<svg viewBox="0 0 ${box} ${box}" aria-hidden="true">${paths}</svg>`;
}

/// One glyph in the 24-unit box and 1.6 weight the desktop controls use.
function glyphBody(body) {
  return `<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">${body}</svg>`;
}

/// The shared glyph bodies. Electron, Tauri and the VS Code panel use these
/// drawings rather than a character from the machine's fonts.
const GLYPHS = {
  back: '<path d="M15 6l-6 6 6 6M9 12h12"/>',
  forward: '<path d="M9 6l6 6-6 6M3 12h12"/>',
  sidebar: '<path d="M4 4h16v16H4ZM9 4v16"/>',
  home: '<path d="M3 11l9-8 9 8M5 10v11h5v-7h4v7h5V10"/>',
  folder: '<path d="M3 6h7l2 3h9v11H3ZM3 9h18"/>',
  pencil:
    '<path d="M16.5 3.75 20.25 7.5M7.5 16.5 4.5 17.25 5.25 14.25 18 1.5 21.75 5.25ZM10.5 4.5H3v16.5h16.5V13.5"/>',
  plus: '<path d="M12 5v14M5 12h14" stroke-width="1.8"/>',
  more: '<path d="M5 12h.01M12 12h.01M19 12h.01" stroke-width="2.4"/>',
  search: '<circle cx="10.5" cy="10.5" r="6.5"/><path d="m15.4 15.4 4.6 4.6"/>',
  settings:
    '<circle cx="12" cy="12" r="3.8" stroke-width="1.8"/><path d="M9.93 4.79 10.26 2.15h3.48l.33 2.64 1.57.65 2.1-1.63 2.45 2.45-1.63 2.1.65 1.57 2.64.33v3.48l-2.64.33-.65 1.57 1.63 2.1-2.45 2.45-2.1-1.63-1.57.65-.33 2.64h-3.48l-.33-2.64-1.57-.65-2.1 1.63-2.45-2.45 1.63-2.1-.65-1.57-2.64-.33v-3.48l2.64-.33.65-1.57-1.63-2.1 2.45-2.45 2.1 1.63Z" stroke-width="1.8"/>',
  customize:
    '<path d="M4 7h16M4 12h16M4 17h16"/><circle cx="9" cy="7" r="2" fill="currentColor"/><circle cx="15" cy="12" r="2" fill="currentColor"/><circle cx="8" cy="17" r="2" fill="currentColor"/>',
  chat: '<path d="M21 15a4 4 0 0 1-4 4H8l-5 3 1.5-5A7 7 0 0 1 3 13V8a4 4 0 0 1 4-4h10a4 4 0 0 1 4 4Z"/>',
  globe:
    '<circle cx="12" cy="12" r="9"/><path d="M3 12h18M12 3c5 5 5 13 0 18-5-5-5-13 0-18Z"/>',
  bell: '<path d="M5 17V10a7 7 0 0 1 14 0v7l2 2H3ZM10 22h4"/>',
  activity: '<path d="M3 12h4l2.2-7 4.1 14 2.3-7H21"/>',
  bolt: '<path d="M13 2.5 5 13.5h6l-1 8 8-11h-6l1-8Z" stroke-width="1.8"/>',
  reset:
    '<path d="M4.5 12a7.5 7.5 0 1 0 2.2-5.3" stroke-width="1.8"/><path d="M4.5 3.8v3.4h3.4" stroke-width="1.8"/>',
  download: '<path d="M12 4v10m-4-4 4 4 4-4M5 14v5h14v-5"/>',
  keyboard:
    '<rect x="2.5" y="6.5" width="19" height="11" rx="2.5"/><path d="M6 10h.01M9.5 10h.01M13 10h.01M16.5 10h.01M7.5 14h9"/>',
  pin: '<path d="m14 4 6 6-3 1-4 4-1 5-3-6-5-3 5-1 4-4Z"/>',
  "pull-request":
    '<circle cx="6" cy="5" r="2.4"/><circle cx="6" cy="19" r="2.4"/><circle cx="18" cy="19" r="2.4"/><path d="M6 7.4v9.2M18 16.6v-8a2 2 0 0 0-2-2H8"/>',
  archive: '<path d="M4 8h16M5 8v11h14V8M3 4h18v4H3ZM9 12h6"/>',
  "chevron-down": '<path d="M5.5 8.6 12 14.8l6.5-6.2"/>',
  "chevron-right": '<path d="m8.6 5.5 6.2 6.5-6.2 6.5"/>',
  copy:
    '<rect x="8" y="8" width="11" height="11" rx="2"/><path d="M16 8V6a2 2 0 0 0-2-2H6a2 2 0 0 0-2 2v8a2 2 0 0 0 2 2h2"/>',
  paperclip:
    '<path d="M21.44 11.05l-9.19 9.19a6 6 0 0 1-8.49-8.49l9.19-9.19a4 4 0 0 1 5.66 5.66l-9.2 9.19a2 2 0 0 1-2.83-2.83l8.49-8.48" stroke-width="1.8"/>',
  lock: '<path d="M8 10V7a4 4 0 0 1 8 0v3"/><rect x="5.5" y="10" width="13" height="9.5" rx="2.4" stroke-width="1.8"/>',
  "arrow-up": '<path d="M12 20V5M5.5 11.5 12 5l6.5 6.5" stroke-width="1.8"/>',
  "arrow-down": '<path d="M12 4v15M5.5 12.5 12 19l6.5-6.5" stroke-width="1.8"/>',
  square: '<rect x="7" y="7" width="10" height="10" rx="2" fill="currentColor" stroke="none"/>',
  branch:
    '<circle cx="6.5" cy="5.5" r="2.2"/><circle cx="6.5" cy="18.5" r="2.2"/><circle cx="17.5" cy="8.5" r="2.2"/><path d="M6.5 7.7v8.6M8.7 16.4c5.2 0 8.8-2.6 8.8-5.7"/>',
  worktree:
    '<rect x="2.5" y="4" width="9.5" height="11" rx="2"/><path d="M7.2 8h4.6M7.2 11.5h3.2"/><rect x="15" y="6" width="6.5" height="12" rx="2"/>',
  details:
    '<circle cx="5" cy="6" r="1.8" stroke-width="1.8"/><circle cx="5" cy="18" r="1.8" stroke-width="1.8"/><path d="M10.5 6H21M10.5 12H21M10.5 18H21M3 12h4" stroke-width="1.8"/>',
  close: '<path d="M18 6 6 18M6 6l12 12" stroke-width="1.8"/>',
  check: '<path d="m5.5 12.5 4.2 4.2 8.8-9"/>',
  changes:
    '<rect x="4" y="3" width="16" height="18" rx="3" stroke-width="1.8"/><path d="M8.5 9.5h7M12 6v7M8.5 16.5h7" stroke-width="1.8"/>',
  image:
    '<rect x="3" y="4.5" width="18" height="15" rx="2.5"/><circle cx="9" cy="10" r="1.7"/><path d="m20.5 15.5-5-4-8.5 8"/>',
  file: '<path d="M14 3H7a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h10a2 2 0 0 0 2-2V8z"/><path d="M14 3v5h5"/>',
  terminal:
    '<rect x="2.5" y="4" width="19" height="16" rx="3"/><path d="m7 9.5 3 2.5-3 2.5M12.5 15H17"/>',
  tool: '<path d="M14.7 6.3a1 1 0 0 0 0 1.4l1.6 1.6a1 1 0 0 0 1.4 0l3.77-3.77a6 6 0 0 1-7.94 7.94l-6.91 6.91a2.12 2.12 0 0 1-3-3l6.91-6.91a6 6 0 0 1 7.94-7.94l-3.76 3.76z"/>',
  plug: '<path d="M12 22v-5M9 8V2M15 8V2M18 8v5a4 4 0 0 1-4 4h-4a4 4 0 0 1-4-4V8Z"/>',
};

// The two boxes this window's own drawings are made in, at the weight the panel
// draws each in: a 24-unit box for a glyph that fills its button, a 16-unit one
// at the panel's own 1.5–1.6 for the marks that sit on a text baseline.
const STROKE_24 =
  'fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"';
const STROKE_16 =
  'fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"';

/// The app's own mark: the diamond cut into the two parts of its name — the
/// left half rust (`Ox`, the language the harness is written in) and the right
/// half cyan (`IDE`, the tool it is used as) — held by the ink rim and the
/// caret-width seam between them. The colours are the mark's own rather than
/// `currentColor`, so it reads the same on the home tile, in the sidebar's
/// speaker line and on the dock: it is the drawing `scripts/build-icons.py`
/// rasterises for the app's icons, described here once for the window.
function brandMark() {
  const half = (points, fill) => `<polygon points="${points}" fill="${fill}"/>`;
  return (
    '<svg viewBox="0 0 24 24" aria-hidden="true">' +
    half("3.375 12 11.3 4.075 11.3 19.925", "#2d2d3a") +
    half("4.5 12 11.3 5.2 11.3 18.8", "#d9643a") +
    half("12.7 4.075 20.625 12 12.7 19.925", "#2d2d3a") +
    half("12.7 5.2 19.5 12 12.7 18.8", "#5fd7ff") +
    "</svg>"
  );
}

/// Sizing is the sheet's, next to the box each glyph is drawn in.
const ICONS = {
  // The marks the native table names for Plugins and Skills, worn by the
  // customize sidebar the way that window's own sidebar wears them.
  puzzle: glyphBody(GLYPHS.customize),
  book: glyphBody(GLYPHS.chat),

  refresh: glyph('<path d="M20 7V3m0 4h-4M4 17v4m0-4h4M20 7a8 8 0 0 0-14-2M4 17a8 8 0 0 0 14 2" ' + STROKE_24 + '/>'),
  trash: glyph('<path d="M3 6h18M9 6V3h6v3M6 6l1 15h10l1-15M10 10v7M14 10v7" ' + STROKE_24 + '/>'),
  back: glyphBody(GLYPHS.back),
  globe: glyphBody(GLYPHS.globe),
  folder: glyphBody(GLYPHS.folder),
  file: glyphBody(GLYPHS.file),
  search: glyphBody(GLYPHS.search),
  close: glyphBody(GLYPHS.close),
  copy: glyphBody(GLYPHS.copy),
  plus: glyphBody(GLYPHS.plus),
  more: glyphBody(GLYPHS.more),
  compose: glyphBody(GLYPHS.pencil),
  changes: glyphBody(GLYPHS.changes),
  undo: glyph('<path d="m9 4-5 5 5 5M4 9h10a6 6 0 0 1 0 12h-3" ' + STROKE_24 + '/>'),
  pin: glyphBody(GLYPHS.pin),
  archive: glyphBody(GLYPHS.archive),
  chat: glyphBody(GLYPHS.chat),
  computer: glyph('<rect x="3" y="4" width="18" height="13" rx="2" ' + STROKE_24 + '/><path d="M8 21h8M12 17v4" ' + STROKE_24 + '/>'),
  branch: glyphBody(GLYPHS.branch),
  worktree: glyphBody(GLYPHS.worktree),
  gear: glyphBody(GLYPHS.settings),
  // A request's state is the color the row wears, as it is in the native
  // window: one drawing, three states.
  pull: glyphBody(GLYPHS["pull-request"]),
  check: glyphBody(GLYPHS.check),
  terminal: glyphBody(GLYPHS.terminal),
  tool: glyphBody(GLYPHS.tool),
  plug: glyphBody(GLYPHS.plug),
  caret: glyphBody(GLYPHS["chevron-down"]),
  chevronRight: glyphBody(GLYPHS["chevron-right"]),
  bolt: glyphBody(GLYPHS.bolt),
  reset: glyphBody(GLYPHS.reset),
  power: glyph(
    '<path d="M18.36 6.64a9 9 0 1 1-12.73 0" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round"/>' +
      '<path d="M12 2v10" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round"/>',
  ),
  mark: brandMark(),
  // The composer's own status badge: a dot for a window with nothing to report,
  // and the triangle a failure wears. Drawn here rather than shared with the
  // panel, which has no status badge of its own.
  dot: glyph('<circle cx="8" cy="8" r="3.2" fill="currentColor"/>', 16),
  alert: glyph(
    `<path d="M8 2.4 14.5 13.4H1.5L8 2.4Z" ${STROKE_16}/>` +
      `<path d="M8 6.4v3.2M8 11.8v.01" ${STROKE_16}/>`,
    16,
  ),
};

/// The check a chosen row wears, or nothing at all on a row that is not the one
/// in use: a picker repaints its rows in place, so what it draws is replaced
/// rather than added to.
function setCheck(node, on) {
  node.innerHTML = on ? ICONS.check : "";
}

/// The follow-up behavior the window is built with, read before the preference
/// store has answered: `loadPrefs` replaces it with the stored answer as soon as
/// the window can ask, so this only decides how the first moments behave.
function savedFollowUpBehavior() {
  try {
    return window.localStorage?.getItem("oxide.followUpBehavior") === "steer" ? "steer" : "queue";
  } catch {
    return "queue";
  }
}

/// Every conversation this window is holding in memory: the one on screen and
/// the ones a turn is still writing into. A record is keyed by its session id,
/// or by a draft key until the run that started a thread reports the id it made,
/// and it holds what belongs to that thread rather than to the window — its run,
/// its queue, its totals, its change cards, and its transcript while it is off
/// screen — so one thread's turn goes on painting into its own conversation
/// while the reader reads another.
const threads = new Map();
let nextDraft = 1;
// The thread a paint pass is writing into, when it is not the one on screen.
let paintThread = null;

const storedManualChatOrder = (() => {
  try {
    const value = JSON.parse(localStorage.getItem("oxide.sidebar.manualChatOrder") || "[]");
    return Array.isArray(value) ? value.filter((id) => typeof id === "string") : [];
  } catch {
    return [];
  }
})();

/// The view fields the window itself owns: the thread it has open and where it
/// runs. A background turn's paint pass reads its own thread's folder instead
/// (see `paintingThread`), which is why they are not plain state.
const view = { session: null, project: null, worktree: null };

const state = {
  projects: [],
  projectName: "",
  sessions: [],
  expandedSidebarProjects: new Set(),
  collapsedSidebarProjects: new Set(),
  projectsCollapsed: false,
  chatSort: sidebarPref("chatSort", "updated") === "created"
    ? "updated"
    : sidebarPref("chatSort", "updated"),
  manualChatOrder: storedManualChatOrder,
  sidebarView: sidebarPref("sidebarView", "project"),
  chatFilter: "all",
  // New context sent during a run waits by default. Steering is a deliberate
  // choice because it changes the work already in progress.
  followUpBehavior: savedFollowUpBehavior(),
  // The title this window gave the one thread it holds that the store has not
  // listed, with that thread's id. A run names the thread it is in from the
  // message that started it, and the same thread may be parked and opened again
  // before the store catches up, so the name is kept beside the id it belongs to
  // rather than on whichever field held it last.
  heldThread: null,
  agent: "",
  reasoning: "auto",
  // Whether a picker choice supplies an explicit level while the shared
  // configuration is being saved.
  reasoningPicked: false,
  // The levels the active model advertised, when its listing carried them. The
  // picker shows these instead of the full set, so it never offers a level the
  // model would clamp away.
  reasoningLevels: null,
  supportsReasoning: null,
  reasoningIdentity: "",
  catalogWindow: 0,
  currentProvider: "",
  // Whether the credential is a plan rather than a metered key, which the usage
  // line marks ` (sub)` the way the terminal footer does.
  subscription: false,
  // What the window's own status is saying, and which state its icon wears when
  // no turn is running: read by `paintStatus`. The words are dropped while it is
  // the idle placeholder, so the badge is the icon alone.
  statusText: "",
  statusLevel: "info",
  // The repository the row names: what `git_info` answered for the folder on
  // screen, read when the folder changes, when a turn ends and when the reader
  // comes back to the window.
  git: null,
  // The worktrees of the project on screen, and the one the next turn runs in.
  // A worktree is another checkout of the project on its own branch, so a
  // conversation's work stays out of the folder the reader has open; `null` is
  // that folder itself.
  worktrees: [],
  worktreesError: null,
  providers: [],
  // The provider the connect dialog has selected, kept by name rather than by
  // row: the list is filtered as the reader searches, so a row's position says
  // nothing about which provider is chosen.
  providerName: "",
  models: null,
  pendingApproval: null,
  pendingQuestion: null,
  trust: null,
  detailsOpen: true,
  attachments: [],
  mcps: null,
  // Why the last read of the servers or of the threads failed, kept until the
  // next read answers so the listing's own render paints it.
  mcpError: "",
  sessionsError: "",
  palette: [],
  paletteIndex: 0,
  paletteOpen: false,
  // The language the window draws, and the strings it draws in it. Both come
  // from the core (`language`, which reads `settings.json` and hands over the
  // shared catalog), so this page keeps no table of its own: a key with no
  // entry yet reads as the English it was written as, which is also what the
  // window shows before the answer lands.
  language: "en-GB",
  strings: {},
  currentModel: "",
  // The languages the picker offers, as the core last answered with them.
  languages: [],
};

/// The conversation a paint pass is writing into: the one on screen, unless a
/// background turn's events are being applied.
const paintingThread = () => paintThread || visibleThread();

/// The thread the window has open. A tab that has not started a conversation yet
/// holds a draft record, which the run that sends the first message re-keys with
/// the session id it creates.
function visibleThread() {
  // Read from the window's own view rather than through `state`: a background
  // paint swaps those fields to its own thread, and the thread on screen is the
  // one question that has to keep answering about the window.
  const key = view.session ? `s:${view.session}` : draftKey(activeChatTab());
  const record = threadRecord(key);
  if (record.session == null && view.session) record.session = view.session;
  if (record.project == null && view.project) record.project = view.project;
  if (record.worktree == null && view.worktree) record.worktree = view.worktree;
  return record;
}

function threadRecord(key) {
  let record = threads.get(key);
  if (!record) {
    record = {
      key,
      node: null,
      followLatest: true,
      session: null,
      project: null,
      worktree: null,
      busy: false,
      runId: null,
      stopRequested: false,
      pendingSends: [],
      tools: [],
      currentAssistant: null,
      currentThinking: null,
      currentActivity: null,
      runError: "",
      runStartedAt: 0,
      runElapsed: 0,
      usage: null,
      changes: [],
      contextCorrection: null,
      contextLimits: [],
      contextWindow: 0,
      repositoryChanges: null,
  /// What the index holds for each file the working tree differs in, as
  /// `repository_status` last answered: which staging action applies.
  repositoryIndex: null,
      runSession: null,
      runProject: null,
      runTitle: "",
      clock: null,
      unread: false,
    };
    threads.set(key, record);
  }
  return record;
}

/// The tab's own draft thread, made on demand so a conversation with no session
/// yet still has a record to be painted into and a key to be found by.
function draftKey(tab) {
  if (!tab) return "draft:0";
  tab.draftKey ||= `draft:${nextDraft++}`;
  return tab.draftKey;
}

/// The thread whose run this is, so an event a background turn sends is applied
/// to that conversation rather than to whichever one is on screen.
function threadForRunId(runId) {
  if (runId == null) return null;
  for (const record of threads.values()) if (record.runId === runId) return record;
  return null;
}

/// Applies `fn` to a thread that is not the one on screen: every thread field
/// `fn` reads or writes resolves through `paintThread`, and its transcript is the
/// conversation it holds off screen. The visible thread needs no swap — it is
/// already the one the fields point at.
function withThread(record, fn) {
  if (!record || record === visibleThread()) return fn();
  const previous = paintThread;
  paintThread = record;
  try {
    return fn();
  } finally {
    paintThread = previous;
  }
}

/// The transcript a paint pass writes into: the one on screen for the reader's
/// own thread, and the detached conversation a thread keeps while it is elsewhere.
function transcriptFor(record) {
  if (!record || record === visibleThread()) return el("transcript");
  record.node ||= conversationNode();
  return record.node;
}

function conversationNode() {
  const node = document.createElement("div");
  node.className = "conversation";
  return node;
}

/// Moves the thread on screen into its own record — every node painted into it so
/// far, and the run it belonged to — so another thread can be read, or another
/// tab shown, without the turn losing what it has written.
function parkThread() {
  const transcript = el("transcript");
  const record = visibleThread();
  record.session = state.session;
  record.project = state.project;
  record.worktree = state.worktree;
  record.followLatest = transcript.followLatest !== false;
  if (transcript.children.length) {
    record.node ||= conversationNode();
    record.node.append(...transcript.children);
  }
}

/// Hands a thread's conversation back to the screen, whole — whatever its run
/// painted while the reader was elsewhere is already in it. The caller sets the
/// thread and folder the tab is showing; this only moves the nodes.
function restoreThread(record) {
  if (record) record.unread = false;
  const transcript = el("transcript");
  if (record?.node) {
    transcript.replaceChildren(...record.node.children);
    record.node = null;
  } else {
    transcript.replaceChildren();
  }
  if (record) transcript.followLatest = record.followLatest !== false;
  scrollDown(transcript, true);
}

/// The thread field accessors: every run, queue, card and transcript pointer
/// belongs to a conversation, and the one being painted is the one read.
for (const key of [
  "busy",
  "runId",
  "stopRequested",
  "pendingSends",
  "tools",
  "currentAssistant",
  "currentThinking",
  "currentActivity",
  "runError",
  "runStartedAt",
  "runElapsed",
  "usage",
  "changes",
  "contextCorrection",
  "contextLimits",
  "contextWindow",
  "repositoryChanges",
  "runSession",
  "runProject",
  "runTitle",
]) {
  Object.defineProperty(state, key, {
    get: () => paintingThread()[key],
    set: (value) => {
      paintingThread()[key] = value;
    },
  });
}
for (const key of ["session", "project", "worktree"]) {
  Object.defineProperty(state, key, {
    get: () => (paintThread ? paintThread[key] : view[key]),
    set: (value) => {
      if (paintThread) paintThread[key] = value;
      else view[key] = value;
    },
  });
}

// ---------- helpers ----------

/// One word of the interface in the language on screen: the translated string
/// when the core answered with one, else the English this page was written in.
/// The fallback is what keeps the window whole before the catalog arrives and
/// in a build whose core has no entry for the key yet.
function t(key, fallback, vars) {
  let text = state.strings?.[key] || fallback || "";
  if (vars) {
    for (const [name, value] of Object.entries(vars)) {
      text = text.split(`{${name}}`).join(value);
    }
  }
  return text;
}

/// The English each translatable node was authored with, kept the first time
/// the page localizes it so switching languages twice does not translate a
/// translation.
const i18nDefaults = new WeakMap();

function authored(node, kind, value) {
  let entry = i18nDefaults.get(node);
  if (!entry) {
    entry = {};
    i18nDefaults.set(node, entry);
  }
  if (!(kind in entry)) entry[kind] = value;
  return entry[kind];
}

/// Paints the language the core resolved into every node that names a key, and
/// remembers the value for the dynamic strings (`t`) the page writes later.
function applyLanguage(info) {
  const statusKey = state.statusText === idleStatus() ? "composer.ready"
    : state.statusText === workingStatus() ? "composer.working" : null;
  if (info?.language) state.language = info.language;
  if (Array.isArray(info?.languages)) state.languages = info.languages;
  state.strings = info?.strings || {};
  document.documentElement.lang = state.language;
  for (const node of document.querySelectorAll("[data-i18n]")) {
    const key = node.dataset.i18n;
    node.textContent = t(key, authored(node, "text", node.textContent));
  }
  for (const node of document.querySelectorAll("[data-i18n-title]")) {
    const key = node.dataset.i18nTitle;
    node.title = t(key, authored(node, "title", node.title));
  }
  for (const node of document.querySelectorAll("[data-i18n-aria]")) {
    const key = node.dataset.i18nAria;
    node.setAttribute(
      "aria-label",
      t(key, authored(node, "aria", node.getAttribute("aria-label") || "")),
    );
  }
  for (const node of document.querySelectorAll("[data-i18n-placeholder]")) {
    const key = node.dataset.i18nPlaceholder;
    const native = node.tagName === "INPUT" || node.tagName === "TEXTAREA";
    const text = t(key, authored(node, "placeholder", native ? node.placeholder : node.dataset.placeholder || ""));
    if (native) node.placeholder = text;
    else node.dataset.placeholder = text;
  }
  // The chips and the prompt are painted after this runs; the placeholder the
  // empty composer shows is read out of the dataset here.
  if (el("prompt")) el("prompt").dataset.placeholder = t(
    "composer.placeholder",
    el("prompt").dataset.placeholder || "Ask Oxide…",
  );
  // The project chip's own words are state, not markup: a folder that is open
  // is named by its folder, and only the empty chip is a label to translate.
  updateProjectChip();
  paintUpdateNotice(updateNotice.key, updateNotice.fallback);
  if (!el("projects-page").hidden) renderProjectDirectory();
  if (!el("search-modal").hidden) renderSearchResults();
  renderLanguageRows(info);
  if (statusKey) state.statusText = t(statusKey, statusKey === "composer.ready" ? "Ready" : "Working…");
  repaintLanguage();
  void invoke("set_native_language", { strings: state.strings }).catch(() => {});
}

/// Reads the language the core resolved and paints it. The window starts in
/// English, so a core that cannot answer leaves the page exactly as authored.
let languageProbe = 0;

async function loadLanguage() {
  const project = state.project;
  const probe = ++languageProbe;
  try {
    const info = await invoke("language", { project });
    if (probe !== languageProbe || project !== state.project) return;
    applyLanguage(info);
  } catch {
    // Keep the catalog already on screen when a background read fails.
  }
}

/// The rows of the language picker: each language named in itself, with the one
/// on screen marked. A row writes the choice through the core, which is what
/// makes it the terminal's and the editor panel's language too, then repaints
/// the window in it.
function renderLanguageRows(info) {
  const list = el("language-list");
  if (!list) return;
  list.innerHTML = "";
  const languages = info?.languages?.length
    ? info.languages
    : state.languages.length
      ? state.languages
      : [
          {
            tag: "en-GB",
            label: "English (United Kingdom)",
            nativeLabel: "English (United Kingdom)",
          },
          { tag: "zh-CN", label: "Simplified Chinese", nativeLabel: "简体中文" },
        ];
  for (const language of languages) {
    const row = document.createElement("button");
    row.type = "button";
    row.className = "language-option" + (language.tag === state.language ? " active" : "");
    row.dataset.language = language.tag;
    const name = document.createElement("span");
    name.className = "language-option-name";
    name.textContent = language.nativeLabel || language.label || language.tag;
    const tag = document.createElement("span");
    tag.className = "language-option-tag";
    tag.textContent = language.tag;
    const check = document.createElement("span");
    check.className = "language-option-check";
    setCheck(check, language.tag === state.language);
    row.append(name, tag, check);
    row.onclick = () => switchLanguage(language.tag);
    list.appendChild(row);
  }
}

async function switchLanguage(tag) {
  if (!tag || tag === state.language) {
    el("language-modal").hidden = true;
    return;
  }
  try {
    const project = state.project;
    const probe = ++languageProbe;
    const info = await invoke("set_language", { tag, project });
    if (probe !== languageProbe || project !== state.project) return;
    applyLanguage(info);
    el("language-modal").hidden = true;
    renderSettingsLanguage();
  } catch (error) {
    setStatus(String(error), "error");
    showToast(String(error), "error");
  }
}

/// Repaints the parts of the window the page writes itself. Called after a
/// language switch, since a translation the markup carries is already painted
/// by `applyLanguage`.
function repaintLanguage() {
  paintChatTabs();
  closeSidebarCards();
  if (!el("create-project-modal").hidden) {
    el("create-project-title").textContent = createProjectState.editing ? t("ui.editProject", "Edit project") : t("dialog.createProject.title", "Create project");
    el("create-project-save").textContent = createProjectState.editing ? t("common.save", "Save") : t("dialog.createProject.title", "Create project");
  }
  if (modelMenuOpen()) renderModelMenu();
  updateChips();
  updateSendState();
  updateTrustButton();
  paintStatus();
  paintQueue();
  paintRunClock();
  for (const card of state.changes) if (card.title) paintChanges(card);
  repaintWelcome();
  void renderProjectsTree();
}


function escapeHtml(text) {
  return String(text).replace(/[&<>"']/g, (c) => ({
    "&": "&amp;",
    "<": "&lt;",
    ">": "&gt;",
    '"': "&quot;",
    "'": "&#39;",
  })[c]);
}

function formatTime(secs) {
  if (!secs) return "";
  const date = new Date(secs * 1000);
  const diff = Date.now() - date.getTime();
  const mins = Math.round(diff / 60000);
  if (mins < 1) return t("ui.justNow", "just now");
  if (mins < 60) return t("ui.mAgo", "{value0}m ago", { value0: mins });
  const hours = Math.round(mins / 60);
  if (hours < 24) return t("ui.hAgo", "{value0}h ago", { value0: hours });
  const days = Math.round(hours / 24);
  if (days < 30) return t("ui.dAgo", "{value0}d ago", { value0: days });
  return date.toLocaleDateString(state.language, { month: "short", day: "numeric" });
}

function baseName(path) {
  const trimmed = String(path).replace(/[\\/]+$/, "");
  return trimmed.split(/[\\/]/).pop() || trimmed;
}

/// Status decorates the composer action. Notices keep their words beside it;
/// the idle placeholder needs no separate mark or label.
function setStatus(text, level = "info") {
  state.statusText = text;
  state.statusLevel = level;
  paintStatus();
}

/// Writes text to the clipboard from the page. The host webview's modern
/// clipboard API is preferred; a protected webview that refuses it falls back
/// to the hidden field and the document's own copy command.
async function copyText(text) {
  const value = String(text ?? "");
  try {
    if (globalThis.navigator?.clipboard?.writeText) {
      await globalThis.navigator.clipboard.writeText(value);
      return true;
    }
  } catch {}
  const field = document.createElement("textarea");
  field.value = value;
  field.setAttribute("readonly", "");
  field.style.position = "fixed";
  field.style.opacity = "0";
  field.style.pointerEvents = "none";
  document.body.appendChild(field);
  field.select?.();
  let copied = false;
  try {
    copied = typeof document.execCommand === "function" && document.execCommand("copy");
  } catch {}
  field.remove();
  if (copied) return true;
  try {
    await invoke("write_clipboard", { text: value });
    return true;
  } catch {}
  return false;
}

/// Reads the pasteboard's text through the webview first and the desktop host
/// second. The host path uses the same clipboard reader as the CLI editor, so a
/// copied image becomes a path and a copied file keeps its path.
async function readClipboardText() {
  try {
    if (globalThis.navigator?.clipboard?.readText) {
      return String(await globalThis.navigator.clipboard.readText());
    }
  } catch {}
  return (await invoke("read_clipboard_text")) ?? "";
}

function showToast(text, level = "info", action = null) {
  const toast = document.createElement("div");
  toast.className = "toast";
  toast.dataset.level = level;
  const message = document.createElement("span");
  message.className = "toast-message";
  message.setAttribute("role", level === "error" ? "alert" : "status");
  message.textContent = text;
  toast.appendChild(message);
  let timer;
  let pending = false;
  let hovered = false;
  let focused = false;
  const dismiss = () => { clearTimeout(timer); toast.remove(); };
  const pause = () => clearTimeout(timer);
  const resume = () => {
    pause();
    if (pending || hovered || focused) return;
    timer = setTimeout(dismiss, action || level === "error" ? 10000 : 6000);
  };
  if (action) {
    const button = document.createElement("button");
    button.className = "toast-action";
    button.textContent = action.label;
    button.onclick = async () => {
      if (button.disabled) return;
      button.disabled = true;
      pending = true;
      pause();
      try {
        await action.run();
        dismiss();
      } catch (error) {
        button.disabled = false;
        pending = false;
        showToast(t("ui.couldNotUndo", "Could not undo: {value0}", { value0: error }), "error");
      }
    };
    toast.appendChild(button);
  }
  const close = document.createElement("button");
  close.className = "icon-button toast-close";
  close.innerHTML = ICONS.close;
  close.title = t("ui.dismissNotification", "Dismiss notification");
  close.setAttribute("aria-label", close.title);
  close.onclick = dismiss;
  toast.appendChild(close);
  toast.addEventListener("mouseenter", () => { hovered = true; pause(); });
  toast.addEventListener("mouseleave", () => { hovered = false; resume(); });
  toast.addEventListener("focusin", () => { focused = true; pause(); });
  toast.addEventListener("focusout", (event) => {
    focused = toast.contains(event.relatedTarget);
    resume();
  });
  el("toast-list").appendChild(toast);
  resume();
  return dismiss;
}

/// The action carries the status; a separate line is only needed for a notice.
function paintStatus() {
  const busy = Boolean(state.busy);
  const level = busy ? "busy" : state.statusLevel;
  const badge = el("status");
  badge.dataset.level = level;
  const text = state.statusText || "";
  for (const id of ["send", "stop"]) {
    el(id).dataset.level = level;
    el(id).setAttribute("aria-description", text || IDLE_STATUS);
  }
  // A running turn is reported by the transcript's own clock, which counts it and
  // keeps its total: the composer repeating the word would say the same thing a
  // second time, so the working placeholder is nothing to say here — including
  // the one another thread's turn left behind.
  const silent = !text || text === idleStatus() || text === workingStatus();
  el("status-text").textContent = text;
  el("status-text").hidden = silent;
  badge.hidden = busy || silent;
  const stopHint = t("composer.stopHint", "Stop the running turn");
  el("stop").title = `${stopHint}${silent ? "" : ` · ${text}`}`;
  el("stop").setAttribute("aria-label", el("stop").title);
  badge.title = text;
  badge.setAttribute("aria-label", text || IDLE_STATUS);
}

let runClockTimer = 0;

function runDuration(ms) {
  const seconds = Math.max(0, Math.floor(ms / 1000));
  if (seconds < 60) return `${seconds}s`;
  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) return `${minutes}m ${seconds % 60}s`;
  return `${Math.floor(minutes / 60)}h ${minutes % 60}m`;
}

/// Each turn's own elapsed line, in its own conversation: two threads can be
/// running at once, and the one the reader is not reading keeps its clock with
/// the reply it is writing.
function runClock(record) {
  if (!record.clock) {
    const node = document.createElement("div");
    node.className = "run-time";
    node.setAttribute("role", "timer");
    record.clock = node;
  }
  return record.clock;
}

function paintRunClock() {
  for (const record of threads.values()) {
    if (!record.runStartedAt) continue;
    // A finished turn's line is the one it already painted: a transcript rebuilt
    // from the store does not grow a clock for a turn nobody is running.
    if (!record.busy && !record.clock) continue;
    const active = record.busy;
    const clock = runClock(record);
    const transcript = transcriptFor(record);
    if (!record.busy && clock.parentNode !== transcript) continue;
    if (clock.parentNode !== transcript || transcript.lastElementChild !== clock) {
      clock.remove();
      transcript.appendChild(clock);
    }
    clock.hidden = false;
    const elapsed = active ? Date.now() - record.runStartedAt : record.runElapsed;
    clock.textContent = t(
      active ? "run.workingFor" : "run.workedFor",
      active ? "Working for {duration}" : "Worked for {duration}",
      { duration: runDuration(elapsed) },
    );
    if (record === visibleThread()) scrollDown(transcript);
  }
}

function startRunClock(record = paintingThread()) {
  // A thread's second turn gets its own clock rather than the one the first left
  // standing: the line is the same node, counting this turn.
  record.runStartedAt = Date.now();
  record.runElapsed = 0;
  paintRunClock();
  if (!runClockTimer) runClockTimer = setInterval(paintRunClock, 1000);
}

function finishRunClock(record = paintingThread()) {
  if (record.runStartedAt) record.runElapsed = Math.max(0, Date.now() - record.runStartedAt);
  const running = [...threads.values()].some((entry) => entry.runStartedAt && entry.busy);
  if (!running && runClockTimer) {
    clearInterval(runClockTimer);
    runClockTimer = 0;
  }
}

/// The word the window is idle under: the words are dropped for it and the icon
/// speaks, since a row that says `Ready` is a row saying nothing.
const IDLE_STATUS = t("composer.ready", "Ready");

/// The idle word in the language on screen, which is what the status row
/// compares against before it decides the row has nothing to say.
const idleStatus = () => t("composer.ready", IDLE_STATUS);

/// The word a turn used to be announced with in the composer. The row no longer
/// wears it — the transcript's clock is where a running turn is read — and the
/// value is still recognized so a stale one is treated as silence.
const workingStatus = () => t("composer.working", "Working…");

/// The tokens the CLI's footer abbreviates with, in its own steps: under a
/// thousand as they are, then `1.2k`, `123k`, `1.2M`, `66M`. One rule for the
/// window — the totals line and the context reading both read through it — so a
/// number reads here the way it reads in the terminal.
function formatTokens(count) {
  const value = Number(count) || 0;
  if (value < 1_000) return String(value);
  if (value < 10_000) return `${(value / 1_000).toFixed(1)}k`;
  if (value < 1_000_000) return `${Math.round(value / 1_000)}k`;
  if (value < 10_000_000) return `${(value / 1_000_000).toFixed(1)}M`;
  return `${Math.round(value / 1_000_000)}M`;
}

/// One step's usage added to the totals a thread has spent. The terminal's footer
/// counts the whole conversation, so the tokens, the cache and the spend are
/// cumulative here too — kept as a rule over a totals object rather than over the
/// window's state, since a thread parked with a running turn keeps its own. The
/// `prompt` is that step's own prompt, which is the context the request was sent
/// with — the one number a total cannot carry — and the cache hit rate belongs to
/// the step that reported cache traffic, so a cold step leaves the warm one's rate
/// in place (the same rule `UsageTotals::cache_hit_rate` and the panel follow).
function addTotals(spent, event) {
  const usage = event.usage || {};
  const input = Number(usage.input) || 0;
  const output = Number(usage.output) || 0;
  const cacheRead = Number(usage.cacheRead) || 0;
  const cacheWrite = Number(usage.cacheWrite) || 0;
  const cost = Number(usage.cost) || 0;
  const billedPrompt = input + cacheRead + cacheWrite;
  const prompt = typeof usage.contextTokens === "number" ? usage.contextTokens : billedPrompt;
  const before = spent || {};
  const source = ["main", "compaction", "background"].includes(usage.source) ? usage.source : "main";
  const attribution = { ...(before.attribution || {}) };
  const bucket = attribution[source] || {};
  attribution[source] = {
    modelCalls: (bucket.modelCalls || 0) + (usage.modelCalls ?? 1),
    input: (bucket.input || 0) + input,
    output: (bucket.output || 0) + output,
    cacheRead: (bucket.cacheRead || 0) + cacheRead,
    cacheWrite: (bucket.cacheWrite || 0) + cacheWrite,
    cost: (bucket.cost || 0) + cost,
  };
  const cacheHitRate =
    cacheRead + cacheWrite > 0 && billedPrompt > 0
      ? (cacheRead / billedPrompt) * 100
      : before.cacheHitRate ?? null;
  return {
    input: (before.input || 0) + input,
    output: (before.output || 0) + output,
    cacheRead: (before.cacheRead || 0) + cacheRead,
    cacheWrite: (before.cacheWrite || 0) + cacheWrite,
    cost: (before.cost || 0) + cost,
    prompt: source === "background" ? (before.prompt || 0) : prompt,
    cacheHitRate,
    attribution,
  };
}

/// A step of the thread the reader is in, added to what the window is showing.
function addUsage(event) {
  const totals = addTotals(state.usage, event);
  totals.turn = addTotals(state.usage?.turn, event);
  setUsage(totals);
}

function setUsage({
  input = 0,
  output = 0,
  cacheRead = 0,
  cacheWrite = 0,
  cost = 0,
  prompt = 0,
  cacheHitRate = null,
  attribution = {},
  turn = null,
}) {
  // What the thread has spent, held so one the reader leaves can take its own
  // totals with it and paint them back on return. The percentage is not kept
  // beside them: it is a fraction of the window in force where they are drawn
  // (see `usagePercent`), so a folder switched under a background run cannot
  // label its thread with the window of the folder the reader left.
  state.usage = { input, output, cacheRead, cacheWrite, cost, prompt, cacheHitRate, attribution, turn };
  // The footer is the thread being read's, so a step counted for another one
  // waits with it rather than painting under this conversation.
  if (paintingThread() === visibleThread()) paintUsage();
}

/// The usage line, drawn the way the terminal footer draws its own: the tokens
/// the chat has spent, the cache it read and wrote, the hit rate the provider
/// reported, and the spend — marked ` (sub)` for a provider whose credential is
/// a plan rather than a metered key, where the price table's number is what the
/// plan would have billed. Every segment is left out instead of drawn as a zero,
/// and the context reading is the ring beside them rather than a second word
/// here. The totals are the same ones that ring is measured from, so what the
/// chat spent and the fraction of its window that took can never disagree.
function paintUsage() {
  const totals = state.usage;
  const bits = [];
  if (totals) {
    if (totals.input) bits.push(`↑${formatTokens(totals.input)}`);
    if (totals.output) bits.push(`↓${formatTokens(totals.output)}`);
    if (totals.cacheRead) bits.push(`R${formatTokens(totals.cacheRead)}`);
    if (totals.cacheWrite) bits.push(`W${formatTokens(totals.cacheWrite)}`);
    if (totals.cacheHitRate != null) bits.push(`CH${totals.cacheHitRate.toFixed(1)}%`);
    if (totals.cost > 0 || state.subscription) {
      bits.push(`$${Number(totals.cost || 0).toFixed(3)}${state.subscription ? " (sub)" : ""}`);
    }
  }
  el("usage").textContent = bits.join(" ");
  const details = [bits.join(" · ")];
  if (totals) {
    const prompt = totals.input + totals.cacheRead + totals.cacheWrite;
    if (prompt) details.push(`Session cache hit: ${(100 * totals.cacheRead / prompt).toFixed(1)}%`);
    details.push("CH: latest request with cache traffic");
    for (const [source, label] of [["main", "Main agent"], ["compaction", "Compaction"], ["background", "Background tasks"]]) {
      const bucket = totals.attribution?.[source];
      if (bucket) details.push(`${label}: ${bucket.modelCalls} model calls · ↑${formatTokens(bucket.input)} ↓${formatTokens(bucket.output)} R${formatTokens(bucket.cacheRead)} W${formatTokens(bucket.cacheWrite)} · $${Number(bucket.cost || 0).toFixed(3)}`);
    }
    if (totals.turn) {
      const turn = totals.turn;
      const calls = Object.values(turn.attribution || {}).reduce((sum, bucket) => sum + bucket.modelCalls, 0);
      details.push(`Current/latest run: ${calls} model calls · ↑${formatTokens(turn.input)} ↓${formatTokens(turn.output)} R${formatTokens(turn.cacheRead)} W${formatTokens(turn.cacheWrite)} · $${Number(turn.cost || 0).toFixed(3)}`);
    }
  }
  el("usage").title = details.join("\n");
  // The context reading is painted from these totals wherever they move, rather
  // than from a second reading of its own.
  paintComposerFacts();
}

/// The percent of the window the thread's own totals are measured against: the
/// prompt of its newest request over the window in force where they are drawn,
/// and nothing at all until both are known.
function usagePercent(totals) {
  if (!totals || !totals.prompt || !state.contextWindow) return null;
  return (totals.prompt / state.contextWindow) * 100;
}

let nextTabId = 1;
const chatTabs = [{ id: nextTabId++, title: "New chat", snapshot: null }];
let activeTabId = chatTabs[0].id;
let switchingTab = false;
const activeChatTab = () => chatTabs.find((tab) => tab.id === activeTabId);
const tabChangeBlocked = () => switchingTab || state.handingOff ||
  ["approval", "question", "trust-modal", "confirm-modal", "rename-modal"].some((id) => !el(id).hidden);

function captureChatTab() {
  const tab = activeChatTab();
  if (!tab) return;
  tab.title = el("thread-title").textContent || "New chat";
  tab.session = state.session;
  // The tab's conversation goes into its own record, so a turn still running in
  // it keeps writing there while another tab is on screen.
  parkThread();
  tab.snapshot = {
    project: state.project, projectName: state.projectName, session: state.session, trust: state.trust,
    worktree: state.worktree, worktrees: state.worktrees, agent: state.agent,
    draft: el("prompt").value, attachments: [...state.attachments],
    selectionStart: el("prompt").selectionStart, selectionEnd: el("prompt").selectionEnd,
    inputScrollTop: el("prompt").scrollTop, undo: el("prompt").captureHistory?.(),
    scrollTop: el("transcript").scrollTop,
  };
}

/// The thread a tab is holding: its session when it has one, else its draft.
function tabThread(tab) {
  if (!tab) return null;
  return tab.session ? threads.get(`s:${tab.session}`) || null
    : tab.draftKey ? threads.get(tab.draftKey) || null : null;
}

function paintChatTabs() {
  const list = el("chat-tabs");
  if (!list) return;
  list.replaceChildren();
  list.hidden = chatTabs.length < 2;
  for (const tab of chatTabs) {
    const row = document.createElement("div");
    row.className = `chat-tab${tab.id === activeTabId ? " active" : ""}`;
    const select = document.createElement("button");
    select.className = "chat-tab-select";
    select.id = `chat-tab-${tab.id}`;
    select.setAttribute("role", "tab");
    select.setAttribute("aria-selected", String(tab.id === activeTabId));
    select.setAttribute("aria-controls", "transcript");
    select.tabIndex = tab.id === activeTabId ? 0 : -1;
    const running = Boolean(tabThread(tab)?.busy);
    if (running) {
      const indicator = document.createElement("span");
      indicator.className = "spinner";
      indicator.setAttribute("aria-hidden", "true");
      select.appendChild(indicator);
    }
    const label = document.createElement("span");
    label.className = "chat-tab-title";
    const tabLabel = tab.title === "New chat" ? t("panel.newChat", "New chat") : tab.title;
    label.textContent = tabLabel;
    select.appendChild(label);
    select.title = running ? `${tabLabel} · ${t("ui.running", "Running")}` : tabLabel;
    select.setAttribute("aria-label", select.title);
    select.onclick = () => activateChatTab(tab.id);
    select.onkeydown = (event) => {
      if (!["ArrowLeft", "ArrowRight", "Home", "End"].includes(event.key)) return;
      event.preventDefault();
      const index = chatTabs.indexOf(tab);
      const target = event.key === "Home" ? chatTabs[0] : event.key === "End" ? chatTabs.at(-1)
        : chatTabs[(index + (event.key === "ArrowRight" ? 1 : -1) + chatTabs.length) % chatTabs.length];
      activateChatTab(target.id).then(() => el(`chat-tab-${target.id}`)?.focus());
    };
    const close = document.createElement("button");
    close.className = "chat-tab-close";
    close.innerHTML = ICONS.close;
    close.title = t("ui.close", "Close {value0}", { value0: tabLabel });
    close.setAttribute("aria-label", close.title);
    close.onclick = async (event) => {
      event?.stopPropagation();
      await closeChatTab(tab.id);
      if (chatTabs.length > 1) el(`chat-tab-${activeTabId}`)?.focus();
      else el("prompt").focus();
    };
    row.onauxclick = (event) => {
      if (event.button !== 1) return;
      event.preventDefault();
      closeChatTab(tab.id);
    };
    row.append(select, close);
    list.appendChild(row);
  }
  el("transcript").setAttribute("aria-labelledby", list.hidden ? "thread-title" : `chat-tab-${activeTabId}`);
  if (!list.hidden) el(`chat-tab-${activeTabId}`)?.parentNode?.scrollIntoView?.({ block: "nearest", inline: "nearest" });
  paintActivityDot();
}

async function activateChatTab(id) {
  if (id === activeTabId || tabChangeBlocked()) return;
  const tab = chatTabs.find((entry) => entry.id === id);
  if (!tab) return;
  switchingTab = true;
  try {
    captureChatTab();
    closeAt();
    closeSidebarCards();
    closeQueueMenu();
    clearPaintIdle();
    activeTabId = id;
    const saved = tab.snapshot;
    if (saved) {
      for (const key of ["project", "projectName", "trust", "worktree", "worktrees", "agent"]) state[key] = saved[key];
      // The tab's own thread, not the one its snapshot was taken in: a run may
      // have named the thread the tab was starting after the capture.
      state.session = tab.session ?? saved.session ?? null;
      restoreThread(tabThread(tab));
      el("transcript").scrollTop = saved.scrollTop || 0;
      el("transcript").followScrollTop = el("transcript").scrollTop;
      el("prompt").value = saved.draft;
      el("prompt").restoreHistory?.(saved.undo);
      state.attachments = [...saved.attachments];
    } else {
      state.session = null;
      state.worktree = null;
      resetTranscript();
      el("prompt").value = "";
      el("prompt").restoreHistory?.();
      state.attachments = [];
    }
    closeOverlays();
    closeReview();
    setThreadTitle(tab.title === "New chat" ? "" : tab.title);
    renderAttachments();
    updateChips();
    el("agents").title = t("ui.agents", "Agents: {value0}", { value0: state.agent || "Default" });
    paintQueue();
    updateRunBanner();
    updateSendState();
    paintUsage();
    renderProjectsTree();
    renderMessageNavigator();
    loadGit();
    await loadInfo();
    loadWorktrees();
    if (tab.pendingSession) await openSession(tab.pendingSession);
    el("prompt").focus();
    if (saved) {
      el("prompt").setSelectionRange(saved.selectionStart ?? saved.draft.length, saved.selectionEnd ?? saved.draft.length);
      el("prompt").scrollTop = saved.inputScrollTop || 0;
    }
  } finally {
    switchingTab = false;
    paintChatTabs();
  }
}

async function newChatTab() {
  if (tabChangeBlocked()) return;
  const tab = { id: nextTabId++, title: "New chat", snapshot: null };
  chatTabs.push(tab);
  await activateChatTab(tab.id);
  return tab.id;
}

async function closeChatTab(id) {
  if (tabChangeBlocked()) return;
  const index = chatTabs.findIndex((tab) => tab.id === id);
  if (index < 0) return;
  const closing = chatTabs[index];
  if (chatTabs.length === 1) {
    await newChatTab();
  } else if (activeTabId === id) {
    await activateChatTab(chatTabs[index - 1]?.id ?? chatTabs[index + 1].id);
  }
  chatTabs.splice(chatTabs.findIndex((tab) => tab.id === id), 1);
  // What the tab was showing is kept, so the shortcut that brings a closed tab
  // back reopens the thread rather than an empty one.
  if (closing) {
    const session = closing.session
      ? state.sessions.find((entry) => entry.id === closing.session) || null
      : null;
    state.closedTabs.push({ session, draft: closing.snapshot?.draft || "" });
    if (state.closedTabs.length > 10) state.closedTabs.shift();
  }
  paintChatTabs();
}

function initChatTabs() {
  paintChatTabs();
  el("new-tab").onclick = newChatTab;
}

function messageSummary(node) {
  return (node?.dataset?.prompt || node?.querySelector?.(".body")?.textContent || node?.textContent || "").replace(/\s+/g, " ").trim();
}

function renderMessageNavigator() {
  const nav = el("message-nav");
  if (!nav) return;
  const prompts = [...el("transcript").querySelectorAll(".msg.user")];
  nav.replaceChildren();
  el("message-preview").hidden = true;
  paintJumpLatest();
  nav.hidden = prompts.length < 2;
  prompts.forEach((message, index) => {
    const mark = document.createElement("button");
    mark.className = "message-marker";
    const summary = messageSummary(message) || t("composer.attachment", "Attachment");
    mark.setAttribute("aria-label", t("ui.jumpToMessage", "Jump to message {value0}: {value1}", { value0: index + 1, value1: summary.slice(0, 100) }));
    mark.title = summary;
    const show = () => {
      const preview = el("message-preview");
      preview.replaceChildren();
      const title = document.createElement("strong");
      title.textContent = summary;
      const excerpt = document.createElement("p");
      const siblings = [...el("transcript").children];
      const start = siblings.indexOf(message);
      const end = index + 1 < prompts.length ? siblings.indexOf(prompts[index + 1]) : siblings.length;
      excerpt.textContent = siblings.slice(start + 1, end).filter((node) => node.classList.contains("assistant")).map(messageSummary).join(" ").slice(0, 360);
      preview.append(title, excerpt);
      preview.hidden = false;
      const pane = (document.querySelector(".conversation-pane") || el("transcript")).getBoundingClientRect();
      preview.style.top = `${Math.max(8, Math.min(mark.getBoundingClientRect().top - pane.top - 20, pane.height - preview.getBoundingClientRect().height - 8))}px`;
    };
    const hide = () => { el("message-preview").hidden = true; };
    mark.onmouseenter = show;
    mark.onmouseleave = hide;
    mark.onfocus = show;
    mark.onblur = hide;
    mark.onclick = () => {
      hide();
      el("transcript").followLatest = false;
      message.scrollIntoView({ block: "start", behavior: matchMedia("(prefers-reduced-motion: reduce)").matches ? "instant" : "smooth" });
      message.classList.remove("jump-target");
      message.classList.add("jump-target");
      setTimeout(() => message.classList.remove("jump-target"), 1200);
      for (const button of nav.children) button.classList.toggle("active", button === mark);
    };
    nav.appendChild(mark);
  });
  paintMessagePosition();
}

function paintMessagePosition() {
  const messages = [...el("transcript").querySelectorAll(".msg.user")];
  const top = el("transcript").getBoundingClientRect().top + 80;
  let active = 0;
  messages.forEach((message, index) => { if (message.getBoundingClientRect().top <= top) active = index; });
  [...el("message-nav").children].forEach((button, index) => {
    button.classList.toggle("active", index === active);
    if (index === active) button.setAttribute("aria-current", "true");
    else button.removeAttribute("aria-current");
  });
}

function paintJumpLatest(updateFollow = false) {
  const transcript = el("transcript");
  const button = el("jump-latest");
  if (updateFollow) updateTranscriptFollow(transcript);
  const away = transcript.scrollHeight - transcript.clientHeight - transcript.scrollTop > 2;
  button.hidden = !away;
  if (away) {
    const pane = (document.querySelector(".conversation-pane") || transcript).getBoundingClientRect();
    button.style.top = `${Math.max(8, transcript.getBoundingClientRect().bottom - pane.top - 48)}px`;
  }
}

function initJumpLatest() {
  const transcript = el("transcript");
  transcript.followScrollTop = transcript.scrollTop;
  transcript.addEventListener("wheel", (event) => {
    updateTranscriptFollow(transcript);
    if (event.deltaY < 0 && transcript.scrollTop > 0) transcript.followLatest = false;
  }, { passive: true });
  transcript.addEventListener("scroll", () => paintJumpLatest(true));
  el("jump-latest").onclick = () => scrollDown(transcript, true);
  transcript.addEventListener("load", () => scrollDown(transcript), true);
  transcript.addEventListener("oxide-diagram-layout", () => scrollDown(transcript));
  if (typeof MutationObserver !== "undefined") {
    let queued = false;
    new MutationObserver(() => {
      if (queued) return;
      queued = true;
      requestAnimationFrame(() => { queued = false; scrollDown(transcript); });
    }).observe(transcript, { childList: true, characterData: true, subtree: true, attributes: true, attributeFilter: ["class", "hidden"] });
  }
  window.addEventListener("resize", () => paintJumpLatest());
  if (typeof ResizeObserver !== "undefined") {
    new ResizeObserver(() => {
      if (transcript.followLatest !== false) scrollDown(transcript);
      else paintJumpLatest();
    }).observe(transcript);
  }
}

function initMessageNavigator() {
  el("transcript").addEventListener("scroll", paintMessagePosition);
  if (typeof MutationObserver !== "undefined") {
    let queued = false;
    new MutationObserver(() => {
      if (queued) return;
      queued = true;
      requestAnimationFrame(() => { queued = false; renderMessageNavigator(); });
    }).observe(el("transcript"), { childList: true });
  }
}

function setThreadTitle(text) {
  el("thread-title").textContent = text || "";
  const tab = activeChatTab();
  if (tab) { tab.title = text || "New chat"; tab.session = state.session; }
  paintChatTabs();
}

/// The label a thread is known by, held once: the sidebar row, the header and
/// the sessions list all name the open thread the same way.
function sessionLabel(session) {
  return session.name || session.preview || session.id.slice(0, 8);
}

/// The threads every listing reads — the sidebar's tree and its per-project
/// children, the header and the sessions list: what the store returned, with the
/// threads this window is holding standing in for themselves until the store has
/// them. `all_sessions` lists what is already on disk, and a thread that was just
/// started has not written its first entry yet, so a new thread used to be
/// missing from every one of those lists until its first turn ended — the one
/// thread the reader was looking at. The window can hold two at once: the one on
/// screen, and the one a turn is running in while the reader reads another. Each
/// entry drops out as soon as the store lists that id, since a row is keyed by
/// it.
function listedSessions() {
  const listed = state.sessions || [];
  const seconds = Math.floor(Date.now() / 1000);
  const standing = [];
  // Every thread this window is holding that the store has not written yet: the
  // one on screen, and the ones a turn is running in while the reader is
  // elsewhere. A row standing in for one carries the folder that thread runs in,
  // so opening it goes back to its own work rather than asking the wrong project
  // for a thread it has not got.
  const held = [];
  if (state.session) held.push(visibleThread());
  for (const record of threads.values()) {
    if (record.session && (record.busy || record.unread)) held.push(record);
  }
  for (const record of held) {
    if (record === visibleThread() && !record.session) continue;
    const id = record.session;
    const cwd = record === visibleThread() ? state.project : transcriptProject(record);
    if (!id || !cwd) continue;
    if (listed.some((session) => session.id === id)) continue;
    if (standing.some((session) => session.id === id)) continue;
    // The timestamps are the store's own unit — Unix seconds, which `sessionAge`
    // subtracts from `Date.now() / 1000` — rather than milliseconds, which would
    // read as a thread written in the future and be reported as `just now` for as
    // long as the store has not written it.
    standing.push({
      id,
      name: standingInName(id),
      cwd,
      created_at: seconds,
      modified_at: seconds,
      message_count: 0,
      preview: "",
      path: "",
      // A thread the store has not written has no file, so the commands that read
      // one off disk have nothing to answer for it: its rows are handled in the
      // window instead of through `session_messages`.
      unstored: true,
    });
  }
  return standing.length ? [...standing, ...listed] : listed;
}

/// The title the window holds for a thread the store has not listed: the one the
/// turn that started it named it by, kept beside that thread's id — and not on
/// the run, which the thread outlives — so the header and the sidebar's row go on
/// naming the thread the strip names, through the park and back, rather than
/// showing its id. A thread this window holds no title for has none of its own.
function heldTitle(id) {
  if (id == null || !state.heldThread || state.heldThread.id !== id) return "";
  return state.heldThread.title || "";
}

/// The name an unstored thread is listed under — a name only a thread this window
/// is holding has. The rest of that listing is empty for it, since the store has
/// not written it and nothing else knows it.
function standingInName(id) {
  const record = id ? threads.get(`s:${id}`) : null;
  return record?.runTitle || heldTitle(id) || null;
}

function sessionById(id) {
  if (!id) return null;
  return listedSessions().find((session) => session.id === id) || null;
}

/// The header names the thread on screen — the one that was resumed, or the one
/// on this window's own screen — by what the sidebar's own listing calls it, so
/// the two never disagree; a thread the listing has only as a stand-in is named
/// by the title this window holds for it, which is the one the turn that started
/// it gave it.
function refreshThreadTitle() {
  const session = sessionById(state.session);
  setThreadTitle(session ? sessionLabel(session) : heldTitle(state.session));
}

// ---------- markdown ----------

function anchor(href, label, bare = false, angle = false, title = null) {
  const text = label == null ? escapeHtml(href) : label;
  const marker = bare ? ' data-editor-autolink=""' : angle ? ' data-editor-angle=""' : "";
  const held = title == null ? "" : ` title="${escapeHtml(title)}"`;
  return `<a href="${escapeHtml(href)}"${marker}${held} target="_blank" rel="noreferrer">${text}</a>`;
}

// A bare URL often ends a sentence, so trailing punctuation stays outside the
// link and an unbalanced closing bracket is handed back to the surrounding text.
function autolink(url, stash, bare = false) {
  let href = url.replace(/[.,;:!?]+$/, "");
  let trail = url.slice(href.length);
  while (href.endsWith(")") && href.split("(").length < href.split(")").length) {
    href = href.slice(0, -1);
    trail = ")" + trail;
  }
  return href ? stash(anchor(bare ? `https://${href}` : href, bare ? escapeHtml(href) : undefined, bare)) + trail : url;
}

// Link labels can contain balanced brackets, as issue keys in PR titles do.
function inlineDestination(text, start) {
  let i = start + 1;
  while (/\s/.test(text[i] || "")) i += 1;
  let url = "";
  if (text[i] === "<") {
    const close = text.indexOf(">", i + 1);
    if (close < 0) return null;
    url = text.slice(i + 1, close);
    i = close + 1;
  } else {
    const begin = i;
    let depth = 0;
    while (i < text.length) {
      const ch = text[i];
      if (ch === "(") depth += 1;
      else if (ch === ")") {
        if (depth === 0) break;
        depth -= 1;
      } else if (/\s/.test(ch) && depth === 0) break;
      i += 1;
    }
    url = text.slice(begin, i);
  }
  if (!url) return null;
  while (/\s/.test(text[i] || "")) i += 1;
  let title = null;
  if (text[i] === '"' || text[i] === "'") {
    const quote = text[i];
    const close = text.indexOf(quote, i + 1);
    if (close < 0) return null;
    title = text.slice(i + 1, close);
    i = close + 1;
    while (/\s/.test(text[i] || "")) i += 1;
  }
  if (text[i] !== ")") return null;
  return { url, title, length: i - start + 1 };
}

function inlineLinks(text, stash) {
  const starts = [];
  let output = "";
  let copied = 0;
  for (let i = 0; i < text.length; i += 1) {
    if (text[i] === "\\") {
      i += 1;
    } else if (text[i] === "[") {
      starts.push(i);
    } else if (text[i] === "]" && starts.length) {
      const start = starts.pop();
      // A link may be written `<destination>` and may carry a title, which is
      // kept on the anchor so a read gives the line back as it was written.
      const destination = text[i + 1] === "(" ? inlineDestination(text, i + 1) : null;
      if (!destination) continue;
      const { title, url } = destination;
      // A destination holding a stashed escape is not a URL any more, so the
      // line stays literal rather than building a link out of a placeholder.
      if (url.includes("\u0000")) continue;
      output += text.slice(copied, start) + stash(anchor(
        url, escapeHtml(text.slice(start + 1, i)), false, false, title,
      ));
      i += destination.length;
      copied = i + 1;
      starts.length = 0;
    }
  }
  return output + text.slice(copied);
}

// The named and numeric references a writer may hold: the character they stand
// for is what the reader sees, so an entity in a prompt is not shown as `&amp;`.
const ENTITIES = { amp: "&", lt: "<", gt: ">", quot: '"', apos: "'", nbsp: "\u00a0" };

function decodeEntities(text) {
  return String(text).replace(/&(#[xX]?[0-9a-fA-F]+|[a-zA-Z]+);/g, (all, name) => {
    if (!name.startsWith("#")) return ENTITIES[name.toLowerCase()] ?? all;
    const code = /^#[xX]/.test(name) ? parseInt(name.slice(2), 16) : Number(name.slice(1));
    if (!Number.isFinite(code) || code < 32 || code > 0x10ffff) return all;
    return String.fromCodePoint(code);
  });
}

function inline(text, references = false, hardBreaks = false) {
  const stashed = [];
  const stash = (html) => {
    stashed.push(html);
    return `\u0000${stashed.length - 1}\u0000`;
  };
  // Links and code spans are stashed on the raw text (before escaping), so a
  // URL is never folded into an HTML entity (`&lt;`, `&gt;`, `&amp;`) and an
  // entity is never read as part of the URL. Whatever is left is escaped once,
  // after the placeholders are in place.
  let s = String(text);
  s = s.replace(/(`+)(?!`)([\s\S]*?)(?<!`)\1(?!`)/g, (_, _ticks, code) => stash(`<code>${escapeHtml(code.replace(/\n/g, " "))}</code>`));
  // A backslash makes the next character literal, so an escaped delimiter never
  // opens a span — `\*stars\*` reads as the stars a writer meant, and the
  // editable surface keeps the backslash it takes to say so again.
  s = s.replace(/\\([\\`*_{}\[\]()#+\-.!~>|])/g, (_, char) =>
    stash(references ? `<span data-editor-escape="">${escapeHtml(char)}</span>` : escapeHtml(char)));
  if (references) {
    for (const [start, end] of referenceRanges(s).reverse()) {
      const reference = s.slice(start, end);
      if (reference.includes("\u0000")) continue;
      s = s.slice(0, start) + stash(`<span class="file-reference">${escapeHtml(reference)}</span>`) + s.slice(end);
    }
  }
  s = inlineLinks(s, stash);
  s = s.replace(/<(https?:\/\/[^\s<>]+)>/g, (_, url) => stash(anchor(url, null, false, true)));
  s = s.replace(/\bhttps?:\/\/[^\s<>"'`\u0000]+/g, (url, offset, input) => {
    const delimiter = /(\*\*|\*|~~)$/.exec(input.slice(0, offset))?.[1];
    const punctuation = /[.,;:!?]+$/.exec(url)?.[0] || "";
    const body = punctuation ? url.slice(0, -punctuation.length) : url;
    if (delimiter && body.endsWith(delimiter)) {
      return autolink(body.slice(0, -delimiter.length), stash) + delimiter + punctuation;
    }
    return autolink(url, stash);
  });
  if (references) {
    s = s.replace(/(^|[\s(])((?:[a-z0-9](?:[a-z0-9-]*[a-z0-9])?\.)+(?:com|org|net|io|dev|app|co|edu|gov|ai|me|info|uk|cn|de|fr|jp)(?::\d{1,5})?(?:[/?#][^\s<>"'`\u0000]*)?)(?=$|[\s),.;:!?])/gi,
      (_, prefix, url) => prefix + autolink(url, stash, true));
  }
  if (hardBreaks) {
    s = s.replace(/ {2,}\n|\\\n/g, (breakMark) =>
      stash(references ? `<br data-editor-hard-break="${escapeHtml(breakMark.slice(0, -1))}">` : "<br>"),
    ).replace(/\n/g, " ");
  }
  s = escapeHtml(decodeEntities(s));
  // Emphasis needs a non-space on both sides of its delimiters, so `2 * 3 * 4`
  // stays arithmetic and `** spaced **` stays literal; `_` additionally may not
  // open or close inside a word, which keeps `snake_case_name` a name. Each pass
  // runs twice so a span wrapped in the other delimiter bolds its contents too.
  const triple = () => s
    .replace(/\*\*\*(\S(?:[^*]*?\S)?)\*\*\*/g, (_, body) =>
      stash(references
        ? `<em data-editor-triple="***"><strong data-editor-silent="">${body}</strong></em>`
        : `<em><strong>${body}</strong></em>`))
    .replace(/(^|[^\w_])___(\S(?:[^_]*?\S)?)___(?![\w_])/g, (_, before, body) =>
      `${before}${stash(references
        ? `<em data-editor-triple="___"><strong data-editor-silent="">${body}</strong></em>`
        : `<em><strong>${body}</strong></em>`)}`);
  const del = () => s.replace(/~~(\S(?:[^~]*?\S)?)~~/g, "<del>$1</del>");
  const strong = () => s.replace(/\*\*(\S(?:[^*]*?\S)?)\*\*/g, "<strong>$1</strong>")
    .replace(/(^|[^\w_])__(\S(?:[^_]*?\S)?)__/g, (_, before, body) =>
      `${before}<strong${references ? ' data-editor-marker="underscore"' : ""}>${body}</strong>`);
  const em = () => s.replace(/(^|[^*\w])\*(\S(?:[^*\n]*?\S)?)\*(?![*\w])/g, "$1<em>$2</em>")
    .replace(/(^|[^\w_])_(\S(?:[^_\n]*?\S)?)_(?![\w_])/g, (_, before, body) =>
      `${before}<em${references ? ' data-editor-marker="underscore"' : ""}>${body}</em>`);
  s = del();
  s = triple();
  s = strong();
  s = em();
  s = strong();
  s = em();
  // A stashed span can hold another one — a code span inside a link's label —
  // so placeholders are restored until none is left.
  while (/\u0000\d+\u0000/.test(s)) s = s.replace(/\u0000(\d+)\u0000/g, (_, i) => stashed[Number(i)]);
  return s;
}

const BLOCK_START = /^\s*(```|~~~|#{1,6}\s|[-*+]\s|\d+[.)]\s|>)/;

// A line of nothing but `-`, `*` or `_` runs is a rule, which ends the block
// beside it rather than continuing it.
const RULE = /^\s*((?:-[ \t]*){3,}|(?:\*[ \t]*){3,}|(?:_[ \t]*){3,})$/;

function ruleIsBlock(lines, index, softBreaks) {
  if (!RULE.test(lines[index] || "")) return false;
  if (!softBreaks) return true;
  return index === 0 || lines[index - 1].trim() === "" || Boolean(listItem(lines[index - 1]));
}

// Minimal syntax highlighter for common languages. It scans the raw code and
// escapes as it emits, so it is safe to inject into the DOM.
const LANGS = {
  rust: {
    keywords:
      "as async await break const continue crate dyn else enum extern false fn for if impl in let loop match mod move mut pub ref return self Self static struct super trait true type unsafe use where while Box Option Result String Vec Some None Ok Err".split(
        " ",
      ),
    line: ["//"],
    block: ["/*", "*/"],
  },
  js: {
    keywords:
      "await async break case catch class const continue default delete do else export extends false finally for from function if import in instanceof let new null of return super switch this throw true try typeof undefined var void while yield".split(
        " ",
      ),
    line: ["//"],
    block: ["/*", "*/"],
  },
  python: {
    keywords:
      "and as assert async await break class continue def del elif else except False finally for from global if import in is lambda None nonlocal not or pass raise return True try while with yield".split(
        " ",
      ),
    line: ["#"],
    block: null,
  },
  go: {
    keywords:
      "break case chan const continue default defer else fallthrough for func go goto if import interface map package range return select struct switch type var nil true false".split(
        " ",
      ),
    line: ["//"],
    block: ["/*", "*/"],
  },
  bash: {
    keywords:
      "if then else elif fi for while do done case esac function in return local export echo cd set source".split(
        " ",
      ),
    line: ["#"],
    block: null,
  },
  json: { keywords: [], line: ["//"], block: null },
};

function langOf(lang) {
  const value = String(lang || "").toLowerCase();
  if (["rs", "rust"].includes(value)) return "rust";
  if (["js", "jsx", "ts", "tsx", "javascript", "typescript", "mjs", "cjs"].includes(value)) return "js";
  if (["py", "python"].includes(value)) return "python";
  if (["go", "golang"].includes(value)) return "go";
  if (["sh", "bash", "shell", "zsh", "console"].includes(value)) return "bash";
  if (["json", "jsonc"].includes(value)) return "json";
  return null;
}

const IDENT_START = /[A-Za-z_$]/;
const IDENT = /[A-Za-z0-9_$]/;
const DIGIT = /[0-9]/;

function tok(cls, text) {
  return `<span class="tok-${cls}">${escapeHtml(text)}</span>`;
}

function highlight(code, lang) {
  const spec = LANGS[langOf(lang)];
  if (!spec) return escapeHtml(code);
  const keywords = new Set(spec.keywords);
  const n = code.length;
  let out = "";
  let i = 0;
  while (i < n) {
    const c = code[i];
    if (spec.block && code.startsWith(spec.block[0], i)) {
      const end = code.indexOf(spec.block[1], i + spec.block[0].length);
      const stop = end === -1 ? n : end + spec.block[1].length;
      out += tok("comment", code.slice(i, stop));
      i = stop;
      continue;
    }
    const mark = spec.line.find((prefix) => code.startsWith(prefix, i));
    if (mark) {
      let end = code.indexOf("\n", i);
      if (end === -1) end = n;
      out += tok("comment", code.slice(i, end));
      i = end;
      continue;
    }
    if (c === '"' || c === "'" || c === "`") {
      let j = i + 1;
      while (j < n) {
        if (code[j] === "\\") {
          j += 2;
          continue;
        }
        if (code[j] === c) {
          j += 1;
          break;
        }
        if (c !== "`" && code[j] === "\n") break;
        j += 1;
      }
      const stop = Math.min(j, n);
      out += tok("string", code.slice(i, stop));
      i = stop;
      continue;
    }
    if (DIGIT.test(c) && (i === 0 || !IDENT.test(code[i - 1]))) {
      let j = i;
      while (j < n && /[0-9a-fA-FxX._]/.test(code[j])) j += 1;
      out += tok("number", code.slice(i, j));
      i = j;
      continue;
    }
    if (IDENT_START.test(c)) {
      let j = i;
      while (j < n && IDENT.test(code[j])) j += 1;
      const word = code.slice(i, j);
      out += keywords.has(word) ? tok("keyword", word) : escapeHtml(word);
      i = j;
      continue;
    }
    out += escapeHtml(c);
    i += 1;
  }
  return out;
}

function splitRow(line) {
  let row = line.trim();
  if (row.startsWith("|")) row = row.slice(1);
  let slashes = 0;
  while (row[row.length - 2 - slashes] === "\\") slashes += 1;
  if (row.endsWith("|") && slashes % 2 === 0) row = row.slice(0, -1);
  const cells = [];
  let cell = "";
  let ticks = 0;
  for (let index = 0; index < row.length; index += 1) {
    const char = row[index];
    if (char === "\\") {
      cell += char;
      if (index + 1 < row.length) cell += row[++index];
      continue;
    }
    if (char === "`") {
      let run = 1;
      while (row[index + run] === "`") run += 1;
      if (!ticks) ticks = run;
      else if (ticks === run) ticks = 0;
      cell += "`".repeat(run);
      index += run - 1;
      continue;
    }
    if (char === "|" && !ticks) {
      cells.push(cell.trim());
      cell = "";
      continue;
    }
    cell += char;
  }
  cells.push(cell.trim());
  return cells;
}

function isTableSeparator(line) {
  return /^\s*\|?\s*:?-{1,}:?\s*(\|\s*:?-{1,}:?\s*)*\|?\s*$/.test(line) && line.includes("-");
}

// The alignment a separator row asks for, so the rendered cells line up the way
// the source says and a read of the table can write the row back as it was.
function tableAligns(separator) {
  return splitRow(separator).map((cell) => {
    const left = cell.startsWith(":");
    const right = cell.endsWith(":");
    return left && right ? "center" : left ? "left" : right ? "right" : "";
  });
}

function tableHtml(header, rows, references = false, aligns = [], separator = "") {
  const cell = (tag, text, index) => {
    const align = aligns[index] ? ` data-align="${aligns[index]}"` : "";
    return `<${tag}${align}>${inline(text, references)}</${tag}>`;
  };
  const head = header.map((text, index) => cell("th", text, index)).join("");
  const body = rows
    .map((row) => `<tr>${row.map((text, index) => cell("td", text, index)).join("")}</tr>`)
    .join("");
  // The separator row is drawn, not restated: it is the one line of a table that
  // says how its columns line up, and a read of the table has to give back the
  // row the reader typed rather than one inferred from the headers.
  const row = separator && references ? ` data-editor-separator="${escapeHtml(separator.trim())}"` : "";
  return `<table${row}><thead><tr>${head}</tr></thead><tbody>${body}</tbody></table>`;
}

/// A closing fence is a run of the same character at least as long as the one
/// that opened the block.
function isFenceEnd(line, run) {
  const trimmed = line.trim();
  return trimmed.length >= run.length && trimmed === run[0].repeat(trimmed.length);
}

/// A line that opens a table: the row under it is the separator, so the two are
/// one block rather than a paragraph — which is what keeps a table being typed
/// row by row from being split by the blank line between two paragraphs.
function startsTable(lines, index) {
  return Boolean(lines[index]?.includes("|") && index + 1 < lines.length && isTableSeparator(lines[index + 1]));
}

// A list item's prefix — the indentation, the bullet or number, and the space
// after it — is what a rendered list has to give back, so it is carried on the
// item itself: a composer that normalized `*` to `-`, or renumbered `3.` to
// `1.`, would be editing the text under the reader's caret.
const LIST_ITEM = /^([ \t]*(?:[-*+]|\d+[.)])[ \t]+)(.*)$/;

function listItem(line) {
  const match = LIST_ITEM.exec(line);
  if (!match) return null;
  const prefix = match[1];
  const number = /(\d+)[.)][ \t]+$/.exec(prefix)?.[1];
  return {
    prefix,
    indent: /^[ \t]*/.exec(prefix)[0].length,
    number: number == null ? null : Number(number),
    content: match[2],
    task: /^\[([ xX])\][ \t]+(.*)$/.exec(match[2]),
  };
}

function stripIndent(line, width) {
  let consumed = 0;
  let index = 0;
  while (index < line.length && consumed < width && /[ \t]/.test(line[index])) {
    consumed += 1;
    index += 1;
  }
  return line.slice(index);
}

// One list run, item by item: an item indented past the one before it nests
// inside it, so a nested list keeps its levels instead of being flattened into
// its parent's, and a line under an item that is not a block of its own
// continues the item rather than starting a paragraph beside it.
function renderList(lines, start, softBreaks) {
  const out = [];
  const open = [];
  let i = start;
  let gap = "";
  while (i < lines.length) {
    if (lines[i].trim() === "") {
      let next = i;
      while (next < lines.length && lines[next].trim() === "") next += 1;
      const following = listItem(lines[next] || "");
      if (!open.length || !following || following.indent < open[0].indent) break;
      gap = "\n".repeat(next - i);
      i = next;
    }
    const item = listItem(lines[i]);
    if (!item) break;
    const tag = item.number == null ? "ul" : "ol";
    while (open.length && item.indent < open[open.length - 1].indent) out.push(`</li></${open.pop().tag}>`);
    if (open.length && open[open.length - 1].indent === item.indent && open[open.length - 1].tag !== tag) {
      out.push(`</li></${open.pop().tag}>`);
    }
    let level = open[open.length - 1];
    let opened = false;
    if (level && level.indent === item.indent) {
      out.push("</li>");
    } else {
      const begin = tag === "ol" && item.number !== 1 ? ` start="${item.number}"` : "";
      out.push(`<${tag}${begin}${item.task && tag === "ul" ? ' class="tasks"' : ""}>`);
      level = { tag, indent: item.indent, expected: item.number == null ? null : item.number + 1 };
      open.push(level);
      opened = true;
    }
    let attributes = "";
    if (tag === "ol") {
      // `start` numbers the first item; a number that does not continue the run
      // after it is written on the item itself.
      if (!opened && item.number !== level.expected) attributes += ` value="${item.number}"`;
      level.expected = item.number + 1;
    }
    if (item.task) attributes += ' class="task"';
    const listGap = softBreaks && gap ? ` data-editor-gap="${escapeHtml(gap)}"` : "";
    gap = "";
    const pieces = [];
    let run = [{ text: item.task ? item.task[2] : item.content, pad: "" }];
    const flushRun = () => {
      if (!run.length) return;
      const text = softBreaks
        ? run.map(({ text: line, pad }, index) =>
            `${index ? `<br data-editor-pad="${escapeHtml(pad)}">` : ""}${inline(line, true)}`,
          ).join("")
        : inline(run.map(({ text: line }) => line).join("\n"), false, true);
      pieces.push(text);
      run = [];
    };
    const contentIndent = item.prefix.length;
    let pendingGap = "";
    while (i + 1 < lines.length) {
      const next = lines[i + 1];
      if (next.trim() === "") {
        let following = i + 1;
        while (following < lines.length && lines[following].trim() === "") following += 1;
        const child = lines[following] || "";
        const indent = /^[ \t]*/.exec(child)[0].length;
        if (listItem(child) || indent < contentIndent) break;
        pendingGap = "\n".repeat(following - i - 1);
        i = following - 1;
        continue;
      }
      if (listItem(next)) break;
      const indentText = /^[ \t]*/.exec(next)[0];
      if (indentText.length < contentIndent) break;
      const body = next.slice(indentText.length);
      const rest = [body];
      for (let lookahead = i + 2; lookahead < lines.length && rest.length < 2; lookahead += 1) {
        if (lines[lookahead].trim() === "") break;
        const lookIndent = /^[ \t]*/.exec(lines[lookahead])[0].length;
        if (lookIndent < contentIndent) break;
        rest.push(stripIndent(lines[lookahead], contentIndent));
      }
      if (pendingGap || BLOCK_START.test(body) || RULE.test(body) || startsTable(rest, 0)) {
        flushRun();
        const block = [];
        const pad = next.slice(0, contentIndent);
        while (i + 1 < lines.length) {
          const line = lines[i + 1];
          if (line.trim() === "") {
            let following = i + 1;
            while (following < lines.length && lines[following].trim() === "") following += 1;
            const child = lines[following] || "";
            const indent = /^[ \t]*/.exec(child)[0].length;
            if (listItem(child) || indent < contentIndent) break;
          }
          const child = listItem(line);
          if (child && child.indent > item.indent) break;
          if (line.trim() !== "" && /^[ \t]*/.exec(line)[0].length < contentIndent) break;
          block.push(stripIndent(line, contentIndent));
          i += 1;
        }
        pieces.push(
          softBreaks
            ? `<div data-editor-list-block=""${pendingGap ? ` data-editor-gap="${escapeHtml(pendingGap)}"` : ""} data-editor-pad="${escapeHtml(pad)}">${renderMarkdown(block.join("\n"), softBreaks)}</div>`
            : `<div>${renderMarkdown(block.join("\n"), softBreaks)}</div>`,
        );
        pendingGap = "";
        continue;
      }
      run.push({ text: body, pad: indentText });
      i += 1;
    }
    flushRun();
    const checked = item.task ? item.task[1].toLowerCase() === "x" : false;
    const mark = item.task
      ? softBreaks
        ? `<input type="checkbox" data-editor-task=""${checked ? " checked" : ""} contenteditable="false" tabindex="-1">`
        : `${checked ? "☑" : "☐"} `
      : "";
    const text = pieces.join("");
    const prefix = softBreaks ? ` data-editor-prefix="${escapeHtml(item.prefix)}"` : "";
    out.push(`<li${attributes}${listGap}${prefix}>${mark}${text}`);
    i += 1;
  }
  while (open.length) out.push(`</li></${open.pop().tag}>`);
  return { html: out.join(""), next: i };
}

function paintMarkdown(body, text) {
  const html = renderMarkdown(text);
  if (globalThis.OxideDiagrams) globalThis.OxideDiagrams.paint(body, html);
  else body.innerHTML = html;
}

function renderMarkdown(text, softBreaks = false) {
  const lines = String(text).split("\n");
  const out = [];
  let i = 0;

  while (i < lines.length) {
    const line = lines[i];

    const fence = line.match(/^([ \t]*)(`{3,}|~{3,})(.*)$/);
    if (fence && !(fence[2][0] === "`" && fence[3].includes("`"))) {
      const indent = fence[1];
      const run = fence[2];
      const info = fence[3];
      const code = [];
      i += 1;
      while (i < lines.length && !isFenceEnd(lines[i], run)) {
        code.push(lines[i]);
        i += 1;
      }
      // A fence still being typed has no closing run yet, and reading one back
      // as closed would put a fence the reader never wrote under the caret.
      const closed = i < lines.length;
      const closing = closed ? lines[i] : null;
      const closingRun = closing?.trim() || "";
      const closingIndent = closing ? /^[ \t]*/.exec(closing)[0] : "";
      const language = info.trim().split(/\s+/, 1)[0];
      const complete = closed && language.toLowerCase() === "mermaid" && !softBreaks;
      i += 1;
      const lang = language ? ` data-lang="${escapeHtml(language.toLowerCase())}"` : "";
      const open = closed || !softBreaks ? "" : ' data-editor-open=""';
      const marker = run === "```" || !softBreaks ? "" : ` data-editor-fence="${escapeHtml(run)}"`;
      const editorInfo = softBreaks ? ` data-editor-info="${escapeHtml(info)}"` : "";
      const editorIndent = softBreaks && indent ? ` data-editor-fence-indent="${escapeHtml(indent)}"` : "";
      const editorClose = softBreaks && closed && closingRun !== run ? ` data-editor-close="${escapeHtml(closingRun)}"` : "";
      const editorCloseIndent = softBreaks && closed && closingIndent !== indent
        ? ` data-editor-close-indent="${escapeHtml(closingIndent)}"`
        : "";
      out.push(`<pre${lang}${complete ? ' data-complete="true"' : ""}${marker}${editorInfo}${editorIndent}${editorClose}${editorCloseIndent}${open}><code>${highlight(code.join("\n"), language)}</code></pre>`);
      continue;
    }

    const rule = line.match(RULE);
    if (rule && ruleIsBlock(lines, i, softBreaks)) {
      const marker = rule[1].trim();
      out.push(marker === "---" || !softBreaks ? "<hr />" : `<hr data-editor-marker="${escapeHtml(marker)}" />`);
      i += 1;
      continue;
    }

    const heading = line.match(/^ {0,3}(#{1,6})[ \t]+(.*)$/);
    if (heading) {
      const level = heading[1].length;
      const indent = softBreaks ? /^ {0,3}/.exec(line)[0] : "";
      const attribute = indent ? ` data-editor-heading-indent="${indent}"` : "";
      out.push(`<h${level}${attribute}>${inline(heading[2], softBreaks)}</h${level}>`);
      i += 1;
      continue;
    }

    if (/^\s*>\s?/.test(line)) {
      const quote = [];
      while (i < lines.length && /^\s*>\s?/.test(lines[i])) {
        quote.push(lines[i].replace(/^\s*>\s?/, ""));
        i += 1;
      }
      const openTail = softBreaks && quote.length > 0 && quote[quote.length - 1].trim() === "";
      const prefix = softBreaks ? /^\s*>\s?/.exec(line)?.[0] || "> " : "";
      out.push(
        `<blockquote${prefix ? ` data-editor-prefix="${escapeHtml(prefix)}"` : ""}>${renderMarkdown(quote.join("\n"), softBreaks)}` +
          `${openTail ? '<p data-editor-padding=""><br></p><span data-editor-quote-tail=""></span>' : ""}</blockquote>`,
      );
      continue;
    }

    if (listItem(line)) {
      const list = renderList(lines, i, softBreaks);
      out.push(list.html);
      i = list.next;
      continue;
    }

    // An underlined line is the other way a heading is written; the line that
    // underlines it is carried on the heading so a read gives it back. It comes
    // after the list and quote branches, so their own `-` markers win.
    const underline = !softBreaks && line.trim() && /^ {0,3}(=+|-+)[ \t]*$/.exec(lines[i + 1] || "");
    if (underline) {
      const level = underline[1][0] === "=" ? 1 : 2;
      const marker = softBreaks ? ` data-editor-marker="${escapeHtml(underline[1])}"` : "";
      out.push(`<h${level}${marker}>${inline(line, softBreaks)}</h${level}>`);
      i += 2;
      continue;
    }

    if (startsTable(lines, i)) {
      const header = splitRow(line);
      const separator = lines[i + 1];
      const aligns = tableAligns(separator);
      const rows = [];
      i += 2;
      while (i < lines.length && lines[i].includes("|") && lines[i].trim() !== "") {
        rows.push(splitRow(lines[i]));
        i += 1;
      }
      out.push(tableHtml(header, rows, softBreaks, aligns, separator));
      continue;
    }

    if (line.trim() === "") {
      i += 1;
      continue;
    }

    const para = [line];
    i += 1;
    while (i < lines.length && lines[i].trim() !== "" && !BLOCK_START.test(lines[i]) && !ruleIsBlock(lines, i, softBreaks) && !startsTable(lines, i)) {
      para.push(lines[i]);
      i += 1;
    }
    out.push(`<p>${softBreaks ? para.map((line) => inline(line, true)).join("<br>") : inline(para.join("\n"), false, true)}</p>`);
  }
  return out.join("");
}

/// One rendered diff: every line marked by its own first character, so a line's
/// text can never be mistaken for a `+`/`-` marker.
function diffLines(text) {
  return String(text || "")
    .split("\n")
    .map((line) => {
      let cls = "";
      if (line.startsWith("@@")) cls = "hunk";
      else if (line.startsWith("+")) cls = "add";
      else if (line.startsWith("-")) cls = "del";
      return `<span class="dline ${cls}">${escapeHtml(line)}</span>`;
    })
    .join("");
}

function diffBlock(diff) {
  const el = document.createElement("div");
  el.className = "diff";
  el.innerHTML = `<div class="diff-path">${escapeHtml(diff.path || "diff")}</div><pre>${diffLines(
    diff.text,
  )}</pre>`;
  return el;
}

// ---------- welcome ----------

// How many threads the home state offers before the sidebar becomes the way to
// the rest of them: enough to pick up where the reader left off, not a second
// listing of everything stored.
const MAX_RECENT_THREADS = 5;

/// What fills the transcript before a thread is in it — the window's home. With
/// a project it is the invitation, the folder and the suggestions; with none it
/// is where a thread is picked up again: the newest threads across every folder
/// the sidebar lists, since a window that opens on no project should open on
/// something to do rather than on an empty box. The composer is usable either
/// way — the project chip under it takes the folder.
function renderWelcome() {
  const project = state.project;
  const welcome = document.createElement("div");
  welcome.className = "welcome";
  const mark = document.createElement("div");
  mark.className = "welcome-mark";
  mark.innerHTML = ICONS.mark;
  const title = document.createElement("h1");
  title.textContent = t("home.title", "What should we build?");
  const hint = document.createElement("p");
  hint.textContent = project
    ? project
    : (state.projects || []).length
      ? t(
          "home.pickFolder",
          "Pick a folder with the project chip below, or open a recent thread.",
        )
      : t(
          "home.addFolder",
          "Add a folder to run in — the project chip below, or the + beside Projects.",
        );
  welcome.append(mark, title, hint);
  if (project) {
    const suggestions = document.createElement("div");
    suggestions.className = "suggestions";
    for (const [index, english] of SUGGESTIONS.entries()) {
      const text = t(`home.suggestion${index + 1}`, english);
      const button = document.createElement("button");
      button.dataset.prompt = text;
      button.textContent = text;
      button.onclick = () => {
        el("prompt").value = text;
        el("prompt").focus();
        updateSendState();
      };
      suggestions.appendChild(button);
    }
    welcome.appendChild(suggestions);
  } else {
    const recent = recentThreads();
    if (recent.length) welcome.appendChild(recentList(recent));
  }
  el("transcript").innerHTML = "";
  el("transcript").appendChild(welcome);
}

/// The newest threads across every project the sidebar lists — the same rows it
/// groups under each folder, which is where a window with nothing open resumes
/// one. The stand-in row for a thread the store has not written is left out:
/// there is no thread on screen at home, so there is nothing to stand in for.
function recentThreads() {
  const listed = new Set((state.projects || []).map((project) => project.path));
  return listedSessions()
    .filter((session) => !session.unstored && listed.has(sessionProject(session)))
    .slice(0, MAX_RECENT_THREADS);
}

function recentList(threads) {
  const box = document.createElement("div");
  box.className = "recent";
  const label = document.createElement("div");
  label.className = "recent-label";
  label.textContent = t("sidebar.recentThreads", "Recent threads");
  box.appendChild(label);
  for (const session of threads) {
    const row = document.createElement("button");
    row.type = "button";
    row.className = "recent-thread";
    const name = document.createElement("span");
    name.className = "recent-name";
    name.textContent = sessionLabel(session);
    const meta = document.createElement("span");
    meta.className = "recent-meta";
    // A worktree thread is named under its project, with the checkout beside it:
    // the folder is where the conversation belongs and the checkout is where its
    // files are.
    meta.textContent = [
      projectNameOf(sessionProject(session)),
      session.worktree ? t("worktree.label", "worktree {value0}", { value0: session.worktree }) : "",
      sessionAge(session.modified_at),
    ]
      .filter(Boolean)
      .join(" · ");
    row.append(name, meta);
    row.title = `${name.textContent} — ${meta.textContent}`;
    row.onclick = () => selectSessionFromTree(session);
    box.appendChild(row);
  }
  return box;
}

function clearWelcome() {
  const welcome = el("transcript").querySelector(".welcome");
  if (welcome) welcome.remove();
}

// ---------- projects ----------

async function loadProjects() {
  try {
    state.projects = await invoke("list_projects");
    await loadSessions(); // This will also call renderProjectsTree
    updateProjectChip();
    // The chip can be clicked before this request answers, when it still has
    // nothing to offer — so a picker already on screen is repainted with the
    // folders that just arrived rather than left saying there are none until it
    // is closed and opened again.
    if (!el("projects-modal").hidden) renderProjects();
  } catch (error) {
    showToast(t("ui.failedToLoadProjects", "Failed to load projects: {value0}", { value0: error }), "error");
  }
}

/// The name a folder is listed under: the row the sidebar draws for it, else its
/// basename, so a path with no row still reads as a name.
function projectNameOf(path) {
  const project = (state.projects || []).find((entry) => entry.path === path);
  return project ? project.name : folderBasename(path);
}

/// The folders this window can run in — what the composer's project chip opens,
/// and the same list the sidebar draws: a folder added in the app first, then one
/// that only exists because a thread was started in it. Picking a row opens that
/// project, which is also how a thread starts in it: selecting a project clears
/// the transcript, so the next message opens a thread of its own.
function openProjects() {
  closeOverlays("projects-modal");
  renderProjects();
  el("projects-modal").hidden = false;
}

let projectsOldestFirst = false;

function closeProjectDirectory() {
  if (el("projects-page").hidden) return;
  closeSidebarCards();
  el("projects-page").hidden = true;
  document.body.classList.remove("projects-open");
  el("rail-projects").removeAttribute("aria-current");
  el("rail-home").setAttribute("aria-current", "page");
}

function openProjectDirectory() {
  closeCustomize();
  closeOverlays();
  closeSidebarCards();
  document.body.classList.add("projects-open");
  el("projects-page").hidden = false;
  el("rail-home").removeAttribute("aria-current");
  el("rail-projects").setAttribute("aria-current", "page");
  el("projects-filter").value = "";
  renderProjectDirectory();
  el("projects-filter").focus();
}

function renderProjectDirectory() {
  const box = el("projects-directory");
  box.replaceChildren();
  const query = String(el("projects-filter").value || "").trim().toLocaleLowerCase();
  const updated = (project) => Math.max(project.last_session_at || 0, project.last_opened_at || 0);
  const projects = (state.projects || []).filter((project) =>
    `${project.name} ${project.path}`.toLocaleLowerCase().includes(query),
  ).sort((a, b) => (projectsOldestFirst ? updated(a) - updated(b) : updated(b) - updated(a)) || a.name.localeCompare(b.name));
  el("projects-sort").textContent = projectsOldestFirst ? t("projects.updatedAsc", "Updated ↑") : t("projects.updatedDesc", "Updated ↓");
  el("projects-sort").setAttribute("aria-pressed", String(projectsOldestFirst));
  if (!projects.length) {
    const empty = document.createElement("div");
    empty.className = "dialog-empty";
    empty.textContent = query ? t("projects.noMatches", "No projects match your search.") : t("desktop.empty.projects", "No projects yet. Add a folder to run in.");
    box.appendChild(empty);
    return;
  }
  for (const project of projects) {
    const row = document.createElement("div");
    row.className = "project-directory-row";
    row.dataset.project = project.id;
    const open = document.createElement("button");
    open.type = "button";
    open.className = "project-directory-open";
    open.title = project.path;
    const icon = document.createElement("span");
    icon.innerHTML = ICONS.folder;
    const name = document.createElement("span");
    name.textContent = project.name;
    open.append(icon, name);
    open.onclick = () => { closeProjectDirectory(); return pickProject(project); };
    const age = document.createElement("span");
    age.className = "project-directory-updated";
    age.textContent = sessionAge(updated(project)) || t("projects.noActivity", "No activity");
    const actions = document.createElement("div");
    actions.className = "project-directory-actions";
    const action = (drawing, label) => {
      const button = document.createElement("button");
      button.type = "button";
      button.className = "icon";
      button.title = label;
      button.setAttribute("aria-label", label);
      button.innerHTML = drawing;
      actions.appendChild(button);
      return button;
    };
    const more = action(ICONS.more, t("ui.projectOptionsFor", "Project options for {value0}", { value0: project.name }));
    more.setAttribute("aria-haspopup", "menu");
    more.setAttribute("aria-expanded", "false");
    more.onclick = () => more.getAttribute("aria-expanded") === "true" ? closeSidebarCards() : showProjectActions(project, listedSessions().filter((session) => sessionProject(session) === project.path), more, more);
    const pin = action(ICONS.pin, project.pinned ? t("ui.unpinProject", "Unpin project") : t("ui.pinProject", "Pin project"));
    pin.setAttribute("aria-pressed", String(Boolean(project.pinned)));
    pin.onclick = async () => {
      pin.disabled = true;
      try {
        state.projects = await invoke("set_project_pinned", { id: project.id, pinned: !project.pinned });
        renderProjectsTree();
      } catch (error) {
        showToast(String(error), "error");
      } finally { pin.disabled = false; }
    };
    const compose = action(ICONS.compose, t("ui.newTaskIn", "New task in {value0}", { value0: project.name }));
    compose.onclick = () => { closeProjectDirectory(); return newTaskIn(project); };
    row.append(open, age, actions);
    box.appendChild(row);
  }
}

/// Everything that needs a folder asks for one the same way: the picker of the
/// rows the sidebar draws, or the Add-project dialog when there is nothing to
/// pick at all. A reader who typed a message before choosing is not sent looking
/// for a path they have never added.
function askForProject() {
  if ((state.projects || []).length) {
    openProjects();
    return;
  }
  openCreateProject();
}

function renderProjects() {
  const box = el("project-list");
  box.innerHTML = "";
  const projects = state.projects || [];
  if (!projects.length) {
    box.innerHTML = `<div class="dialog-empty">${escapeHtml(t("desktop.empty.projects", "No projects yet. Add a folder to run in."))}</div>`;
    return;
  }
  for (const project of projects) {
    const row = document.createElement("button");
    row.type = "button";
    row.className = `project-row${project.path === state.project ? " active" : ""}`;
    const main = document.createElement("div");
    main.className = "project-main";
    const label = document.createElement("div");
    label.className = "project-label";
    label.textContent = project.name;
    const path = document.createElement("div");
    path.className = "project-path";
    path.textContent = project.path;
    main.append(label, path);
    row.appendChild(main);
    row.title = project.path;
    row.onclick = () => pickProject(project);
    box.appendChild(row);
  }
}

/// Picking the project already open only puts the picker away: selecting it
/// again would clear a transcript the reader did not ask to leave.
async function pickProject(project) {
  el("projects-modal").hidden = true;
  if (!project || project.path === state.project) return;
  await selectProject(project);
}

async function selectProject(project) {
  closeCustomize();
  // A running turn keeps the folder it started in, and its conversation goes on
  // in its own thread record: the header's strip and the sidebar row the run's
  // own mark keep saying where it is while the window looks at another folder,
  // and the reader can send a message here without waiting for that turn.
  parkThread();
  // A folder picked while nothing was open is the first step of the message
  // already in the box, so the draft goes with it: the chips came from the
  // reader rather than from the project being left. A switch between folders
  // drops them, since they were meant for the one being left behind.
  const wasOpen = Boolean(state.project);
  if (state.project !== project.path) {
    state.agent = "";
    el("agents").title = t("ui.agentsDefault", "Agents: Default");
  }
  state.project = project.path;
  state.projectName = project.name;
  state.session = null;
  state.trust = null;
  // The checkout belonged to the folder being left: a worktree of one project
  // is not a folder of another, so the next message starts in this project's
  // own tree until the reader picks one of its worktrees.
  state.worktree = null;
  state.worktrees = [];
  paintWorktreeChip();
  // The queue belongs to the thread being left with its run, so the composer
  // shows this folder's own — nothing, until it has a message queued.
  paintQueue();
  // The window the ring measures against belonged to the folder being left, and
  // the one arriving is not known until its own facts do: left in place it would
  // label this folder's thread with another folder's window.
  state.contextWindow = 0;
  state.catalogWindow = 0;
  state.contextCorrection = null;
  if (wasOpen) clearAttachments();
  el("projects-modal").hidden = true;
  el("trust-modal").hidden = true;
  updateTrustButton();
  updateChips();
  setStatus(idleStatus());
  renderProjectsTree(); // Update tree view instead of dropdown
  resetTranscript();
  // The folder moved, so the composer's own top row has to be read again rather
  // than carried over from the folder being left. The context ring is painted
  // beside the totals by the same read, and again when the folder's own window
  // arrives.
  loadGit();
  await Promise.all([loadInfo(), loadSessions(), loadTheme(), loadWorktrees()]);
}

function correctionWindow(correction = state.contextCorrection) {
  const learned = [correction, ...(state.contextLimits || [])].find((limit) => limit &&
    limit.project === state.project && limit.session === state.session &&
    limit.provider === state.currentProvider && limit.model === state.currentModel)?.tokens ?? null;
  return learned == null ? null : Math.min(learned, state.catalogWindow || learned);
}

function applyContextInfo(info) {
  state.currentProvider = info.provider || "";
  state.currentModel = info.model || "";
  state.catalogWindow = info.contextWindow || 0;
  state.contextWindow = correctionWindow() ?? state.catalogWindow;
}

async function loadInfo() {
  if (!state.project) return;
  const project = state.project;
  try {
    const info = await invoke("project_info", { project });
    // The selection can change while the request is in flight; a late reply
    // must not replace the new project's trust state or open its dialog.
    if (project !== state.project) return;
    // The project or model moved on, so a levels read still in flight is stale.
    reasoningEpoch += 1;
    const identity = `${project}\n${info.provider}\n${info.model}`;
    const changedModel = state.reasoningIdentity && identity !== state.reasoningIdentity;
    state.reasoningIdentity = identity;
    state.supportsReasoning = info.supportsReasoning !== false &&
      Array.isArray(info.reasoningLevels) && info.reasoningLevels.length > 0;
    state.reasoning = info.reasoning || "auto";
    state.reasoningPicked = Boolean(changedModel);
    state.reasoningLevels = state.supportsReasoning ? ["auto", ...info.reasoningLevels] : null;
    applyContextInfo(info);
    // A plan rather than a metered key: the spend is what the plan would have
    // billed, and the usage line says so.
    state.subscription = Boolean(info.subscription);
    state.trust = info.trust || null;
    // A project may pin its own language in `.oxide/settings.json`, so the
    // language is read again when the folder changes rather than once per
    // window.
    void loadLanguage();
    updateChips();
    void ensureReasoningLevels();
    renderProjectMeta(info);
    updateTrustButton();
    // The window the thread's own totals are measured against arrives with the
    // folder's own facts, so the usage line and the ring are drawn again from
    // it: an empty window until now is why neither could name a percentage.
    paintUsage();
    if (state.trust && state.trust.awaiting) showTrust(state.trust);
  } catch (error) {
    if (project !== state.project) return;
    el("project-meta").textContent = String(error);
    el("project-meta").hidden = false;
  }
}

/// The window a model resolves to is what a catalog that landed after this
/// folder was opened changes, so the lookup is read again and the chip repainted
/// from it — and nothing else is: the level the reader picked in the composer
/// stays picked, since a background lookup is no reason to hand them the level
/// the folder remembers. The model's own levels are left to the reasoning chip,
/// which reads them for the model in use.
async function refreshContextWindow() {
  if (!state.project) return;
  const project = state.project;
  try {
    const info = await invoke("project_info", { project });
    if (project !== state.project || !info) return;
    applyContextInfo(info);
    renderProjectMeta(info);
    paintUsage();
  } catch {
    // A window that could not be read again keeps the one it has: the next full
    // load reports it, and a background lookup is not worth a note of its own.
  }
}

/// The model the composer's own chip names, and the little the header adds: the
/// provider is already that chip, so only what the user has to act on — a
/// missing key, project resources left off — is written beside the title.
function renderProjectMeta(info) {
  state.currentModel = info.model || "";
  state.currentProvider = info.provider || "";
  const window = state.contextWindow ? ` · ${formatTokens(state.contextWindow)}` : "";
  labelControl("model", t("ui.model", "model: {value0}{value1}", { value0: info.model, value1: window }), t("ui.switchModel", "Switch model"));
  el("model-value").textContent = modelDisplayName(info.model);
  const notes = [];
  if (!info.hasKey) notes.push(t("ui.noAPIKey", "no API key"));
  if (info.trust && info.trust.required && !info.trust.trusted) {
    notes.push(t("ui.projectResourcesOff", "project resources off"));
  }
  el("project-meta").textContent = notes.join(" · ");
  el("project-meta").hidden = notes.length === 0;
}

async function openAgents() {
  if (!state.project) { setStatus(t("composer.selectProject", "Select a project first.")); askForProject(); return; }
  const project = state.project;
  try {
    const info = await invoke("project_info", { project });
    if (state.project !== project) return;
    const list = el("agents-list");
    list.replaceChildren();
    for (const agent of [{ name: "", description: t("ui.defaultAgent", "Default agent") }, ...(info.agents || [])]) {
      const row = document.createElement("button");
      row.className = "details-row";
      row.textContent = agent.name || t("ui.default", "Default");
      row.title = agent.description || agent.name;
      row.setAttribute("aria-pressed", String(state.agent === agent.name));
      row.onclick = () => {
        state.agent = agent.name;
        el("agents").title = t("ui.agents", "Agents: {value0}", { value0: agent.name || "Default" });
        el("agents-modal").hidden = true;
        el("agents").focus();
      };
      list.appendChild(row);
    }
    closeOverlays("agents-modal");
    el("agents-modal").hidden = false;
    list.querySelector("button")?.focus();
  } catch (error) { setStatus(String(error), "error"); }
}

function updateTrustButton() {
  const button = el("trust");
  if (!button) return;
  button.hidden = false;
  button.classList.toggle("active", Boolean(state.trust && state.trust.trusted));
  const label = state.trust && state.trust.trusted
    ? t("ui.projectAccessTrustedClickToReview", "Project access: trusted · click to review")
    : t("ui.projectAccessProjectResourcesAreNotTrusted", "Project access: project resources are not trusted");
  button.title = label;
  button.setAttribute("aria-label", label);
}

function showTrust(trust) {
  state.trust = trust;
  const box = el("trust-resources");
  box.innerHTML = "";
  for (const name of trust.resources || []) {
    const code = document.createElement("code");
    code.textContent = name;
    box.appendChild(code);
  }
  closeOverlays("trust-modal");
  el("trust-modal").hidden = false;
}

async function answerTrust(trusted) {
  if (!state.project) return;
  el("trust-modal").hidden = true;
  const project = state.project;
  try {
    const info = await invoke("set_project_trust", { project, trusted });
    if (project !== state.project) return;
    state.trust = info.trust || null;
    renderProjectMeta(info);
    updateTrustButton();
    showToast(trusted ? t("ui.projectTrusted", "Project trusted") : t("ui.projectResourcesLeftUntrusted", "Project resources left untrusted"));
  } catch (error) {
    showToast(t("ui.trustUpdateFailed", "Trust update failed: {value0}", { value0: error }), "error");
  }
}

function reasoningLabel(level) {
  return t(`reasoning.${level}`, level.charAt(0).toUpperCase() + level.slice(1));
}

/// The composer's one model control, `Model · Effort ⌄`, which the Tauri window
/// draws from the same rules: the effort half is left off for a model that
/// offers nothing but Auto.
function updateChips() {
  const effort = hasReasoningLevels();
  el("model-value").textContent = state.currentModel ? modelDisplayName(state.currentModel) : t("composer.model", "Model");
  const model = state.currentModel ? t("ui.model", "model: {value0}{value1}", { value0: state.currentModel, value1: state.contextWindow ? ` · ${formatTokens(state.contextWindow)}` : "" }) : t("composer.model", "Model");
  const thinking = effort ? ` · ${t("ui.thinking", "thinking: {value0}", { value0: t(`reasoning.${state.reasoning}`, state.reasoning) })}` : "";
  labelControl("model", model + thinking, t("ui.switchModel", "Switch model"));
  el("reasoning-value").hidden = !effort;
  el("reasoning-value").textContent = reasoningLabel(state.reasoning);
  if (modelMenuOpen()) renderModelSummary();
  updateProjectChip();
  paintWorktreeChip();
  // A folder switch resets the transcript, which is where the header's own
  // banner is decided.
  updateRunBanner();
}

/// The project chip names the folder the message goes to, and asks for one while
/// there is none: a project is not selected for the reader on the way in, so the
/// chip is where they pick it. It keeps its words rather than hiding them in a
/// tooltip, since with nothing open it reads as the action it is. The marker it
/// wears in that state is its own (`unset`): the sheet's `.empty` is the app's
/// placeholder class, which centers whatever wears it — and this chip sits on the
/// composer's top row, where the two controls are the folder and the checkout.
function updateProjectChip() {
  const open = Boolean(state.project);
  const placeholder = t("composer.chooseProject", "Choose a project");
  const hint = t("composer.chooseProjectHint", "Pick the folder this window runs in");
  const name = open ? projectNameOf(state.project) : placeholder;
  el("project-name").textContent = name;
  el("project").classList.toggle("unset", !open);
  el("project").title = open ? t("ui.project", "project: {value0}\n{value1}", { value0: name, value1: hint }) : `${placeholder}\n${hint}`;
  el("project").setAttribute("aria-label", open ? t("project.label", "project: {value0}", { value0: name }) : placeholder);
}

/// Keep each selector's full value and keyboard hint accessible.
function labelControl(id, label, hint) {
  const chip = el(id);
  chip.title = hint ? `${label}\n${hint}` : label;
  chip.setAttribute("aria-label", label);
}

/// The levels the context colors escalate through, which the terminal's footer
/// and the panel's own gauge use too: the arc says the same thing in every
/// front-end rather than each drawing its own thresholds.
function contextLevel(percent) {
  if (percent == null) return "ok";
  if (percent > 90) return "high";
  return percent > 70 ? "warn" : "ok";
}

/// The repository the row names — read again when the folder changes, when a
/// turn ends, since a run may switch branch, and when the reader comes back to
/// the window, since a branch switched in a terminal is theirs to see here.
async function loadGit() {
  const project = runFolder();
  if (!project) {
    state.git = null;
    paintComposerFacts();
    return;
  }
  try {
    const info = await invoke("git_info", { project });
    if (project !== runFolder()) return;
    state.git = info || null;
  } catch {
    // A folder whose repository could not be read has no branch to show, which
    // is the same row as a folder that is in no repository at all: the row says
    // what it knows and nothing else.
    if (project !== runFolder()) return;
    state.git = null;
  }
  paintComposerFacts();
  if (!el("details-panel").hidden || reviewState?.card.repository) refreshProjectDetails();
}

/// The composer's own top row, above the message box: the folder's chip, the
/// branch it is on and the checkout the next turn runs in. The totals and the
/// context reading are the box's own row, beside the action that sends.
/// Nothing but the folder chip is shown
/// before a folder is open, since there is no turn to place and no window to
/// measure: the row then says the one thing the reader can act on.
function paintComposerFacts() {
  const open = Boolean(state.project);
  const info = state.git;
  const branch = open && info?.repo ? info.branch || info.detached : "";
  const chip = el("git");
  chip.hidden = !branch;
  el("git-branch").textContent = branch;
  if (branch) {
    const root = info.root ? `\n${info.root}` : "";
    chip.title = info.branch
      ? t("ui.onBranch", "On branch {value0}{value1}", { value0: branch, value1: root })
      : t("ui.hEADIsDetachedAt", "HEAD is detached at {value0}{value1}", { value0: branch, value1: root });
  }

  const ring = el("context-ring");
  ring.hidden = !open;
  if (!open) return;
  const fill = el("context-ring-fill");
  // The ring's own circumference, read off the circle the sheet draws rather
  // than kept beside it: a dash as long as the circle is the ring at 100%, so
  // the filled part is what the thread's last request took of the window.
  const length = 2 * Math.PI * (Number(fill.getAttribute("r")) || 12.25);
  const percent = usagePercent(state.usage);
  const spent = percent == null ? 0 : Math.max(0, Math.min(percent, 100));
  fill.setAttribute(
    "stroke-dasharray",
    `${((length * spent) / 100).toFixed(2)} ${length.toFixed(2)}`,
  );
  ring.dataset.level = contextLevel(percent);
  // The percent the arc is filled to, inside it: the number and the arc are one
  // reading, and a thread that has spent nothing yet has none to spell out.
  el("context-percent").textContent = percent == null ? "" : `${Math.min(100, Math.max(0, percent)).toFixed(0)}%`;
  const limit = state.contextWindow ? formatTokens(state.contextWindow) : "";
  let label = t("ui.noRequestYet", "No request yet");
  if (percent != null) {
    label = `${percent.toFixed(0)}% · ${formatTokens(state.usage?.prompt || 0)}${
      limit ? `/${limit}` : ""
    }`;
  } else if (limit) {
    label = t("ui.noRequestYetWindow", "No request yet · {value0} window", { value0: limit });
  }
  ring.title = label;
  ring.setAttribute("aria-label", t("ui.contextWindow", "context window: {value0}", { value0: label }));
}

// ---------- worktrees ----------

/// The folder a turn runs in: the worktree the reader picked, or the project
/// itself. The branch row, a new thread and the thread being opened all read
/// this rather than the folder on screen, so a window reading a worktree thread
/// reads the checkout that thread actually runs in.
function runFolder() {
  return state.worktree || state.project;
}

/// The project a thread belongs to. A thread in a worktree belongs to the
/// project the checkout was made from — the host names it — and everything that
/// groups threads reads this rather than the thread's own folder.
function sessionProject(session) {
  return session.project || session.cwd;
}

function worktreeNamed(path) {
  if (!path) return null;
  return (state.worktrees || []).find((entry) => entry.path === path) || null;
}

/// The worktree a folder is, when it is one of the project's: a thread records
/// the checkout it ran in as its folder, which is how the window knows to send
/// its next message there.
function worktreePathFor(folder) {
  if (!folder || folder === state.project) return null;
  return folder;
}

/// The worktree the thread on screen runs in, if it runs in one: the thread's
/// own checkout when a thread is open, and the one the next message would use
/// when none is.
function threadWorktree() {
  const session = state.session ? sessionById(state.session) : null;
  return worktreeNamed(session ? session.cwd : state.worktree);
}

/// Reads the project's worktrees again, keeping why it could not for the
/// listing's own render to paint. The selected checkout stays with its thread
/// even if it disappears: sending that thread into the local folder would put
/// its next edits in a different tree.
async function loadWorktrees() {
  if (!state.project) {
    state.worktrees = [];
    state.worktreesError = null;
    paintWorktreeChip();
    return;
  }
  const project = state.project;
  try {
    const listed = await invoke("worktrees", { project });
    if (project !== state.project) return;
    state.worktrees = Array.isArray(listed) ? listed : [];
    state.worktreesError = null;
  } catch (error) {
    if (project !== state.project) return;
    state.worktrees = [];
    state.worktreesError = String(error);
  }
  paintWorktreeChip();
  paintThreadWorktree();
  if (!el("worktrees-modal").hidden) renderWorktrees();
}

/// The composer's own top row: the checkout the next turn runs in. It is a fact
/// of the folder like the branch beside it rather than a control on the row
/// inside the box, so it is drawn as one and names the worktree in the row's own
/// dim text, wearing the accent while a thread writes to a copy — which is the
/// difference between a thread that writes to a copy and one that writes to the
/// folder itself.
function paintWorktreeChip() {
  const chip = el("worktree");
  if (!chip) return;
  chip.hidden = !state.project;
  const entry = worktreeNamed(state.worktree);
  chip.classList.toggle("active", Boolean(entry));
  const name = el("worktree-label");
  if (name) {
    name.textContent = entry ? entry.name : "";
    name.hidden = !entry;
  }
  const label = entry ? t("ui.worktree", "worktree: {value0}", { value0: entry.name }) : t("composer.worktree", "Worktree");
  chip.title = entry
    ? t("ui.switchTheCheckoutTheNextTurnRunsIn", "{value0}\n{value1}\nSwitch the checkout the next turn runs in", { value0: label, value1: entry.path })
    : t("ui.worktreeRunAThreadInACopyOfTheProjectOnItsOwnBranch", "Worktree\nRun a thread in a copy of the project on its own branch");
  chip.setAttribute("aria-label", label);
  paintThreadWorktree();
}

/// The checkout the thread on screen runs in, beside the thread's name: the
/// same project can hold several worktrees, and two threads of it are not the
/// same files.
function paintThreadWorktree() {
  const badge = el("thread-worktree");
  if (!badge) return;
  const entry = state.project ? threadWorktree() : null;
  badge.hidden = !entry;
  if (!entry) {
    badge.innerHTML = "";
    return;
  }
  badge.innerHTML = `${ICONS.worktree}<span>${escapeHtml(entry.name)}</span>`;
  badge.title = t("ui.worktreeOn", "Worktree {value0} on {value1}\n{value2}", { value0: entry.name, value1: entry.branch, value2: entry.path });
}

function openWorktrees() {
  if (!state.project) {
    setStatus(t("composer.selectProject", "Select a project first."));
    askForProject();
    return;
  }
  closeOverlays("worktrees-modal");
  el("worktrees-modal").hidden = false;
  el("worktree").setAttribute("aria-expanded", "true");
  el("worktree-name").value = "";
  el("worktree-base").value = "";
  el("worktree-copy").checked = false;
  el("worktree-detached").checked = true;
  renderWorktrees();
  loadWorktrees();
}

function closeWorktrees() {
  el("worktrees-modal").hidden = true;
  el("worktree").setAttribute("aria-expanded", "false");
  el("worktree").focus();
}

/// The checkouts a thread can run in: the folder itself, then every worktree
/// the project holds. Picking one is picking the files the next thread works on,
/// so a thread that is already open is left — the checkout belongs to the task,
/// which is what makes two threads of one project readable side by side.
function renderWorktrees() {
  const box = el("worktree-list");
  if (!box) return;
  box.innerHTML = "";
  if (state.worktreesError) {
    box.innerHTML = `<div class="dialog-empty">${escapeHtml(t("desktop.worktreeReadError", "Could not read this project's worktrees: {error}", { error: state.worktreesError }))}</div>`;
    return;
  }
  el("worktree-branch").hidden = !state.worktree;
  el("worktree-handoff").hidden = !state.session;
  el("worktree-handoff").textContent = state.worktree ? t("ui.handOffChatToLocal", "Hand off chat to Local") : t("dialog.worktrees.handoff", "Hand off chat to Worktree");
  box.appendChild(worktreeRow(null));
  for (const entry of state.worktrees || []) box.appendChild(worktreeRow(entry));
}

function worktreeRow(entry) {
  // The row is the checkout and the ✕ inside it is the way to remove it, so it
  // is a row rather than a button: a control inside a control is not one the
  // page can hand a press to unambiguously.
  const row = document.createElement("div");
  row.className = "worktree-row" + (entry ? "" : " local");
  row.tabIndex = 0;
  row.setAttribute("role", "button");
  const selected = entry ? state.worktree === entry.path : !state.worktree;
  row.classList.toggle("active", selected);
  row.setAttribute("aria-pressed", String(selected));
  const name = document.createElement("div");
  name.className = "worktree-label";
  name.textContent = entry ? entry.name : t("ui.localCheckout", "Local checkout");
  row.appendChild(name);
  const detail = document.createElement("div");
  detail.className = "worktree-detail";
  const folder = state.project ? baseName(state.project) : "";
  detail.textContent = entry
    ? `${entry.branch} · ${entry.path}`
    : t("ui.thisProjectSOwnFolder", "this project's own folder{value0}", { value0: folder ? ` · ${folder}` : "" });
  row.appendChild(detail);
  row.title = entry
    ? t("ui.runAThreadIn", "Run a thread in {value0}\n{value1} · {value2}", { value0: entry.name, value1: entry.branch, value2: entry.path })
    : t("ui.runAThreadInThisProjectSOwnFolder", "Run a thread in this project's own folder");
  const take = () => selectWorktree(entry ? entry.path : null);
  row.onclick = take;
  row.onkeydown = (event) => {
    if (event.key !== "Enter" && event.key !== " ") return;
    event.preventDefault();
    take();
  };
  if (!entry) return row;

  // Removing a checkout is destructive, so it is asked about in the window's own
  // dialog rather than on the press, and it stays inside the row's own press.
  const remove = document.createElement("button");
  remove.type = "button";
  remove.className = "row-remove worktree-remove";
  remove.title = t("ui.removeTheWorktree", "Remove the worktree {value0}", { value0: entry.name });
  remove.setAttribute("aria-label", remove.title);
  remove.innerHTML = ICONS.close;
  remove.onclick = (event) => {
    event.stopPropagation();
    removeWorktree(entry);
  };
  row.appendChild(remove);
  return row;
}

function selectWorktree(path) {
  if (!state.project) return;
  const entry = worktreeNamed(path);
  const next = entry ? entry.path : null;
  if (next === state.worktree) {
    closeWorktrees();
    return;
  }
  if (!canStartNewChat()) return;
  state.worktree = next;
  // The checkout belongs to the task: a thread already on screen was written in
  // one place, and its next message is not the place to move it to.
  if (state.session) {
    const attachments = state.attachments;
    newChat();
    state.attachments = attachments;
    renderAttachments();
    updateSendState();
  }
  paintWorktreeChip();
  loadGit();
  showToast(
    entry
      ? t("ui.threadsRunInTheWorktreeOn", "Threads run in the worktree {value0} on {value1}.", { value0: entry.name, value1: entry.branch })
      : t("ui.threadsRunInThisProjectSOwnFolder", "Threads run in this project's own folder."),
  );
  closeWorktrees();
}

/// Makes a worktree and starts a thread in it. The name is the reader's, or one
/// derived from the project when they left it empty.
async function createWorktree() {
  if (!state.project) {
    setStatus(t("composer.selectProject", "Select a project first."));
    return;
  }
  if (!canStartNewChat()) return;
  const input = el("worktree-name");
  const requested = String(input.value || "").trim();
  const project = state.project;
  const source = runFolder();
  const button = el("worktree-create");
  if (button.disabled) return;
  button.disabled = true;
  try {
    const entry = await invoke("create_worktree", {
      project,
      name: requested || null,
      base: String(el("worktree-base").value || "").trim() || null,
      detached: el("worktree-detached").checked,
      copyChanges: el("worktree-copy").checked,
      source,
    });
    if (project !== state.project) return;
    input.value = "";
    await loadWorktrees();
    if (!state.worktrees.some((item) => item.path === entry.path)) state.worktrees.push(entry);
    selectWorktree(entry.path);
    showToast(t("ui.createdTheWorktreeAt", "Created the worktree {value0} at {value1}.", { value0: entry.name, value1: entry.branch }));
  } catch (error) {
    showToast(t("ui.couldNotCreateTheWorktree", "Could not create the worktree: {value0}", { value0: error }), "error");
  } finally {
    button.disabled = false;
  }
}

async function removeWorktree(entry) {
  const project = state.project;
  if (busyRefusal(t("ui.removingAWorktree", "removing a worktree"))) return;
  const ok = await confirmDialog(
    t("ui.removeWorktree", "Remove worktree"),
    t("ui.removeTheWorktreeAtItsCommitsWillBeKeptOnABranchInTheRepositoryUncommittedChange", "Remove the worktree {value0} at {value1}?\n\nIts commits will be kept on a branch in the repository. Uncommitted changes prevent removal.", { value0: entry.name, value1: entry.path }),
    t("ui.removeWorktree", "Remove worktree"),
  );
  if (!ok || project !== state.project || busyRefusal(t("ui.removingAWorktree", "removing a worktree"))) return;
  try {
    const removed = await invoke("remove_worktree", { project, name: entry.name });
    if (project !== state.project) return;
    if (state.worktree === entry.path) {
      state.worktree = null;
      if (state.session) newChat();
      loadGit();
    }
    await loadWorktrees();
    showToast(t("ui.removedItsBranchIsStillInTheRepository", "Removed {value0}. Its branch {value1} is still in the repository.", { value0: entry.name, value1: removed.branch }));
  } catch (error) {
    showToast(t("ui.couldNotRemoveTheWorktree", "Could not remove the worktree: {value0}", { value0: error }), "error");
  }
}

async function createWorktreeBranch() {
  if (busyRefusal(t("ui.creatingABranch", "creating a branch"))) return;
  const project = state.project;
  const entry = worktreeNamed(state.worktree);
  if (!entry) return;
  const branch = await promptDialog(
    `oxide/${entry.name}`,
    t("dialog.worktrees.createBranch", "Create branch here"),
  );
  if (!branch || project !== state.project || busyRefusal(t("ui.creatingABranch", "creating a branch"))) return;
  try {
    await invoke("create_worktree_branch", { project, name: entry.name, branch });
    if (project !== state.project) return;
    await loadWorktrees();
    await loadGit();
    await refreshProjectDetails();
    showToast(t("ui.createdBranch", "Created branch {value0}.", { value0: branch }));
  } catch (error) {
    showToast(t("ui.couldNotCreateBranch", "Could not create branch: {value0}", { value0: error }), "error");
  }
}

async function handoffSession() {
  if (!state.session || busyRefusal(t("ui.handingOffThisChat", "handing off this chat"))) return;
  const project = state.project;
  const id = state.session;
  const source = runFolder();
  const toWorktree = !state.worktree;
  const confirmed = await confirmDialog(t("ui.handOffChat", "Hand off chat"), t("ui.moveThisChatAndItsChangesToTheDestinationMustHaveNoUncommittedChanges", "Move this chat and its changes to {value0}? The destination must have no uncommitted changes.", { value0: toWorktree ? t("ui.itsWorktree", "its worktree") : t("ui.local", "Local") }), t("ui.handOff", "Hand off"));
  if (!confirmed || state.session !== id || project !== state.project || busyRefusal(t("ui.handingOffThisChat", "handing off this chat"))) return;
  state.handingOff = true;
  el("worktree-handoff").disabled = true;
  const dismissHandoff = showToast(t("ui.movingChatAndChanges", "Moving chat and changes…"));
  try {
    const result = await invoke("handoff_session", { project, source, id, toWorktree });
    if (state.session !== id || project !== state.project) return;
    await loadWorktrees();
    await loadSessions();
    const session = sessionById(id) || { id, cwd: result.cwd, project, name: state.threadTitle };
    state.applyingHandoff = true;
    await openSession({ ...session, cwd: result.cwd });
    state.applyingHandoff = false;
    closeWorktrees();
    showToast(t("ui.chatMovedTo", "Chat moved to {value0}.", { value0: toWorktree ? t("composer.worktree", "Worktree") : t("ui.local", "Local") }));
  } catch (error) {
    showToast(t("ui.couldNotHandOffChat", "Could not hand off chat: {value0}", { value0: error }), "error");
  } finally {
    dismissHandoff();
    state.handingOff = false;
    state.applyingHandoff = false;
    el("worktree-handoff").disabled = false;
    updateSendState();
  }
}

// ---------- threads ----------

/// Loads the shared session store, keeping why it could not be read for the
/// listing's own render to paint — a failure painted from the await lands under
/// a press like any other repaint, the same way `loadMcps` does — and returns
/// the same reason.
async function loadSessions() {
  try {
    state.sessions = await invoke("all_sessions");
    state.sessionsError = "";
    renderProjectsTree();
    refreshThreadTitle();
    repaintWelcome();
    return "";
  } catch (error) {
    showToast(t("ui.failedToLoadThreads", "Failed to load threads: {value0}", { value0: error }), "error");
    state.sessionsError = String(error);
    // The home state is painted before either listing answers, so a store that
    // cannot be read still lets the folders that did arrive repaint it: the
    // reader looks at what the sidebar holds rather than at "Add a folder".
    repaintWelcome();
    return String(error);
  }
}

/// Repaints the home state while the transcript holds nothing but it, whichever
/// way the thread listing went. The welcome lists the newest threads across
/// every project, so the listing that just arrived is what fills it — but once a
/// message has been sent, or a finished turn's card has landed, the transcript
/// is the thread, and a listing arriving late (every turn ends by reading it
/// again) must not clear it. The rows are the store's own order, newest first.
function repaintWelcome() {
  const transcript = el("transcript");
  const only = transcript.children;
  if (only.length === 1 && only[0].classList?.contains("welcome")) renderWelcome();
}

/// Opens a stored thread for reading. A turn may be running in another thread
/// while this happens: that thread keeps its own conversation, and opening this
/// one only moves the window. A thread this window is already holding is handed
/// back as it stands rather than read again from a store that is a step behind.
let sessionRead = 0;
async function openSession(session) {
  if (state.handingOff && !state.applyingHandoff) return;
  pushNav(session);
  const read = ++sessionRead;
  const ownerTab = activeChatTab();
  if (ownerTab) ownerTab.pendingSession = session;
  const tabId = activeTabId;
  // The thread being left keeps its conversation, run and cards in its record —
  // parked before the tab is pointed at the one being opened, so what is on
  // screen goes to the record it belongs to.
  parkThread();
  state.session = session.id;
  if (ownerTab) {
    ownerTab.session = session.id;
    ownerTab.draftKey = undefined;
  }
  setThreadTitle(sessionLabel(session));
  // The thread runs where its own header says it does: a thread in a worktree
  // is read back from, and continued in, that checkout rather than in the
  // folder the reader has open.
  state.worktree = worktreePathFor(session.cwd);
  paintWorktreeChip();
  // Coming back to a thread this window is still holding: its own transcript is
  // the thread — with everything the run did while the reader was away,
  // including the reply the store has not been told about yet — so it is handed
  // back rather than read again from a listing that is a step behind.
  // A thread this window holds is handed back when its own turn is still in
  // flight, or ended while the reader was elsewhere: what it painted is not in
  // the store yet. Everything else is read again, since the store is the copy
  // every front-end shares and this window's may have paused on it.
  const held = threads.get(`s:${session.id}`);
  if (held?.node && !held.busy && !held.unread) {
    held.node = null;
  }
  if (held?.node) {
    restoreThread(held);
    if (ownerTab) delete ownerTab.pendingSession;
    closeReview();
    setThreadTitle(sessionLabel(session));
    paintQueue();
    paintUsage();
    updateRunBanner();
    updateSendState();
    loadSessions();
    return;
  }
  // The transcript is built fresh from the store, so the pointers into the one
  // it replaces are dropped: the next delta of a reply would otherwise be written
  // into a node that is no longer on screen.
  clearPaintIdle();
  state.changes = [];
  state.repositoryChanges = null;
  closeReview();
  updateRunBanner();
  el("transcript").replaceChildren();
  renderMessageNavigator();
  try {
    const data = await invoke("session_messages", { project: session.cwd, id: session.id });
    if (tabId !== activeTabId || read !== sessionRead || state.session !== session.id) return;
    if (ownerTab) delete ownerTab.pendingSession;
    const transcript = el("transcript");
    transcript.innerHTML = "";
    renderStoredTranscript(transcript, data.messages);
    scrollDown(el("transcript"), true);
    setThreadTitle(sessionLabel(session));
    paintThreadWorktree();
    loadGit();
    state.contextCorrection = null;
    state.contextLimits = (data.contextLimits || []).filter((limit) =>
      Number.isFinite(limit.window) && limit.window > 0 && limit.provider && limit.model,
    ).map((limit) => ({ tokens: limit.window, provider: limit.provider, model: limit.model,
      project: state.project, session: state.session }));
    state.contextWindow = correctionWindow() ?? state.catalogWindow;
    if (data.usage) {
      setUsage({
        input: data.usage.input,
        output: data.usage.output,
        cacheRead: data.usage.cacheRead,
        cacheWrite: data.usage.cacheWrite,
        cost: data.usage.cost,
        // The prompt of the newest recorded request is the context the thread was
        // last run with, so a thread opened again draws its context reading — the
        // arc and the percent inside it — from that rather than from nothing until
        // the next turn runs, and the hit rate the store kept is what the totals
        // line's `CH` shows.
        prompt: data.usage.contextTokens || 0,
        cacheHitRate: data.usage.cacheHitRate ?? null,
        attribution: data.usage.attribution || {},
      });
    }
  } catch (error) {
    showToast(t("ui.failedToOpenThread", "Failed to open thread: {value0}", { value0: error }), "error");
  }
  loadSessions();
}

function resetTranscript() {
  parkThread();
  const tab = activeChatTab();
  if (tab) {
    tab.session = null;
    tab.draftKey = `draft:${nextDraft++}`;
  }
  state.session = null;
  // The transcript is not the running turn's own any more, so its banner says
  // where that turn is — if it is not the one this welcome belongs to.
  state.changes = [];
  closeReview();
  if (!el("details-panel").hidden) refreshProjectDetails();
  el("transcript").innerHTML = "";
  el("transcript").followLatest = true;
  el("transcript").followScrollTop = el("transcript").scrollTop;
  paintJumpLatest();
  el("usage").textContent = "";
  state.usage = null;
  // The totals a thread carried leave with it, so the line and the ring go back
  // to what a fresh conversation shows: a window with nothing spent yet.
  paintUsage();
  setThreadTitle("");
  renderWelcome();
  updateRunBanner();
}

/// A turn belongs to this window's own process — its tools write files and its
/// stream is read here — so anything that would replace what is on screen, or the
/// process itself, is refused while one runs.
function busyRefusal(what) {
  if (state.handingOff) {
    setStatus(t("ui.waitForTheHandoffBefore", "Wait for the handoff before {value0}.", { value0: what }));
    return true;
  }
  if (!state.busy) return false;
  setStatus(t("ui.aTurnIsRunningStopItBefore", "A turn is running; stop it before {value0}.", { value0: what }));
  return true;
}

/// A running turn owns the thread it is on: clearing the transcript under it
/// would paint the rest of its events into a chat that no longer exists, and the
/// session id it is about to report would be dropped with the queued messages.
/// Every path that starts a fresh thread asks here first, as the VS Code panel
/// refuses its own new chat while a turn is live.
function canStartNewChat() {
  return !busyRefusal(t("ui.startingANewThread", "starting a new thread"));
}

function newChat() {
  if (!canStartNewChat()) return;
  state.pendingSends = [];
  paintQueue();
  resetTranscript();
  clearAttachments();
  loadSessions();
  el("prompt").focus();
}

/// Starts a task in `project`, selecting it first when it is not the active one.
/// The composer belongs to a project, so a task always lands in a real one.
async function newTaskIn(project) {
  // Asked before the project is switched, so a refusal leaves the running
  // thread's window exactly as it was.
  if (!canStartNewChat()) return;
  if (project && project.path !== state.project) await selectProject(project);
  newChat();
}

/// The sidebar's head: a thread in the project this window is in, and, when
/// nothing is open yet, the picker that asks which folder to run in. A project is
/// not chosen for the reader on the way in, so it is not chosen here either —
/// the thread it starts is one they named the folder for, and with no project at
/// all nothing to run in, so it asks for one the way every other control does.
async function newChatFromSidebar() {
  closeCustomize();
  if (!canStartNewChat()) return;
  if (state.project) {
    newChat();
    return;
  }
  askForProject();
}

// ---------- chat ----------

function bubble(kind, text, attachments = [], phase = "final_answer") {
  const wrap = document.createElement("div");
  wrap.className = `msg ${kind}${kind === "assistant" ? ` ${phase}` : ""}`;
  wrap.chatAttachments = attachments;
  const body = document.createElement("div");
  body.className = "body";
  paintMarkdown(body, text);
  if (kind === "user") wrap.dataset.prompt = text;
  const images = attachments.filter((attachment) => isPaintableImage(attachment.dataUrl));
  if (images.length) {
    const strip = document.createElement("div");
    strip.className = "msg-attachments";
    for (const attachment of images) {
      strip.appendChild(openableImage(attachment.dataUrl, attachment.name));
    }
    body.appendChild(strip);
  }
  wrap.append(speakerLabel(kind), body);
  return wrap;
}

/// The speaker line a message carries. The assistant's is the app's own mark
/// beside its name, drawn rather than typed, so the transcript names the
/// speaker the way the home state's mark does — and a `user` label is left to
/// the sheet, which hides it.
function speakerLabel(kind) {
  const label = document.createElement("div");
  label.className = "label";
  if (kind !== "assistant") {
    label.textContent = t("ui.you", "you");
    return label;
  }
  const mark = document.createElement("span");
  mark.className = "label-mark";
  mark.setAttribute("aria-hidden", "true");
  mark.innerHTML = ICONS.mark;
  const name = document.createElement("span");
  name.textContent = "Oxide";
  label.append(mark, name);
  return label;
}

/// Scrolls a transcript to its newest line. The one on screen unless a caller
/// names another: the run's own output goes to its own thread, which is parked
/// — detached, and so with nothing to scroll — while the reader is elsewhere.
function scrollDown(node = el("transcript"), force = false) {
  updateTranscriptFollow(node);
  if (force) node.followLatest = true;
  if (node.followLatest !== false) {
    node.scrollTop = node.scrollHeight;
    node.followScrollTop = node.scrollTop;
    node.followScrollHeight = node.scrollHeight;
    node.followClientHeight = node.clientHeight;
  }
  if (node === el("transcript")) paintJumpLatest();
}

/// Drops the pointers into the transcript that was being painted: the reply in
/// progress, the tool cards a step opened and the timers ticking them. A step
/// that committed its output leaves them behind, and so does a transcript built
/// from the store — either way they would be written into nodes that are no
/// longer on screen.
function clearPaint() {
  for (const tool of state.tools) {
    if (tool.timer) clearInterval(tool.timer);
  }
  state.currentAssistant = null;
  state.tools = [];
  state.currentThinking = null;
  state.currentActivity = null;
}

/// The pointers into the transcript on screen, dropped only when no turn is in
/// flight: a run paints into its own transcript — the parked one while the reader
/// is reading another thread — so its pointers are not this view's to clear, and
/// a turn that ends clears them itself (`resetTurn`).
function clearPaintIdle() {
  if (!state.busy) clearPaint();
}

function resetTurn() {
  clearPaint();
  // A run that ended took its requests with it, so a question it left waiting
  // is not answerable any more: the dialog goes away, as the extension settles
  // a card a finished run left behind.
  closeQuestion();
}

/// The composer's action while idle is Send. During a run it follows the saved
/// Queue/Steer preference; Cmd/Ctrl+Shift+Enter inverts it once.
const lastInputs = new Map();
const inputHistoryKey = () => JSON.stringify([state.project, state.session]);

// Tool previews can shrink and clamp/anchor scrollTop before a scroll event.
// Keep following when a shorter preview or taller viewport moves the end.
function updateTranscriptFollow(transcript) {
  const top = transcript.scrollTop;
  const previous = transcript.followScrollTop ?? top;
  const height = transcript.scrollHeight;
  const view = transcript.clientHeight;
  const shrank = height < (transcript.followScrollHeight ?? height)
    || view > (transcript.followClientHeight ?? view);
  if (top < previous && !shrank) transcript.followLatest = false;
  else if (top > previous && height - view - top <= 2) transcript.followLatest = true;
  transcript.followScrollTop = top;
  transcript.followScrollHeight = height;
  transcript.followClientHeight = view;
}

// Reference decoration is syntax only; sending still resolves against real files.
function referenceRanges(text) {
  const ranges = [];
  for (let start = 0; start < text.length; start += 1) {
    if (text[start] !== "@" || (start > 0 && !/\s/.test(text[start - 1]))) continue;
    if (ranges.length && start < ranges[ranges.length - 1][1]) continue;
    const line = text.slice(start + 1).split("\n", 1)[0].slice(0, 4096);
    if (!line || /^\s/.test(line)) continue;
    let length = 0;
    if (line[0] === '"' || line[0] === "'") {
      const close = line.indexOf(line[0], 1);
      length = close < 0 ? line.length : close + 1;
      const suffix = /^#\d+(?:-\d+)?/.exec(line.slice(length));
      if (suffix) length += suffix[0].length;
    } else {
      const words = [...line.matchAll(/\S+/g)].slice(0, 128);
      length = words[0]?.[0].replace(/[.,;!?)\]]+$/, "").length || 0;
      for (const word of words) {
        if (word.index > 0 && word[0].startsWith("@")) break;
        const candidate = word[0].replace(/[.,;!?)\]]+$/, "");
        if (/\.[a-zA-Z0-9]{1,12}(?:#\d+(?:-\d+)?)?$/.test(candidate)) {
          length = word.index + candidate.length;
          break;
        }
        if (word.index === 0 && word[0].endsWith("/")) break;
      }
    }
    if (length > 0) ranges.push([start, start + 1 + length]);
  }
  return ranges;
}

// The caret a read of the rendered composer marks its position with, and the
// placeholder a typed newline in the editable tail is held behind so that
// trimming the wrappers a block adds cannot eat one the reader typed.
const CARET = "\uE000oxide-caret\uE001";
const TAIL_NEWLINE = "\uE002";
const QUOTE_TAIL = "\uE003";

// The Markdown a rendered editable surface stands for — the inverse of
// `renderMarkdown`, which is what the composer is: every keystroke paints the
// model, and the next one reads it back, so the two have to agree or the text
// under the caret changes by itself.
function markdownToText(node, point = null, root = node) {
  if (node.nodeType === 3) {
    const raw = node.nodeValue || "";
    // A typed newline waits behind a placeholder so the trim readMarkdown does
    // cannot eat one the reader wrote.
    const text = node.parentNode?.hasAttribute?.("data-editor-tail") ? raw.replace(/\n/g, TAIL_NEWLINE) : raw;
    return point?.node === node ? text.slice(0, point.offset) + CARET + text.slice(point.offset) : text;
  }
  if (node.hasAttribute?.("data-editor-quote-tail")) return QUOTE_TAIL;
  if (node.tagName === "INPUT" && node.hasAttribute("data-editor-task")) {
    const task = node.checked || node.hasAttribute("checked") ? "[x] " : "[ ] ";
    return point?.node === node ? CARET + task : task;
  }
  const children = Array.from(node.childNodes || []);
  const parts = children.map((child, i) =>
    (point?.node === node && point.offset === i ? CARET : "") + markdownToText(child, point, root),
  );
  let text = parts.reduce((all, part, index) => {
    // What the reader typed after the last block belongs under it, not after the
    // blank line that block wrote to separate itself from the next one.
    if (children[index]?.hasAttribute?.("data-editor-tail")) return all.replace(/\n+$/, "") + part;
    if ((node === root || node.tagName === "BLOCKQUOTE" || node.hasAttribute?.("data-editor-list-block"))
      && /\n$/.test(all) && /^\n/.test(part)) {
      return all.replace(/\n+$/, "") + "\n\n" + part.replace(/^\n+/, "");
    }
    return all + part;
  }, "");
  if (point?.node === node && point.offset === children.length) text += CARET;
  const tag = node.tagName;
  if (node.hasAttribute?.("data-editor-padding")) return "";
  if (tag === "BR") {
    const hardBreak = node.getAttribute?.("data-editor-hard-break");
    if (hardBreak != null) return `${hardBreak}\n`;
    const pad = node.getAttribute?.("data-editor-pad")
      ?? (node.parentNode?.hasAttribute?.("data-editor-pad") ? node.parentNode.getAttribute("data-editor-pad") : "");
    return `\n${pad}`;
  }
  if (tag === "STRONG" || tag === "B" || tag === "EM" || tag === "I") {
    // The delimiter the writer used, so `__bold__` is not rewritten to `**bold**`.
    if (node.hasAttribute?.("data-editor-silent")) return text;
    const triple = node.getAttribute?.("data-editor-triple");
    if (triple) return `${triple}${text}${triple}`;
    const bare = tag === "STRONG" || tag === "B";
    const marker = node.getAttribute("data-editor-marker");
    const delimiter = marker === "underscore" ? (bare ? "__" : "_") : marker || (bare ? "**" : "*");
    return `${delimiter}${text}${delimiter}`;
  }
  if (tag === "DEL" || tag === "S") return `~~${text}~~`;
  if (tag === "PRE") {
    const fence = node.getAttribute("data-editor-fence") || "```";
    const close = node.getAttribute("data-editor-close") || fence;
    const indent = node.getAttribute("data-editor-fence-indent") || "";
    const closeIndent = node.getAttribute("data-editor-close-indent") ?? indent;
    const info = node.getAttribute("data-editor-info") ?? node.getAttribute("data-lang") ?? "";
    const open = `${indent}${fence}${info}\n${text}`;
    return node.hasAttribute("data-editor-open") ? `\n\n${open}\n\n` : `\n\n${open}\n${closeIndent}${close}\n\n`;
  }
  if (tag === "CODE" && node.parentNode?.tagName !== "PRE") {
    const run = Math.max(1, ...(text.match(/`+/g) || []).map((ticks) => ticks.length + 1));
    return `${"`".repeat(run)}${text}${"`".repeat(run)}`;
  }
  if (tag === "A") {
    const url = node.getAttribute("href") || "";
    const title = node.getAttribute("title");
    if (node.hasAttribute("data-editor-angle")) return `<${text}>`;
    if (!node.hasAttribute("data-editor-autolink") && text.replace(CARET, "") !== url) {
      const destination = /\s/.test(url) ? `<${url}>` : url;
      return `[${text}](${destination}${title == null ? "" : ` "${title}"`})`;
    }
    return text;
  }
  if (node.hasAttribute?.("data-editor-escape")) return `\\${text}`;
  if (/^H[1-6]$/.test(tag)) {
    const underline = node.getAttribute("data-editor-marker");
    if (underline) return `\n\n${text}\n${underline}\n\n`;
    const indent = node.getAttribute("data-editor-heading-indent") || "";
    return `\n\n${indent}${"#".repeat(Number(tag[1]))} ${text}\n\n`;
  }
  if (tag === "BLOCKQUOTE") {
    const heldTail = text.includes(QUOTE_TAIL);
    const quote = text.replaceAll(QUOTE_TAIL, "").trim();
    const lines = quote === "" ? [""] : quote.split("\n");
    if (heldTail && quote !== "") lines.push("");
    const prefix = node.getAttribute("data-editor-prefix") || "> ";
    return `\n\n${lines.map((line) => `${prefix}${line}`).join("\n")}\n\n`;
  }
  if (tag === "LI") {
    const gap = node.getAttribute("data-editor-gap") || "";
    const prefix = node.getAttribute("data-editor-prefix") || "- ";
    const line = text.replace(/^☑ /, "[x] ").replace(/^☐ /, "[ ] ");
    return `${gap}${prefix}${line}${line.endsWith("\n") || line === "" ? "" : "\n"}`;
  }
  if (tag === "UL" || tag === "OL") {
    return node.parentNode?.tagName === "LI" ? `\n${text}` : `\n\n${text}\n`;
  }
  if (tag === "HR") return `\n\n${node.getAttribute("data-editor-marker") || "---"}\n\n`;
  if (tag === "TH" || tag === "TD") return text + " | ";
  if (tag === "TR") return `| ${text.replace(/ \| $/, "")} |\n`;
  if (tag === "THEAD") {
    const typed = node.parentNode?.getAttribute?.("data-editor-separator");
    const cells = Array.from(children[0]?.children || []);
    const align = (cell) => {
      const value = cell?.getAttribute?.("data-align");
      return value === "left" ? ":---" : value === "center" ? ":---:" : value === "right" ? "---:" : "---";
    };
    return `${text}${typed ? `${typed}\n` : `| ${cells.map(align).join(" | ")} |\n`}`;
  }
  if (tag === "TABLE") return `\n\n${text}\n\n`;
  if (tag === "P") return `\n\n${text}\n\n`;
  if (tag === "DIV" && node.hasAttribute?.("data-editor-list-block")) {
    const gap = node.getAttribute("data-editor-gap") || "";
    const pad = node.getAttribute("data-editor-pad") || "";
    const body = text.replace(/^\n+|\n+$/g, "");
    return `${gap}\n${body.split("\n").map((line) => (line ? `${pad}${line}` : "")).join("\n")}`;
  }
  if (tag === "DIV" && node !== root) return `\n${text || "\n"}`;
  return text;
}

// The Markdown a rendered surface stands for, with the caret marker where the
// point falls. A blank line a block writes around itself belongs to the
// renderer, not to the reader: what they typed waits in the tail behind its own
// placeholder, so trimming on either side of the marker is safe — a typed
// newline is not a `\n` yet, and one no block asked for never survives.
function readMarkdown(surface, point = null) {
  const marked = markdownToText(surface, point).replace(/\u00a0/g, " ");
  const caret = marked.indexOf(CARET);
  const body = caret < 0 ? marked : marked.slice(0, caret) + marked.slice(caret + CARET.length);
  const lead = /^\n+/.exec(body)?.[0].length || 0;
  const text = body.replace(/^\n+/, "").replace(/\n+$/, "");
  const at = caret < 0 ? -1 : Math.max(0, Math.min(text.length, caret - lead));
  const written = at < 0 ? text : text.slice(0, at) + CARET + text.slice(at);
  return written.replaceAll(TAIL_NEWLINE, "\n");
}

// One rendered, editable surface. The value/selection facade keeps completion,
// recall and sending on Markdown while the browser edits the rendered nodes.
function installMarkdownInput(editor, render, handlesTab = () => true) {
  let source = "";
  let painted = "";
  let composing = false;
  let savedStart = 0;
  let savedEnd = 0;
  const history = [""];
  let historyIndex = 0;
  const marker = CARET;

  function read(point = null) {
    return readMarkdown(editor, point);
  }

  function selectionOffset(end) {
    const selection = window.getSelection?.();
    if (!selection?.rangeCount || !editor.contains(selection.anchorNode)) return end ? savedEnd : savedStart;
    const range = selection.getRangeAt(0);
    const node = end ? range.endContainer : range.startContainer;
    const offset = end ? range.endOffset : range.startOffset;
    if (node.nodeType === 3 && node.parentNode?.hasAttribute?.("data-editor-tail") && offset === node.nodeValue.length) {
      return source.length;
    }
    // At the end, retain the caller's exact Markdown (including its spacing).
    const marked = read({ node, offset });
    const at = marked.indexOf(marker);
    if (node === editor && offset === editor.childNodes.length) return source.length;
    return Math.max(0, at);
  }

  function pointAt(offset) {
    if (offset >= source.length && editor.lastChild) {
      const tail = editor.querySelector?.("[data-editor-tail]");
      if (tail) return { node: tail.firstChild, offset: tail.firstChild.nodeValue.length };
      let container = editor.lastElementChild || editor;
      while (/^(UL|OL)$/.test(container.tagName) && container.lastElementChild) {
        container = container.lastElementChild;
      }
      if (container.lastChild?.nodeType === 3) {
        return { node: container.lastChild, offset: container.lastChild.nodeValue.length };
      }
      return { node: container, offset: container.childNodes.length };
    }
    const walker = document.createTreeWalker(editor, 5);
    let best = { node: editor, offset: 0 };
    let distance = Infinity;
    while (walker.nextNode()) {
      const node = walker.currentNode;
      const text = node.nodeType === 3;
      const count = text ? node.nodeValue.length : node.childNodes.length;
      const start = read({ node, offset: 0 }).indexOf(marker);
      const candidate = text ? Math.max(0, Math.min(count, offset - start)) : 0;
      for (const index of text ? [candidate] : Array.from({ length: count + 1 }, (_, i) => i)) {
        const at = read({ node, offset: index }).indexOf(marker);
        if (at < 0 || Math.abs(at - offset) >= distance) continue;
        best = { node, offset: index };
        distance = Math.abs(at - offset);
        if (distance === 0) return best;
      }
    }
    return best;
  }

  function select(start, end = start) {
    savedStart = start;
    savedEnd = end;
    if (!document.createRange) return;
    const a = pointAt(start);
    const b = pointAt(end);
    const range = document.createRange();
    range.setStart(a.node, a.offset);
    range.setEnd(b.node, b.offset);
    const selection = window.getSelection();
    selection.removeAllRanges();
    selection.addRange(range);
  }

  function paint(caret = null) {
    editor.style.height = "auto";
    // Markdown ignores trailing blank lines. Keep the editable suffix outside
    // its renderer, with a final caret stop outside any link or code span.
    const trailingWhitespace = source.match(/\s*$/)[0];
    const beforeTrailing = source.length - trailingWhitespace.length;
    const trailingLineStart = source.lastIndexOf("\n", beforeTrailing - 1) + 1;
    const trailingLine = source.slice(trailingLineStart, beforeTrailing);
    const trailing = trailingWhitespace.includes("\n")
      || (source && source.trim() === "")
      || /^[ \t]*>$/.test(trailingLine)
      ? trailingWhitespace
      : "";
    let split = source.length - trailing.length;
    const lineStart = source.lastIndexOf("\n", split - 1) + 1;
    const line = source.slice(lineStart, split);
    // A list or heading marker is a block only after its separator is typed.
    // That separator is whitespace, so it belongs to the rendered block rather
    // than the editable tail; otherwise "- " briefly paints as plain text.
    const markerOnly =
      /^[ \t]*(?:[-*+]|\d+[.)])(?:[ \t]+(?:\[[ xX]\])?[ \t]*)?$/.test(line) ||
      /^[ \t]*#{1,6}[ \t]*$/.test(line);
    if (markerOnly) split += /^[ \t]+/.exec(trailing)?.[0].length || 0;
    const body = source.slice(0, split);
    const whitespace = source.slice(split);
    editor.innerHTML = body ? render(body) : "";
    if ((whitespace || (source && !editor.textContent)) && document.createTextNode) {
      const tail = document.createElement("span");
      tail.setAttribute("data-editor-tail", "");
      tail.appendChild(document.createTextNode(whitespace));
      const last = editor.lastChild;
      const container = last?.tagName === "P" ? last : editor;
      container.appendChild(tail);
      if (whitespace.includes("\n")) {
        const padding = document.createElement("br");
        padding.setAttribute("data-editor-padding", "");
        container.appendChild(padding);
      }
    }
    painted = editor.innerHTML;
    savedStart = savedEnd = caret ?? source.length;
    if (caret !== null) select(caret);
  }

  function remember() {
    if (history[historyIndex] === source) return;
    history.splice(historyIndex + 1);
    history.push(source);
    if (history.length > 100) history.shift();
    historyIndex = history.length - 1;
  }

  function commit() {
    if (composing || editor.innerHTML === painted) return;
    const selection = window.getSelection?.();
    const point = selection?.rangeCount && editor.contains(selection.focusNode)
      ? { node: selection.focusNode, offset: selection.focusOffset } : null;
    const marked = read(point);
    const caret = marked.indexOf(marker);
    source = marked.replace(marker, "");
    remember();
    paint(caret < 0 ? null : caret);
  }

  Object.defineProperties(editor, {
    value: { configurable: true, get: () => source, set: (value) => {
      source = String(value);
      remember();
      paint();
    } },
    selectionStart: { configurable: true, get: () => selectionOffset(false), set: (value) => { savedStart = value; } },
    selectionEnd: { configurable: true, get: () => selectionOffset(true), set: (value) => { savedEnd = value; } },
  });
  editor.setSelectionRange = select;
  // A tab owns its undo stack as well as its text: restoring a draft must not
  // make Undo pull a message out of the tab the reader just left.
  editor.captureHistory = () => ({ entries: [...history], index: historyIndex });
  editor.restoreHistory = (saved) => {
    history.splice(0, history.length, ...(saved?.entries || [source]));
    historyIndex = saved?.index ?? 0;
  };
  function leaveInlineCode(direction) {
    const selection = window.getSelection?.();
    if (!selection?.rangeCount || !selection.isCollapsed || !editor.contains(selection.focusNode)) return false;
    const node = selection.focusNode;
    const code = (node.nodeType === 3 ? node.parentElement : node)?.closest?.("code");
    if (!code || code.parentElement?.tagName === "PRE") return false;
    const edge = document.createRange();
    edge.selectNodeContents(code);
    if (direction > 0) edge.setStart(node, selection.focusOffset);
    else edge.setEnd(node, selection.focusOffset);
    if (edge.toString() !== "") return false;
    // Place the caret outside the element, past the hidden Markdown delimiter.
    // A text offset at the same visible position still belongs to the code.
    if (direction > 0) edge.setStartAfter(code);
    else edge.setStartBefore(code);
    edge.collapse(true);
    selection.removeAllRanges();
    selection.addRange(edge);
    savedStart = savedEnd = selectionOffset(false);
    return true;
  }
  function lineBreakEdit(start, end) {
    if (start !== end) return { start, end, text: "\n" };
    const lineStart = source.lastIndexOf("\n", start - 1) + 1;
    const lineEnd = source.indexOf("\n", start);
    const line = source.slice(lineStart, lineEnd < 0 ? source.length : lineEnd);
    // A quote is a block in Markdown too. A soft break in one stays in the
    // quote, so the next line carries the same prefix into the renderer.
    const quote = /^([ \t]*>[ \t]?)/.exec(line);
    if (quote) return { start, end, text: `\n${quote[1]}` };
    if (source.slice(start).trim() !== "") return { start, end, text: "\n" };
    // A list item carries on into the next one, the way the native composer
    // does; an item with nothing in it is the way out. Leaving removes that
    // item's marker rather than leaving an orphan bullet behind the caret.
    const item = /^([ \t]*(?:[-*+]|(\d+)[.)])[ \t]+)(\[[ xX]\][ \t]+)?(.*)$/.exec(line);
    if (!item) return { start, end, text: "\n" };
    if (!item[4].trim()) return { start: lineStart, end, text: "" };
    const prefix = item[2] == null ? item[1] : item[1].replace(/\d+/, String(Number(item[2]) + 1));
    return { start, end, text: `\n${prefix}${item[3] ? "[ ] " : ""}` };
  }
  function insertLineBreak() {
    const start = selectionOffset(false);
    const end = selectionOffset(true);
    const edit = lineBreakEdit(start, end);
    source = source.slice(0, edit.start) + edit.text + source.slice(edit.end);
    remember();
    paint(edit.start + edit.text.length);
    editor.dispatchEvent(new Event("input", { bubbles: true }));
  }
  function indentList(direction) {
    const start = selectionOffset(false);
    const end = selectionOffset(true);
    if (start !== end) return false;
    const lineStart = source.lastIndexOf("\n", start - 1) + 1;
    const lineEnd = source.indexOf("\n", start);
    const line = source.slice(lineStart, lineEnd < 0 ? source.length : lineEnd);
    const item = listItem(line);
    if (!item) return false;
    if (direction > 0) {
      const previousEnd = lineStart - 1;
      const previousStart = source.lastIndexOf("\n", previousEnd - 1) + 1;
      const previous = previousEnd >= 0 ? listItem(source.slice(previousStart, previousEnd)) : null;
      if (!previous || previous.indent > item.indent) return true;
      source = source.slice(0, lineStart) + "  " + source.slice(lineStart);
      remember();
      paint(start + 2);
    } else {
      const indent = /^[ \t]*/.exec(line)[0];
      const remove = Math.min(2, indent.length);
      if (!remove) return true;
      source = source.slice(0, lineStart) + source.slice(lineStart + remove);
      remember();
      paint(Math.max(lineStart, start - remove));
    }
    editor.dispatchEvent(new Event("input", { bubbles: true }));
    return true;
  }
  function adjustHeading(direction) {
    const start = selectionOffset(false);
    const end = selectionOffset(true);
    if (start !== end) return false;
    const lineStart = source.lastIndexOf("\n", start - 1) + 1;
    const lineEnd = source.indexOf("\n", start);
    const line = source.slice(lineStart, lineEnd < 0 ? source.length : lineEnd);
    const heading = /^([ \t]*)(#{1,6})[ \t]+/.exec(line);
    if (!heading) return false;
    const level = heading[2].length;
    const next = direction > 0 ? Math.min(6, level + 1) : level - 1;
    const prefix = heading[1] + "#".repeat(Math.max(0, next)) + (next > 0 ? " " : "");
    source = source.slice(0, lineStart) + prefix + source.slice(lineStart + heading[0].length);
    const delta = prefix.length - heading[0].length;
    remember();
    paint(Math.max(lineStart, start + delta));
    editor.dispatchEvent(new Event("input", { bubbles: true }));
    return true;
  }
  function taskPrefix(start) {
    const lineStart = source.lastIndexOf("\n", start - 1) + 1;
    const lineEnd = source.indexOf("\n", start);
    const line = source.slice(lineStart, lineEnd < 0 ? source.length : lineEnd);
    const match = /^([ \t]*)\[([ xX]?)\]$/.exec(line);
    if (!match || (lineEnd >= 0 && start !== lineEnd)) return null;
    return { lineStart, indent: match[1], checked: match[2].toLowerCase() === "x" };
  }
  function emptyBlockAt(start) {
    const lineStart = source.lastIndexOf("\n", start - 1) + 1;
    const lineEnd = source.indexOf("\n", start);
    const line = source.slice(lineStart, lineEnd < 0 ? source.length : lineEnd);
    const atLineEnd = start === (lineEnd < 0 ? source.length : lineEnd);
    if (atLineEnd && /^[ \t]*(?:>[ \t]*|#{1,6}[ \t]*)$/.test(line)) {
      return { start: lineStart, end: lineEnd < 0 ? source.length : lineEnd + 1 };
    }
    const before = source.slice(0, start);
    const openEnd = before.endsWith("\n") ? before.length - 1 : before.length;
    const openStart = source.lastIndexOf("\n", openEnd - 1) + 1;
    const fence = /^\s*(`{3,}|~{3,})[ \t]*[\w+.#-]*\s*$/.exec(source.slice(openStart, openEnd));
    if (!fence || (start !== lineStart && start !== openEnd)) return null;
    const closeEnd = source.indexOf("\n", start);
    const next = source.slice(start, closeEnd < 0 ? source.length : closeEnd);
    const close = isFenceEnd(next, fence[1]) ? (closeEnd < 0 ? source.length : closeEnd + 1) : start;
    return { start: openStart, end: close };
  }
  editor.addEventListener("beforeinput", (event) => {
    if (composing || event.isComposing) return;
    if (event.inputType.startsWith("delete")) {
      let start = selectionOffset(false);
      let end = selectionOffset(true);
      const target = event.getTargetRanges?.()[0];
      if (target && editor.contains(target.startContainer) && editor.contains(target.endContainer)) {
        const first = read({ node: target.startContainer, offset: target.startOffset }).indexOf(marker);
        const last = read({ node: target.endContainer, offset: target.endOffset }).indexOf(marker);
        if (first >= 0 && last >= first) { start = first; end = last; }
      }
      if (event.inputType === "deleteContentBackward" && start === end) {
        const empty = emptyBlockAt(start);
        if (empty) {
          event.preventDefault();
          source = source.slice(0, empty.start) + source.slice(empty.end);
          remember();
          paint(empty.start);
          editor.dispatchEvent(new Event("input", { bubbles: true }));
          return;
        }
        const lineStart = source.lastIndexOf("\n", start - 1) + 1;
        const lineEnd = source.indexOf("\n", start);
        const line = source.slice(lineStart, lineEnd < 0 ? source.length : lineEnd);
        const item = LIST_ITEM.exec(line);
        if (item && start === lineStart + item[1].length) {
          event.preventDefault();
          source = source.slice(0, lineStart) + source.slice(start);
          remember();
          paint(lineStart);
          editor.dispatchEvent(new Event("input", { bubbles: true }));
          return;
        }
      }
      if (start === end) {
        if (event.inputType.endsWith("Backward")) {
          const count = event.inputType === "deleteWordBackward"
            ? source.slice(0, start).match(/\S+\s*$/)?.[0].length || 1 : 1;
          start = Math.max(0, start - count);
        } else if (event.inputType.endsWith("Forward")) {
          const count = event.inputType === "deleteWordForward"
            ? source.slice(end).match(/^\s*\S+/)?.[0].length || 1 : 1;
          end = Math.min(source.length, end + count);
        }
      }
      let referenceDeleted = false;
      // Read rendered references so code spans and ordinary @ text stay editable.
      for (const reference of editor.querySelectorAll(".file-reference")) {
        const first = read({ node: reference, offset: 0 }).indexOf(marker);
        const last = read({ node: reference, offset: reference.childNodes.length }).indexOf(marker);
        if (first >= 0 && start < last && end > first) {
          start = Math.min(start, first);
          end = Math.max(end, last);
          referenceDeleted = true;
        }
      }
      if (!referenceDeleted) return;
      event.preventDefault();
      source = source.slice(0, start) + source.slice(end);
      remember();
      paint(start);
      editor.dispatchEvent(new Event("input", { bubbles: true }));
      return;
    }
    if (!["insertText", "insertLineBreak", "insertParagraph"].includes(event.inputType)) return;
    if (event.inputType === "insertText" && event.data === " ") leaveInlineCode(1);
    if (event.inputType === "insertText" && event.data === " ") {
      const start = selectionOffset(false);
      const task = taskPrefix(start);
      if (task && start === selectionOffset(true)) {
        event.preventDefault();
        const prefix = `${task.indent}- [${task.checked ? "x" : " "}] `;
        const caret = task.lineStart + prefix.length;
        source = source.slice(0, task.lineStart) + prefix + source.slice(start);
        remember();
        paint(caret);
        editor.dispatchEvent(new Event("input", { bubbles: true }));
        return;
      }
    }
    // Applying text to the Markdown model avoids inheriting a code/bold span
    // when the caret is immediately after its closing delimiter.
    event.preventDefault();
    if (event.inputType === "insertLineBreak" || event.inputType === "insertParagraph") {
      insertLineBreak();
      return;
    }
    const start = selectionOffset(false);
    const end = selectionOffset(true);
    const edit = { start, end, text: event.data || "" };
    source = source.slice(0, edit.start) + edit.text + source.slice(edit.end);
    remember();
    paint(edit.start + edit.text.length);
    editor.dispatchEvent(new Event("input", { bubbles: true }));
  });
  editor.addEventListener("compositionstart", () => { composing = true; });
  editor.addEventListener("compositionend", () => { composing = false; commit(); editor.dispatchEvent(new Event("input", { bubbles: true })); });
  editor.addEventListener("input", commit);
  editor.addEventListener("keydown", (event) => {
    if (event.isComposing || composing) return;
    if (event.key === "Enter" && event.shiftKey && !event.metaKey && !event.ctrlKey && !event.altKey) {
      event.preventDefault();
      insertLineBreak();
      return;
    }
    if (event.key === "Tab" && !event.metaKey && !event.ctrlKey && !event.altKey && handlesTab()) {
      const direction = event.shiftKey ? -1 : 1;
      if (indentList(direction) || adjustHeading(direction)) {
        event.preventDefault();
        return;
      }
    }
    if (!event.metaKey && !event.ctrlKey && !event.altKey && !event.shiftKey &&
        ["ArrowLeft", "ArrowRight"].includes(event.key) && leaveInlineCode(event.key === "ArrowRight" ? 1 : -1)) {
      event.preventDefault();
      return;
    }
    if (!(event.metaKey || event.ctrlKey) || !["z", "y"].includes(event.key.toLowerCase())) return;
    event.preventDefault();
    const redo = event.key.toLowerCase() === "y" || event.shiftKey;
    historyIndex = Math.max(0, Math.min(history.length - 1, historyIndex + (redo ? 1 : -1)));
    source = history[historyIndex];
    paint(source.length);
    editor.dispatchEvent(new Event("input", { bubbles: true }));
  });
  editor.addEventListener("click", (event) => {
    const box = event.target?.closest?.("input[data-editor-task]");
    if (!box || !editor.contains(box)) return;
    const marked = read({ node: box, offset: 0 }).indexOf(CARET);
    let at = -1;
    for (let distance = 0; marked >= 0 && distance <= 4 && at < 0; distance += 1) {
      at = [marked - distance, marked + distance].find(
        (index) => index >= 0 && /^\[[ xX]\]/.test(source.slice(index)),
      ) ?? -1;
    }
    if (at < 0 || source[at] !== "[") return;
    event.preventDefault();
    event.stopPropagation();
    const checked = source[at + 1].toLowerCase() !== "x";
    source = source.slice(0, at + 1) + (checked ? "x" : " ") + source.slice(at + 2);
    remember();
    paint();
    editor.dispatchEvent(new Event("input", { bubbles: true }));
  });
  editor.addEventListener("paste", (event) => {
    if (Array.from(event.clipboardData?.items || []).some((item) => item.kind === "file")) return;
    const text = event.clipboardData?.getData?.("text/plain");
    if (text === undefined) return;
    event.preventDefault();
    const start = selectionOffset(false);
    const end = selectionOffset(true);
    source = source.slice(0, start) + text + source.slice(end);
    remember();
    paint(start + text.length);
    editor.dispatchEvent(new Event("input", { bubbles: true }));
  });
}

function updateSendState() {
  const hasText =
    el("prompt").value.trim().length > 0 || state.attachments.length > 0;
  // The corner acts on the thread this composer is showing: its own turn, so a
  // message queued or steered here goes to this conversation and a Stop ends
  // this one — another thread's turn has its own corner when the reader opens it.
  const busyHere = state.busy;
  el("composer").classList.toggle("filled", hasText);
  el("send").classList.toggle("enabled", hasText);
  el("send").disabled = !hasText;
  el("send").hidden = state.busy && !hasText;
  el("stop").hidden = !state.busy || hasText;
  el("send").title = busyHere
    ? state.followUpBehavior === "steer"
      ? t("composer.steerHint", "Steer the active response (Enter)")
      : t("composer.queueHint", "Queue as the next turn (Enter)")
    : t("composer.sendHint", "Send (Enter)");
  // The accessible name follows the action the button now performs, the way the
  // panel's own corner does: a screen reader hears Queue or Steer, not the
  // label the button was built with.
  el("send").setAttribute(
    "aria-label",
    busyHere
      ? state.followUpBehavior === "steer"
        ? t("composer.steer", "Steer")
        : t("composer.queue", "Queue")
      : t("composer.send", "Send"),
  );
}

function setFollowUpBehavior(mode) {
  const next = mode === "steer" ? "steer" : "queue";
  state.followUpBehavior = next;
  // The window's own preference is the store's to keep, so the next launch reads
  // the same answer the settings sheet shows.
  if (state.prefs?.follow_up !== next) void savePrefs({ follow_up: next });
  paintQueue();
  paintLiveChanges();
  updateSendState();
}

let queueMenuTrigger = null;

function closeQueueMenu() {
  if (queueMenuTrigger) queueMenuTrigger.setAttribute("aria-expanded", "false");
  queueMenuTrigger = null;
  const menu = el("queue-menu");
  menu.hidden = true;
  menu.innerHTML = "";
}

function openQueueMenu(trigger, index) {
  const menu = el("queue-menu");
  if (queueMenuTrigger === trigger && !menu.hidden) {
    closeQueueMenu();
    return;
  }
  closeQueueMenu();
  queueMenuTrigger = trigger;
  trigger.setAttribute("aria-expanded", "true");
  for (const [action, label, direction] of [
    ["move", t("composer.moveUp", "Move up"), -1],
    ["move", t("composer.moveDown", "Move down"), 1],
    ["edit", t("composer.edit", "Edit"), 0],
    [
      "behavior",
      state.followUpBehavior === "queue"
        ? t("composer.useSteer", "Use Steer for new messages")
        : t("composer.useQueue", "Use Queue for new messages"),
      0,
    ],
  ]) {
    const button = document.createElement("button");
    button.type = "button";
    button.setAttribute("role", "menuitem");
    button.dataset.action = action;
    button.dataset.index = String(action === "behavior" ? -1 : index);
    button.dataset.direction = String(direction);
    button.disabled = action === "move" && (index + direction < 0 || index + direction >= state.pendingSends.length);
    button.textContent = label;
    menu.appendChild(button);
  }
  menu.hidden = false;
  const rect = trigger.getBoundingClientRect();
  const width = menu.offsetWidth || 176;
  const height = menu.offsetHeight || 132;
  menu.style.left = `${Math.max(8, Math.min(rect.right - width, window.innerWidth - width - 8))}px`;
  menu.style.top = `${rect.top - height >= 8 ? rect.top - height - 4 : rect.bottom + 4}px`;
}

function paintQueue() {
  const list = el("queue-list");
  closeQueueMenu();
  list.innerHTML = "";
  list.hidden = state.pendingSends.length === 0;
  state.pendingSends.forEach((pending, index) => {
    const row = document.createElement("div");
    row.className = "queued-message";
    row.draggable = state.pendingSends.length > 1;
    row.dataset.index = String(index);
    const mark = document.createElement("span");
    mark.className = "queued-message-mark";
    mark.innerHTML = glyph(
      `<path d="M7 7h10M7 12h7M7 17h5M3.5 6.5v11" ${STROKE_24}/>` +
        `<path d="m1.5 15.5 2 2 2-2" ${STROKE_24}/>`,
    );
    const text = document.createElement("span");
    text.className = "queued-message-text";
    text.textContent =
      pending.prompt.split(/\r?\n/, 1)[0] || t("composer.attachment", "Attachment");
    const actions = document.createElement("span");
    actions.className = "queued-message-actions";
    for (const [action, label, direction, icon] of [
      ["steer", t("composer.steer", "Steer"), 0, `<svg viewBox="0 0 16 16" aria-hidden="true"><path d="M2 4v2.5A3.5 3.5 0 0 0 5.5 10H14m-3-3 3 3-3 3" ${STROKE_16}/></svg>`],
      ["delete", t("common.delete", "Delete"), 0, `<svg viewBox="0 0 24 24" aria-hidden="true"><path d="M4 7h16M10 11v6M14 11v6M6 7l1 13h10l1-13M9 7V4h6v3" ${STROKE_24}/></svg>`],
    ]) {
      const button = document.createElement("button");
      button.type = "button";
      button.dataset.action = action;
      button.dataset.index = String(index);
      button.dataset.direction = String(direction);
      button.className = `queued-${action}`;
      button.title = label;
      button.setAttribute("aria-label", label);
      button.innerHTML = action === "steer" ? `${icon}<span>${label}</span>` : icon;
      actions.appendChild(button);
    }
    const more = document.createElement("button");
    more.type = "button";
    more.className = "queued-more";
    more.dataset.menu = String(index);
    more.title = t("ui.moreQueuedMessageActions", "More queued-message actions");
    more.setAttribute("aria-label", more.title);
    more.setAttribute("aria-haspopup", "menu");
    more.setAttribute("aria-expanded", "false");
    more.setAttribute("aria-controls", "queue-menu");
    more.innerHTML = ICONS.more;
    actions.appendChild(more);
    row.append(mark, text, actions);
    list.appendChild(row);
  });
}

function paintRunningChrome() {
  document.body.classList.toggle("running", state.busy);
  paintLiveChanges();
  paintDockToggles();
  paintChatTabs();
  paintAwake();
}

function setBusy() {
  const starting = !state.busy;
  state.busy = true;
  if (starting) {
    if (state.usage) state.usage = { ...state.usage, turn: null };
    state.runError = "";
    state.runStartedAt = 0;
    startRunClock();
  }
  paintRunningChrome();
  updateSendState();
  // The status badge's icon is the state: a turn starting turns it into the
  // spinner, wherever the words it is wearing came from.
  paintStatus();
}

function setIdle() {
  const record = paintingThread();
  finishRunClock(record);
  record.busy = false;
  record.runId = null;
  record.runSession = null;
  record.runProject = null;
  paintRunningChrome();
  updateSendState();
  updateRunBanner();
  paintStatus();
  paintRunClock();
}

/// The transcript the turn being painted writes into: the one on screen while
/// the reader is in its thread, and the conversation the thread keeps off screen
/// while they are reading another. A run writes into its own thread either way —
/// a delta, a tool card, the card of a turn that changed files — so nothing it
/// does while the reader is away is lost or painted under the thread they moved
/// to.
function runTranscript() {
  return transcriptFor(paintingThread());
}

/// The finished turns whose cards a thread holds. Each thread keeps its own, so
/// the Undo an older card drops is the newest card *of that thread* rather than
/// of whichever conversation happens to be on screen.
function changesFor() {
  return paintingThread().changes;
}

/// The folder a thread's transcript belongs to: the folder its own turn ran in,
/// and the window's for a thread that has not started one.
function transcriptProject(record = paintingThread()) {
  return record.project || record.runProject || view.project;
}

/// A turn running in a thread the reader is not looking at, so the header's strip
/// can say where it is and open it. The thread on screen has its own composer,
/// which acts on its own run.
/// Whether a turn is running in the thread with this session id, wherever the
/// reader happens to be reading.
function turnInThread(id) {
  if (!id) return false;
  for (const record of threads.values()) {
    if (record.busy && record.session === id) return true;
  }
  return false;
}

/// A turn running in a thread the reader is not looking at, so the header's strip
/// can say where it is and open it. The thread on screen has its own composer,
/// which acts on its own run.
function backgroundRun() {
  const visible = visibleThread();
  let finished = null;
  for (const record of threads.values()) {
    if (record === visible) continue;
    if (record.busy) return record;
    if (record.unread) finished = record;
  }
  return finished;
}

/// The thread the strip names, by the label the sidebar's own row carries for it.
function runThreadLabel(record = backgroundRun()) {
  if (!record) return t("ui.anotherThread", "another thread");
  const listed = record.session ? sessionById(record.session) : null;
  return record.runTitle || (listed ? sessionLabel(listed) : "") || t("ui.anotherThread", "another thread");
}

/// The header says where a turn in another thread is running, and the whole strip
/// is the way back to it: the rest of the window — the transcript, the composer —
/// is about the thread being read, so one line has to say what is still going and
/// which conversation it belongs to. A run in the thread on screen is read from
/// its own composer and needs no strip.
function updateRunBanner() {
  const banner = el("run-banner");
  if (!banner) return;
  const record = backgroundRun();
  banner.hidden = !record;
  if (record) {
    const label = runThreadLabel(record);
    const text = record.busy
      ? t("ui.aTurnIsRunningIn", "A turn is running in “{value0}”", { value0: label })
      : t("ui.aTurnFinishedIn", "A turn finished in “{value0}”", { value0: label });
    el("run-banner-text").textContent = text;
    banner.title = t("ui.open", "Open “{value0}”", { value0: label });
    banner.setAttribute("aria-label", t("ui.openIt", "{value0}. Open it.", { value0: text }));
  }
  paintQueue();
  paintRunningChrome();
  updateSendState();
}

/// The banner's click: the thread the running turn is in, opened for reading, so
/// the Stop or the queue it takes is in reach again. The run's own folder is where
/// its thread lives, so a reader who moved to another project is taken back there
/// first. It is the same door the sidebar's own row for that thread is.
async function openRun() {
  const record = backgroundRun();
  if (!record) return;
  await openThreadRecord(record);
}

/// Opens one held thread — its own conversation, cards and totals — in the tab on
/// screen, moving the window to the folder it ran in first when that is another
/// project's.
async function openThreadRecord(record) {
  if (!record) return;
  record.unread = false;
  if (record.project && record.project !== state.project) {
    const project = state.projects.find((entry) => entry.path === record.project);
    if (project) await selectProject(project);
  }
  const id = record.session;
  if (!id) {
    // A turn that has not reported its thread yet: the record is the thread, so
    // the tab is pointed at the draft and handed what it has painted.
    parkThread();
    const tab = activeChatTab();
    if (tab) {
      tab.draftKey = record.key;
      tab.session = null;
    }
    state.session = null;
    state.worktree = record.worktree;
    restoreThread(record);
    paintUsage();
    paintWorktreeChip();
    closeReview();
    refreshThreadTitle();
    updateRunBanner();
    updateSendState();
    el("prompt").focus();
    return;
  }
  state.worktree = record.worktree;
  paintWorktreeChip();
  const listed = sessionById(id);
  if (listed && !listed.unstored) {
    await selectSessionFromTree(listed);
    return;
  }
  parkThread();
  const tab = activeChatTab();
  if (tab) {
    tab.session = id;
    tab.draftKey = undefined;
  }
  state.session = id;
  restoreThread(record);
  state.changes = record.changes;
  closeReview();
  refreshThreadTitle();
  updateRunBanner();
  updateSendState();
  loadGit();
  el("prompt").focus();
}

// ---------- attachments ----------

/// `data:<mime>;base64,...` — the only shape the clipboard and FileReader give
/// us, and the shape the backend turns back into a media part.
function dataUrlMime(dataUrl) {
  const match = /^data:([^;,]+)[;,]/.exec(dataUrl || "");
  return match ? match[1].toLowerCase() : "";
}

/// Enforce the core's own limit (`media::MAX_ATTACHMENT_BYTES`): a file past it
/// is refused before it is read into a data URL, since the bytes exist several
/// times over once they do. Whether a file may travel is the core's call — it
/// reads a file's own bytes (`media::load_attachment`: any image, a PDF, or any
/// text file, with an image format a provider does not take converted rather
/// than refused) — and the type a browser declared says nothing about what the
/// file holds: a picked `.md` can arrive as `application/octet-stream`, and
/// gating on that is how a valid attachment is turned away from the core that
/// would have taken it.
const MAX_ATTACHMENT_BYTES = 20 * 1024 * 1024;

/// The image formats this webview can draw, which are the ones a chip shows a
/// thumbnail for. Every other attachment — a TIFF the core converts, a PDF, a
/// text file — wears the file glyph.
const PAINTABLE_IMAGE_MIMES = [
  "image/png",
  "image/jpeg",
  "image/gif",
  "image/webp",
  "image/bmp",
];

function formatBytes(bytes) {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${Math.round(bytes / 1024)} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}

/// Whether a chip draws the picture itself rather than a file glyph. A TIFF or
/// a HEIC is an image the core sends but this browser cannot paint.
function isPaintableImage(dataUrl) {
  return PAINTABLE_IMAGE_MIMES.includes(dataUrlMime(dataUrl));
}

function addAttachment(name, dataUrl) {
  const mime = dataUrlMime(dataUrl);
  if (state.attachments.some((attachment) => attachment.dataUrl === dataUrl)) {
    setStatus(t("ui.alreadyAttached", "Already attached"));
    return false;
  }
  if (state.attachments.length >= 8) {
    setStatus(t("ui.atMost8AttachmentsPerMessage", "At most 8 attachments per message"));
    return false;
  }
  state.attachments.push({
    name: name || (mime === "application/pdf" ? "document.pdf" : "attachment"),
    dataUrl,
  });
  renderAttachments();
  updateSendState();
  return true;
}

function readFileAsDataUrl(file) {
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onload = () => resolve(reader.result);
    reader.onerror = () => reject(reader.error || new Error("read failed"));
    reader.readAsDataURL(file);
  });
}

/// Matches the core's downscale target so a pasted screenshot is not shipped at
/// full resolution.
const MAX_IMAGE_EDGE = 1568;

function loadImage(dataUrl) {
  return new Promise((resolve, reject) => {
    const image = new Image();
    image.onload = () => resolve(image);
    image.onerror = () => reject(new Error("could not decode image"));
    image.src = dataUrl;
  });
}

/// Downscales an image data URL on a canvas, keeping the original when it is
/// already small enough, the format is not a still image, or canvas fails.
async function resizeImageDataUrl(dataUrl, mime) {
  if (mime === "image/gif") return dataUrl;
  let image;
  try {
    image = await loadImage(dataUrl);
  } catch (error) {
    return dataUrl;
  }
  const longest = Math.max(image.width, image.height);
  if (!longest || longest <= MAX_IMAGE_EDGE) return dataUrl;
  const scale = MAX_IMAGE_EDGE / longest;
  try {
    const canvas = document.createElement("canvas");
    canvas.width = Math.max(1, Math.round(image.width * scale));
    canvas.height = Math.max(1, Math.round(image.height * scale));
    canvas.getContext("2d").drawImage(image, 0, 0, canvas.width, canvas.height);
    const type =
      mime === "image/jpeg" ? "image/jpeg" : mime === "image/webp" ? "image/webp" : "image/png";
    const resized = canvas.toDataURL(type, 0.9);
    // Keep the smaller dimensions even when the re-encoded bytes are not
    // shorter (a highly compressed source), so the 1568px guarantee holds.
    return resized && resized.startsWith("data:image/") ? resized : dataUrl;
  } catch (error) {
    return dataUrl;
  }
}

async function addAttachmentFiles(files, { fallbackToClipboard = false } = {}) {
  let failed = false;
  for (const file of files) {
    if (file.size > MAX_ATTACHMENT_BYTES) {
      setStatus(
        t("ui.isTheAttachmentLimitIs", "{value0} is {value1}; the attachment limit is {value2}", { value0: file.name, value1: formatBytes(file.size), value2: formatBytes(MAX_ATTACHMENT_BYTES) }),
      );
      continue;
    }
    // Nothing but the size is decided before the read: the core reads the
    // bytes the data URL carries, so a type the browser guessed at is no gate.
    // It only says whether this webview might paint the chip as a picture.
    try {
      let dataUrl = await readFileAsDataUrl(file);
      const mime = dataUrlMime(dataUrl);
      if (mime.startsWith("image/")) {
        dataUrl = await resizeImageDataUrl(dataUrl, mime);
      }
      addAttachment(file.name, dataUrl);
    } catch (error) {
      failed = true;
      setStatus(t("ui.couldNotRead", "Could not read {value0}: {value1}", { value0: file.name, value1: error }), "error");
    }
  }
  // macOS refuses a webview's read of a file in the Desktop, Documents or
  // Downloads folder, but the harness reads the pasteboard through the same
  // core read the terminal's Ctrl+V uses, so what the pasteboard itself
  // carries is attached rather than the paste failing.
  if (failed && fallbackToClipboard) await addClipboardAttachment();
}

/// The clipboard as the harness reads it, for a paste the webview had no bytes
/// for. A text paste never reaches here: only a paste that named a file does.
async function addClipboardAttachment() {
  let attachment;
  try {
    attachment = await invoke("read_clipboard");
  } catch (error) {
    return;
  }
  if (attachment) addAttachment(attachment.name, attachment.dataUrl);
}

function renderAttachments() {
  const box = el("attachments");
  box.innerHTML = "";
  box.hidden = state.attachments.length === 0;
  state.attachments.forEach((attachment, index) => {
    const chip = document.createElement("div");
    chip.className = "attachment";
    if (isPaintableImage(attachment.dataUrl)) {
      chip.appendChild(openableImage(attachment.dataUrl, attachment.name));
    } else {
      const icon = document.createElement("div");
      icon.className = "att-file";
      icon.innerHTML = ICONS.file;
      chip.appendChild(icon);
    }
    const label = document.createElement("div");
    label.className = "att-name";
    label.textContent = attachment.name;
    chip.appendChild(label);
    const remove = document.createElement("button");
    remove.className = "att-remove";
    remove.innerHTML = ICONS.close;
    remove.title = t("ui.removeAttachment", "Remove attachment");
    remove.onclick = () => {
      state.attachments.splice(index, 1);
      renderAttachments();
      updateSendState();
    };
    chip.appendChild(remove);
    box.appendChild(chip);
  });
}

function attachmentPayload() {
  return state.attachments.map((attachment) => ({
    dataUrl: attachment.dataUrl,
    name: attachment.name,
  }));
}

function clearAttachments() {
  state.attachments = [];
  renderAttachments();
  updateSendState();
}

let imageReturnFocus = null;

function openImage(dataUrl, name = t("ui.attachmentPreview", "Attachment preview")) {
  imageReturnFocus = document.activeElement || null;
  el("image-view-img").src = dataUrl;
  el("image-view-img").alt = name || t("ui.attachmentPreview", "Attachment preview");
  el("image-view-img").title = name || t("ui.attachmentPreview", "Attachment preview");
  el("image-view-name").textContent = name || t("ui.attachmentPreview", "Attachment preview");
  closeOverlays("image-modal");
  el("image-modal").hidden = false;
  el("image-view-close").focus();
}

function closeImage() {
  if (el("image-modal").hidden) return;
  el("image-modal").hidden = true;
  const target = imageReturnFocus;
  imageReturnFocus = null;
  target?.focus?.();
}

/// A thumbnail that opens the full preview. A real button makes it focusable
/// and activatable with Enter/Space, without extra key handling.
function openableImage(dataUrl, name) {
  const button = document.createElement("button");
  button.type = "button";
  button.className = "att-open";
  button.title = t("ui.openPreview", "Open preview");
  button.setAttribute("aria-label", name ? t("common.openNamed", "Open {value0}", { value0: name }) : t("ui.openImagePreview", "Open image preview"));
  const img = document.createElement("img");
  img.src = dataUrl;
  img.alt = name || "attachment";
  // WebKit drags a picture out of the page by default, and a drag that starts
  // on it takes the click that opens the preview with it, so the press lands on
  // an image that never answers. An image is draggable unless it says otherwise.
  img.draggable = false;
  button.appendChild(img);
  button.onclick = () => openImage(dataUrl, name);
  return button;
}

async function send(opposite = false) {
  if (state.handingOff) { busyRefusal(t("ui.sendingAMessage", "sending a message")); return; }
  const textarea = el("prompt");
  const prompt = textarea.value.trim();
  const attachments = attachmentPayload();
  if (!prompt && attachments.length === 0) return;
  // The message is on its way, so a list hanging off the reference that was
  // being typed into it has nothing left to complete.
  closeAt();

  // A leading slash draws the client's own command first: the app answers the
  // ones it owns (the MCP list, a picker, a new thread) instead of sending
  // them to the model, while everything else stays a prompt for the agent to
  // resolve.
  if (prompt.startsWith("/") && attachments.length === 0) {
    closePalette();
    if (await runSlashCommand(prompt)) {
      textarea.value = "";
      textarea.style.height = "auto";
      updateSendState();
      return;
    }
  }

  // The composer belongs to a project: a message with nowhere to run is answered
  // with the picker rather than sent against the directory the app was launched
  // in, which is $HOME on one platform and `/` on another, not a folder the user
  // picked. What was typed stays in the box while the folder is chosen.
  if (!state.project) {
    setStatus(t("composer.selectProject", "Select a project first."));
    askForProject();
    return;
  }

  if (state.busy) {
    // The run is this thread's own, so a message typed here is the correction or
    // the follow-up it looks like: queued for the next turn of this conversation,
    // or steered into the response on screen.
    const defaultQueue = state.followUpBehavior === "queue";
    const followUp = opposite ? !defaultQueue : defaultQueue;
    lastInputs.set(inputHistoryKey(), textarea.value);
    textarea.value = "";
    clearAttachments();
    clearWelcome();
    if (followUp) {
      state.pendingSends.push({
        prompt,
        attachments,
        project: runFolder(),
        session: state.session,
        showBubble: true,
      });
      paintQueue();
      updateSendState();
      setStatus(t("ui.queued", "Queued: {value0}", { value0: prompt.split(/\r?\n/, 1)[0] || "an attachment" }));
      return;
    }
    el("transcript").appendChild(
      bubble("user", prompt, attachments),
    );
    scrollDown(el("transcript"), true);
    if (state.runId == null) {
      // The turn has not reported its id yet: the message waits rather than
      // being lost, and starts this thread's next turn.
      state.pendingSends.push({ prompt, attachments, showBubble: false });
      paintQueue();
      updateSendState();
      return;
    }
    const accepted = await invoke("steer_run", {
      runId: state.runId,
      message: prompt,
      followUp: false,
      attachments: attachments.length ? attachments : null,
    });
    updateSendState();
    if (!accepted) {
      // A message that arrived as the run finished is the next turn of the
      // thread and folder it was typed in, not of whatever is on screen by then.
      state.pendingSends.push({
        prompt,
        attachments,
        project: runFolder(),
        session: state.session,
        showBubble: false,
      });
      setStatus(t("ui.theResponseFinishedStartingThisAsTheNextTurn", "The response finished; starting this as the next turn…"));
      // The end event may have beaten the command reply to the renderer.
      await startNextPendingSend();
      return;
    }
    setStatus(t("composer.steering", "Steering the active response…"));
    return;
  }

  await startPrompt(prompt, attachments);
}

/// Starts a turn in `thread` — the conversation on screen unless a queued message
/// names its own — and records the run on that thread, so its events paint into
/// that conversation wherever the reader is while it works.
async function startPrompt(prompt, attachments, showBubble = true, thread = null) {
  const record = thread || visibleThread();
  const here = record === visibleThread();
  if (here) {
    // The conversation on screen is the one the composer describes: its folder
    // and the checkout the reader picked travel with the message.
    record.project = state.project;
    record.worktree = state.worktree;
    record.turnOptions = {
      agent: state.agent || null,
      reasoning: state.reasoningPicked ? state.reasoning : null,
    };
  }
  // Queued turns keep their conversation's choices while another chat is open.
  const turnOptions = record.turnOptions || { agent: null, reasoning: null };
  const project = record.worktree || record.project || view.project;
  if (!project) return;
  const session = record.session;
  record.project ||= project;
  record.runProject = project;
  record.runSession = session;
  record.runTitle = "";
  record.stopRequested = false;
  if (here) {
    clearWelcome();
    lastInputs.set(inputHistoryKey(), el("prompt").value);
    if (showBubble) {
      el("prompt").value = "";
      clearAttachments();
    }
    resetTurn();
    record.runError = "";
    record.busy = true;
    startRunClock(record);
    paintRunningChrome();
    updateSendState();
    setStatus(idleStatus());
  } else {
    record.runError = "";
    record.busy = true;
    record.tools = [];
    record.currentAssistant = null;
    record.currentThinking = null;
    record.currentActivity = null;
    startRunClock(record);
  }
  if (showBubble) {
    transcriptFor(record).appendChild(bubble("user", prompt, attachments));
    scrollDown(transcriptFor(record), true);
  }
  paintRunClock();
  try {
    await reasoningSave;
    record.runId = await invoke("send_prompt", {
      project,
      prompt,
      // No thread on screen means the reader is starting one: `new` has the core
      // create it. `latest` would append this message to whichever thread was
      // used last, which is a thread they never chose — and one the sidebar
      // would go on listing unchanged.
      session: session || "new",
      agent: turnOptions.agent,
      reasoning: turnOptions.reasoning,
      attachments: attachments.length ? attachments : null,
    });
    if (record.stopRequested && record.busy) await stopThread(record);
  } catch (error) {
    withThread(record, () => {
      reportRunError(String(error));
      setIdle();
    });
    if (here) paintQueue();
  }
}

/// The turn a thread has queued start once its last one ends — the message the
/// reader composed while it worked, in this thread whether or not they are still
/// looking at it.
async function startNextPendingSend(thread = visibleThread()) {
  const record = thread || visibleThread();
  if (record.busy || record.pendingSends.length === 0) return;
  const pending = record.pendingSends.shift();
  if (record === visibleThread()) paintQueue();
  await startPrompt(pending.prompt, pending.attachments, pending.showBubble !== false, record);
}

async function queueAction(action, index, direction = 0) {
  if (action === "behavior") {
    setFollowUpBehavior(state.followUpBehavior === "queue" ? "steer" : "queue");
    return;
  }
  if (index < 0 || index >= state.pendingSends.length) return;
  if (action === "delete") {
    state.pendingSends.splice(index, 1);
  } else if (action === "move") {
    const target = Math.max(0, Math.min(state.pendingSends.length - 1, index + direction));
    if (target !== index) {
      const [pending] = state.pendingSends.splice(index, 1);
      state.pendingSends.splice(target, 0, pending);
      setStatus(t("ui.movedQueuedMessageToPositionOf", "Moved queued message to position {value0} of {value1}.", { value0: target + 1, value1: state.pendingSends.length }));
    }
  } else if (action === "edit") {
    if (el("prompt").value.length > 0 || state.attachments.length > 0) {
      setStatus(t("ui.sendOrClearTheCurrentDraftBeforeEditingAQueuedMessage", "Send or clear the current draft before editing a queued message."));
      return;
    }
    const [pending] = state.pendingSends.splice(index, 1);
    el("prompt").value = pending.prompt;
    state.attachments = pending.attachments.map((attachment) => ({ ...attachment }));
    renderAttachments();
    el("prompt").style.height = "auto";
    el("prompt").style.height = `${Math.min(el("prompt").scrollHeight, 220)}px`;
    el("prompt").focus();
  } else if (action === "steer") {
    if (!state.busy || state.runId == null) return;
    const pending = state.pendingSends[index];
    const accepted = await invoke("steer_run", {
      runId: state.runId,
      message: pending.prompt,
      followUp: false,
      attachments: pending.attachments.length ? pending.attachments : null,
    });
    if (!accepted) return;
    state.pendingSends.splice(index, 1);
    state.currentActivity = null;
    el("transcript").appendChild(bubble("user", pending.prompt, pending.attachments));
    scrollDown(el("transcript"), true);
    setStatus(t("composer.steering", "Steering the active response…"));
  }
  paintQueue();
  updateSendState();
}

async function stop() {
  await stopThread(visibleThread());
}

/// Ends one thread's turn. The corner offers this for the thread in the composer,
/// and the message the turn left behind stays in that thread.
async function stopThread(record) {
  if (!record?.busy) return;
  record.stopRequested = true;
  if (record === visibleThread()) setStatus(t("composer.stopping", "Stopping…"));
  if (record.runId == null) return;
  try {
    await invoke("cancel_run", { runId: record.runId });
  } catch (error) {
    withThread(record, () => reportRunError(String(error)));
  }
}

function ensureAssistant() {
  if (state.currentAssistant) return state.currentAssistant;
  const wrap = document.createElement("div");
  wrap.className = "msg assistant commentary";
  const label = speakerLabel("assistant");
  const body = document.createElement("div");
  body.className = "body";
  wrap.append(label, body);
  runTranscript().appendChild(wrap);
  state.currentAssistant = { wrap, body, text: "", phase: "commentary" };
  return state.currentAssistant;
}

function setAssistantPhase(phase) {
  const assistant = state.currentAssistant;
  if (!assistant) return;
  assistant.phase = phase === "final_answer" ? "final_answer" : "commentary";
  assistant.wrap.classList.toggle("commentary", assistant.phase === "commentary");
  assistant.wrap.classList.toggle("final_answer", assistant.phase === "final_answer");
}

function appendText(delta) {
  state.currentActivity = null;
  const assistant = ensureAssistant();
  assistant.text += delta;
  paintMarkdown(assistant.body, assistant.text);
  scrollDown(runTranscript());
}

function discardAttempt() {
  if (state.currentThinking) {
    state.currentThinking.remove();
    state.currentThinking = null;
  }
  if (state.currentAssistant) {
    state.currentAssistant.wrap.remove();
    state.currentAssistant = null;
  }
}

function appendThinking(delta) {
  state.currentActivity = null;
  const transcript = runTranscript();
  if (!state.currentThinking) {
    const block = document.createElement("div");
    block.className = "thinking";
    block.textContent = "✦ ";
    transcript.appendChild(block);
    state.currentThinking = block;
  }
  state.currentThinking.textContent += delta;
  scrollDown(transcript);
}

// Leading shell noise (`export PATH=…;`, `cd …;`) and label/utility statements
// that would fill the card header without saying what the call does.
const CMD_NOISE = /^(?:export\b|cd\b|date\b|pwd\b|true\b|:|echo\b|printf\b|set\b)/;

function toolArgs(argsJson) {
  try {
    const args = JSON.parse(argsJson || "{}");
    return args && typeof args === "object" ? args : {};
  } catch {
    return {};
  }
}

/// The executable text a call carried. A shell card's command is copied exactly
/// as it was passed to the tool, without the shortened summary the header shows.
function toolCommand(name, argsJson) {
  if (name !== "bash") return "";
  return String(toolArgs(argsJson).command || "");
}

/// The one-line subject a tool card shows. Shell calls drop the boilerplate
/// (the PATH export every desktop shell needs, the `cd` prefix, `echo` labels)
/// and keep the first real statement; other tools show their subject.
function toolSummary(name, argsJson) {
  const args = toolArgs(argsJson);
  const subject =
    name === "grep" || name === "find"
      ? args.pattern
      : args.command || args.path || args.pattern || args.query || args.url || args.prompt;
  const raw = String(subject || "")
    .replace(/\s+/g, " ")
    .trim();
  let text = raw;
  let title = raw;
  if ((name === "grep" || name === "find") && raw) {
    const path = String(args.path || ".").trim();
    text = `${JSON.stringify(raw)}${path && path !== "." ? ` · ${path}` : ""}`;
    title = t("ui.in", "{value0} {value1} in {value2}", { value0: name === "grep" ? t("ui.searchFor", "Search for") : t("ui.find", "Find"), value1: JSON.stringify(raw), value2: path || "." });
  }
  if (name === "bash" && raw) {
    const parts = raw
      .split(/\s*(?:&&|\|\||;)\s*/)
      .map((part) => part.trim())
      .filter(Boolean);
    text = parts.find((part) => !CMD_NOISE.test(part)) || parts[parts.length - 1] || raw;
  }
  return {
    text,
    title: title || text,
  };
}

/// The names and marks Codex gives a tool call. MCP tools keep their own name,
/// while the built-ins say what the reader asked for rather than exposing the
/// engine's wire name.
function toolDisplayName(name) {
  return {
    tool: "Tool result",
    "": "Tool result",
    bash: "Command",
    read: "Read",
    write: "Write",
    edit: "Edit",
    find: "Find",
    grep: "Search",
    ls: "List",
    web_search: "Web search",
    web_fetch: "Web fetch",
    webfetch: "Web fetch",
  }[name] || name;
}

function toolDisplayIcon(name) {
  if (isMcpTool(name)) return ICONS.plug;
  if (name === "read") return ICONS.file;
  if (name === "write" || name === "edit") return ICONS.compose;
  if (name === "find" || name === "grep") return ICONS.search;
  if (name === "ls") return ICONS.folder;
  if (name === "web_search" || name === "web_fetch" || name === "webfetch") return ICONS.globe;
  if (name === "bash") return ICONS.terminal;
  return ICONS.tool;
}

/// The action a grouped tool row names. A run of calls should read like the
/// work the reader watched happen, not like a count of engine events.
function toolActivityAction(name) {
  const action = {
    read: "Read",
    bash: "Ran",
    write: "Edited",
    edit: "Edited",
    find: "Searched",
    grep: "Searched",
    ls: "Listed",
    web_search: "Searched web",
    web_fetch: "Fetched",
    webfetch: "Fetched",
  }[name] || toolDisplayName(name);
  return { action, icon: toolDisplayIcon(name) };
}

/// A tool a connected server answers, or one of the resource reads the core
/// names as MCP rather than built-in: the same test `tools.rs` applies when it
/// keeps a result.
function isMcpTool(name) {
  return name.includes("__") ||
    ["list_mcp_resources", "list_mcp_resource_templates", "read_mcp_resource"].includes(name);
}

/// The first `lines` of `text`, plus how many were dropped, for the collapsed
/// card preview.
function previewShell(text, limit) {
  const all = String(text || "").trimEnd().split("\n");
  const stderr = all.indexOf("[stderr]");
  const diagnostics = stderr < 0 ? [] : all.slice(stderr + 1, -1);
  const warningsOnly = diagnostics.some((line) => /Warning:|warnings\.warn\(/.test(line)) &&
    !diagnostics.some((line) => /\b(?:\w*error|\w*exception|fatal):|traceback\b/i.test(line));
  const preferred = all.at(-1) === "[exit: 0]" && stderr > 0 &&
    all.slice(0, stderr).some((line) => line.trim()) && warningsOnly
    ? [...all.slice(0, stderr), all.at(-1)] : all;
  // Blank lines and decorative rules at the tail should not displace results.
  const meaningful = preferred.filter((line) => line.trim() && !/^[\s═─━_=-]{3,}$/.test(line));
  const shown = (meaningful.length ? meaningful : preferred).slice(-limit);
  return { text: shown.join("\n"), more: all.length - shown.length, earlier: preferred === all && meaningful.length === preferred.length };
}


function previewText(text, lines = 3) {
  const all = String(text || "").replace(/\s+$/, "");
  if (!all) return { text: "", more: 0 };
  const split = all.split("\n");
  if (split.length <= lines) return { text: all, more: 0 };
  return { text: split.slice(0, lines).join("\n"), more: split.length - lines };
}

function formatDuration(ms) {
  if (ms < 1000) return `${ms}ms`;
  const seconds = ms / 1000;
  if (seconds < 60) return `${seconds.toFixed(1)}s`;
  return `${Math.floor(seconds / 60)}m ${Math.round(seconds % 60)}s`;
}

/// A run of adjacent tools is one quiet activity row. The first call keeps its
/// own card; when a second joins it the header replaces the separate rows until
/// the reader opens it. Text or reasoning between calls closes the run, so
/// progress commentary is never swallowed into a later activity.
function createToolActivity(transcript) {
  const block = document.createElement("div");
  block.className = "tool-activity single";
  const head = document.createElement("div");
  head.className = "tahead";
  head.hidden = true;
  const caret = document.createElement("span");
  caret.className = "ta-caret";
  caret.setAttribute("aria-hidden", "true");
  caret.innerHTML = ICONS.caret;
  const mark = document.createElement("span");
  mark.className = "ta-icon";
  mark.setAttribute("aria-hidden", "true");
  const label = document.createElement("span");
  label.className = "ta-label";
  const kinds = document.createElement("span");
  kinds.className = "ta-kinds";
  kinds.setAttribute("aria-hidden", "true");
  const detail = document.createElement("span");
  detail.className = "ta-detail";
  const status = document.createElement("span");
  status.className = "ta-state";
  head.append(caret, mark, label, kinds, detail, status);
  const body = document.createElement("div");
  body.className = "tabody";
  block.append(head, body);
  transcript.appendChild(block);
  const activity = {
    block,
    head,
    body,
    mark,
    label,
    kinds,
    detail,
    status,
    tools: [],
    expanded: false,
    owner: paintingThread(),
  };
  head.onclick = () => toggleToolActivity(activity);
  head.onkeydown = (event) => {
    if (event.key !== "Enter" && event.key !== " ") return;
    event.preventDefault();
    toggleToolActivity(activity);
  };
  return activity;
}

function paintToolActivity(activity) {
  const count = activity.tools.length;
  const failed = activity.tools.filter((tool) => tool.isError).length;
  const runningTool = [...activity.tools].reverse().find((tool) => !tool.done);
  const current = runningTool || activity.tools.at(-1);
  const unresolved = failed > 0 && !runningTool && Boolean(activity.tools.at(-1)?.isError);
  const recovered = failed > 0 && !runningTool && !unresolved;
  let summary = t("ui.toolCalls", "{count} tool calls", { count });
  if (failed) {
    summary += ` · ${recovered
      ? t("ui.recoveredToolCalls", "{count} recovered", { count: failed })
      : t("ui.failedToolCalls", "{count} failed", { count: failed })}`;
  }
  activity.block.classList.toggle("single", count === 1);
  activity.block.classList.toggle("multi", count > 1);
  activity.block.classList.toggle("failed", unresolved);
  activity.head.hidden = count === 1;
  activity.body.hidden = count > 1 && !activity.expanded;
  activity.head.tabIndex = count > 1 ? 0 : -1;
  if (count > 1) {
    activity.head.setAttribute("role", "button");
    activity.head.setAttribute("aria-expanded", activity.expanded ? "true" : "false");
  } else {
    activity.head.removeAttribute("role");
    activity.head.removeAttribute("aria-expanded");
  }
  activity.label.textContent = t("ui.toolActivity", "Tool activity");
  activity.label.hidden = count <= 1;
  activity.mark.innerHTML = current ? toolDisplayIcon(current.name) : ICONS.power;
  activity.mark.hidden = count > 1;
  activity.kinds.hidden = count <= 1;
  if (count > 1) {
    const actions = [];
    const grouped = new Map();
    for (const tool of activity.tools) {
      const item = toolActivityAction(tool.name);
      let group = grouped.get(item.action);
      if (!group) {
        group = { ...item, count: 0 };
        grouped.set(item.action, group);
        actions.push(group);
      }
      group.count += 1;
    }
    activity.kinds.replaceChildren();
    for (const item of actions) {
      const kind = document.createElement("span");
      kind.className = "ta-kind";
      const icon = document.createElement("span");
      icon.className = "ta-kind-icon";
      icon.innerHTML = item.icon;
      const text = document.createElement("span");
      text.textContent = item.action;
      kind.append(icon, text);
      if (item.count > 1) {
        const count = document.createElement("span");
        count.className = "ta-kind-count";
        count.textContent = String(item.count);
        kind.appendChild(count);
      }
      activity.kinds.appendChild(kind);
    }
  } else {
    activity.kinds.replaceChildren();
  }
  activity.detail.textContent = current ? toolSummary(current.name, current.args).text : "";
  activity.status.innerHTML = runningTool
    ? `<span class="spinner"></span>${t("ui.running", "Running")}`
    : unresolved
      ? t("ui.failed", "Failed")
      : "";
  activity.head.setAttribute(
    "aria-label",
    count > 1
      ? `${summary}${runningTool ? ", running" : ""}. ${activity.expanded ? "Collapse" : "Expand"} tool activity`
      : summary,
  );
  activity.head.title = count > 1
    ? `${summary} — ${activity.expanded
      ? t("ui.hideToolActivity", "Hide tool activity")
      : t("ui.showToolActivity", "Show tool activity")}`
    : "";
}

function toggleToolActivity(activity) {
  if (activity.tools.length < 2) return;
  activity.expanded = !activity.expanded;
  paintToolActivity(activity);
  scrollDown(runTranscript());
}

function placeTool(tool, transcript = runTranscript()) {
  let activity = state.currentActivity;
  if (!activity) {
    activity = createToolActivity(transcript);
    state.currentActivity = activity;
  }
  tool.activity = activity;
  activity.tools.push(tool);
  activity.body.appendChild(tool.block);
  paintToolActivity(activity);
}

function toolCopyButton(label, textFor) {
  const button = document.createElement("button");
  button.type = "button";
  button.className = "tool-copy";
  button.innerHTML = ICONS.copy;
  button.title = label;
  button.setAttribute("aria-label", label);
  let reset = null;
  button.onclick = async (event) => {
    event?.stopPropagation?.();
    if (!(await copyText(textFor()))) {
      showToast(t("ui.couldNotCopy", "Could not copy to the clipboard."), "error");
      return;
    }
    clearTimeout(reset);
    button.innerHTML = ICONS.check;
    button.classList.add("copied");
    button.title = t("ui.copied", "Copied");
    button.setAttribute("aria-label", button.title);
    reset = setTimeout(() => {
      button.innerHTML = ICONS.copy;
      button.classList.remove("copied");
      button.title = label;
      button.setAttribute("aria-label", label);
    }, 1600);
  };
  button.onkeydown = (event) => event.stopPropagation?.();
  return button;
}

/// Builds one tool card. `startTool` wires it for a live call; the stored
/// transcript calls it too so a reopened thread reads the same way.
function createToolCard(name, args) {
  const block = document.createElement("div");
  block.chatTool = { name, args };
  block.className = `tool running${name === "bash" ? " tool-command" : ""}`;
  const head = document.createElement("div");
  head.className = "thead";
  // The caret the card folds by: the same drawing the change card and its rows
  // fold by, shown once the card has output behind it (`.foldable`).
  const caret = document.createElement("span");
  caret.className = "tool-caret";
  caret.setAttribute("aria-hidden", "true");
  caret.innerHTML = ICONS.caret;
  const tname = document.createElement("span");
  tname.className = "tname";
  const nameIcon = document.createElement("span");
  nameIcon.className = "tool-icon";
  nameIcon.setAttribute("aria-hidden", "true");
  nameIcon.innerHTML = toolDisplayIcon(name);
  const nameText = document.createElement("span");
  nameText.textContent = toolDisplayName(name);
  tname.append(nameIcon, nameText);
  const targ = document.createElement("span");
  targ.className = "targ";
  const summary = toolSummary(name, args);
  targ.textContent = summary.text;
  if (summary.title) head.title = summary.title;
  const tstate = document.createElement("span");
  tstate.className = "tstate";
  tstate.innerHTML = '<span class="spinner"></span>running';
  const actions = document.createElement("span");
  actions.className = "tool-actions";
  const command = toolCommand(name, args);
  if (command) {
    actions.appendChild(toolCopyButton(
      t("ui.copyCommand", "Copy command"),
      () => command,
    ));
  }
  const outputCopy = toolCopyButton(
    t("ui.copyOutput", "Copy output"),
    () => tool.full,
  );
  outputCopy.hidden = true;
  actions.appendChild(outputCopy);
  head.append(caret, tname, targ, actions, tstate);
  const pre = document.createElement("pre");
  pre.className = "tbody";
  const hint = document.createElement("div");
  hint.className = "thint";
  hint.hidden = true;
  const tool = {
    name,
    args,
    owner: paintingThread(),
    block,
    head,
    pre,
    hint,
    tstate,
    actions,
    outputCopy,
    done: false,
    full: "",
    live: "",
    expanded: false,
    started: performance.now(),
    timer: null,
  };
  // The hint is the card's own "click to expand": a reader who clicks it is
  // asking for the output the card folded away, so it toggles the card exactly
  // as the head row does. Both take Enter or Space the way a button does, so
  // the fold is not a mouse-only gesture.
  for (const toggle of [head, hint]) {
    toggle.onclick = () => toggleTool(tool);
    toggle.onkeydown = (event) => {
      if (event.key !== "Enter" && event.key !== " ") return;
      event.preventDefault();
      toggleTool(tool);
    };
  }
  block.append(head, pre, hint);
  return tool;
}

function startTool(name, args) {
  // A new tool call opens a new step: the next text lands after this card
  // instead of merging into the previous step's narration.
  state.currentAssistant = null;
  state.currentThinking = null;
  const tool = createToolCard(name, args);
  const transcript = runTranscript();
  placeTool(tool, transcript);
  state.tools.push(tool);
  startToolTimer(tool);
  scrollDown(transcript);
  return tool;
}

/// Keeps a running card honest: a spinner plus how long the tool has been
/// going, so a long call is not a static "running".
function startToolTimer(tool) {
  tool.timer = setInterval(() => {
    if (tool.done) return;
    const elapsed = Math.max(0, Math.round(performance.now() - tool.started));
    tool.tstate.innerHTML = `<span class="spinner"></span>${formatDuration(elapsed)}`;
  }, 1000);
  // Timers must not keep a headless test process alive.
  if (tool.timer && typeof tool.timer.unref === "function") tool.timer.unref();
}

/// The added and removed line counts a card's one-line header shows, counted
/// from the `+`/`-` markers the same way `oxide_core::changes` counts them, so
/// a call's numbers agree with the change listing's.
function diffCounts(text) {
  let added = 0;
  let removed = 0;
  for (const line of String(text || "").split("\n")) {
    if (line.startsWith("+")) added += 1;
    else if (line.startsWith("-")) removed += 1;
  }
  if (!added && !removed) return "";
  return `+${added} −${removed}`;
}

function hasChat() {
  return Boolean(runFolder() && (state.session || state.busy));
}

function paintLiveChanges() {
  const pill = el("live-changes");
  const changes = state.repositoryChanges;
  pill.hidden = !hasChat() || changes?.project !== runFolder() || !changes.files.length;
  if (pill.hidden) return;
  const count = changes.files.length;
  pill.innerHTML = `<span>${escapeHtml(t(count === 1 ? "changes.fileChanged" : "changes.filesChanged", count === 1 ? "{count} file changed" : "{count} files changed", { count }))}</span>${statsHtml(changes.added, changes.removed)}`;
}

/// Marks a card finished. A call that changed a file reads as the one line its
/// header builds — the path, and how many lines moved — and keeps its own diff
/// for the reader who expands the card: the turn's changes are listed together
/// by the change card, so the same diff is not painted twice.
function finishTool(tool, output, { isError = false, diff = null, elapsed = 0, attachments = [] } = {}) {
  // A call that landed may have written a file, which is what the dock's own
  // Changes row is read out of (see `scheduleProjectChanges`).
  scheduleProjectChanges();
  if (tool.timer) {
    clearInterval(tool.timer);
    tool.timer = null;
  }
  tool.done = true;
  tool.full = output || "";
  tool.outputCopy.hidden = !tool.full;
  tool.block.classList.remove("running");
  if (isError) tool.block.classList.add("error");
  tool.quiet =
    !isError &&
    (tool.name === "grep" || tool.name === "find") &&
    /^(?:No matches found|No files found matching pattern)\s*$/.test(tool.full);
  if (tool.quiet) tool.block.classList.add("quiet");
  const counts = diff ? diffCounts(diff.text) : "";
  tool.tstate.textContent = tool.quiet
    ? t("ui.0Matches", "0 matches{value0}", { value0: elapsed ? ` · ${formatDuration(elapsed)}` : "" })
    : isError
      ? "Failed"
      : counts;
  tool.tstate.setAttribute(
    "aria-label",
    isError ? "Failed" : tool.quiet ? tool.tstate.textContent : "Completed",
  );
  if (diff) {
    tool.diffEl = diffBlock(diff);
    tool.diffEl.hidden = true;
    tool.block.appendChild(tool.diffEl);
  }
  const images = attachments.filter((attachment) => isPaintableImage(attachment.dataUrl));
  if (images.length) {
    const strip = document.createElement("div");
    strip.className = "msg-attachments";
    for (const attachment of images) {
      strip.appendChild(openableImage(attachment.dataUrl, attachment.name));
    }
    tool.block.appendChild(strip);
  }
  tool.inline = Boolean(diff);
  tool.isError = isError;
  paintTool(tool);
  if (tool.activity) paintToolActivity(tool.activity);
}

/// Collapsed cards show the first lines of output plus how much was hidden;
/// clicking swaps in the full result. A card carrying a diff is one line until
/// it is expanded, so an edit does not repeat the turn's change listing.
function paintTool(tool) {
  if (!tool.done) {
    tool.pre.textContent = tool.live;
    return;
  }
  if (tool.diffEl) tool.diffEl.hidden = !tool.expanded;
  if (tool.isError) {
    tool.pre.hidden = false;
    tool.pre.textContent = tool.full;
    tool.hint.hidden = true;
    markToolToggle(tool, false);
    return;
  }
  if (tool.expanded) {
    tool.pre.hidden = false;
    tool.pre.textContent = tool.full;
    tool.hint.hidden = true;
    tool.block.classList.add("expanded");
    markToolToggle(tool, true);
    return;
  }
  tool.block.classList.remove("expanded");
  if (tool.quiet) {
    tool.pre.hidden = true;
    tool.hint.hidden = true;
    markToolToggle(tool, false);
    return;
  }
  if (tool.inline) {
    tool.pre.hidden = true;
    tool.hint.hidden = true;
    markToolToggle(tool, true);
    return;
  }
  tool.pre.hidden = false;
  const { text, more, earlier } = tool.name === "bash" ? previewShell(tool.full, 3) : previewText(tool.full, 3);
  tool.pre.textContent = text;
  tool.hint.hidden = more === 0;
  tool.hint.textContent = tool.name === "bash"
    ? t(earlier ? "surface.EarlierLines" : "surface.MoreLines", earlier ? "Show {count} earlier lines" : "Show {count} more lines", { count: more })
    : t(more === 1 ? "changes.moreLine" : "changes.moreLines", more === 1 ? "⋯ {count} more line · click to expand" : "⋯ {count} more lines · click to expand", { count: more });
  markToolToggle(tool, more > 0);
}

/// A card's two handles are buttons only while it has output to fold: once
/// marked, they are reachable with Tab and toggle with Enter or Space, and a
/// card that shows everything it has stays the text it looks like — which is
/// what `foldable` draws, the caret and the pointer both. A card can only gain
/// output to fold, so the marks are never taken back.
function markToolToggle(tool, expandable) {
  tool.block.classList.toggle("foldable", expandable);
  for (const toggle of [tool.head, tool.hint]) {
    toggle.tabIndex = expandable ? 0 : -1;
    if (expandable) {
      toggle.setAttribute("role", "button");
      toggle.setAttribute("aria-expanded", tool.expanded ? "true" : "false");
    } else {
      toggle.removeAttribute("role");
      toggle.removeAttribute("aria-expanded");
    }
  }
}

function toggleTool(tool) {
  if (!tool.done || tool.quiet || tool.isError) return;
  tool.expanded = !tool.expanded;
  paintTool(tool);
}

/// Replays a stored session: user/assistant text as bubbles, each turn's tool
/// calls as finished cards, with results matched back by `toolCallId`.
function renderStoredTranscript(container, messages) {
  state.currentActivity = null;
  const results = new Map();
  for (const message of messages) {
    if (message.role === "tool" && message.toolCallId) {
      results.set(message.toolCallId, message);
    }
  }
  for (const message of messages) {
    if (message.role === "user") {
      state.currentActivity = null;
      container.appendChild(bubble("user", message.content, message.attachments || []));
    } else if (message.role === "assistant") {
      if (typeof message.thinking === "string" && message.thinking.trim()) {
        state.currentActivity = null;
        const thinking = document.createElement("div");
        thinking.className = "thinking";
        thinking.textContent = `✦ ${message.thinking}`;
        container.appendChild(thinking);
      }
      if (message.content) {
        state.currentActivity = null;
        const phase = Array.isArray(message.toolCalls) && message.toolCalls.length
          ? "commentary"
          : "final_answer";
        container.appendChild(bubble("assistant", message.content, [], phase));
      }
      for (const call of message.toolCalls || []) {
        const result = results.get(call.id);
        const output = result?.content || "";
        const isError = output.startsWith("error:");
        const displayOutput = isError ? output.replace(/^error:\s*/, "") : output;
        const tool = createToolCard(call.name || "tool", call.arguments);
        finishTool(tool, displayOutput, { isError, attachments: result?.attachments || [] });
        placeTool(tool, container);
      }
    }
  }
  state.currentActivity = null;
}

// ---------- turn changes ----------

// Rows a card shows before the rest are folded behind a "Show N more files"
// button.
const CHANGES_VISIBLE = 3;

// The badge per status, spelling the core's `ChangeStatus`.
const CHANGE_LETTERS = { added: "A", modified: "M", deleted: "D" };

/// The card a finished turn leaves: every file the run changed, with the totals
/// its header shows. The listing comes from the turn's own snapshot, so a file a
/// formatter, a shell command or an MCP server wrote is listed beside the ones a
/// tool call named.
function renderChanges(payload) {
  const changes = payload && payload.changes;
  if (!changes || !Array.isArray(changes.files) || !changes.files.length) return;
  // The card belongs to the thread the turn changed, whichever thread the reader
  // is reading when it lands: its transcript is the one on screen or the one the
  // thread keeps off screen, and its own folder travels with the card so an Undo
  // reaches the work tree it really changed. `runTranscript` answers with that
  // thread's transcript, and `changesFor` with its cards, so an older turn's Undo
  // is dropped against the newest card *of that thread*.
  const record = paintingThread();
  const transcript = runTranscript();
  const list = changesFor();
  // The transcript it lands in belongs to one folder — the thread's own, which is
  // the window's only when that thread is this project's. A turn the frame places
  // in another folder leaves no card here: its Undo would reach into a work tree
  // this transcript is not about.
  const folder = transcriptProject();
  const project = payload.project || folder;
  if (project !== folder) return;
  const card = {
    project,
    // The state this turn left behind, which its own Undo checks the work tree
    // still holds (see `undoChanges`).
    after: payload.after || null,
    files: changes.files,
    added: changes.added || 0,
    removed: changes.removed || 0,
    baseline: payload.baseline || null,
    // The cards of its own thread, which its Undo reads to tell whether it is the
    // newest turn there.
    changes: list,
    all: false,
    undone: false,
    rows: [],
  };
  // Only the newest turn of a thread can be put back: an older card's baseline is
  // the state before *that* turn, so restoring it would take every change made
  // since with it, including the turns whose cards follow it.
  list.push(card);
  for (const older of list) {
    if (older !== card) paintChanges(older);
  }
  transcript.appendChild(changesCard(card));
  scrollDown(transcript);
  if (record === visibleThread() && project === runFolder() && !el("details-panel").hidden) {
    refreshProjectDetails();
  }
}

/// `+12 −3`; compact turn cards also show a side with zero changed lines.
function statsHtml(added, removed, showZero = false) {
  const add = added || showZero ? `<span class="stats-add">+${added}</span>` : "";
  const del = removed || showZero ? `<span class="stats-del">−${removed}</span>` : "";
  return add + del;
}

function statusBadge(status) {
  const key = CHANGE_LETTERS[status] ? status : "modified";
  return `<span class="change-status ${key}" title="${escapeHtml(t(`changes.${key}`, key))}">${CHANGE_LETTERS[key]}</span>`;
}

function changeRow(file, onOpen = null) {
  const row = document.createElement("button");
  row.type = "button";
  row.className = "change-row";
  if (onOpen) {
    row.classList.add("turn-change-row");
    const split = Math.max(file.path.lastIndexOf("/"), file.path.lastIndexOf("\\")) + 1;
    row.innerHTML =
      `<span class="change-path"><span class="change-directory">${escapeHtml(file.path.slice(0, split))}</span><span class="change-filename">${escapeHtml(file.path.slice(split))}</span></span>` +
      `<span class="change-stats">${file.binary ? t("changes.binary", "binary") : statsHtml(file.added, file.removed, true)}</span>`;
    row.title = `${file.path} · ${t(`changes.${file.status}`, file.status)}`;
    row.setAttribute("aria-label", t("ui.viewChangesFor", "View changes for {value0}", { value0: file.path }));
    row.onclick = onOpen;
  } else {
    row.innerHTML =
      statusBadge(file.status) +
      `<span class="change-path">${escapeHtml(file.path)}</span>` +
      `<span class="change-stats">${file.binary ? t("changes.binary", "binary") : statsHtml(file.added, file.removed) || t("changes.noLines", "no line changes")}</span>` +
      `<span class="change-chev">${ICONS.caret}</span>`;
  }
  return row;
}

function changesCard(card) {
  const block = document.createElement("div");
  block.className = "changes";

  const head = document.createElement("div");
  head.className = "changes-head";
  const icon = document.createElement("span");
  icon.className = "changes-icon";
  icon.setAttribute("aria-hidden", "true");
  icon.innerHTML = ICONS.changes;
  const summary = document.createElement("div");
  summary.className = "changes-summary";
  const title = document.createElement("span");
  title.className = "changes-title";
  const total = document.createElement("span");
  total.className = "changes-total";
  const note = document.createElement("span");
  note.className = "changes-note";
  note.hidden = true;

  const actions = document.createElement("div");
  actions.className = "changes-actions";
  const review = document.createElement("button");
  review.type = "button";
  review.className = "changes-review";
  review.textContent = t("topbar.viewChanges", "View changes");
  review.onclick = (event) => {
    event.stopPropagation();
    openReview(card);
  };
  const undo = document.createElement("button");
  undo.type = "button";
  undo.className = "changes-undo";
  undo.textContent = t("ui.undo", "Undo");
  undo.onclick = (event) => {
    event.stopPropagation();
    undoChanges(card);
  };
  actions.append(undo, review);

  const list = document.createElement("div");
  list.className = "changes-list";
  const more = document.createElement("button");
  more.type = "button";
  more.className = "changes-more";

  const moreLabel = document.createElement("span");
  moreLabel.className = "changes-more-label";
  const moreIcon = document.createElement("span");
  moreIcon.className = "changes-more-caret";
  moreIcon.setAttribute("aria-hidden", "true");
  moreIcon.innerHTML = ICONS.caret;
  more.append(moreLabel, moreIcon);
  summary.append(title, total);
  head.append(icon, summary, note, actions);
  block.append(head, list, more);

  card.block = block;
  card.list = list;
  card.more = more;
  card.moreLabel = moreLabel;
  card.note = note;
  card.undo = undo;
  card.review = review;
  card.title = title;
  card.total = total;
  for (const [index, file] of card.files.entries()) {
    const row = changeRow(file, () => openReview(card, index));
    card.rows.push({ file, row });
    list.appendChild(row);
  }
  more.onclick = (event) => {
    event.stopPropagation();
    card.all = !card.all;
    paintChanges(card);
  };
  paintChanges(card);
  return block;
}

function paintChanges(card) {
  card.review.textContent = t("topbar.viewChanges", "View changes");
  card.undo.innerHTML = ICONS.undo + escapeHtml(t("ui.undo", "Undo"));
  if (card.undone) card.note.textContent = t("ui.undone", "Undone");
  const count = card.files.length;
  card.title.textContent = t(count === 1 ? "changes.editedFile" : "changes.editedFiles", count === 1 ? "Edited {count} file" : "Edited {count} files", { count });
  card.total.innerHTML = statsHtml(card.added, card.removed, Boolean(card.added || card.removed));
  // An undo restores the state before its own turn, so it is only offered while
  // that turn is the newest one of its own thread: anything made after it would
  // go too.
  const list = card.changes || state.changes;
  card.undo.hidden = card.undone || list[list.length - 1] !== card;
  const shown = card.all ? card.rows : card.rows.slice(0, CHANGES_VISIBLE);
  const visible = new Set(shown);
  for (const entry of card.rows) entry.row.hidden = !visible.has(entry);
  const hidden = count - shown.length;
  card.more.hidden = count <= CHANGES_VISIBLE;
  card.more.setAttribute("aria-expanded", String(card.all));
  card.moreLabel.textContent = card.all ? t("sidebar.showLess", "Show less") : t(hidden === 1 ? "changes.moreFile" : "changes.moreFiles", hidden === 1 ? "Show {count} more file" : "Show {count} more files", { count: hidden });
  card.more.classList.toggle("expanded", card.all);
}

/// Puts the project back to the state the turn started from. The baseline is
/// the snapshot the run was marked against, so this reaches exactly what the
/// card lists rather than the repository's last commit — and the turn's own
/// project and post-turn state travel with it, so the restore lands in the work
/// tree the card came from and is refused once anything has changed there since
/// the turn ended.
async function undoChanges(card) {
  if (!card.baseline || card.undone) return;
  const count = card.files.length;
  const ok = await confirmDialog(
    t("ui.undoChanges", "Undo changes"),
    t(count === 1 ? "changes.undoConfirmOne" : "changes.undoConfirm", count === 1 ? "Put {count} file back to how it was before this turn?" : "Put {count} files back to how they were before this turn?", { count }),
    t("ui.undoChanges", "Undo changes"),
  );
  if (!ok) return;
  try {
    await invoke("undo_turn", {
      project: card.project || state.project,
      baseline: card.baseline,
      after: card.after,
    });
  } catch (error) {
    showToast(t("ui.couldNotUndo", "Could not undo: {value0}", { value0: error }), "error");
    return;
  }
  card.undone = true;
  card.note.textContent = t("ui.undone", "Undone");
  card.note.hidden = false;
  card.undo.hidden = true;
  showToast(t("ui.putTheTurnSFilesBack", "Put the turn's files back"));
}

// ---------- review ----------

// The card a review is open on, the file it is showing, and the sides each file
// was read at — asked for once per file, so walking a listing back and forth
// does not read the same file again. The listing is the card's own, so the
// review never disagrees with the rows behind it.
let reviewState = null;

// Unchanged lines kept around a change before the rest of a stretch folds behind
// its count: the same reach the terminal's preview keeps, and enough to place a
// change in the file it belongs to.
const REVIEW_CONTEXT = 3;

function openReview(card, index = 0) {
  reviewState = { card, index, sides: new Map(), reads: new Map(), expanded: new Set(), openFiles: new Set(card.files.slice(index, index + 1).map((file) => file.path)) };
  el("review-title").textContent = t("ui.changes", "Changes");
  el("review-total").innerHTML = statsHtml(card.added, card.removed);
  closeOverlays("review-modal");
  openPanel("review");
  el("review-scope").value = "turn";
  paintDockToggles();
  paintReview();
  el("review-files").querySelectorAll(".change-row")[index]?.scrollIntoView?.({ block: "nearest" });
  el("review-close").focus();
}

function closeReview() {
  reviewState = null;
  el("review-modal").hidden = true;
  syncPanel();
  return paintDockToggles();
}

/// Paints the card's own listing and the file the review is showing.
function paintReview() {
  if (!reviewState) return;
  const { card } = reviewState;
  el("review-title").textContent = t("ui.changes", "Changes");
  el("review-total").innerHTML = statsHtml(card.added, card.removed);
  const repository = card.project === runFolder() ? state.git : null;
  const branch = repository?.repo ? repository.branch || repository.detached : "";
  el("review-repository-row").hidden = !branch;
  el("review-total").hidden = Boolean(branch);
  el("review-branch").textContent = branch;
  el("review-repository-stats").innerHTML = statsHtml(card.added, card.removed);
  el("review-repository-root").textContent = repository?.root ? baseName(repository.root) : "";
  el("review-repository-root").title = repository?.root || "";
  // Staging is the repository's to offer: a turn's own card is what the turn did,
  // and the files it names are already what the working tree holds.
  for (const id of ["review-stage-all", "review-unstage-all"]) {
    if (el(id)) el(id).hidden = !card.repository;
  }
  const list = el("review-files");
  list.innerHTML = "";
  if (!card.files.length) {
    list.appendChild(reviewNote(t("ui.noChanges", "No changes.")));
    return;
  }
  card.files.forEach((file, index) => {
    const row = changeRow(file);
    row.classList.toggle("active", index === reviewState.index);
    row.setAttribute("aria-expanded", String(reviewState.openFiles.has(file.path)));
    row.setAttribute("aria-controls", `review-file-${index}`);
    row.classList.toggle("open", reviewState.openFiles.has(file.path));
    row.onclick = () => {
      reviewState.index = index;
      if (reviewState.openFiles.has(file.path)) reviewState.openFiles.delete(file.path);
      else reviewState.openFiles.add(file.path);
      paintReview();
    };
    list.appendChild(row);
    const actions = card.repository && reviewState.openFiles.has(file.path)
      ? changeActions(file)
      : null;
    if (actions) list.appendChild(actions);
    if (reviewState.openFiles.has(file.path)) {
      const body = document.createElement("div");
      body.className = "review-inline-diff";
      body.id = `review-file-${index}`;
      list.appendChild(body);
      paintReviewFile(file, body);
    }
  });
}

/// The buttons one file in the review carries: stage what the working tree holds,
/// take a staged change back, or revert to the branch's copy. Reverting throws
/// work away, so it is the one that asks first.
function changeActions(file) {
  const entry = state.repositoryIndex?.get(file.path) || null;
  const canStage = !entry || entry.worktree || entry.untracked;
  const canUnstage = !entry || entry.staged;
  const actions = document.createElement("div");
  actions.className = "change-actions";
  const button = (label, title, action) => {
    const node = document.createElement("button");
    node.type = "button";
    node.className = "change-action";
    node.dataset.shape = `${action}:${file.path}`;
    node.textContent = label;
    node.title = title;
    node.setAttribute("aria-label", title);
    node.onclick = (event) => {
      event.stopPropagation?.();
      void shapeChange(action, file.path);
    };
    return node;
  };
  if (canStage) {
    actions.appendChild(button(t("review.stage", "Stage"), t("review.stageHint", "Stage {value0}", { value0: file.path }), "stage"));
  }
  if (canUnstage) {
    actions.appendChild(button(t("review.unstage", "Unstage"), t("review.unstageHint", "Take {value0} back out of the index", { value0: file.path }), "unstage"));
  }
  if (entry && !entry.untracked) {
    actions.appendChild(button(t("review.revert", "Revert"), t("review.revertHint", "Discard the change to {value0}", { value0: file.path }), "revert"));
  }
  const open = document.createElement("button");
  open.type = "button";
  open.className = "change-action";
  open.dataset.open = file.path;
  open.textContent = t("file.open", "Open");
  open.title = t("file.openHint", "Open {value0}", { value0: file.path });
  open.setAttribute("aria-label", open.title);
  open.onclick = (event) => {
    event.stopPropagation?.();
    void openFile(file.path);
  };
  actions.appendChild(open);
  return actions.children.length ? actions : null;
}

/// Runs one staging action and repaints what git now holds, so the pane never
/// shows the change the reader thought they made.
async function shapeChange(action, path, hunk = null) {
  const project = reviewState?.card?.project || runFolder();
  if (!project) return;
  if (action === "revert" || action === "revert-all" || action === "revert-hunk") {
    const confirmed = await confirmDialog(
      t("review.revertTitle", "Revert the change?"),
      path
        ? t("review.revertFile", "{value0} goes back to what the branch has. The work is lost.", { value0: path })
        : t("review.revertEverything", "Every tracked file goes back to what the branch has. The work is lost."),
      t("review.revert", "Revert"),
    );
    if (!confirmed) return;
  }
  const args = { project, action };
  if (path) args.path = path;
  if (hunk) args.hunk = hunk;
  try {
    await invoke("shape_change", args);
  } catch (error) {
    showToast(t("review.shapeFailed", "Git refused: {value0}", { value0: error }), "error");
    return;
  }
  await readRepositoryIndex(project);
  await readProjectChanges(project);
}

async function readRepositoryIndex(project) {
  try {
    const states = await invoke("repository_status", { project });
    state.repositoryIndex = new Map((states || []).map((entry) => [entry.path, entry]));
  } catch {
    state.repositoryIndex = null;
  }
}

function paintReviewFile(file, target) {
  if (!reviewState) return;
  const current = reviewState;
  const showing = () => reviewState === current && current.openFiles.has(file.path);
  const sides = current.sides.get(file.path);
  if (sides !== undefined) {
    target.replaceChildren(...reviewBody(file, sides));
    return;
  }
  target.replaceChildren(reviewNote(t("ui.readingTheChange", "Reading the change⋯")));
  const args = { project: current.card.project, path: file.path };
  if (!current.card.repository) args.baseline = current.card.baseline;
  let reading = current.reads.get(file.path);
  if (!reading) {
    reading = current.card.repository
      ? invoke("repository_sides", args)
      : invoke("change_sides", args);
    current.reads.set(file.path, reading);
  }
  reading
    .then((read) => {
      if (!showing()) return;
      current.sides.set(file.path, read);
      paintReviewFile(file, target);
    })
    .catch((error) => {
      if (!showing()) return;
      current.sides.set(file.path, { failed: String(error) });
      paintReviewFile(file, target);
    });
}


function reviewNote(text) {
  const note = document.createElement("div");
  note.className = "review-note";
  note.textContent = text;
  return note;
}

/// What stands in for a diff that cannot be painted, in the words the row's own
/// diff uses.
function reviewBody(file, sides) {
  if (sides.failed) return [reviewNote(sides.failed)];
  if (sides.binary) return [reviewNote(t("ui.binaryFileNoTextDiff", "Binary file — no text diff."))];
  if (sides.omitted) return [reviewNote(t("ui.fileTooLargeToDiffLineByLine", "File too large to diff line by line."))];
  const lines = sides.lines || [];
  if (!lines.length) return [reviewNote(t("ui.noTextualChanges", "No textual changes."))];
  return [reviewSplit(file, lines)];
}

/// Both sides of the change: each line on the side it holds, with the number it
/// has there, and a long stretch neither side changed folded behind its count —
/// a reader sees where a change lands without scrolling past a file's whole
/// length. Every folded stretch of a file stays where it was put.
function reviewSplit(file, lines) {
  const split = document.createElement("div");
  split.className = "split";
  let index = 0;
  while (index < lines.length) {
    if (lines[index].kind !== "context") {
      split.appendChild(diffRow(lines[index]));
      index += 1;
      continue;
    }
    let end = index;
    while (end < lines.length && lines[end].kind === "context") end += 1;
    const open = reviewState.expanded.has(`${file.path}:${index}`);
    if (open || end - index <= REVIEW_CONTEXT * 2 + 1) {
      for (let at = index; at < end; at += 1) split.appendChild(diffRow(lines[at]));
    } else {
      for (let at = index; at < index + REVIEW_CONTEXT; at += 1) {
        split.appendChild(diffRow(lines[at]));
      }
      split.appendChild(unmodifiedBar(file, index, end - index - REVIEW_CONTEXT * 2));
      for (let at = end - REVIEW_CONTEXT; at < end; at += 1) {
        split.appendChild(diffRow(lines[at]));
      }
    }
    index = end;
  }
  return split;
}

/// The count a folded stretch shows. Clicking it opens that stretch in place,
/// which the review paints again from the sides it already holds.
function unmodifiedBar(file, start, hidden) {
  const bar = document.createElement("button");
  bar.type = "button";
  bar.className = "unmodified";
  bar.textContent = t(hidden === 1 ? "changes.contextLine" : "changes.contextLines", hidden === 1 ? "{count} unmodified line" : "{count} unmodified lines", { count: hidden });
  bar.onclick = () => {
    reviewState.expanded.add(`${file.path}:${start}`);
    paintReview();
  };
  return bar;
}

/// One line of the split: the number it has on each side and the text that side
/// holds, so a removed line leaves the new column empty and an added one leaves
/// the old column empty.
function diffRow(line) {
  const row = document.createElement("div");
  row.className = `split-row ${line.kind}`;
  const side = (number, text) => {
    const cell = document.createElement("span");
    cell.className = "split-num";
    cell.textContent = number == null ? "" : String(number);
    const body = document.createElement("span");
    body.className = "split-text";
    body.textContent = text;
    return [cell, body];
  };
  row.append(
    ...side(line.old, line.kind === "add" ? "" : line.text),
    ...side(line.new, line.kind === "remove" ? "" : line.text),
  );
  return row;
}

/// Walks the review's files, the way a changes view does.
function walkReview(step) {
  if (!reviewState) return;
  const count = reviewState.card.files.length;
  if (!count) return;
  reviewState.index = (reviewState.index + step + count) % count;
  reviewState.openFiles.add(reviewState.card.files[reviewState.index].path);
  paintReview();
  el("review-files").querySelectorAll(".change-row")[reviewState.index]?.scrollIntoView?.({ block: "nearest" });
}

function reviewKey(event) {
  if (!reviewState || event.target?.tagName === "SELECT") return;
  const step = event.key === "ArrowDown" ? 1 : event.key === "ArrowUp" ? -1 : 0;
  if (!step) return;
  event.preventDefault();
  walkReview(step);
}

let detailsRead = 0;
// The dock's Changes row is read out of the repository — a listing that walks
// every file the work tree has changed — so a turn writing files leaves it
// saying what the tree held when it came up. A call the turn lands is a reason
// to read it again, paced: a burst of calls is one listing, not one per call.
let detailsChangesRead = 0;
const DOCK_CHANGES_SPACING = 2000;
let detailsChangesDue = 0;
let detailsChangesTimer = null;

function paintDockToggles() {
  // The selected chat owns the dock throughout its lifetime. The welcome
  // page has no chat, even when another conversation is running.
  const panel = el("details-panel");
  document.body.classList.toggle("chat-open", hasChat());
  const wasHidden = panel.hidden;
  panel.hidden = !state.detailsOpen || !hasChat() || !el("bottom-panel").hidden || !dockHasRoom();
  el("details-toggle").hidden = !hasChat();
  el("changes-toggle").hidden = !hasChat();
  paintLiveChanges();
  el("changes-toggle").setAttribute("aria-expanded", String(!el("bottom-panel").hidden));
  el("details-toggle").setAttribute("aria-expanded", String(!panel.hidden));
  if (wasHidden && !panel.hidden) return refreshProjectDetails();
}

function openProjectDetails() {
  state.detailsOpen = true;
  if (!el("bottom-panel").hidden) return hidePanel();
  return paintDockToggles();
}

/// The requests the thread on screen reported, listed under the project card,
/// and — while it has reported none — the row that attaches an existing one.
function paintDetailsPulls(project) {
  const session = (state.sessions || []).find((entry) => entry.id === state.session);
  const pulls = session?.pulls || [];
  const list = el("details-pulls");
  list.replaceChildren(
    ...pulls.map((pull) => {
      const row = document.createElement("button");
      row.type = "button";
      row.className = "details-row details-pull-row";
      row.title = pullTitle(pull);
      const icon = document.createElement("span");
      icon.setAttribute("aria-hidden", "true");
      icon.className = `details-pull-icon state-${pull.state || "unknown"}`;
      icon.innerHTML = ICONS.pull;
      const name = document.createElement("span");
      name.className = "details-pull-name";
      name.textContent = pull.label;
      row.append(icon, name);
      const stateLabel = pullStateLabel(pull);
      if (stateLabel) {
        const badge = document.createElement("span");
        badge.className = `details-pull state-${pull.state}`;
        badge.textContent = stateLabel;
        row.append(badge);
      }
      row.onclick = () => openExternal(pull.url);
      return row;
    }),
  );
  el("details-pulls-block").hidden = !pulls.length;
}

async function refreshProjectDetails() {
  const project = runFolder();
  const read = ++detailsRead;
  el("details-project").textContent = state.project ? projectNameOf(state.project) : t("composer.chooseProject", "Choose a project");
  el("details-project").title = state.project || "";
  const checkout = el("details-checkout");
  checkout.innerHTML = ICONS.worktree + `<span>${escapeHtml(state.worktree ? baseName(state.worktree) : t("ui.localCheckout", "Local checkout"))}</span>`;
  checkout.title = project || "";
  const changes = state.repositoryChanges;
  el("details-stats").innerHTML = changes?.project === project ? statsHtml(changes.added, changes.removed, true) : "";
  el("details-changes").disabled = !project;
  el("details-more").disabled = !project;
  const branchRow = el("details-branch");
  const branch = project ? state.git?.branch || state.git?.detached || "" : "";
  // The request the thread on screen reported: the branch row is where the
  // reader looks for the checkout's own work, so it is also what opens the
  // request, in the state it was last seen in.
  const pull = threadPull();
  branchRow.hidden = !state.git?.repo || !project;
  branchRow.innerHTML =
    ICONS.branch +
    `<span>${escapeHtml(branch)}</span>` +
    (pull ? `<span class="details-pull state-${pull.state || "unknown"}">${escapeHtml(pull.label)}</span>` : "");
  branchRow.disabled = !pull;
  branchRow.title = pull
    ? pullTitle(pull)
    : t("ui.currentBranch", "Current branch: {value0}", { value0: branch });
  branchRow.onclick = () => {
    if (pull) openExternal(pull.url);
  };
  paintDetailsPulls(project);
  el("details-sources").textContent = project ? t("ui.loadingSources", "Loading sources…") : t("ui.selectAProjectToSeeItsSources", "Select a project to see its sources.");
  if (!project) return;
  const current = () => read === detailsRead && project === runFolder();
  await Promise.all([
    readProjectChanges(project),
    invoke("mcp_servers", { project }).then((servers) => {
      if (!current()) return;
      state.sourceServers = { project, servers };
      paintProjectSources();
    }).catch((error) => {
      if (current()) el("details-sources").textContent = t("ui.couldNotLoadSources", "Could not load sources: {value0}", { value0: error });
    }),
  ]);
}

function chatSources() {
  const sources = new Map();
  for (const node of el("transcript").children) {
    for (const attachment of node.chatAttachments || []) {
      sources.set(attachment.dataUrl, { kind: "attachment", name: attachment.name || t("composer.attachment", "Attachment"), attachment });
    }
    if (/^(?:webfetch|web_search|websearch|search_query)$/.test(node.chatTool?.name || "")) {
      if (!sources.has("web")) sources.set("web", { kind: "web", name: t("ui.webSearch", "Web search"), node });
    }
  }
  if (state.sourceServers?.project === runFolder()) {
    for (const server of state.sourceServers.servers) sources.set(`mcp:${server.name}`, { kind: "mcp", name: server.name, server });
  }
  return [...sources.values()];
}

function sourceRow(source) {
  const row = document.createElement("button");
  row.className = "details-row source-row";
  row.title = source.name;
  const attachment = source.attachment;
  if (attachment && isPaintableImage(attachment.dataUrl)) {
    const thumbnail = document.createElement("img");
    thumbnail.className = "source-thumbnail";
    thumbnail.src = attachment.dataUrl;
    thumbnail.alt = "";
    thumbnail.draggable = false;
    row.appendChild(thumbnail);
  } else {
    const icon = document.createElement("span");
    icon.className = "source-icon";
    icon.setAttribute("aria-hidden", "true");
    icon.innerHTML = source.kind === "mcp"
      ? ICONS.power
      : source.kind === "web" ? ICONS.globe : ICONS.file;
    row.appendChild(icon);
  }
  const name = document.createElement("span");
  name.className = "source-name";
  name.textContent = source.name;
  row.appendChild(name);
  if (source.server) {
    const status = document.createElement("span");
    status.className = "source-state";
    status.textContent = mcpStatusText(source.server);
    status.title = status.textContent;
    row.title = `${source.name}: ${status.textContent}`;
    row.appendChild(status);
  }
  row.onclick = () => {
    if (attachment && isPaintableImage(attachment.dataUrl)) openImage(attachment.dataUrl, attachment.name);
    else if (source.kind === "mcp") openMcps();
    else if (source.kind === "web") {
      el("sources-modal").hidden = true;
      source.node.scrollIntoView?.({ block: "center", behavior: "smooth" });
    }
    else if (attachment) { setStatus(t("ui.isIncludedInThisChat", "{value0} is included in this chat.", { value0: attachment.name || "Attachment" })); }
    else openSources();
  };
  return row;
}

function paintProjectSources() {
  const sources = chatSources();
  const box = el("details-sources");
  box.replaceChildren(...sources.slice(0, 5).map(sourceRow));
  if (!sources.length) box.textContent = t("ui.noSourcesYet", "No sources yet.");
  if (!el("sources-modal").hidden) el("sources-list").replaceChildren(...sources.map(sourceRow));
}

function openSources() {
  closeOverlays("sources-modal");
  el("sources-modal").hidden = false;
  paintProjectSources();
  el("sources-close").focus();
}

/// Reads the Changes row from the repository: what the work tree holds now,
/// whether a tool call, a formatter or a shell command put it there. The row's
/// own newest read is what paints it, so a live read while the turn works cannot
/// be overwritten by the one that opened the dock.
async function readProjectChanges(project) {
  const read = ++detailsChangesRead;
  const current = () => read === detailsChangesRead && project === runFolder() && hasChat();
  try {
    const changes = await invoke("repository_changes", { project });
    if (!current()) return;
    state.repositoryChanges = { ...changes, project };
    paintLiveChanges();
    paintProjectSources();
    el("details-stats").innerHTML = statsHtml(changes.added, changes.removed);
    el("details-changes").title = t("ui.changedFiles", "{value0} changed files", { value0: changes.files.length });
    if (reviewState?.card.repository) showRepositoryCard({ ...changes, project, repository: true });
  } catch (error) {
    if (!current()) return;
    state.repositoryChanges = null;
    paintLiveChanges();
    el("details-stats").textContent = "";
    el("details-changes").title = String(error);
    if (reviewState?.card.repository) {
      el("review-total").innerHTML = "";
      el("review-files").replaceChildren(reviewNote(String(error)));
    }
  }
}

/// Keeps that row in step with a turn that is writing files: the read in the
/// moment of the call that changed something, and the rest of a burst behind one
/// timer, so a run editing in a loop reads the repository a few times a second at
/// most.
function scheduleProjectChanges() {
  // The dock describes the thread on screen, so a background turn's edit is not
  // its business: its own thread reads the listing when the reader opens it.
  if (paintingThread() !== visibleThread()) return;
  if (!state.busy || !hasChat()) return;
  const now = Date.now();
  if (now >= detailsChangesDue) {
    detailsChangesDue = now + DOCK_CHANGES_SPACING;
    readProjectChanges(runFolder());
    return;
  }
  if (detailsChangesTimer) return;
  detailsChangesTimer = setTimeout(() => {
    detailsChangesTimer = null;
    detailsChangesDue = Date.now() + DOCK_CHANGES_SPACING;
    if (state.busy && hasChat()) readProjectChanges(runFolder());
  }, detailsChangesDue - now);
}

function showRepositoryCard(card) {
  const previous = reviewState?.card.repository && reviewState.card.project === card.project ? reviewState : null;
  const selected = previous?.card.files[previous.index]?.path;
  reviewState = {
    card,
    index: Math.max(0, card.files.findIndex((file) => file.path === selected)),
    sides: new Map(),
    reads: new Map(),
    expanded: previous?.expanded || new Set(),
    openFiles: previous?.openFiles || new Set(card.files.slice(0, 1).map((file) => file.path)),
  };
  el("review-scope").value = "repository";
  paintReview();
}

async function openRepositoryChanges() {
  if (!hasChat()) return;
  openPanel("review");
  el("review-scope").value = "repository";
  el("review-title").textContent = t("ui.changes", "Changes");
  el("review-total").innerHTML = "";
  el("review-files").replaceChildren(reviewNote(t("ui.readingChanges", "Reading changes…")));
  paintDockToggles();
  const project = runFolder();
  reviewState = { card: { repository: true, project, files: [] } };
  await readRepositoryIndex(project);
  await readProjectChanges(project);
}

function changeReviewScope() {
  if (el("review-scope").value === "repository") return openRepositoryChanges();
  const card = state.changes.filter((card) => !card.undone).at(-1);
  if (card) return openReview(card);
  reviewState = null;
  el("review-total").innerHTML = "";
  el("review-files").replaceChildren(reviewNote(t("ui.noChangesFromATurnInThisChatYet", "No changes from a turn in this chat yet.")));
}

/// The still-running card a tool event belongs to. Results are emitted in call
/// order, so the oldest running card with a matching name is the right one.
function activeTool(name) {
  return (
    state.tools.find((tool) => !tool.done && tool.name === name) ||
    state.tools.find((tool) => !tool.done)
  );
}

function reportRunError(message) {
  state.runError = t("common.errorDetail", "Error: {value0}", { value0: message });
  state.currentActivity = null;
  const notice = document.createElement("div");
  notice.className = "run-error";
  notice.setAttribute("role", "alert");
  notice.textContent = state.runError;
  runTranscript().appendChild(notice);
  if (paintingThread() === visibleThread()) setStatus(state.runError, "error");
  scrollDown(runTranscript());
}

/// Says something about the turn being painted only where that turn is the thread
/// on screen: a background turn's phase is its own thread's to show when the
/// reader opens it, not this window's status line.
function runStatus(text, level = "info") {
  if (paintingThread() === visibleThread()) setStatus(text, level);
}

/// Applies one event of the turn being painted. The caller has already pointed
/// `paintingThread` at the conversation it belongs to, so every field here is
/// that thread's own.
function handleEvent(event) {
  updateTranscriptFollow(runTranscript());
  if (event.runId != null) state.runId = event.runId;
  switch (event.type) {
    case "session_title": {
      const title = String(event.title || "").trim();
      if (!title) break;
      const record = paintingThread();
      record.runTitle = title;
      for (const tab of chatTabs) if (tab.session === record.session) tab.title = title;
      paintChatTabs();
      if (state.heldThread?.id === record.session) state.heldThread.title = title;
      if (record === visibleThread()) {
        refreshThreadTitle();
        loadSessions();
      }
      updateRunBanner();
      break;
    }
    case "message_update": {
      const inner = event.assistantMessageEvent || {};
      if (inner.type === "text_delta") appendText(inner.delta || "");
      else if (inner.type === "thinking_delta") appendThinking(inner.delta || "");
      break;
    }
    case "message_phase":
      setAssistantPhase(event.phase);
      break;
    case "tool_call":
      startTool(event.toolName || "tool", event.arguments);
      break;
    case "tool_execution_update": {
      const tool = activeTool(event.toolName);
      if (tool) {
        tool.live += event.partialResult || "";
        tool.pre.textContent = tool.live;
        scrollDown(runTranscript());
      }
      break;
    }
    case "tool_execution_end": {
      const tool = activeTool(event.toolName);
      if (tool) {
        finishTool(tool, event.result || tool.live, {
          isError: event.isError,
          diff: event.diff,
          attachments: event.attachments || [],
          elapsed: Math.max(0, Math.round(performance.now() - tool.started)),
        });
        scrollDown(runTranscript());
      }
      break;
    }
    case "thinking_done":
      // The step's output is committed. Clearing the in-progress pointers means
      // a later step's text opens a new bubble, and a retry only discards its
      // own attempt instead of a previous step's reply.
      state.currentAssistant = null;
      state.currentThinking = null;
      break;
    case "auto_retry_start":
      // A retry re-streams the response from the start, so the partial output
      // of the failed attempt is dropped instead of being extended.
      discardAttempt();
      runStatus(t("ui.retrying", "Retrying ({value0}/{value1})…", { value0: event.attempt, value1: event.maxAttempts }));
      break;
    case "context_window": {
      if (!(event.tokens > 0) || !event.provider || !event.model) break;
      const record = paintingThread();
      record.contextCorrection = {
        tokens: event.tokens,
        provider: event.provider,
        model: event.model,
        project: record.project || runFolder(),
        session: record.session,
      };
      record.contextWindow = correctionWindow() ?? state.catalogWindow;
      if (record === visibleThread()) {
        state.contextWindow = record.contextWindow;
        paintComposerFacts();
      }
      break;
    }
    case "usage": {
      addUsage(event);
      break;
    }
    case "compaction":
      runStatus(t("ui.compactedEarlierMessages", "Compacted {value0} earlier messages", { value0: event.summarized }));
      break;
    case "error":
      reportRunError(event.message);
      break;
    default:
      break;
  }
}

// ---------- approvals ----------

function showApproval(request) {
  notifyWaiting("approval", request.tool);
  state.pendingApproval = request.id;
  el("approval-tool").textContent = request.tool;
  el("approval-detail").textContent = request.detail || "";
  closeOverlays("approval");
  el("approval").hidden = false;
}

async function answerApproval(decision) {
  if (state.pendingApproval == null) return;
  const id = state.pendingApproval;
  state.pendingApproval = null;
  el("approval").hidden = true;
  await invoke("resolve_approval", { id, decision });
}

// ---------- questions ----------

// A skill that needs a decision asks through the `ask` tool, and the answer is
// painted here one question at a time, the way the dialog it is modelled on
// reads: the question's own wording, its options as rows (radio buttons, or
// checkboxes when several may be picked) with a row for an answer in the user's
// own words, and `Next` walking to the question after it. A call that asks
// several things shows `N of M questions` with a dash per question beside it,
// so the reader always knows which one is being asked. Every question's fields
// stay built, so stepping back keeps what was already answered.
function showQuestion(request) {
  const questions = Array.isArray(request.questions) ? request.questions : [];
  // A request with nothing in it is not a question: the dialog stays shut and
  // nothing is announced.
  if (!questions.length) return;
  notifyWaiting(
    "question",
    request.title || request.prompt || questions[0]?.question || questions[0]?.header || "",
  );
  state.pendingQuestion = { id: request.id, questions, index: 0 };
  const body = el("question-body");
  body.replaceChildren();
  questions.forEach((question, index) => body.appendChild(questionBlock(question, index)));
  closeOverlays("question");
  el("question").hidden = false;
  showQuestionStep(0);
}

// The fields of one question: its options, then the row that takes an answer in
// the user's own words. A single choice offers that row the way the dialog does
// — as one of the choices, with its field under it — so the typed text is the
// answer instead of a second one beside the picked label.
function questionBlock(question, index) {
  const block = document.createElement("div");
  block.className = "question";
  block.hidden = true;
  const options = Array.isArray(question.options) ? question.options : [];
  const name = `question-${index}`;
  options.forEach((option, position) => {
    const row = document.createElement("label");
    row.className = "question-option";
    const input = document.createElement("input");
    input.type = question.multiSelect ? "checkbox" : "radio";
    input.name = name;
    input.value = option.label;
    input.dataset.question = String(index);
    // Every option carries the same class: a single-select question's answer
    // is the checked radio, a multi-select one's the checked boxes, and the
    // widget itself already says which it is.
    input.className = "question-choice";
    const label = document.createElement("span");
    label.className = "question-label";
    label.textContent = option.label;
    row.appendChild(input);
    row.appendChild(label);
    if (option.description) {
      const description = document.createElement("span");
      description.className = "question-detail";
      description.textContent = option.description;
      row.appendChild(description);
    }
    block.appendChild(row);
    // A single-select list needs one of its options chosen, never a blank
    // group the user can submit by accident.
    if (!question.multiSelect && position === 0) input.checked = true;
  });
  const free = document.createElement("input");
  free.className = "question-free";
  free.type = "text";
  free.dataset.question = String(index);
  if (options.length && !question.multiSelect) {
    const row = document.createElement("label");
    row.className = "question-option question-own";
    const own = document.createElement("input");
    own.type = "radio";
    own.name = name;
    // The row's own value is empty, so the collector never sends its label as
    // an answer: choosing it is what lets the typed text answer instead.
    own.value = "";
    own.dataset.question = String(index);
    own.className = "question-choice question-own-choice";
    const label = document.createElement("span");
    label.className = "question-label";
    label.textContent = t("ui.typeYourOwnAnswer", "Type your own answer");
    free.placeholder = t("ui.typeYourAnswer", "Type your answer…");
    own.addEventListener("change", () => {
      if (own.checked) free.focus();
    });
    free.addEventListener("input", () => {
      // Typing is what picks the row, so the caret never sits in a field whose
      // text the answer ignores.
      if (free.value.trim()) own.checked = true;
    });
    row.append(own, label);
    block.append(row, free);
  } else {
    free.placeholder = options.length ? t("ui.orTypeYourOwnAnswer", "Or type your own answer…") : t("ui.typeYourAnswer", "Type your answer…");
    block.appendChild(free);
  }
  return block;
}

// Paints the step on screen: the question being asked, how many there are, and
// the navigation this position allows. The block itself is already built, so
// stepping only shows and hides.
function showQuestionStep(index) {
  const pending = state.pendingQuestion;
  if (!pending) return;
  const total = pending.questions.length;
  pending.index = Math.min(Math.max(index, 0), total - 1);
  const question = pending.questions[pending.index] || {};
  const options = Array.isArray(question.options) ? question.options : [];
  const blocks = Array.from(el("question-body").children);
  blocks.forEach((block, position) => {
    block.hidden = position !== pending.index;
  });
  // One question needs no counter: there is nothing to step through.
  el("question-step-row").hidden = total < 2;
  el("question-step").textContent = t("ui.ofQuestions", "{value0} of {value1} questions", { value0: pending.index + 1, value1: total });
  const progress = el("question-progress");
  progress.replaceChildren();
  if (total > 1) {
    for (let position = 0; position < total; position += 1) {
      const dash = document.createElement("span");
      dash.className = `question-dash ${position < pending.index ? "done" : position === pending.index ? "current" : "todo"}`;
      progress.appendChild(dash);
    }
  }
  const header = el("question-header");
  header.textContent = question.header || "";
  header.hidden = !question.header;
  el("question-title").textContent = question.question || "";
  const hint = el("question-hint");
  hint.textContent = options.length
    ? question.multiSelect
      ? t("ui.selectAllThatApply", "Select all that apply")
      : t("ui.selectOneAnswer", "Select one answer")
    : "";
  hint.hidden = !hint.textContent;
  el("question-back").hidden = pending.index === 0;
  el("question-submit").textContent = pending.index + 1 < total ? t("common.next", "Next") : t("common.submit", "Submit");
  const block = blocks[pending.index];
  // A question answered in the user's own words puts the caret where the answer
  // goes; one that offers options is left alone, so a keystroke is not taken by
  // a field nobody was asked to fill in.
  if (block && !options.length) {
    const focus = block.querySelector(".question-free");
    if (focus) focus.focus();
  }
}

// `Next` moves to the question after this one and only the last step submits,
// since the agent reads one set of answers for the whole call.
function questionNext() {
  const pending = state.pendingQuestion;
  if (!pending) return;
  if (pending.index + 1 < pending.questions.length) {
    showQuestionStep(pending.index + 1);
    return;
  }
  answerQuestion(false);
}

// Every answer echoes the question it belongs to, so the model reads them in
// the terms it asked them in.
function collectAnswers() {
  const pending = state.pendingQuestion;
  if (!pending) return [];
  const body = el("question-body");
  return pending.questions.map((question, index) => {
    const values = [];
    body
      .querySelectorAll(`.question-choice[data-question="${index}"]:checked`)
      .forEach((input) => {
        const value = String(input.value || "").trim();
        if (value) values.push(value);
      });
    const text = body.querySelector(`.question-free[data-question="${index}"]`);
    const typed = text ? text.value.trim() : "";
    // Only a single choice has a row asking for the user's own words, and its
    // text answers the question when that row is the chosen one.
    const own = body.querySelector(`.question-own-choice[data-question="${index}"]`);
    if (typed && (!own || own.checked)) values.push(typed);
    return { question: question.question, values };
  });
}

// Hides the question dialog without answering it, for a request that is gone:
// the run that asked it ended, or it timed out with nobody answering.
function closeQuestion() {
  if (!state.pendingQuestion) return;
  state.pendingQuestion = null;
  el("question").hidden = true;
}

// `dismiss` (Dismiss) answers with nothing, which the agent reports to the model
// as a question the user did not answer. A submission with nothing filled in is
// that same dismissal rather than a set of blank answers, so the agent reads it
// the way the dialog's own Dismiss does.
async function answerQuestion(dismiss = false) {
  const pending = state.pendingQuestion;
  if (!pending) return;
  const answers = dismiss
    ? []
    : collectAnswers().filter((answer) => answer.values.length > 0);
  closeQuestion();
  await invoke("resolve_question", { id: pending.id, answers });
}

// ---------- providers ----------

// Every panel that can take the window's keyboard: most cover the app, while
// the MCP and session listings open above the composer instead.
const OVERLAYS = [
  "approval",
  "question",
  "connect-modal",
  "sources-modal",
  "create-project-modal",
  "projects-modal",
  "worktrees-modal",
  "model-menu",
  "themes-modal",
  "language-modal",
  "permissions-modal",
  "trust-modal",
  "agents-modal",
  "confirm-modal",
  "rename-modal",
  "image-modal",
  "sessions-modal",
  "update-modal",
  "help-modal",
  "command-menu",
  "search-modal",
  "archived-modal",
  "file-modal",
];

function closeOverlays(except) {
  el("composer-tools").open = false;
  for (const id of OVERLAYS) {
    if (id !== except) el(id).hidden = true;
    else el(id).classList.remove("rail-panel");
  }
  // The review's state is its card and the file it is showing, so closing the
  // panel drops it along with the markup rather than leaving it half-open.

  // The palette lives above the composer rather than in an overlay, but any
  // dialog that opens takes the keyboard with it.
  if (except !== "command-palette") closePalette();
  syncSettingsDropdowns();
  syncRailPanels();
}

const RAIL_PANELS = {
  "rail-more": ["command-menu", openCommandMenu],
  "rail-worktrees": ["worktrees-modal", openWorktrees],
};

function syncRailPanels() {
  for (const [control, [id]] of Object.entries(RAIL_PANELS)) {
    const panel = el(id);
    const open = !panel.hidden && panel.classList.contains("rail-panel");
    el(control).setAttribute("aria-expanded", String(open));
    if (panel.classList.contains("overlay")) panel.setAttribute("aria-modal", String(!open));
    if (panel.hidden) panel.classList.remove("rail-panel");
  }
}

function toggleRailPanel(control) {
  const [id, open] = RAIL_PANELS[control];
  const panel = el(id);
  if (!panel.hidden && panel.classList.contains("rail-panel")) {
    closeOverlays();
    el(control).focus();
    return;
  }
  closeSettings();
  closeCustomize();
  open();
  // A missing project opens the project picker instead of the worktree list.
  if (panel.hidden) return;
  panel.classList.add("rail-panel");
  syncRailPanels();
  if (!panel.contains(document.activeElement)) {
    panel.querySelector("input, button, [tabindex='0']")?.focus();
  }
}

const SETTINGS_DROPDOWNS = { "model-menu": "model" };

function syncSettingsDropdowns() {
  for (const [id, control] of Object.entries(SETTINGS_DROPDOWNS)) {
    el(control).setAttribute("aria-expanded", String(!el(id).hidden));
  }
}

function positionSettingsDropdowns() {
  for (const [id, control] of Object.entries(SETTINGS_DROPDOWNS)) {
    const box = el(id);
    if (box.hidden) continue;
    const anchor = el(control).getBoundingClientRect();
    box.style.right = `${Math.max(12, window.innerWidth - anchor.right)}px`;
    box.style.bottom = `${window.innerHeight - anchor.top + 8}px`;
    box.style.maxHeight = `${Math.max(80, Math.min(380, anchor.top - 20))}px`;
  }
}

/// Closing hands the keyboard back to whatever opened the menu. Every path goes
/// through here — a model, Escape, Tab, or the chip — so a reader who chose is
/// not left at the top of the page behind it.
function closeModelMenu(restoreFocus = true) {
  if (!modelMenuOpen()) return;
  modelCatalogEpoch += 1;
  effortDragging = false;
  el("model-menu").hidden = true;
  syncSettingsDropdowns();
  const target = modelMenuReturnFocus || el("model");
  modelMenuReturnFocus = null;
  if (restoreFocus) target?.focus?.();
}

// macOS's WKWebView does not implement `window.confirm`/`window.prompt`, so
// destructive and rename actions go through these in-app dialogs instead.
let confirmResolution = null;
// What the open confirm dialog offers beside Confirm, so its second button is
// wired once rather than per dialog.
let confirmAlt = null;
let renameResolution = null;

function confirmDialog(title, message, confirmLabel = t("dialog.confirm.ok", "Confirm"), alt = null) {
  el("confirm-title").textContent = title;
  el("confirm-message").textContent = message;
  el("confirm-ok").textContent = confirmLabel;
  confirmAlt = alt;
  const altButton = el("confirm-alt");
  altButton.hidden = !alt;
  altButton.textContent = alt ? alt.label : "";
  closeOverlays("confirm-modal");
  el("confirm-modal").hidden = false;
  el("confirm-ok").focus();
  return new Promise((resolve) => {
    confirmResolution = resolve;
  });
}

function resolveConfirm(value) {
  const resolve = confirmResolution;
  confirmResolution = null;
  confirmAlt = null;
  el("confirm-modal").hidden = true;
  if (resolve) resolve(value);
}

function promptDialog(value, title = t("dialog.rename.title", "Rename thread")) {
  el("rename-title").textContent = title;
  el("rename-input").value = value || "";
  closeOverlays("rename-modal");
  el("rename-modal").hidden = false;
  el("rename-input").focus();
  el("rename-input").select();
  return new Promise((resolve) => {
    renameResolution = resolve;
  });
}

function resolveRename(value) {
  const resolve = renameResolution;
  renameResolution = null;
  el("rename-modal").hidden = true;
  if (resolve) resolve(value);
}

async function openConnect() {
  state.providers = await invoke("list_providers");
  // The provider already in use is the one to open on, then a stored one, then
  // the first row — never a search that was left over from last time.
  const preferred =
    state.providers.find((provider) => provider.active) ??
    state.providers.find((provider) => provider.stored) ??
    state.providers[0];
  state.providerName = preferred ? preferred.name : "";
  el("provider-filter").value = "";
  el("login-key").value = "";
  renderProviders();
  closeOverlays("connect-modal");
  el("connect-modal").hidden = false;
}

/// Picks the provider the dialog is on. A key typed for the provider before it
/// goes: the credential belongs to the provider it was pasted for.
function selectProvider(name) {
  if (name === state.providerName) return;
  state.providerName = name;
  el("login-key").value = "";
}

/// The providers whose name, label or description mention the query, in the
/// table's own order.
function filteredProviders() {
  const query = el("provider-filter").value.trim().toLowerCase();
  if (!query) return state.providers;
  return state.providers.filter((provider) =>
    `${provider.name} ${provider.label} ${provider.description}`.toLowerCase().includes(query),
  );
}

function renderProviders() {
  const box = el("provider-list");
  const rows = filteredProviders();
  // A search that hides the selected provider moves the selection to the first
  // row the reader can see, so Save connects what is on screen.
  if (!rows.some((provider) => provider.name === state.providerName)) {
    selectProvider(rows.length ? rows[0].name : "");
  }
  box.innerHTML = "";
  if (!rows.length) {
    box.innerHTML =
      `<div class="empty" style="margin:8px 0">${escapeHtml(t("desktop.empty.providers", "No provider matches that search."))}</div>`;
    renderLoginFields();
    return;
  }
  for (const provider of rows) {
    const row = document.createElement("button");
    row.type = "button";
    row.className = "provider" + (provider.name === state.providerName ? " active" : "");
    const marks = [];
    if (provider.active) marks.push(t("ui.inUse", "in use"));
    if (provider.stored) marks.push(t("provider.storedMark", "stored"));
    row.innerHTML =
      `<span class="p-name">${escapeHtml(provider.label)}</span>` +
      `<span class="p-desc">${escapeHtml(provider.name)} · ${escapeHtml(provider.description)}</span>` +
      (marks.length ? `<span class="badge">${escapeHtml(marks.join(" · "))}</span>` : "");
    row.onclick = () => {
      selectProvider(provider.name);
      renderProviders();
    };
    box.appendChild(row);
  }
  renderLoginFields();
}

/// What the selected provider needs: a key field whose placeholder says what
/// leaving it empty means, or the sentence that replaces it for a provider that
/// asks for no key at all — a server on this machine, or one that authorizes
/// with a credential the machine already holds, which is read where it lives and
/// is not the key this box would collect.
function renderLoginFields() {
  const provider = state.providers.find((one) => one.name === state.providerName);
  const field = el("login-key-field");
  const note = el("login-key-note");
  const keyless = Boolean(provider && (provider.local || provider.credential === "external"));
  field.hidden = keyless;
  note.hidden = !keyless;
  if (!provider) {
    el("login-key").placeholder = t("dialog.connect.keyPlaceholder", "Paste your API key");
    return;
  }
  el("login-key").placeholder = provider.stored
    ? t("ui.leaveEmptyToReuseTheStoredKey", "Leave empty to reuse the stored key")
    : t("dialog.connect.keyPlaceholder", "Paste your API key");
  if (provider.local) {
    note.textContent = t("ui.isAServerOnThisMachineNoKeyNeeded", "{value0} is a server on this machine — no key needed.", { value0: provider.label });
  } else if (keyless) {
    note.textContent = t("ui.authorizesWithACredentialThisMachineAlreadyHasNoKeyNeeded", "{value0} authorizes with a credential this machine already has — no key needed.", { value0: provider.label });
  }
}

async function saveConnect() {
  const provider = state.providers.find((one) => one.name === state.providerName);
  if (!provider) {
    showToast(t("ui.chooseAProviderFirst", "Choose a provider first."));
    return;
  }
  // A login reads the provider's own catalog before it answers, so the model the
  // chip names is one that account really serves — which is a round trip, and
  // this dialog waits on it.
  const dismissConnecting = showToast(t("ui.connecting", "Connecting {value0}…", { value0: provider.label }));
  try {
    const outcome = await invoke("login", {
      provider: provider.name,
      key: el("login-key").value || null,
      model: el("login-model").value || null,
      baseUrl: el("login-url").value || null,
    });
    el("connect-modal").hidden = true;
    el("login-key").value = "";
    if (state.project) await loadInfo();
    else if (outcome?.model) {
      // No folder to read a window off, so the chip names the model alone.
      labelControl("model", t("model.label", "model: {value0}", { value0: outcome.model }), t("ui.switchModel", "Switch model"));
      state.currentModel = outcome.model;
      el("model-value").textContent = modelDisplayName(outcome.model);
    }
    showToast(t("provider.connected", "Connected {value0}", { value0: provider.label }));
  } catch (error) {
    showToast(t("ui.loginFailed", "Login failed: {value0}", { value0: error }), "error");
  } finally {
    dismissConnecting();
  }
}

// ---------- model and effort ----------
//
// One menu, the one the Tauri window draws: the composer's model chip opens it on
// a summary page — the effort in use above the model it runs on, a slider across
// the model's levels and a reset to Auto — and the model's name opens the second
// page, the catalog. A level is applied as the slider moves and the menu stays
// up; a model is applied and puts the menu away. Escape steps back from the
// catalog to the summary, then closes; Tab or a press outside closes.

let modelCatalogEpoch = 0;
// Where the keyboard was before the menu took it.
let modelMenuReturnFocus = null;
// A press on one of the slider's stops is held until the button comes up, so a
// drag across the track walks the levels the way the Tauri slider does.
let effortDragging = false;
// The newest read of the model's levels. A response that arrives after the
// project or model moved on is dropped rather than painting its levels over the
// new selection's.
let reasoningEpoch = 0;

const MODEL_WORDS = {
  gpt: "GPT",
  deepseek: "DeepSeek",
  claude: "Claude",
  gemini: "Gemini",
  qwen: "Qwen",
  codex: "Codex",
};

/// A model id the way the chip and the catalog name it, by the rules the Tauri
/// window's `model_display_name` follows: `gpt-5` reads as `GPT 5`.
function modelDisplayName(model) {
  return String(model || "")
    .split(/[-_]/)
    .filter(Boolean)
    .map((part) => MODEL_WORDS[part.toLowerCase()] || part.charAt(0).toUpperCase() + part.slice(1))
    .join(" ");
}

/// A catalog entry is an id, or an object carrying one and the provider's own
/// name for it.
function modelEntry(model) {
  if (model && typeof model === "object") {
    const id = String(model.id || model.name || "");
    const named = model.name && model.name !== id;
    return { id, name: modelDisplayName(model.name || id), detail: named ? id : "" };
  }
  return { id: String(model), name: modelDisplayName(model), detail: "" };
}

function modelMenuOpen() {
  return !el("model-menu").hidden;
}

function modelPickerShown() {
  return modelMenuOpen() && !el("model-picker").hidden;
}

/// The levels the slider walks: Auto, then the model's own advertised levels.
function reasoningChoices() {
  return state.supportsReasoning !== false && Array.isArray(state.reasoningLevels)
    ? state.reasoningLevels : ["auto"];
}

/// Whether the effort half of the menu is drawn: a model with a level beyond
/// Auto, or a saved level the model no longer offers, which the slider's Auto
/// stop and the reset recover from.
function hasReasoningLevels() {
  return reasoningChoices().some((level) => level !== "auto") || state.reasoning !== "auto";
}

function reasoningHint(level) {
  return t(`reasoning.hint.${level}`, REASONING_HINTS[level] || "");
}

async function openModelMenu(page = "summary", refresh = false) {
  // The catalog and the levels are read from this project's config, and the
  // request would otherwise resolve against the directory the app was launched
  // in — the same reason the message box asks for a folder before it sends.
  if (!state.project) {
    setStatus(t("composer.selectProject", "Select a project first."));
    askForProject();
    return;
  }
  if (!modelMenuOpen()) modelMenuReturnFocus = document.activeElement || null;
  if (!state.reasoningLevels) await ensureReasoningLevels();
  closeOverlays("model-menu");
  el("model-menu").hidden = false;
  syncSettingsDropdowns();
  if (page === "models") await showModelPicker(refresh);
  else showModelSummary();
}

function openModels(refresh = false) {
  return openModelMenu("models", refresh);
}

function openReasoning() {
  return openModelMenu("summary");
}

/// The summary page, with the keyboard on the slider — the control a reader
/// who opened the menu is there for — or on the model when there is no effort.
function showModelSummary() {
  modelCatalogEpoch += 1;
  el("model-picker").hidden = true;
  el("model-summary").hidden = false;
  renderModelSummary();
  positionSettingsDropdowns();
  (hasReasoningLevels() ? el("effort-slider") : el("model-page")).focus();
}

async function showModelPicker(refresh = false) {
  el("model-summary").hidden = true;
  el("model-picker").hidden = false;
  el("model-list").innerHTML = `<div class="empty" style="margin:14px">${escapeHtml(t("desktop.empty.loadingModels", "Loading\u2026"))}</div>`;
  el("model-filter").value = "";
  positionSettingsDropdowns();
  el("model-filter").focus();
  const project = state.project;
  const epoch = ++modelCatalogEpoch;
  const stale = () => epoch !== modelCatalogEpoch || state.project !== project || !modelPickerShown();
  try {
    const models = await invoke("list_models", { project, refresh: refresh === true });
    if (stale()) return;
    state.models = models;
    renderModels();
    if (refresh === true) await ensureReasoningLevels(true);
  } catch (error) {
    if (stale()) return;
    renderCatalogError(String(error));
  }
}

/// A catalog that could not be read says why and offers the way to fix it.
function renderCatalogError(message) {
  const box = el("model-list");
  box.innerHTML = "";
  const text = document.createElement("div");
  text.className = "empty model-error";
  text.textContent = message;
  box.appendChild(text);
  const connect = document.createElement("button");
  connect.className = "ghost model-connect";
  connect.textContent = t("composer.connect", "Connect a provider");
  connect.onclick = () => {
    closeModelMenu(false);
    void openConnect();
  };
  box.appendChild(connect);
}

function renderModelMenu() {
  if (modelPickerShown()) renderModels();
  else renderModelSummary();
}

function renderModelSummary() {
  const effort = hasReasoningLevels();
  for (const id of ["effort-icon", "effort-name", "effort-reset", "effort-slider"]) el(id).hidden = !effort;
  el("effort-icon").innerHTML = ICONS.bolt;
  el("effort-name").textContent = reasoningLabel(state.reasoning);
  el("model-page-name").textContent = state.currentModel ? modelDisplayName(state.currentModel) : t("composer.model", "Model");
  el("model-page").setAttribute("aria-label", `${t("ui.switchModel", "Switch model")}: ${state.currentModel || t("composer.model", "Model")}`);
  el("model-page-caret").innerHTML = ICONS.chevronRight;
  el("effort-reset").innerHTML = ICONS.reset;
  el("effort-reset").disabled = state.reasoning === "auto";
  renderEffortSlider(reasoningChoices());
}

function effortOffset(index, count) {
  return String(count > 1 ? index / (count - 1) : 0);
}

/// The slider: a dot for each level and the knob on the one in use, drawn first
/// so the stops above it take every press. A saved level the model no longer
/// offers has no stop, so the knob is left off until one is chosen.
function renderEffortSlider(levels) {
  const slider = el("effort-slider");
  const at = levels.indexOf(state.reasoning);
  slider.innerHTML = "";
  const track = document.createElement("span");
  track.className = "effort-track";
  slider.appendChild(track);
  if (at >= 0) {
    const knob = document.createElement("span");
    knob.className = "effort-knob";
    knob.style.setProperty("--at", effortOffset(at, levels.length));
    slider.appendChild(knob);
  }
  levels.forEach((level, index) => {
    const stop = document.createElement("span");
    stop.className = "effort-stop" + (index === at ? " active" : "");
    stop.dataset.level = level;
    stop.title = `${reasoningLabel(level)} \u2014 ${reasoningHint(level)}`;
    stop.style.setProperty("--at", effortOffset(index, levels.length));
    stop.onpointerdown = (event) => {
      event.preventDefault?.();
      effortDragging = true;
      chooseEffort(level);
    };
    stop.onpointerenter = () => {
      if (effortDragging) chooseEffort(level);
    };
    stop.onclick = () => chooseEffort(level);
    slider.appendChild(stop);
  });
  slider.setAttribute("aria-valuemin", "0");
  slider.setAttribute("aria-valuemax", String(Math.max(0, levels.length - 1)));
  slider.setAttribute("aria-valuenow", String(Math.max(0, at)));
  slider.setAttribute("aria-valuetext", `${reasoningLabel(state.reasoning)}, ${reasoningHint(state.reasoning)}`);
}

/// One press can reach a stop twice (the press, then its click), and a drag
/// crosses the stop it started on: a level already in use is not saved again.
function chooseEffort(level) {
  if (level !== state.reasoning) setReasoning(level);
}

/// The slider's keys: Left/Down for less, Right/Up for more, Home and End for
/// either end. They stop at the ends rather than wrapping, as a slider does.
function stepEffort(key) {
  const levels = reasoningChoices();
  const at = Math.max(0, levels.indexOf(state.reasoning));
  const step = { ArrowLeft: -1, ArrowDown: -1, ArrowRight: 1, ArrowUp: 1 }[key];
  let next = null;
  if (step) next = Math.min(levels.length - 1, Math.max(0, at + step));
  else if (key === "Home") next = 0;
  else if (key === "End") next = levels.length - 1;
  if (next === null) return false;
  chooseEffort(levels[next]);
  return true;
}

function renderModels() {
  const box = el("model-list");
  box.innerHTML = "";
  if (!state.models) return;
  if (state.models.error) {
    renderCatalogError(String(state.models.error));
    return;
  }
  const filter = el("model-filter").value.trim().toLowerCase();
  for (const provider of state.models.providers || []) {
    const models = (provider.models || [])
      .map(modelEntry)
      .filter((model) => !filter || model.id.toLowerCase().includes(filter) || model.name.toLowerCase().includes(filter));
    if (!models.length && !provider.error) continue;
    const header = document.createElement("div");
    header.className = "model-provider" + (provider.active ? " active" : "");
    header.textContent = provider.provider + (provider.active ? t("ui.active", " (active)") : "");
    box.appendChild(header);
    if (provider.error) {
      const error = document.createElement("div");
      error.className = "empty model-error";
      error.textContent = `${provider.provider}: ${provider.error}`;
      box.appendChild(error);
    }
    for (const model of models) {
      const row = document.createElement("button");
      const current = Boolean(provider.active && model.id === provider.current);
      row.className = "model" + (current ? " active" : "");
      row.dataset.model = model.id;
      row.title = model.id;
      row.setAttribute("aria-pressed", String(current));
      const name = document.createElement("span");
      name.className = "model-name";
      name.textContent = model.name;
      const id = document.createElement("span");
      id.className = "model-id";
      id.textContent = model.detail;
      const check = document.createElement("span");
      check.className = "model-check";
      check.setAttribute("aria-hidden", "true");
      setCheck(check, current);
      row.append(name, ...(model.detail ? [id] : []), check);
      row.onclick = async () => {
        await reasoningSave;
        await invoke("set_model", { provider: provider.provider, model: model.id });
        closeModelMenu();
        await loadInfo();
      };
      box.appendChild(row);
    }
  }
  if (!box.children.length) {
    box.innerHTML = `<div class="empty" style="margin:14px">${escapeHtml(t("desktop.empty.models", "No models found."))}</div>`;
  }
}

function ensureReasoningLevels(refresh = false) {
  // A cold model cache has no advertised levels yet; ask the host, which warms
  // it from the provider's listing, and repaint the open menu with the model's
  // own levels. The effort stays hidden until metadata arrives, and a late
  // answer for a project or model the reader has left is dropped.
  if (!state.project || (!refresh && state.reasoningLevels)) return Promise.resolve();
  const epoch = ++reasoningEpoch;
  const project = state.project;
  return invoke("reasoning_levels", { project })
    .then((info) => {
      if (epoch !== reasoningEpoch || state.project !== project) return;
      if (info) {
        state.supportsReasoning = info.supportsReasoning !== false &&
          Array.isArray(info.reasoningLevels) && info.reasoningLevels.length > 0;
        state.reasoningLevels = state.supportsReasoning ? ["auto", ...info.reasoningLevels] : ["auto"];
        if (state.supportsReasoning && !state.reasoningPicked && state.reasoningLevels.includes(info.reasoning)) {
          state.reasoning = info.reasoning;
        }
        if (state.supportsReasoning && !state.reasoningLevels.includes(state.reasoning)) {
          state.reasoning = "auto";
          state.reasoningPicked = true;
        }
        updateChips();
      }
    })
    .catch(() => {
      updateChips();
    });
}

// ---------- themes ----------

function themeProject() {
  return state.project || "";
}

async function openThemes() {
  closeSettings();
  closeOverlays("themes-modal");
  el("themes-modal").hidden = false;
  const box = el("theme-list");
  box.innerHTML = `<div class="theme-empty">${escapeHtml(t("desktop.empty.loadingThemes", "Loading themes\u2026"))}</div>`;
  try {
    const themes = await invoke("list_themes", { project: themeProject() });
    const names = themes.names || [];
    const entries = await Promise.all(
      names.map(async (name) => {
        try {
          const { colors } = await invoke("theme_colors", { project: themeProject(), name });
          return { name, colors };
        } catch {
          return { name, colors: null };
        }
      }),
    );
    renderThemes(entries, themes.current);
  } catch (error) {
    box.innerHTML = "";
    showToast(t("ui.themesFailed", "Themes failed: {value0}", { value0: error }), "error");
  }
}

/// The language picker: the window's own, the terminal's and the editor
/// panel's language in one place, since the choice is written to the shared
/// `settings.json` rather than kept beside this window.
function openLanguage() {
  closeSettings();
  const modal = el("language-modal");
  const open = modal.hidden;
  closeOverlays("language-modal");
  modal.hidden = !open;
  if (open) renderLanguageRows(null);
}

function closeLanguage() {
  el("language-modal").hidden = true;
}

const THEME_SWATCH_SLOTS = ["background", "panel", "accent", "text", "success", "tool"];

function renderThemes(entries, current, box = el("theme-list")) {
  box.innerHTML = "";
  if (!entries.length) {
    box.innerHTML = `<div class="theme-empty">${escapeHtml(t("desktop.empty.themes", "No themes available."))}</div>`;
    return;
  }
  for (const { name, colors } of entries) {
    const row = document.createElement("button");
    row.type = "button";
    row.className = "theme-option" + (name === current ? " active" : "");
    row.dataset.theme = name;
    row.setAttribute("aria-pressed", String(name === current));

    const swatches = document.createElement("span");
    swatches.className = "theme-swatches";
    for (const slot of THEME_SWATCH_SLOTS) {
      const swatch = document.createElement("span");
      swatch.className = "theme-swatch";
      if (colors && colors[slot]) swatch.style.background = colors[slot];
      swatches.appendChild(swatch);
    }
    row.appendChild(swatches);

    const label = document.createElement("span");
    label.className = "theme-option-name";
    label.textContent = name;
    row.appendChild(label);

    const check = document.createElement("span");
    check.className = "theme-option-check";
    setCheck(check, name === current);
    row.appendChild(check);

    row.onclick = () => selectTheme(name, row, box);
    box.appendChild(row);
  }
}

async function selectTheme(name, row, box = el("theme-list")) {
  for (const node of box.children) node.disabled = true;
  try {
    const result = await invoke("set_theme", { project: themeProject(), name });
    applyTheme(result.colors);
    for (const node of box.querySelectorAll(".theme-option")) {
      const isActive = node === row;
      node.classList.toggle("active", isActive);
      node.setAttribute("aria-pressed", String(isActive));
      const check = node.querySelector(".theme-option-check");
      if (check) setCheck(check, isActive);
    }
  } catch (error) {
    showToast(t("ui.themeFailed", "Theme failed: {value0}", { value0: error }), "error");
  } finally {
    for (const node of box.children) node.disabled = false;
  }
}

function applyTheme(colors) {
  if (!colors) return;
  const root = document.documentElement;
  const map = {
    background: "--bg",
    sidebar: "--sidebar",
    panel: "--panel",
    panel_2: "--panel-2",
    panel_3: "--panel-3",
    text: "--text",
    faint: "--faint",
    border: "--border",
    accent: "--accent",
    user: "--user",
    assistant: "--assistant",
    success: "--success",
    tool: "--tool",
    error: "--error",
    info: "--info",
    dim: "--dim",
    tool_pending_bg: "--tool-bg",
  };
  for (const [slot, variable] of Object.entries(map)) {
    if (colors[slot]) root.style.setProperty(variable, colors[slot]);
  }
}

async function loadTheme() {
  try {
    const themes = await invoke("list_themes", { project: themeProject() });
    const name = themes.current || "dark";
    const colors = await invoke("theme_colors", { project: themeProject(), name });
    applyTheme(colors.colors);
  } catch (error) {
    // Keep the default palette.
  }
}

// ---------- permissions ----------

async function openPermissions() {
  // The approvals are per-project, so with nothing selected the dialog itself
  // explains what to do instead of leaving a one-off line in the status bar.
  const box = el("approval-list");
  closeOverlays("permissions-modal");
  el("permissions-modal").hidden = false;
  if (!state.project) {
    el("permissions-clear").disabled = true;
    box.innerHTML =
      `<div class="empty" style="margin:6px">${escapeHtml(t("desktop.empty.selectApprovals", "Select a project to review its tool approvals."))}</div>`;
    return;
  }
  el("permissions-clear").disabled = false;
  const list = await invoke("list_approvals", { project: state.project });
  box.innerHTML = "";
  if (!list.length) {
    box.innerHTML = `<div class="empty" style="margin:6px">${escapeHtml(t("desktop.empty.approvals", "No saved approvals."))}</div>`;
  } else {
    for (const tool of list) {
      const row = document.createElement("div");
      row.className = "theme";
      row.textContent = tool;
      box.appendChild(row);
    }
  }
}

async function clearApprovals() {
  if (!state.project) return;
  await invoke("clear_approvals", { project: state.project });
  openPermissions();
}

// ---------- updates ----------

/// The check the dialog is showing, so the Release notes button opens the
/// release the body was painted from rather than one read again.
let updateCheck = null;
/// The install the dialog is showing, as the shell reports it — `{ stage,
/// version, received, total }` — or null while none is running. One install at
/// a time is the app's own rule, which the engine holds, so this is also what
/// says a click on Update has nothing to start: a click while this is set
/// adopts the install already under way rather than starting a second one.
let updateInstall = null;
/// The check the dialog is waiting on. The menu item and the sidebar button ask
/// the same question and either may be pressed again before the first answer
/// arrives, so every request takes a number: a reply for a number the dialog has
/// moved past is dropped rather than painted over the newer answer — which would
/// show a release, or a failure, from a question nobody is waiting on. An
/// install reporting itself takes the dialog over the same way, since a check
/// asked for while one works is about a build that is already being replaced.
let updateProbe = 0;
/// The install this session finished, if any. The window is still the build
/// that started, so a check compares against that older version and keeps
/// resolving the release now on disk; while the install is remembered, a check
/// that answers with that release reports what is left to do rather than
/// offering the same install again. Held in an object so a front-end check can
/// put the dialog back to the state a fresh window is in.
const installedUpdate = { answer: null };

const updateNotice = { key: null, fallback: "" };

function paintUpdateNotice(key, fallback) {
  Object.assign(updateNotice, { key, fallback });
  const button = el("rail-update");
  button.hidden = !key;
  if (key) {
    button.title = t(key, fallback);
    button.setAttribute("aria-label", button.title);
  }
}

function openUpdateNotice() {
  if (!installedUpdate.answer || updateInstall) return openUpdate();
  closeOverlays("update-modal");
  el("update-modal").hidden = false;
  updateProbe += 1;
  paintInstalled(installedUpdate.answer);
}

/// Opens the dialog on the app's own release train (`desktop-v*`) and offers to
/// install it. The resolution is `oxide_core::updates`, shared with the
/// terminal, so the release this window offers is a release of this app and not
/// of the CLI — and when this installation is one the app may replace (a bundle
/// it can write to, an AppImage, an installation this app's own installer made)
/// the dialog installs it in place.
///
/// What it paints is the step an install is at, when there is one to be at: the
/// launch installs a release on its own, and a download already running is what
/// the reader is being shown.
async function openUpdate() {
  closeOverlays("update-modal");
  el("update-modal").hidden = false;
  // A fresh check replaces the release the last one resolved, so a check that
  // fails does not leave a Release notes button pointing at the old release.
  updateCheck = null;
  const probe = ++updateProbe;
  if (updateInstall) {
    paintDownloading(updateInstall);
    return;
  }
  paintChecking();
  const answer = await readUpdate();
  // The dialog may have been closed while the check was in flight, and a
  // repaint here would put it back; a check started since — the menu and the
  // button pressed one after the other — owns the dialog now.
  if (probe !== updateProbe || el("update-modal").hidden) return;
  if (answer.failed) paintUpdateFailure(t("ui.couldNotCheckForUpdates", "Could not check for updates"), answer.failed);
  else paintUpdate(answer.check);
}

/// Runs the check and reports either its answer or what went wrong, so both
/// callers paint one shape instead of each handling a rejected promise.
async function readUpdate() {
  try {
    return { check: await invoke("check_updates") };
  } catch (error) {
    return { failed: String(error) };
  }
}

function checkingLine() {
  return `<div class="update-line">${escapeHtml(t("desktop.empty.checkingRelease", "Checking the newest release\u2026"))}</div>`;
}

/// Sets the one heading and the one line under it, which every step writes and
/// none may leave behind from the step before it.
function updateHead(title, note) {
  el("update-title").textContent = title;
  el("update-note").textContent = note || "";
}

/// The one button of each step, so only the actions that belong to the step on
/// screen are offered: a control the reader cannot use yet, or can no longer
/// use, is not one to hand them.
const UPDATE_BUTTONS = [
  "update-page",
  "update-close",
  "update-later",
  "update-cancel",
  "update-background",
  "update-download",
  "update-install",
  "update-restart",
];

function updateButtons(shown) {
  for (const id of UPDATE_BUTTONS) {
    el(id).hidden = !shown[id];
    el(id).disabled = false;
  }
  el("update-close").textContent = t("common.close", "Close");
}

/// The dialog before anything is known: the check is in flight, and the only
/// thing to do about it is leave.
function paintChecking() {
  updateHead(t("dialog.update.title", "Updates"), "");
  el("update-body").innerHTML = checkingLine();
  paintChangelog(null);
  paintProgressBar(null);
  updateButtons({ "update-close": true });
}

/// Paints what the check found: the release that is out with what it changed,
/// or the version this build is on and nothing to do about it.
function paintUpdate(check) {
  updateCheck = check || null;
  if (!check) {
    updateHead(t("dialog.update.title", "Updates"), "");
    el("update-body").innerHTML = "";
    paintChangelog(null);
    paintProgressBar(null);
    updateButtons({ "update-close": true });
    return;
  }
  // An install this session finished is the newest word on this installation,
  // and the check cannot see it: the running build is still the one that
  // started, so the release it resolved is the one already on disk. Painting it
  // would offer an install that has happened.
  const installed = installedUpdate.answer;
  if (installed && check.updateAvailable && check.latest === installed.version) {
    paintInstalled(installed);
    return;
  }
  const available = check.updateAvailable === true;
  paintUpdateNotice(available ? "ui.updateAvailable" : null, "Update available");
  updateHead(
    available
      ? t("dialog.update.available", "Update Available (v{version})", {
          version: check.latest,
        })
      : t("dialog.update.current", "Oxide {version} is up to date", {
          version: check.current,
        }),
    available
      ? check.installable === true
        ? t("ui.installingDownloadsTheReleaseForThisMachineAndPutsItInThisAppSPlace", "Installing downloads the release for this machine and puts it in this app's place. ") +
          t("update.keepsRunning", "The app keeps running until it is restarted.")
        : t(
            "ui.downloadingFetchesTheInstallerIntoYourDownloadsFolder",
            "Downloading fetches this release's installer into your Downloads folder. Open it to install the update.",
          )
      : "",
  );
  paintFacts(check);
  paintChangelog(check);
  paintProgressBar(null);
  // Only the app itself decides whether it can replace this installation: a
  // copy in a system directory, a distribution's package or a checkout's build
  // cannot be written over. Such a copy still gets the release itself: the
  // Download button fetches the installer into the reader's Downloads folder,
  // and the release page stays where the notes are.
  const install = available && check.installable === true;
  const download = available && Boolean(check.asset && check.asset.url);
  updateButtons({
    "update-close": true,
    "update-page": Boolean(check.releaseUrl) && (!install || !check.notes),
    "update-install": install,
    "update-download": download,
  });
  if (available) el("update-close").textContent = t("common.dismiss", "Dismiss");
  // The button says what it does rather than naming the release again: the
  // dialog's own title already carries the version it is about.
  if (install) el("update-install").textContent = t("dialog.update.install", "Update");
  if (download) {
    el("update-download").className = install ? "ghost" : "primary";
    el("update-download").textContent = t("dialog.update.download", "Download");
  }
}

/// The facts a check leaves: what this machine has, what the release is, and
/// which installation the app found itself in.
function paintFacts(check) {
  const lines = [t("update.currentVersion", "Current {value0}", { value0: mono(check.current) }), t("ui.latest", "Latest {value0}", { value0: mono(check.latest) })];
  if (check.installation || check.path) {
    lines.push(
      `<span class="update-path">${escapeHtml(check.installation || "unknown")} \u00b7 ${escapeHtml(check.path || "")}</span>`,
    );
  }
  if (check.asset && check.asset.name) {
    lines.push(t("ui.download", "Download {value0}", { value0: escapeHtml(check.asset.name) }));
  }
  if (check.advice) lines.push(`<span class="update-advice">${escapeHtml(check.advice)}</span>`);
  el("update-body").innerHTML = lines
    .map((line) => `<div class="update-line">${line}</div>`)
    .join("");
}

/// The release's own notes, under the version and the day it went out: what a
/// reader wants before deciding to install is what changed, and the release job
/// already wrote it as Markdown. A release that carried none says nothing here
/// rather than leaving an empty box behind.
function paintChangelog(check) {
  const box = el("update-changelog");
  const notes = String((check && check.notes) || "").trim();
  if (!check || !notes) {
    box.hidden = true;
    box.innerHTML = "";
    return;
  }
  const date = check.releasedAt
    ? `<span class="update-changelog-date">${escapeHtml(check.releasedAt)}</span>`
    : "";
  box.innerHTML =
    `<div class="update-changelog-head"><span class="update-changelog-version">Oxide ${escapeHtml(check.latest)}</span>${date}</div>` +
    `<div class="update-changelog-body">${renderMarkdown(notes)}</div>`;
  box.hidden = false;
}

/// The download as it happens. The step is one step to the reader — the release
/// is being fetched and put in place — so the title names that step and the line
/// under it says what the engine is doing with it, since looking the release up
/// and checking its checksum are not a download.
///
/// The stages that carry no bytes carry no bar either: a bar filled to 25% that
/// was replaced by an indeterminate one reading "0.0 MB downloaded" the moment
/// the transfer ended is a number going backwards in front of the reader.
function paintDownloading(install) {
  paintUpdateNotice("ui.downloadingUpdate", "Downloading update");
  const version = install.version || (updateCheck && updateCheck.latest) || "";
  const named = version ? `Oxide ${version}` : t("ui.theRelease", "The release");
  const stage = install.stage || "downloading";
  const transferring = stage === "downloading";
  const doing =
    {
      checking: t("ui.isBeingLookedUp", "{value0} is being looked up", { value0: named }),
      verifying: t("ui.isBeingVerified", "{value0} is being verified", { value0: named }),
      installing: t("ui.isBeingPutInPlace", "{value0} is being put in place", { value0: named }),
    }[stage] ||
    (version
      ? t("ui.oxideIsBeingDownloadedAndPutInPlace", "Oxide {value0} is being downloaded and put in place", { value0: version })
      : t("ui.theReleaseIsBeingDownloadedAndPutInPlace", "The release is being downloaded and put in place"));
  updateHead(
    stage === "installing" ? t("ui.installingUpdate", "Installing update") : t("ui.downloadingUpdate", "Downloading update"),
    t("ui.theAppKeepsRunningUntilItIsRestarted", "{value0}. The app keeps running until it is restarted.", { value0: doing }),
  );
  paintProgressBar(transferring ? install : null);
  const lines = [];
  if (transferring) {
    const asset = (updateCheck && updateCheck.asset && updateCheck.asset.name) || "";
    if (asset) lines.push(t("ui.downloading", "Downloading {value0}", { value0: escapeHtml(asset) }));
    lines.push(downloadedLine(install));
  } else {
    lines.push(escapeHtml(doing));
  }
  el("update-body").innerHTML = lines
    .map((line) => `<div class="update-line">${line}</div>`)
    .join("");
  // Cancel is offered while the install can still be stopped, and no longer once
  // it cannot: the swap that puts the release in place is not a step to interrupt
  // halfway, so a button that closed the dialog while the install went on would
  // be one promising something it cannot do.
  updateButtons({
    "update-cancel": UPDATE_CANCELABLE.includes(stage),
    "update-background": true,
  });
}

/// The stages of an install a reader can still stop: the release can be let go of
/// while it is being looked up, while it is on its way and while it is being
/// checked — and not once it is being put in place.
const UPDATE_CANCELABLE = ["checking", "downloading", "verifying"];

/// How far the transfer has got, in the words a reader reads it in: a fraction of
/// a size the response announced, or the bytes alone when it announced none.
function downloadedLine(install) {
  const total = Number(install.total) || 0;
  const received = Number(install.received) || 0;
  if (total <= 0) return t("ui.downloaded", "{value0} downloaded", { value0: megabytes(received) });
  const share = Math.min(100, Math.round((received / total) * 100));
  return t("ui.of", "{value0} of {value1} · {value2}%", { value0: megabytes(received), value1: megabytes(total), value2: share });
}

/// The bar, which is the transfer's own: it moves with the bytes written, works
/// while none have been announced, and is taken away when there is nothing being
/// downloaded. Nothing else on screen is this function's to write.
function paintProgressBar(install) {
  const box = el("update-progress");
  if (!install) {
    box.hidden = true;
    return;
  }
  const total = Number(install.total) || 0;
  const received = Number(install.received) || 0;
  const known = total > 0;
  const share = known ? Math.min(100, Math.round((received / total) * 100)) : 0;
  el("update-progress-fill").style.width = `${known ? share : 100}%`;
  box.classList.toggle("indeterminate", !known);
  box.hidden = false;
}

function megabytes(bytes) {
  return `${(Number(bytes) / (1024 * 1024)).toFixed(1)} MB`;
}

/// Installs the release the dialog offered: the app downloads the artifact its
/// own release train publishes for this platform, checks it against the digest
/// that release carries, and puts it in this installation's place.
async function installUpdate() {
  // One install at a time, and this is what says which one: an install already
  // running — the launch's own, or one this window started — is the one the
  // click adopts, since a second over the same installation is what the app's
  // own guard refuses and the download in front of the reader is the one they
  // asked for.
  if (updateInstall) {
    paintDownloading(updateInstall);
    return;
  }
  const install = {
    // The name this install answers to, minted here rather than read off the
    // engine: the reader can ask to stop it in the moment between this request
    // going out and the install starting, and a cancel that names its install is
    // that one's to obey whatever order the two requests arrive in.
    token: newUpdateToken(),
    stage: "checking",
    version: (updateCheck && updateCheck.latest) || "",
    received: 0,
    total: 0,
  };
  updateInstall = install;
  // The install owns the dialog until it reports: a check started meanwhile —
  // the sidebar button is still there — is a look at the installation as it was
  // when the check started, which the install's own report outranks.
  updateProbe += 1;
  paintDownloading(install);
  let answer;
  try {
    answer = await invoke("install_update", { token: install.token });
  } catch (error) {
    answer = { ok: false, text: String(error) };
  }
  // The reader stopped it, in a dialog they have already closed: there is
  // nothing here to report.
  if (answer && answer.cancelled === true) return;
  // What gates this report is the install the window is showing, not the check
  // counter: the engine reports its stages through the same dialog, so a counter
  // that moved under them would drop the very answer that says how the install
  // ended.
  if (!updateInstall || updateInstall.token !== install.token) return;
  if (!answer || answer.ok !== true) {
    updateInstall = null;
    reportUpdateFailure((answer && answer.text) || t("ui.theInstallDidNotFinish", "The install did not finish."));
    return;
  }
  // The install owns the dialog from here, whatever a check started while it
  // worked has to say: that check asked about the build running when it
  // started, and the release it would offer is the one now in place.
  handleUpdateReady(answer);
}

/// The name this window gives each install it asks for, which is what a cancel
/// addresses. The seed is a page's own: a window opened again starts its count
/// over, and a name left behind by the window before it is not one it can
/// cancel by accident.
const UPDATE_TOKEN_SEED = `${Date.now().toString(36)}-`;
let updateTokens = 0;

function newUpdateToken() {
  updateTokens += 1;
  return `ui-${UPDATE_TOKEN_SEED}${updateTokens}`;
}

/// Downloads the release the dialog offered into the reader's Downloads folder.
/// This is the action a copy the app cannot replace in place gets — a system
/// installation, a checkout's build — so the newest release is reachable from
/// the dialog without the release page. It is the install's own bar: the file is
/// the one an install would fetch and verify, only it lands where the reader
/// opens it.
async function downloadUpdate() {
  if (updateInstall) {
    paintDownloading(updateInstall);
    return;
  }
  const install = {
    token: newUpdateToken(),
    stage: "checking",
    version: (updateCheck && updateCheck.latest) || "",
    received: 0,
    total: 0,
  };
  updateInstall = install;
  updateProbe += 1;
  paintDownloading(install);
  let answer;
  try {
    answer = await invoke("download_update", { token: install.token });
  } catch (error) {
    answer = { ok: false, text: String(error) };
  }
  if (answer && answer.cancelled === true) return;
  if (!updateInstall || updateInstall.token !== install.token) return;
  updateInstall = null;
  if (!answer || answer.ok !== true) {
    reportUpdateFailure(
      (answer && answer.text) || t("ui.theDownloadDidNotFinish", "The download did not finish."),
    );
    return;
  }
  paintDownloaded(answer);
}

/// Stops the install the dialog is watching. The reader asked for it, so nothing
/// is put in place and nothing is reported as having gone wrong: the next check
/// offers the release again.
function cancelUpdate() {
  const token = updateInstall && updateInstall.token;
  paintUpdateNotice("ui.updateAvailable", "Update available");
  updateInstall = null;
  updateProbe += 1;
  el("update-modal").hidden = true;
  if (!token) return;
  invoke("cancel_update", { token }).catch((error) =>
    showToast(t("ui.couldNotStopTheUpdate", "Could not stop the update: {value0}", { value0: error }), "error"),
  );
}

/// An install that went wrong, wherever the reader is: the dialog is repainted
/// when it is the one they are looking at, so the step it was on is not left
/// standing over an install that has ended — and when it is not, the same words
/// go to the composer's own status line, which is where an install nobody is
/// watching reports itself.
function reportUpdateFailure(text) {
  paintUpdateNotice("ui.updateAvailable", "Update available");
  if (el("update-modal").hidden) {
    showToast(t("update.installError", "Could not install the update: {value0}", { value0: text }), "error");
    return;
  }
  paintUpdateFailure(t("ui.couldNotInstallTheUpdate", "Could not install the update"), text);
}

/// Reports an install that landed. The version named is the one the install
/// itself resolved and put on disk rather than the one that was offered a
/// download ago, since a release published in between is the one that went on.
/// The running app is still the build that started, so what the dialog asks for
/// is the restart that runs the release rather than claiming this window is
/// already the new version.
function paintInstalled(answer) {
  paintUpdateNotice(
    answer.pending ? "ui.installingUpdate" : "dialog.update.restart",
    answer.pending ? "Installing update" : "Restart and install update",
  );
  // A Windows installer is a program this app starts and cannot wait on: it
  // asks for elevation and for Oxide to be closed, and the user may cancel it.
  // What is known here is that it is running, so that is what the dialog says
  // rather than a version it cannot claim is in place.
  const pending = answer.pending === true;
  updateHead(
    pending ? t("ui.theOxideInstallerIsRunning", "The Oxide {value0} installer is running", { value0: answer.version }) : t("dialog.update.restart", "Restart and install update"),
    pending
      ? t("ui.finishTheInstallerThenOpenOxideAgainToRunTheNewVersion", "Finish the installer, then open Oxide again to run the new version.")
      : t("ui.updateDownloadedYouNeedToRestartOxideToInstallTheUpdate", "Update downloaded. You need to restart Oxide to install the update."),
  );
  const lines = [];
  if (answer.version) lines.push(t("ui.release", "Release {value0}", { value0: mono(answer.version) }));
  if (answer.asset) lines.push(t("update.downloadedAsset", "Downloaded {value0}", { value0: escapeHtml(answer.asset) }));
  if (answer.path) lines.push(`<span class="update-path">${escapeHtml(answer.path)}</span>`);
  if (answer.text) lines.push(`<span class="update-advice">${escapeHtml(answer.text)}</span>`);
  el("update-body").innerHTML = lines
    .map((line) => `<div class="update-line">${line}</div>`)
    .join("");
  paintChangelog(null);
  paintProgressBar(null);
  // Only a release this app put in place is one to restart into: a Windows
  // installer owns what happens next, and the app it asks to be closed is this
  // one.
  updateButtons(
    pending ? { "update-close": true } : { "update-later": true, "update-restart": true },
  );
}

/// A download that landed: the installer is in the reader's Downloads folder,
/// and opening it from there is the install the release page's would be. No
/// restart is offered, because nothing was put in place to restart into.
function paintDownloaded(answer) {
  paintUpdateNotice("ui.updateAvailable", "Update available");
  const version = (answer && answer.version) || "";
  updateHead(
    t("dialog.update.downloaded", "Oxide {version} is downloaded", { version }),
    t(
      "ui.openTheInstallerFromYourDownloadsFolderToFinishTheUpdate",
      "Open the installer from your Downloads folder to finish the update.",
    ),
  );
  const lines = [];
  if (answer && answer.path) lines.push(`<span class="update-path">${escapeHtml(answer.path)}</span>`);
  if (answer && answer.text) lines.push(`<span class="update-advice">${escapeHtml(answer.text)}</span>`);
  el("update-body").innerHTML = lines
    .map((line) => `<div class="update-line">${line}</div>`)
    .join("");
  paintChangelog(null);
  paintProgressBar(null);
  updateButtons({ "update-close": true });
}

/// A failure to check or to install: the CLI's own words, which say which of the
/// two it was — a release lookup that could not reach GitHub, or a download that
/// did not verify.
function paintUpdateFailure(title, text) {
  updateHead(
    title,
    title === "Could not check for updates"
      ? t("ui.theCheckAsksGitHubForTheNewestReleaseSoAMachineWithNoNetworkOrARequestTheNetwork", "The check asks GitHub for the newest release, so a machine with no network — or a request the network refuses — says so here.")
      : "",
  );
  el("update-body").innerHTML = `<div class="update-error">${escapeHtml(text)}</div>`;
  paintChangelog(null);
  paintProgressBar(null);
  updateButtons({
    "update-close": true,
    "update-page": Boolean(updateCheck && updateCheck.releaseUrl),
  });
}

// ---------- the install the shell reports ----------

/// The launch looked for a release of this app and is installing one on its own,
/// so the window hears about it rather than asking — and the dialog is where a
/// reader is told. It is opened on what the install is doing; each step after
/// this one arrives as a stage of its own.
function handleUpdateAvailable() {
  paintUpdateNotice("ui.updateAvailable", "Update available");
  return openUpdate();
}

/// A step of an install. The dialog paints it when it is on screen and simply
/// remembers it otherwise: a reader who put the dialog away is still owed the
/// restart that runs what lands, which is reported on its own below.
///
/// The engine names the install a stage belongs to, and the window keeps that
/// name: the install it asked for keeps its own object — the click that started
/// it is waiting on the same one — while an install it was only told about (the
/// launch's own) is adopted under the name the engine gave it, which is what its
/// Cancel then addresses.
function handleUpdateProgress(payload) {
  paintUpdateNotice("ui.downloadingUpdate", "Downloading update");
  const token = (payload && payload.token) || "";
  const reported = {
    stage: (payload && payload.stage) || "",
    version: (payload && payload.version) || "",
    received: (payload && payload.received) || 0,
    total: (payload && payload.total) || 0,
    token: token || (updateInstall && updateInstall.token) || "",
  };
  if (updateInstall && (!token || updateInstall.token === token)) {
    Object.assign(updateInstall, reported);
  } else {
    updateInstall = reported;
  }
  // The install is newer news than any check still in flight: a dialog that
  // painted the release over the download the reader is watching would be
  // asking about a build that is already being replaced.
  updateProbe += 1;
  if (!el("update-modal").hidden) paintDownloading(updateInstall);
}

/// An install that landed. The running app is still the build that started, so
/// the dialog is opened on the restart that runs what is now on disk — even for
/// a download the reader sent to the background, since this is the one thing
/// left to do about it. The rail keeps it reachable after Later.
function handleUpdateReady(answer) {
  installedUpdate.answer = answer;
  updateInstall = null;
  updateProbe += 1;
  closeOverlays("update-modal");
  el("update-modal").hidden = false;
  paintInstalled(answer);
}

/// An install that could not finish, or one the reader stopped. A failure the
/// reader is watching is repainted into the dialog it was watching it in — the
/// step it was on must not be left standing over an install that has ended — and
/// one that arrived while the dialog was away is the same status line the
/// launch's own install reports itself by. A cancel is neither: the reader
/// asked for it, and the dialog they asked it in is already gone.
function handleUpdateFailed(payload) {
  paintUpdateNotice("ui.updateAvailable", "Update available");
  updateInstall = null;
  updateProbe += 1;
  if (payload && payload.cancelled === true) {
    el("update-modal").hidden = true;
    return;
  }
  reportUpdateFailure((payload && payload.message) || "unknown error");
}

/// Asks what the install this process is running has already said.
///
/// The launch's own install starts before this page does, so its first steps were
/// emitted into a window with nothing listening; the newest of them is kept
/// beside the app's state for exactly this window, and the report the restart
/// hangs on is not one to miss. What it answers is painted through the same
/// handlers the events go to, since it is the event it would have heard — and
/// the steps that do not open the dialog themselves are opened here, because a
/// window that has just started is one nobody has dismissed it in.
function catchUpOnLaunchUpdate() {
  return invoke("launch_update")
    .then((heard) => {
      if (!heard || !heard.event) return;
      const payload = heard.payload || {};
      if (heard.event === "update-available") return handleUpdateAvailable(payload);
      if (heard.event === "update-ready") return handleUpdateReady(payload);
      if (heard.event === "update-failed") return handleUpdateFailed(payload);
      if (heard.event === "update-progress") return openInstallUpdate(payload);
    })
    .catch(() => {});
}

/// Opens the dialog on an install already running: the reader is being shown the
/// download rather than asked to start one.
function openInstallUpdate(payload) {
  handleUpdateProgress(payload);
  if (el("update-modal").hidden) {
    closeOverlays("update-modal");
    el("update-modal").hidden = false;
  }
  paintDownloading(updateInstall);
}

/// Restarts Oxide, which is what runs a release an install has put in place: the
/// process running is still the build that started, so only a new one is the new
/// version. A turn is work this process owns, so a restart mid-turn is refused
/// the way starting a new thread is.
function restartApp() {
  if (busyRefusal(t("ui.restartingOxide", "restarting Oxide"))) return;
  if ([...threads.values()].some((record) => record.busy)) {
    setStatus(t("ui.aTurnIsRunningStopItBefore", "A turn is running; stop it before {value0}.", { value0: t("ui.restartingOxide", "restarting Oxide") }));
    return;
  }
  invoke("restart_app").catch((error) => showToast(t("ui.couldNotRestartOxide", "Could not restart Oxide: {value0}", { value0: error }), "error"));
}

/// Opens the release the dialog is showing in the platform browser, the way a
/// link in a reply is opened.
function openReleaseNotes() {
  if (!updateCheck || !updateCheck.releaseUrl) return;
  invoke("open_url", { url: updateCheck.releaseUrl }).catch((error) =>
    showToast(t("ui.couldNotOpenLink", "Could not open link: {value0}", { value0: error }), "error"),
  );
}

function mono(value) {
  return `<code>${escapeHtml(value || "")}</code>`;
}

// ---------- customization pages ----------

let customizeView = "";
let customizeGeneration = 0;
let pluginCatalog = { installed: [], marketplaces: [] };
let pluginSource = "all";
let pluginBusy = false;

function closeCustomize() {
  closeProjectDirectory();
  closeSettings();
  customizeView = "";
  customizeGeneration++;
  document.body.classList.remove("customizing");
  document.body.classList.toggle("sidebar-hidden", !state.prefs.sidebar_visible);
  el("customize-sidebar").hidden = true;
  el("customize-page").hidden = true;
  el("mcps-modal").hidden = true;
  el("rail-customize").removeAttribute("aria-current");
  el("rail-home").setAttribute("aria-current", "page");
  paintNavigation();
}

async function openCustomize(view = "plugins") {
  closeSettings();
  closeOverlays();
  customizeView = view;
  const generation = ++customizeGeneration;
  document.body.classList.add("customizing");
  document.body.classList.remove("sidebar-hidden");
  el("customize-sidebar").hidden = false;
  el("customize-page").hidden = false;
  for (const [name, id] of [["plugins", "plugins-page"], ["skills", "skills-page"], ["mcps", "mcps-modal"]]) {
    el(id).hidden = name !== view;
    el(`customize-${name}`).setAttribute("aria-current", name === view ? "page" : "false");
  }
  el("rail-home").removeAttribute("aria-current");
  el("rail-customize").setAttribute("aria-current", "page");
  paintNavigation();
  el(view === "mcps" ? "mcps-title" : `${view}-title`).focus();
  const catalog = loadPluginCatalog();
  if (view === "mcps") await loadMcps();
  if (view === "skills") {
    const box = el("skills-list");
    box.innerHTML = "";
    if (!state.project) appendProjectPrompt(box);
    else {
      box.textContent = "Loading skills…";
      try {
        const project = state.project;
        const entries = await invoke("list_commands", { project });
        if (generation !== customizeGeneration || project !== state.project) return;
        box.innerHTML = "";
        for (const skill of entries.filter((entry) => entry.kind === "skill")) {
          const row = document.createElement("article");
          row.className = "plugin-card";
          const text = document.createElement("div");
          const title = document.createElement("h3");
          title.textContent = skill.name;
          const description = document.createElement("p");
          description.textContent = skill.description;
          text.append(title, description);
          row.appendChild(text);
          box.appendChild(row);
        }
        if (!box.children.length) box.textContent = "No skills found in this project. Add a skill or install a plugin that provides skills.";
      } catch (error) {
        if (generation === customizeGeneration) box.textContent = `Could not load skills: ${error}`;
      }
    }
  }
  await catalog;
}

function appendProjectPrompt(box) {
  const message = document.createElement("p");
  message.textContent = "Choose a project to manage its resources.";
  const button = document.createElement("button");
  button.type = "button";
  button.className = "primary";
  button.textContent = "Choose a project";
  button.onclick = () => { closeCustomize(); askForProject(); };
  box.append(message, button);
}

let pluginCatalogGeneration = 0;
async function loadPluginCatalog() {
  const generation = ++pluginCatalogGeneration;
  el("plugin-message").textContent = "Loading plugins…";
  try {
    const catalog = await invoke("plugin_catalog");
    if (generation !== pluginCatalogGeneration) return;
    pluginCatalog = catalog;
    el("plugin-message").textContent = "";
    renderPlugins();
    return true;
  } catch (error) {
    if (generation === pluginCatalogGeneration) el("plugin-message").textContent = `Could not load plugins: ${error}`;
    return false;
  }
}

function pluginButton(label, action, name, marketplace) {
  const button = document.createElement("button");
  button.type = "button";
  const icon = { install: "plus", enable: "power", disable: "power", uninstall: "trash", update_marketplace: "refresh", remove_marketplace: "trash" }[action];
  button.innerHTML = ICONS[icon] || "";
  if (action.endsWith("marketplace")) {
    const text = document.createElement("span");
    text.textContent = label;
    button.appendChild(text);
  }
  button.title = `${label} ${name}`;
  button.disabled = pluginBusy;
  button.setAttribute("aria-label", `${label} ${name}`);
  button.onclick = () => mutatePlugin(action, name, marketplace);
  return button;
}

function renderPlugins() {
  const installed = el("customize-installed");
  installed.innerHTML = "";
  for (const plugin of pluginCatalog.installed) {
    const button = document.createElement("button");
    button.type = "button";
    button.innerHTML = ICONS.puzzle;
    const label = document.createElement("span");
    label.textContent = plugin.name;
    button.appendChild(label);
    button.title = plugin.enabled ? "Enabled" : "Disabled";
    button.onclick = () => {
      pluginSource = "installed";
      el("plugin-search").value = plugin.name;
      void openCustomize("plugins");
    };
    installed.appendChild(button);
  }
  if (!pluginCatalog.installed.length) installed.textContent = "No plugins installed";
  const tabs = el("plugin-tabs");
  tabs.innerHTML = "";
  const sources = [["all", "All"], ["installed", "Installed"], ...pluginCatalog.marketplaces.map((marketplace) => [marketplace.name, marketplace.name])];
  if (!sources.some(([key]) => key === pluginSource)) pluginSource = "all";
  for (const [key, label] of sources) {
    const button = document.createElement("button");
    button.type = "button";
    button.textContent = label;
    button.setAttribute("aria-pressed", String(pluginSource === key));
    button.onclick = () => { pluginSource = key; renderPlugins(); };
    tabs.appendChild(button);
  }
  const box = el("plugin-list");
  box.innerHTML = "";
  const query = el("plugin-search").value.trim().toLowerCase();
  let shown = 0;
  const appendGroup = (label, plugins, marketplace) => {
    const matches = plugins.filter((plugin) => `${plugin.name} ${plugin.description || ""}`.toLowerCase().includes(query));
    if (!matches.length) return;
    shown += matches.length;
    const group = document.createElement("section");
    group.className = "plugin-group";
    const heading = document.createElement("h2");
    heading.textContent = label;
    group.appendChild(heading);
    const grid = document.createElement("div");
    grid.className = "plugin-grid";
    for (const plugin of matches) {
      const card = document.createElement("article");
      card.className = "plugin-card";
      const glyph = document.createElement("span");
      glyph.className = "plugin-glyph";
      glyph.innerHTML = ICONS.puzzle;
      glyph.setAttribute("aria-hidden", "true");
      const text = document.createElement("div");
      text.className = "plugin-copy";
      const name = document.createElement("h3");
      name.textContent = plugin.name;
      const description = document.createElement("p");
      description.textContent = plugin.description || "No description provided.";
      const detail = document.createElement("small");
      const local = pluginCatalog.installed.find((entry) => entry.name === plugin.name);
      detail.textContent = [
        plugin.version,
        local?.managed ? "Built in" : null,
        local ? (local.enabled ? "Enabled" : "Disabled") : "Available",
      ].filter(Boolean).join(" · ");
      text.append(name, description, detail);
      const actions = document.createElement("div");
      actions.className = "plugin-actions";
      if (local) {
        actions.append(pluginButton(local.enabled ? "Disable" : "Enable", local.enabled ? "disable" : "enable", local.name));
        if (!local.managed) actions.append(pluginButton("Uninstall", "uninstall", local.name));
      } else actions.appendChild(pluginButton("Install", "install", plugin.name, marketplace));
      card.append(glyph, text, actions);
      grid.appendChild(card);
    }
    group.appendChild(grid);
    box.appendChild(group);
  };
  if (pluginSource === "all" || pluginSource === "installed") appendGroup("Installed", pluginCatalog.installed);
  for (const marketplace of pluginCatalog.marketplaces) {
    if (pluginSource !== "all" && pluginSource !== marketplace.name) continue;
    const controls = document.createElement("div");
    controls.className = "marketplace-controls";
    const source = document.createElement("span");
    source.textContent = `${marketplace.name} · ${marketplace.source}`;
    controls.append(source, pluginButton("Update", "update_marketplace", marketplace.name), pluginButton("Remove marketplace", "remove_marketplace", marketplace.name));
    box.appendChild(controls);
    if (marketplace.error) {
      const error = document.createElement("p");
      error.textContent = marketplace.error;
      box.appendChild(error);
    }
    appendGroup(marketplace.name, marketplace.plugins, marketplace.name);
  }
  if (!shown) {
    const empty = document.createElement("p");
    empty.textContent = query ? "No plugins match your search." : "No plugins yet. Add a marketplace to discover and install plugins.";
    box.appendChild(empty);
  }
  el("plugins-add").disabled = pluginBusy;
  el("plugins-refresh").disabled = pluginBusy;
}

async function mutatePlugin(action, name, marketplace = null) {
  if (pluginBusy) return;
  pluginBusy = true;
  renderPlugins();
  el("plugin-message").textContent = `Working on ${name}…`;
  try {
    const message = await invoke("change_plugin", { action, name, marketplace });
    if (action === "add_marketplace") {
      el("marketplace-form").hidden = true;
      el("plugins-add").setAttribute("aria-expanded", "false");
      el("marketplace-source").value = "";
    }
    if (await loadPluginCatalog()) el("plugin-message").textContent = message;
    state.palette = [];
  } catch (error) {
    el("plugin-message").textContent = `Could not update plugins: ${error}`;
  } finally {
    pluginBusy = false;
    renderPlugins();
  }
}

// ---------- MCP servers ----------

/// The servers this project loads, with the state the core reports: the same
/// listing `oxide mcp list` prints and the terminal's `/mcp` shows.
async function openMcps() {
  await openCustomize("mcps");
}

let mcpGeneration = 0;
async function loadMcps() {
  const generation = ++mcpGeneration;
  const project = state.project;
  el("mcp-list").innerHTML = "";
  if (!project) {
    state.mcps = [];
    state.mcpError = "";
    appendProjectPrompt(el("mcp-list"));
    return;
  }
  el("mcp-list").innerHTML = "<p>Checking servers…</p>";
  try {
    const servers = await invoke("mcp_servers", { project });
    if (generation !== mcpGeneration || project !== state.project) return;
    state.mcps = servers;
    state.mcpError = "";
  } catch (error) {
    if (generation !== mcpGeneration || project !== state.project) return;
    state.mcpError = String(error);
  }
  renderMcps();
}

function mcpStatusText(server) {
  const keys = { connected: "ui.connected", "needs-auth": "ui.needsAuth", "needs-trust": "ui.needsTrust", disabled: "ui.disabled" };
  if (server.state === "error" && server.status?.startsWith("Error: ")) return t("common.errorDetail", "Error: {value0}", { value0: server.status.slice(7) });
  return t(keys[server.state], server.status || server.state || "");
}

/// How a server's tools reach the model, in the language on screen. The order is
/// the one the exposure chip cycles through.
const MCP_EXPOSURES = ["deferred", "direct", "hidden"];
function mcpExposureLabel(exposure) {
  const keys = { deferred: ["ui.exposureDeferred", "deferred"], direct: ["ui.exposureDirect", "direct"], hidden: ["ui.exposureHidden", "hidden"] };
  const [key, fallback] = keys[exposure] || ["", exposure || ""];
  return key ? t(key, fallback) : fallback;
}

function renderMcps() {
  const box = el("mcp-list");
  box.innerHTML = "";
  if (state.mcpError) {
    box.innerHTML = `<div class="mcp-empty">${escapeHtml(t("desktop.mcpReadError", "Could not list MCP servers: {error}", { error: state.mcpError }))}</div>`;
    return;
  }
  const servers = state.mcps || [];
  if (!servers.length) {
    box.innerHTML =
      `<div class="mcp-empty">${t("desktop.mcpEmpty", "No MCP servers configured. Add one with {command}, or a {file} in the project.", { command: "<code>oxide mcp add</code>", file: "<code>.mcp.json</code>" })}</div>`;
    return;
  }
  for (const server of servers) {
    const row = document.createElement("div");
    row.className = "mcp-row";

    const head = document.createElement("div");
    head.className = "mcp-head";
    const name = document.createElement("span");
    name.className = "mcp-name";
    name.textContent = server.name;
    const status = document.createElement("span");
    status.className = `mcp-status state-${server.state}`;
    status.textContent = mcpStatusText(server);
    const actions = document.createElement("div");
    actions.className = "mcp-actions";
    // The exposure chip says how the server's tools reach the model and cycles
    // the three modes; the toggle beside it only turns the server on or off.
    const exposure = document.createElement("button");
    exposure.type = "button";
    exposure.className = `mcp-exposure exposure-${server.exposure || "deferred"}`;
    exposure.textContent = mcpExposureLabel(server.exposure);
    exposure.title = t(
      "ui.mcpExposureHint",
      "Exposure of {value0}: {value1}. Click to change: deferred, direct, hidden.",
      { value0: server.name, value1: mcpExposureLabel(server.exposure) },
    );
    exposure.setAttribute("aria-label", exposure.title);
    exposure.onclick = () => cycleMcpExposure(server, exposure);
    actions.appendChild(exposure);
    if (server.enabled && server.state === "needs-auth") {
      const authorize = document.createElement("button");
      authorize.type = "button";
      authorize.className = "mcp-authorize";
      authorize.textContent = t("ui.authorize", "Authorize");
      authorize.title = t("ui.authorizeHint", "Open {value0} in the browser to authorize it", {
        value0: server.name,
      });
      authorize.setAttribute("aria-label", t("ui.authorizeServer", "Authorize {value0}", {
        value0: server.name,
      }));
      authorize.onclick = () => authorizeMcp(server, authorize);
      actions.appendChild(authorize);
    }
    const toggle = document.createElement("button");
    toggle.type = "button";
    // Icon-only: the listing is a row per server, so the switch is sized to the
    // row and its meaning is in the tooltip and the accessible name.
    toggle.className = `icon mcp-toggle${server.enabled ? " on" : ""}`;
    toggle.innerHTML = ICONS.power;
    toggle.title = server.enabled
      ? t("ui.disableTurnItOffInTheFileThatDefinesIt", "Disable {value0}: turn it off in the file that defines it", { value0: server.name })
      : t("ui.enableTurnItBackOn", "Enable {value0}: turn it back on", { value0: server.name });
    toggle.setAttribute("aria-label", toggle.title);
    toggle.onclick = () => toggleMcp(server, toggle);
    actions.appendChild(toggle);
    // A server defined outside this project can be turned over here alone: the
    // button writes an override in the project's own config and leaves the
    // shared entry, and every other project, untouched.
    if (server.scope !== "project") {
      const local = document.createElement("button");
      local.type = "button";
      local.className = "mcp-local";
      local.textContent = server.enabled
        ? t("ui.offInThisProject", "Off only in this project")
        : t("ui.onInThisProject", "On only in this project");
      local.title = server.enabled
        ? t(
            "ui.offInThisProjectHint",
            "Turn {value0} off in this project only; the shared configuration is unchanged",
            { value0: server.name },
          )
        : t(
            "ui.onInThisProjectHint",
            "Turn {value0} on in this project only; the shared configuration is unchanged",
            { value0: server.name },
          );
      local.setAttribute("aria-label", local.title);
      local.onclick = () => toggleMcpLocal(server, local);
      actions.appendChild(local);
    }
    head.append(name, status, actions);

    const detail = document.createElement("div");
    detail.className = "mcp-detail";
    detail.textContent = [server.transport, server.detail]
      .filter(Boolean)
      .join(" · ") +
      (server.tools > 0 ? ` · ${t("ui.mcpTools", "{count} tools", { count: server.tools })}` : "");
    const source = document.createElement("div");
    source.className = "mcp-source";
    source.textContent = t("ui.source", "source: {value0}", { value0: server.source });

    row.append(head, detail);
    if (server.description) {
      const description = document.createElement("div");
      description.className = "mcp-description";
      description.textContent = server.description;
      row.appendChild(description);
    }
    row.appendChild(source);
    box.appendChild(row);
  }
}

/// Turns a server over for this project alone, through an override in the
/// project's own config, and redraws from the core's answer.
async function toggleMcpLocal(server, button) {
  if (!state.project) {
    setStatus(t("composer.selectProject", "Select a project first."));
    return;
  }
  button.disabled = true;
  const enabled = !server.enabled;
  const project = state.project;
  const generation = ++mcpGeneration;
  const dismissProgress = showToast(`${enabled ? "Turning on" : "Turning off"} ${server.name} in this project…`);
  try {
    const servers = await invoke("set_mcp_server", {
      project,
      name: server.name,
      enabled,
      scope: "project",
      local: true,
    });
    if (generation !== mcpGeneration || project !== state.project) return;
    state.mcps = servers;
    state.mcpError = "";
    renderMcps();
    showToast(`${server.name} ${enabled ? "enabled" : "disabled"} in this project`);
  } catch (error) {
    button.disabled = false;
    showToast(t("ui.couldNotChange", "Could not change {value0}: {value1}", { value0: server.name, value1: error }), "error");
  } finally {
    dismissProgress();
  }
}

/// Writes the next exposure into the file that defines the server and redraws
/// from the core's answer, like the power switch beside it.
async function cycleMcpExposure(server, button) {
  if (!state.project) {
    setStatus(t("composer.selectProject", "Select a project first."));
    return;
  }
  const current = MCP_EXPOSURES.indexOf(server.exposure);
  const exposure = MCP_EXPOSURES[(current + 1) % MCP_EXPOSURES.length];
  button.disabled = true;
  const project = state.project;
  const generation = ++mcpGeneration;
  const dismissProgress = showToast(t("ui.changingMcpExposure", "Changing {value0} to {value1}…", {
    value0: server.name,
    value1: mcpExposureLabel(exposure),
  }));
  try {
    const servers = await invoke("set_mcp_exposure", {
      project,
      name: server.name,
      exposure,
      scope: server.scope,
    });
    if (generation !== mcpGeneration || project !== state.project) return;
    state.mcps = servers;
    state.mcpError = "";
    renderMcps();
    showToast(t("ui.mcpExposureChanged", "{value0} is now {value1}", {
      value0: server.name,
      value1: mcpExposureLabel(exposure),
    }));
  } catch (error) {
    button.disabled = false;
    showToast(t("ui.couldNotChange", "Could not change {value0}: {value1}", { value0: server.name, value1: error }), "error");
  } finally {
    dismissProgress();
  }
}

async function authorizeMcp(server, button) {
  if (!state.project) {
    setStatus(t("composer.selectProject", "Select a project first."));
    return;
  }
  button.disabled = true;
  button.textContent = t("ui.authorizing", "Authorizing…");
  const project = state.project;
  const generation = ++mcpGeneration;
  const dismissProgress = showToast(t("ui.openingAuthorizationPage", "Opening the authorization page for {value0}…", {
    value0: server.name,
  }));
  try {
    const servers = await invoke("authorize_mcp_server", {
      project,
      name: server.name,
      scope: server.scope,
    });
    if (generation !== mcpGeneration || project !== state.project) return;
    state.mcps = servers;
    state.mcpError = "";
    renderMcps();
    showToast(t("ui.mcpAuthorized", "{value0} authorized", { value0: server.name }));
  } catch (error) {
    button.disabled = false;
    button.textContent = t("ui.authorize", "Authorize");
    showToast(t("ui.couldNotAuthorize", "Could not authorize {value0}: {value1}", {
      value0: server.name,
      value1: error,
    }), "error");
  } finally {
    dismissProgress();
  }
}

async function toggleMcp(server, button) {
  if (!state.project) {
    setStatus(t("composer.selectProject", "Select a project first."));
    return;
  }
  button.disabled = true;
  const enabled = !server.enabled;
  const project = state.project;
  const generation = ++mcpGeneration;
  const dismissProgress = showToast(`${enabled ? "Turning on" : "Turning off"} ${server.name}…`);
  try {
    const servers = await invoke("set_mcp_server", {
      project,
      name: server.name,
      enabled,
      scope: server.scope,
    });
    if (generation !== mcpGeneration || project !== state.project) return;
    state.mcps = servers;
    state.mcpError = "";
    renderMcps();
    showToast(`${server.name} ${enabled ? "enabled" : "disabled"}`);
  } catch (error) {
    button.disabled = false;
    showToast(t("ui.couldNotChange", "Could not change {value0}: {value1}", { value0: server.name, value1: error }), "error");
  } finally {
    dismissProgress();
  }
}

// ---------- sessions ----------

/// The threads stored for this project, newest first: what `/session` opens,
/// and the same list the sidebar draws beside the project. The terminal's
/// `/session` picker and the VS Code panel's dialog show the same sessions, so a
/// thread started in one front-end is reachable from the others.
async function openSessions() {
  if (!state.project) {
    setStatus(t("composer.selectProject", "Select a project first."));
    return;
  }
  closeOverlays("sessions-modal");
  el("sessions-modal").hidden = false;
  el("sessions-list").innerHTML = `<div class="dialog-empty">${escapeHtml(t("desktop.empty.loadingThreads", "Loading threads\u2026"))}</div>`;
  await loadSessions();
  renderSessions();
}

function renderSessions() {
  const box = el("sessions-list");
  box.innerHTML = "";
  // The store could not be read, so the empty listing that would be painted
  // from no threads is not a fact about this project.
  if (state.sessionsError) {
    box.innerHTML = `<div class="dialog-empty">${escapeHtml(t("desktop.threadReadError", "Could not read this project's threads: {error}", { error: state.sessionsError }))}</div>`;
    return;
  }
  const threads = listedSessions().filter((session) => sessionProject(session) === state.project);
  if (!threads.length) {
    box.innerHTML =
      `<div class="dialog-empty">${escapeHtml(t("desktop.empty.threads", "No threads for this project yet. Send a message to start one."))}</div>`;
    return;
  }
  for (const session of threads) {
    const running = turnInThread(session.id);
    const row = document.createElement("button");
    row.type = "button";
    row.className =
      "session-row" +
      (session.id === state.session ? " active" : "") +
      (running ? " running" : "");

    const name = session.name || session.preview || session.id.slice(0, 8);
    const meta = [
      session.worktree ? t("worktree.label", "worktree {value0}", { value0: session.worktree }) : "",
      sessionAge(session.modified_at),
      sessionMessages(session.message_count),
    ]
      .filter(Boolean)
      .join(" · ");

    const main = document.createElement("div");
    main.className = "session-main";
    const label = document.createElement("div");
    label.className = "session-label";
    label.textContent = name;
    const detail = document.createElement("div");
    detail.className = "session-meta";
    detail.textContent = meta;
    main.append(label, detail);
    row.appendChild(main);
    // The thread a turn is running in says so here too: this list is opened to
    // pick a conversation to read, and one of them is still being written. The
    // mark sits on the title's own line, after the text rather than at the row's
    // edge — and outside the label, so a long title's ellipsis cannot clip it.
    if (running) {
      const mark = document.createElement("span");
      mark.className = "session-run";
      const spinner = document.createElement("span");
      spinner.className = "spinner";
      mark.appendChild(spinner);
      row.appendChild(mark);
    }
    // The strings, not the elements they were written into: a tooltip built
    // from a node reads as `[object HTMLDivElement]`.
    row.title = running
      ? t("ui.aTurnIsRunning", "{value0}a turn is running", { value0: meta ? `${name} — ${meta} · ` : `${name} — ` })
      : meta
        ? `${name} — ${meta}`
        : name;
    row.onclick = () => {
      el("sessions-modal").hidden = true;
      selectSessionFromTree(session);
    };
    box.appendChild(row);
  }
}

/// The pull request a thread reported, as its row draws it.
function pullOf(session) {
  return (session.pulls || [])[0] || null;
}

/// The request the thread on screen reported, if any.
function threadPull() {
  const session = (state.sessions || []).find((entry) => entry.id === state.session);
  return session ? pullOf(session) : null;
}

/// The words a state is drawn in before the catalog is asked for them.
const PULL_STATES = { open: "Open", merged: "Merged", closed: "Closed" };

/// A request's state in the reader's language, or nothing while the forge has
/// not been read.
function pullStateLabel(pull) {
  return pull.state
    ? t(`pull.state.${pull.state}`, PULL_STATES[pull.state] || pull.state)
    : "";
}

function pullTitle(pull) {
  const request = t("ui.pullRequest", "Pull request {value0}", { value0: pull.label });
  const state = pullStateLabel(pull);
  return state ? `${request} · ${state}` : request;
}

/// Opens a link in the machine's browser: the window cannot navigate to a
/// remote page itself.
function openExternal(url) {
  invoke("open_url", { url }).catch((error) =>
    showToast(t("ui.couldNotOpenLink", "Could not open link: {value0}", { value0: error }), "error"),
  );
}

/// The mark a thread's row carries for its pull request: colored by state,
/// named by its label, and a click that opens the request.
function pullButton(pull) {
  const button = document.createElement("button");
  button.type = "button";
  button.className = `session-pull state-${pull.state || "unknown"}`;
  button.title = pullTitle(pull);
  button.setAttribute("aria-label", button.title);
  button.innerHTML = ICONS.pull;
  button.onclick = (event) => {
    event.stopPropagation();
    openExternal(pull.url);
  };
  return button;
}

/// How long ago a thread was last written, in the shape the CLI's own picker
/// uses for the same number.
function sessionAge(seconds) {
  if (!seconds) return "";
  const elapsed = Math.max(0, Math.floor(Date.now() / 1000) - Number(seconds));
  if (elapsed < 60) return t("ui.justNow", "just now");
  if (elapsed < 3600) return t("ui.mAgo", "{value0}m ago", { value0: Math.floor(elapsed / 60) });
  if (elapsed < 86400) return t("ui.hAgo", "{value0}h ago", { value0: Math.floor(elapsed / 3600) });
  return t("ui.dAgo", "{value0}d ago", { value0: Math.floor(elapsed / 86400) });
}

function sessionMessages(count) {
  if (count === undefined || count === null) return "";
  return t(Number(count) === 1 ? "session.messageCountOne" : "session.messageCount", Number(count) === 1 ? "{count} message" : "{count} messages", { count });
}

// ---------- slash commands ----------

/// Reads the catalog for the project on screen: the rows the `/` menu draws.
/// The catalog holds one spelling per command, so there is nothing here to
/// resolve a typed name through — the name is the name.
async function loadCommands() {
  const entries = await invoke("list_commands", { project: state.project || "" });
  state.palette = entries || [];
  return state.palette;
}

/// The built-ins this app performs itself. A name that is not here is sent on as
/// a prompt, so a project command, prompt template or skill still reaches the
/// agent through the CLI's own resolution.
async function runSlashCommand(text) {
  const parts = String(text).trim().split(/\s+/);
  const name = parts[0].replace(/^\//, "").toLowerCase();
  const args = parts.slice(1).join(" ").trim();
  switch (name) {
    case "help":
      toggleHelp();
      return true;
    case "mcp":
      // Only the bare command is the app's own: `/mcp list` is an argument the
      // agent may have something to say about, so it stays a prompt.
      if (args) return false;
      if (!state.project) {
        setStatus(t("composer.selectProject", "Select a project first."));
        return true;
      }
      await openMcps();
      return true;
    case "side": {
      if (!state.session || !state.project) {
        setStatus(t("side.noThread", "Open a chat first: a side chat starts from one."));
        return true;
      }
      try {
        const forked = await invoke("fork_session", {
          project: state.project,
          id: state.session,
          name: t("side.name", "Side chat"),
        });
        clearPrediction();
        await newChatTab();
        await openSession(forked);
        setStatus(t("side.started", "This is a side chat: it starts from the chat you were in and does not join it."));
      } catch (error) {
        setStatus(t("side.failed", "Could not start a side chat: {value0}", { value0: error }), "error");
      }
      return true;
    }
    case "model":
      await openModels();
      return true;
    case "agent":
      await openAgents();
      return true;
    case "theme":
      await openThemes();
      return true;
    case "language": {
      // The bare command opens the picker the globe opens; a tag typed after it
      // is the switch outright, which is what a script or a front-end without a
      // dialog wants.
      const wanted = args.trim();
      if (!wanted) {
        openLanguage();
      } else if (wanted.toLowerCase() === state.language.toLowerCase()) {
        setStatus(t("language.current", "Language: {language}", { language: state.language }));
      } else {
        await switchLanguage(wanted);
      }
      return true;
    }
    case "permissions":
      await openPermissions();
      return true;
    case "trust":
      if (state.trust) showTrust(state.trust);
      else showToast(t("ui.nothingToDecideThisProjectNeedsNoTrustDecision", "Nothing to decide — this project needs no trust decision."));
      return true;
    case "connect":
      await openConnect();
      if (args) {
        const wanted = args.toLowerCase();
        const match = state.providers.find(
          (provider) =>
            provider.name === wanted || provider.label.toLowerCase() === wanted,
        );
        if (match) {
          // A named provider is put in the search box, so a row far down a long
          // list is the one on screen rather than selected somewhere off it.
          state.providerName = match.name;
          el("provider-filter").value = match.name;
          renderProviders();
        } else {
          el("login-key").focus();
        }
      }
      return true;
    case "logout": {
      if (!state.providers.length) state.providers = await invoke("list_providers");
      const stored = state.providers.filter((entry) => entry.stored);
      const wanted = args.toLowerCase();
      const target = args
        ? stored.find(
            (entry) => entry.name === wanted || entry.label.toLowerCase() === wanted,
          )
        : stored.length === 1
          ? stored[0]
          : null;
      if (!target) {
        setStatus(
          stored.length
            ? t("ui.nameAProviderLogout", "Name a provider: /logout {value0}", { value0: stored.map((entry) => entry.name).join("|") })
            : t("ui.noProviderIsConnected", "No provider is connected."),
        );
        return true;
      }
      const ok = await confirmDialog(
        t("ui.signOut", "Sign out"),
        t("ui.forgetTheStoredCredentialFor", "Forget the stored credential for {value0}?", { value0: target.label }),
        t("ui.signOut", "Sign out"),
      );
      if (!ok) return true;
      await invoke("logout", { provider: target.name });
      await loadInfo();
      showToast(t("ui.signedOutOf", "Signed out of {value0}", { value0: target.label }));
      return true;
    }
    case "session":
      // Only the bare command is the app's own: `/session <id>` is passed on as
      // a prompt, the way `/mcp list` is.
      if (args) return false;
      await openSessions();
      return true;
    case "new":
      // A new thread needs a folder to be new in, so the command asks for one
      // when none is open rather than naming a project the reader never picked.
      await newChatFromSidebar();
      return true;
    case "reasoning":
      // The bare command opens the picker the chip opens; a level typed after it
      // is a pick outright — the same bargain as clicking its row — which is what
      // a front-end without a dialog needs.
      // The active model's own advertised levels win when its listing carried
      // them, so a typed level the model would clamp away is reported here
      // rather than silently changed by the run.
      if (!args) {
        await openReasoning();
      } else if (reasoningChoices().includes(args)) {
        pickReasoning(args);
      } else {
        setStatus(t("ui.reasoningMustBeOneOf", "Reasoning must be one of {value0}.", { value0: reasoningChoices().join(", ") }));
      }
      return true;
    case "attach":
      el("attach-input").click();
      return true;
    case "usage":
      setStatus(el("usage").textContent ? t("ui.thisChat", "This chat: {value0}", { value0: el("usage").textContent }) : t("ui.noUsageReportedYet", "No usage reported yet."));
      return true;
    default: {
      // A client command the app does not implement yet is still worth naming,
      // rather than sending `/logout` to the model as a prompt — and one the
      // catalog gives to another front-end is named for the one that performs
      // it, since a command this app will never run is not one still to come.
      const builtin = state.palette.find((entry) => entry.kind === "client" && entry.name === name);
      if (!builtin) return false;
      const elsewhere = (builtin.front_ends || [])
        .filter((front) => front !== "desktop")
        .map((front) => FRONT_END_LABELS[front] || front);
      if ((builtin.front_ends || []).length && elsewhere.length) {
        setStatus(t("ui.isTheSCommand", "/{value0} is the {value1}'s command.", { value0: builtin.name, value1: elsewhere.join(" and ") }));
        return true;
      }
      setStatus(t("ui.isNotAvailableInTheDesktopAppYet", "/{value0} is not available in the desktop app yet.", { value0: builtin.name }));
      return true;
    }
  }
}

// ---------- `@path` completion ----------

// The rows the app offered for the reference at the caret, the range of the
// value they replace, which row is highlighted, and the sequence number the
// answer belongs to. The app decides the token and the rows (`oxide_core::at`,
// the same rules the terminal completes with); the view only ever splices in the
// row that was taken and never reads a token itself.
let atRows = [];
let atStart = 0;
let atEnd = 0;
let atIndex = 0;
let atSeq = 0;

/// Closes the list and settles the question that was open, so an answer still
/// on its way (walking a project is not instant) cannot pop the list back up
/// after Escape, or over a box that has been sent and emptied.
function closeAt() {
  const box = el("at-list");
  if (box) box.hidden = true;
  atRows = [];
  atIndex = 0;
  atSeq += 1;
}

function atOpen() {
  const box = el("at-list");
  return Boolean(box) && !box.hidden && atRows.length > 0;
}

/// Asks the app what the caret is sitting in. Nothing is asked of a value with
/// no `@` in it at all, which is most of them.
// A reference starts at a word boundary at or before the caret; without one the
// core has no token to complete, so the round trip is skipped.
function atReferenceBeforeCaret(value, caret) {
  for (let index = value.indexOf("@"); index >= 0 && index < caret; index = value.indexOf("@", index + 1)) {
    if (index === 0 || /\s/.test(value[index - 1])) return true;
  }
  return false;
}

async function requestAt() {
  const prompt = el("prompt");
  if (!prompt || !state.project || !atReferenceBeforeCaret(prompt.value, prompt.selectionStart || 0)) {
    closeAt();
    return;
  }
  const seq = ++atSeq;
  let answer = null;
  try {
    answer = await invoke("at_suggestions", {
      project: state.project,
      text: prompt.value,
      caret: prompt.selectionStart || 0,
    });
  } catch (error) {
    if (seq === atSeq) closeAt();
    return;
  }
  // A list for a value the reader has already typed past would put its rows
  // under a caret that has moved, so the sequence number settles it.
  if (seq !== atSeq) return;
  applyAt(answer);
}

function applyAt(answer) {
  const prompt = el("prompt");
  const start = Number(answer?.start);
  const end = Number(answer?.end);
  const token = prompt && Number.isFinite(start) && Number.isFinite(end)
    ? prompt.value.slice(start, end)
    : "";
  const rows = (answer && answer.rows) || [];
  if (!rows.length) {
    closeAt();
    return;
  }
  atStart = start;
  atEnd = end;
  atRows = rows;
  atIndex = 0;
  renderAt();
}

function renderAt() {
  const box = el("at-list");
  const list = el("at-rows");
  const prompt = el("prompt");
  if (!box || !list || !prompt) return;
  if (!atRows.length) {
    box.hidden = true;
    return;
  }
  // Anchored to the message box the reference is being typed in, growing
  // upward from it, like the command palette.
  const rect = prompt.getBoundingClientRect();
  box.style.left = `${rect.left}px`;
  box.style.width = `${rect.width}px`;
  box.style.top = `${Math.max(12, rect.top - 8)}px`;
  box.style.transform = "translateY(-100%)";
  box.hidden = false;
  list.innerHTML = "";
  atRows.forEach((row, index) => {
    const item = document.createElement("button");
    item.type = "button";
    item.className = "palette-item at-row" + (index === atIndex ? " active" : "");
    item.setAttribute("role", "option");
    item.setAttribute("aria-selected", index === atIndex ? "true" : "false");
    item.title =
      row.kind === "folder"
        ? t("ui.aFolderInThisProject", "{value0} — a folder in this project", { value0: row.label })
        : t("ui.referencedInThisMessage", "{value0} — referenced in this message", { value0: row.label });
    item.innerHTML =
      `<span class="at-glyph">${row.kind === "folder" ? "\u25b8" : "\u00b7"}</span>` +
      `<span class="at-path">${escapeHtml(row.label)}</span>`;
    // The press would take the focus out of the message box and drop the caret
    // the row is completing, so it is swallowed and the click still arrives.
    item.addEventListener("mousedown", (event) => event.preventDefault());
    item.onclick = () => acceptAt(index);
    list.appendChild(item);
  });
}

/// Takes a row: the reference is replaced by what the app said it stands for,
/// with a space after a file so the next word can be typed and without one after
/// a folder, so the query goes on narrowing inside it.
function acceptAt(index) {
  const row = atRows[index];
  const prompt = el("prompt");
  if (!row || !prompt) return;
  const value = prompt.value;
  // A range the value has moved out from under would splice the row into the
  // middle of another word, so a stale list closes instead.
  if (!value.slice(atStart, atEnd).startsWith("@")) {
    closeAt();
    return;
  }
  prompt.value = value.slice(0, atStart) + row.insert + value.slice(atEnd);
  const caret = atStart + row.insert.length;
  prompt.setSelectionRange(caret, caret);
  closeAt();
  prompt.style.height = "auto";
  prompt.style.height = `${Math.min(prompt.scrollHeight, 220)}px`;
  updateSendState();
  // A folder opens its own list; a file's reference is done, which the app
  // answers with no rows.
  requestAt();
}

function moveAt(step) {
  if (!atRows.length) return;
  atIndex = (atIndex + step + atRows.length) % atRows.length;
  renderAt();
  const selected = el("at-rows").children[atIndex];
  // Only the row is brought into view, and only if it is not already: the list
  // is its own scroller.
  if (selected && selected.scrollIntoView) selected.scrollIntoView({ block: "nearest" });
}

/// The keys the list owns while it is open, so typing a reference does not send
/// half of it. Returns true when the key was consumed.
function atKey(event) {
  if (!atOpen()) return false;
  if (event.key === "ArrowDown" || event.key === "ArrowUp") {
    moveAt(event.key === "ArrowDown" ? 1 : -1);
    return true;
  }
  if (event.key === "Tab" || (event.key === "Enter" && !event.shiftKey)) {
    acceptAt(atIndex);
    return true;
  }
  if (event.key === "Escape") {
    closeAt();
    return true;
  }
  return false;
}

/// The keys that move the caret without changing the value, so a reference the
/// caret has been moved into completes too.
const CARET_KEYS = new Set([
  "ArrowLeft",
  "ArrowRight",
  "ArrowUp",
  "ArrowDown",
  "Home",
  "End",
  "PageUp",
  "PageDown",
]);

// ---------- command palette ----------

function openPalette() {
  const box = el("command-palette");
  const prompt = el("prompt");
  if (!box || !prompt || !prompt.offsetParent) return;
  const rect = prompt.getBoundingClientRect();
  box.style.left = `${rect.left}px`;
  box.style.width = `${rect.width}px`;
  box.style.top = `${Math.max(12, rect.top - 8)}px`;
  box.style.transform = "translateY(-100%)";
  box.hidden = false;
  state.paletteOpen = true;
  renderPalette();
}

function closePalette() {
  const box = el("command-palette");
  if (box) box.hidden = true;
  state.paletteOpen = false;
}

/// The entries the slot can become, loaded when the palette opens rather than
/// kept warm: the list depends on the selected project's own commands.
async function refreshPaletteEntries() {
  try {
    await loadCommands();
  } catch (error) {
    state.palette = state.palette || [];
  }
}

/// The front-ends the catalog names a command for, as the core labels them.
const FRONT_END_LABELS = {
  terminal: "terminal",
  desktop: t("ui.desktopApp", "desktop app"),
  panel: t("ui.vSCodePanel", "VS Code panel"),
};

/// Whether this app is one of the front-ends a command is offered to. A row the
/// app cannot perform would otherwise be a `/agent` in its menu answered by a
/// note saying it is not here.
function offeredHere(entry) {
  const frontEnds = entry.front_ends || [];
  // A CLI that predates the field: the desktop's own names were marked then, so
  // nothing is left out on their account.
  if (!frontEnds.length) return true;
  return frontEnds.includes("desktop");
}

function paletteMatches() {
  const text = el("prompt").value;
  if (!text.startsWith("/")) return null;
  const query = text.slice(1).toLowerCase();
  if (/\s/.test(query)) return null;
  return state.palette.filter(
    (entry) => offeredHere(entry) && entry.name.toLowerCase().includes(query),
  );
}

function renderPalette() {
  const list = el("palette-list");
  const matches = paletteMatches();
  if (!matches) {
    closePalette();
    return;
  }
  list.innerHTML = "";
  if (!matches.length) {
    list.innerHTML = `<div class="palette-empty">${escapeHtml(t("desktop.empty.commands", "No matching command."))}</div>`;
    return;
  }
  state.paletteIndex = Math.min(state.paletteIndex, matches.length - 1);
  matches.forEach((entry, index) => {
    const row = document.createElement("button");
    row.type = "button";
    row.className = "palette-item" + (index === state.paletteIndex ? " active" : "");
    row.setAttribute("role", "option");
    // A skill's own kind is worth saying: it is loaded as instructions rather
    // than run as a command, and the CLI keeps the two apart.
    const badge = entry.kind === "skill" ? "skill" : entry.source;
    row.innerHTML =
      `<span class="cmd">/${escapeHtml(entry.name)}</span>` +
      (entry.arguments ? `<span class="args">${escapeHtml(entry.arguments)}</span>` : "") +
      `<span class="desc">${escapeHtml(entry.description)}</span>` +
      `<span class="source">${escapeHtml(badge)}</span>`;
    row.onclick = () => {
      state.paletteIndex = index;
      runPaletteEntry(entry);
    };
    list.appendChild(row);
  });
}

/// Choosing an entry runs it when it takes no arguments beyond what the app
/// knows, and otherwise completes it in the composer so the arguments can be
/// typed before it is sent — the same split the catalog describes.
function runPaletteEntry(entry) {
  closePalette();
  const prompt = el("prompt");
  if (entry.kind === "client" && !entry.arguments) {
    prompt.value = "";
    updateSendState();
    // The command is performed as it is taken, and the answer is handed back so
    // a caller that needs to know what it did can wait for it.
    return runSlashCommand(`/${entry.name}`);
  }
  prompt.value = `/${entry.name}${entry.arguments ? " " : ""}`;
  prompt.focus();
  prompt.style.height = "auto";
  prompt.style.height = `${Math.min(prompt.scrollHeight, 220)}px`;
  updateSendState();
}

/// Moves the selection in the open palette. Returns true when the key was
/// consumed so the composer does not also act on it.
function paletteKey(event) {
  const matches = paletteMatches();
  if (!state.paletteOpen || !matches) return false;
  if (event.key === "ArrowDown" || event.key === "ArrowUp") {
    const step = event.key === "ArrowDown" ? 1 : -1;
    state.paletteIndex =
      (state.paletteIndex + step + matches.length) % Math.max(1, matches.length);
    renderPalette();
    return true;
  }
  if ((event.key === "Enter" && !event.shiftKey) || event.key === "Tab") {
    if (!matches.length) return false;
    runPaletteEntry(matches[state.paletteIndex] || matches[0]);
    return true;
  }
  return false;
}

// ---------- preferences, shortcuts and the command menu ----------

/// What the window behaves like before the store has answered, and what a
/// preference this build has no control for falls back to. The names are the
/// store's own, so a patch travels to `write_prefs` untouched.
const DEFAULT_PREFS = {
  send_key: "enter",
  follow_up: "queue",
  font_scale: 1,
  terminal_font: "Oxide Terminal",
  terminal_font_size: 12.5,
  sidebar_visible: true,
  predictions: false,
  notifications: "background",
  notify_approvals: true,
  notify_questions: true,
  keep_awake: false,
  shortcuts: {},
};

const MIN_FONT_SCALE = 0.7;
const MAX_FONT_SCALE = 1.6;
const MIN_TERMINAL_FONT_SIZE = 8;
const MAX_TERMINAL_FONT_SIZE = 28;

state.prefs = { ...DEFAULT_PREFS };
state.commandRows = [];
state.commandIndex = 0;
state.shortcutRecording = null;
state.navStack = [];
state.navIndex = -1;
state.navigating = false;
state.closedTabs = [];

function clampFontScale(value) {
  const scale = Math.round(Number(value) * 10) / 10;
  if (!Number.isFinite(scale)) return 1;
  return Math.min(MAX_FONT_SCALE, Math.max(MIN_FONT_SCALE, scale));
}

function clampTerminalFontSize(value) {
  const size = Math.round(Number(value) * 2) / 2;
  if (!Number.isFinite(size)) return 12.5;
  return Math.min(MAX_TERMINAL_FONT_SIZE, Math.max(MIN_TERMINAL_FONT_SIZE, size));
}

/// The store's answer, made safe to paint: a field this build cannot use is
/// left at the value the window ships with rather than drawn as it arrived.
function normalizePrefs(raw) {
  const prefs = { ...DEFAULT_PREFS, shortcuts: {} };
  if (!raw || typeof raw !== "object") return prefs;
  if (raw.send_key === "mod-enter" || raw.send_key === "enter") prefs.send_key = raw.send_key;
  if (raw.follow_up === "steer" || raw.follow_up === "queue") prefs.follow_up = raw.follow_up;
  prefs.font_scale = clampFontScale(raw.font_scale);
  if (typeof raw.terminal_font === "string" && raw.terminal_font.trim()) {
    prefs.terminal_font = raw.terminal_font.trim();
  }
  prefs.terminal_font_size = clampTerminalFontSize(raw.terminal_font_size);
  if (typeof raw.sidebar_visible === "boolean") prefs.sidebar_visible = raw.sidebar_visible;
  if (typeof raw.predictions === "boolean") prefs.predictions = raw.predictions;
  if (["off", "background", "always"].includes(raw.notifications)) prefs.notifications = raw.notifications;
  if (typeof raw.notify_approvals === "boolean") prefs.notify_approvals = raw.notify_approvals;
  if (typeof raw.notify_questions === "boolean") prefs.notify_questions = raw.notify_questions;
  if (typeof raw.keep_awake === "boolean") prefs.keep_awake = raw.keep_awake;
  if (raw.shortcuts && typeof raw.shortcuts === "object") {
    for (const [action, chord] of Object.entries(raw.shortcuts)) {
      const text = typeof chord === "string" ? chord.trim().toLowerCase() : "";
      if (action.trim() && text) prefs.shortcuts[action] = text;
    }
  }
  return prefs;
}

async function loadPrefs() {
  let answer = null;
  try {
    answer = await invoke("read_prefs", {});
  } catch {
    answer = null;
  }
  state.prefs = normalizePrefs(answer);
  state.followUpBehavior = state.prefs.follow_up;
  applyPrefs();
}

/// Puts the preferences into the window: the sidebar's own state, the scale the
/// whole page is drawn at, and the controls the settings sheet shows. The store
/// is written beside this, so what is painted is what the next launch reads.
async function savePrefs(patch) {
  const shortcuts = { ...state.prefs.shortcuts };
  for (const [action, chord] of Object.entries(patch.shortcuts || {})) {
    if (chord === null || chord === undefined) delete shortcuts[action];
    else shortcuts[action] = String(chord).trim().toLowerCase();
  }
  state.prefs = normalizePrefs({ ...state.prefs, ...patch, shortcuts });
  if (Object.prototype.hasOwnProperty.call(patch, "follow_up")) {
    state.followUpBehavior = state.prefs.follow_up;
    paintQueue();
    paintLiveChanges();
    updateSendState();
  }
  applyPrefs();
  try {
    const answer = await invoke("write_prefs", { patch });
    if (answer) {
      state.prefs = normalizePrefs(answer);
      applyPrefs();
    }
  } catch (error) {
    showToast(
      t("settings.couldNotSave", "Could not save that setting: {value0}", { value0: error }),
      "error",
    );
  }
}

function applyPrefs() {
  document.body.classList.toggle("sidebar-hidden", !state.prefs.sidebar_visible);
  el("sidebar-toggle").setAttribute("aria-expanded", String(state.prefs.sidebar_visible));
  void invoke("set_zoom", { scale: state.prefs.font_scale }).catch(() => {});
  const terminalFont = state.prefs.terminal_font || "Oxide Terminal";
  document.body.style.setProperty(
    "--terminal-font",
    `${terminalFont}, "Oxide Terminal", ui-monospace, SFMono-Regular, Menlo, Consolas, monospace`,
  );
  document.body.style.setProperty(
    "--terminal-font-size",
    `${clampTerminalFontSize(state.prefs.terminal_font_size)}px`,
  );
  void document.fonts
    ?.load?.(`${clampTerminalFontSize(state.prefs.terminal_font_size)}px "${terminalFont}"`)
    ?.then?.(() => fitTerminal())
    ?.catch?.(() => {});
  paintSettingsControls();
  fitTerminal();
}

function resizeText(step) {
  void savePrefs({ font_scale: step === 0 ? 1 : clampFontScale(state.prefs.font_scale + step * 0.1) });
}

function toggleSidebar() {
  void savePrefs({ sidebar_visible: !state.prefs.sidebar_visible });
}

/// The actions the window performs from the keyboard, in the order the settings
/// sheet lists them. `chords` are the bindings it ships with and the reader's
/// override replaces them whole, so this table is the only list: the sheet, the
/// dispatcher and the menu the window asks for all read it.
const SHORTCUTS = [
  {
    id: "focus-active",
    key: "settings.actionFocusActive",
    fallback: "Focus on active chats",
    chords: ["mod+alt+u"],
    run: () => toggleActiveFocus(),
  },
  {
    id: "command-menu",
    key: "settings.actionCommandMenu",
    fallback: "Command menu",
    chords: ["mod+k", "mod+shift+p"],
    run: () => openCommandMenu(),
  },
  {
    id: "settings",
    key: "settings.actionSettings",
    fallback: "Open settings",
    chords: ["mod+,"],
    run: () => openSettings(),
  },
  {
    id: "find",
    key: "settings.actionFind",
    fallback: "Find in this chat",
    chords: ["mod+f"],
    run: () => openFind(),
  },
  {
    id: "search-chats",
    key: "settings.actionSearchChats",
    fallback: "Search chats",
    chords: ["mod+shift+f"],
    run: () => openSearch(),
  },
  {
    id: "new-tab",
    key: "settings.actionNewTab",
    fallback: "New tab",
    chords: ["mod+t"],
    run: () => newChatTab(),
  },
  {
    id: "close-tab",
    key: "settings.actionCloseTab",
    fallback: "Close tab",
    chords: ["mod+w"],
    run: () => closeChatTab(activeTabId),
  },
  {
    id: "reopen-tab",
    key: "settings.actionReopenTab",
    fallback: "Reopen closed tab",
    chords: ["mod+shift+t"],
    run: () => reopenClosedTab(),
  },
  {
    id: "close-other-tabs",
    key: "settings.actionCloseOtherTabs",
    fallback: "Close other tabs",
    chords: ["mod+alt+w"],
    run: () => closeOtherTabs(),
  },
  {
    id: "next-tab",
    key: "settings.actionNextTab",
    fallback: "Next tab",
    chords: ["ctrl+tab"],
    run: () => walkTabs(1),
  },
  {
    id: "previous-tab",
    key: "settings.actionPreviousTab",
    fallback: "Previous tab",
    chords: ["ctrl+shift+tab"],
    run: () => walkTabs(-1),
  },
  {
    id: "toggle-panel",
    key: "settings.actionTogglePanel",
    fallback: "Show or hide the panel",
    chords: ["mod+j"],
    run: () => togglePanel(),
  },
  {
    id: "terminal",
    key: "settings.actionTerminal",
    fallback: "Open the terminal",
    chords: ["ctrl+`", "ctrl+~"],
    run: () => openTerminal(),
  },
  {
    id: "split-terminal-right",
    key: "settings.actionSplitTerminalRight",
    fallback: "Split terminal right",
    chords: ["mod+d"],
    run: () => splitTerminal("row"),
  },
  {
    id: "split-terminal-down",
    key: "settings.actionSplitTerminalDown",
    fallback: "Split terminal down",
    chords: ["mod+shift+d"],
    run: () => splitTerminal("column"),
  },
  {
    id: "toggle-sidebar",
    key: "settings.actionToggleSidebar",
    fallback: "Show or hide the sidebar",
    chords: ["mod+b"],
    run: () => toggleSidebar(),
  },
  {
    id: "back",
    key: "settings.actionBack",
    fallback: "Back to the previous chat",
    chords: ["mod+["],
    run: () => navBack(),
  },
  {
    id: "forward",
    key: "settings.actionForward",
    fallback: "Forward to the next chat",
    chords: ["mod+]"],
    run: () => navForward(),
  },
  {
    id: "open-folder",
    key: "settings.actionOpenFolder",
    fallback: "Open a folder",
    chords: ["mod+o"],
    run: () => openProjects(),
  },
  {
    id: "font-larger",
    key: "settings.actionLargerText",
    fallback: "Larger text",
    chords: ["mod+=", "mod+shift+="],
    run: () => resizeText(1),
  },
  {
    id: "font-smaller",
    key: "settings.actionSmallerText",
    fallback: "Smaller text",
    chords: ["mod+-"],
    run: () => resizeText(-1),
  },
  {
    id: "font-reset",
    key: "settings.actionResetText",
    fallback: "Reset text size",
    chords: ["mod+0"],
    run: () => resizeText(0),
  },
  {
    id: "cycle-reasoning",
    key: "settings.actionCycleReasoning",
    fallback: "Cycle reasoning effort",
    chords: ["ctrl+r"],
    run: () => cycleReasoning(),
  },
  {
    id: "shortcut-help",
    key: "settings.actionShortcutHelp",
    fallback: "Keyboard shortcuts",
    chords: ["ctrl+/"],
    run: () => toggleHelp(),
  },
  {
    id: "clear-unread",
    key: "settings.actionClearUnread",
    fallback: "Clear unread marks",
    chords: ["shift+escape"],
    run: () => clearUnread(),
  },
];

/// How a binding is spelled in the store: the modifiers in one order, then the
/// key. `mod` is what every shortcut has always meant here — ⌘ on a Mac and
/// Ctrl everywhere else, either of them accepted — so a binding recorded with
/// Ctrl keeps working with ⌘.
function parseChord(chord) {
  const parts = String(chord).trim().toLowerCase().split("+");
  const key = parts.pop() || "";
  const parsed = { mod: false, ctrl: false, shift: false, alt: false, key };
  for (const part of parts) {
    if (part === "mod") parsed.mod = true;
    else if (part === "ctrl" || part === "control") parsed.ctrl = true;
    else if (part === "shift") parsed.shift = true;
    else if (part === "alt" || part === "opt" || part === "option") parsed.alt = true;
  }
  return parsed;
}

function canonicalChord(chord) {
  const parts = parseChord(chord);
  return `${parts.mod ? 1 : 0}${parts.ctrl ? 1 : 0}${parts.shift ? 1 : 0}${parts.alt ? 1 : 0}:${parts.key}`;
}

function isMac() {
  return /mac/i.test(navigator.userAgentData?.platform || navigator.platform || "");
}

/// A chord as the reader reads it: `⌘⇧K` on a Mac, `Ctrl+Shift+K` elsewhere.
function formatChord(chord) {
  const parts = parseChord(chord);
  const mac = isMac();
  const keys = { escape: "Esc", tab: "Tab", enter: "Enter", space: "Space", "[": "[", "]": "]" };
  const label = keys[parts.key] || (parts.key.length === 1 ? parts.key.toUpperCase() : parts.key);
  const tokens = [];
  if (parts.ctrl) tokens.push(mac ? "⌃" : "Ctrl");
  if (parts.mod) tokens.push(mac ? "⌘" : "Ctrl");
  if (parts.alt) tokens.push(mac ? "⌥" : "Alt");
  if (parts.shift) tokens.push(mac ? "⇧" : "Shift");
  tokens.push(label);
  return mac ? tokens.join("") : tokens.join("+");
}

function chordMatches(chord, event) {
  const parts = parseChord(chord);
  if (!parts.key) return false;
  const mod = Boolean(event.metaKey || event.ctrlKey);
  // `mod` is either of the platform's modifiers; a chord that names `ctrl`
  // itself asks for that one key, so `Ctrl+R` and `⌘R` stay different bindings.
  if (parts.mod && !mod) return false;
  if (!parts.mod && !parts.ctrl && mod) return false;
  if (parts.ctrl && !event.ctrlKey) return false;
  if (parts.alt !== Boolean(event.altKey)) return false;
  // A shifted symbol already carries Shift in its character. Ctrl+~, for
  // example, arrives as Ctrl+Shift+`, but Shortcuts names the key people type.
  const impliedShift = parts.key === "~" && !parts.shift && event.shiftKey;
  if (parts.shift !== Boolean(event.shiftKey) && !impliedShift) return false;
  if (String(event.key).toLowerCase() === parts.key) return true;
  // Option rewrites the character on a Mac (⌥U is a dead accent key), so an
  // Alt chord falls back to the physical key it names.
  return parts.alt && parts.key.length === 1 && event.code === `Key${parts.key.toUpperCase()}`;
}

function shortcutChords(action) {
  const override = state.prefs.shortcuts[action.id];
  return override ? [override] : action.chords;
}

/// The action a key event performs, or `null` when nothing in the table claims
/// it — which leaves the composer's own keys and the platform's text editing
/// alone.
function matchShortcut(event) {
  for (const action of SHORTCUTS) {
    for (const chord of shortcutChords(action)) {
      if (chordMatches(chord, event)) return action;
    }
  }
  return null;
}

function shortcutConflict(chord, exceptId) {
  const wanted = canonicalChord(chord);
  for (const action of SHORTCUTS) {
    if (action.id === exceptId) continue;
    for (const other of shortcutChords(action)) {
      if (canonicalChord(other) === wanted) return action;
    }
  }
  return null;
}

/// The binding a key press should be recorded as, or `null` for the modifiers on
/// their own and for Escape, which leaves the row as it was.
function chordFromEvent(event) {
  const key = String(event.key).toLowerCase();
  if (!key || ["meta", "control", "shift", "alt", "escape"].includes(key)) return null;
  const parts = [];
  if (event.metaKey || event.ctrlKey) parts.push("mod");
  if (event.altKey) parts.push("alt");
  if (event.shiftKey) parts.push("shift");
  parts.push(key === " " ? "space" : key);
  return parts.join("+");
}

/// What the window's menu bar should show beside each item. The page is the one
/// that knows which binding is in force, so it tells the window rather than the
/// menu keeping a second copy of the table.
function announceShortcuts() {
  const bindings = {};
  for (const action of SHORTCUTS) {
    const chord = shortcutChords(action)[0];
    if (chord) bindings[action.id] = chord;
  }
  void invoke("set_native_shortcuts", { bindings }).catch(() => {});
}

// ---------- navigation history ----------

/// The chats this window has shown, in the order it showed them, so `⌘[` and
/// `⌘]` walk back and forth the way a browser does. Only a thread is a place:
/// a draft tab has nothing to go back to.
function pushNav(session) {
  if (state.navigating || !session?.id) return;
  const current = state.navStack[state.navIndex];
  if (current?.id === session.id) return;
  state.navStack = state.navStack.slice(0, state.navIndex + 1);
  state.navStack.push(session);
  if (state.navStack.length > 40) state.navStack.shift();
  state.navIndex = state.navStack.length - 1;
  paintNavigation();
}

function paintNavigation() {
  el("nav-back").disabled = !customizeView && el("settings-modal").hidden && state.navIndex <= 0;
  el("nav-forward").disabled = state.navIndex >= state.navStack.length - 1;
}

async function navTo(index) {
  closeCustomize();
  const session = state.navStack[index];
  if (!session) return;
  state.navIndex = index;
  paintNavigation();
  state.navigating = true;
  try {
    await openSession(session);
  } finally {
    state.navigating = false;
  }
}

function navBack() {
  if (!el("projects-page").hidden) { closeProjectDirectory(); return; }
  if (!el("settings-modal").hidden) { closeSettings(); return; }
  if (customizeView) { closeCustomize(); return; }
  if (state.navIndex > 0) void navTo(state.navIndex - 1);
}

function navForward() {
  if (state.navIndex < state.navStack.length - 1) void navTo(state.navIndex + 1);
}

function clearUnread() {
  for (const session of state.sessions) session.unread = false;
  for (const record of threads.values()) record.unread = false;
  renderProjectsTree();
  paintChatTabs();
  paintActivityDot();
}

let settingsThemeProbe = 0;
async function loadSettingsThemes() {
  const probe = ++settingsThemeProbe;
  const project = themeProject();
  const box = el("settings-theme-list");
  box.innerHTML = "<p>Loading themes…</p>";
  try {
    const themes = await invoke("list_themes", { project });
    const entries = await Promise.all((themes.names || []).map(async (name) => {
      const result = await invoke("theme_colors", { project, name });
      return { name, colors: result.colors };
    }));
    if (probe !== settingsThemeProbe || project !== themeProject()) return;
    renderThemes(entries, themes.current, box);
  } catch (error) {
    if (probe === settingsThemeProbe) box.textContent = `Could not load themes: ${error}`;
  }
}

function renderSettingsLanguage() {
  const select = el("settings-language");
  select.innerHTML = "";
  const languages = state.languages.length ? state.languages : [
    { tag: "en-GB", nativeLabel: "English (United Kingdom)" },
    { tag: "zh-CN", nativeLabel: "简体中文" },
  ];
  for (const language of languages) {
    const option = document.createElement("option");
    option.value = language.tag;
    option.textContent = language.nativeLabel || language.label || language.tag;
    select.appendChild(option);
  }
  select.value = state.language;
}

// ---------- settings page ----------

function openSettings(section = "general") {
  closeCustomize();
  closeOverlays();
  el("settings-search").value = "";
  filterSettingsSections("");
  document.body.classList.add("settings-open");
  el("rail-home").removeAttribute("aria-current");
  el("settings").setAttribute("aria-current", "page");
  el("settings-modal").hidden = false;
  el("settings").setAttribute("aria-expanded", "true");
  showSettingsSection(section);
  paintSettingsControls();
  paintNavigation();
  el(SETTINGS_SECTIONS[section]?.tab || "settings-tab-general").focus();
}

function closeSettings() {
  if (el("settings-modal").hidden) return;
  state.shortcutRecording = null;
  el("settings-modal").hidden = true;
  document.body.classList.remove("settings-open");
  el("settings").removeAttribute("aria-current");
  el("rail-home").setAttribute("aria-current", "page");
  paintNavigation();
  el("settings").setAttribute("aria-expanded", "false");
  el("settings").focus();
}

/// The sheet's sections, by the controls that open and show each one. Naming
/// them here rather than walking the markup is what lets a control be checked
/// as wired: every id in this table has a listener by name.
const SETTINGS_SECTIONS = {
  general: { tab: "settings-tab-general", page: "settings-page-general" },
  appearance: { tab: "settings-tab-appearance", page: "settings-page-appearance" },
  notifications: { tab: "settings-tab-notifications", page: "settings-page-notifications" },
  memories: { tab: "settings-tab-memories", page: "settings-page-memories" },
  keyboard: { tab: "settings-tab-keyboard", page: "settings-page-keyboard" },
};

const SETTINGS_NAV = [
  ["settings-group-personal", ["settings-tab-general", "settings-tab-appearance", "settings-tab-notifications"]],
  ["settings-group-integrations", ["settings-open-mcps", "settings-open-plugins", "settings-open-skills"]],
  ["settings-group-workspace", ["settings-tab-memories", "settings-tab-keyboard"]],
];

const SETTINGS_SEARCH_TERMS = {
  "settings-tab-general": "general send enter follow up queue steer",
  "settings-tab-appearance": "appearance theme dark light language font text sidebar",
  "settings-tab-notifications": "notifications alerts approvals questions awake",
  "settings-open-mcps": "mcp servers oauth authorize tools integrations",
  "settings-open-plugins": "plugins extensions marketplace integrations",
  "settings-open-skills": "skills capabilities integrations",
  "settings-tab-memories": "memories remember context workspace",
  "settings-tab-keyboard": "keyboard shortcuts keybindings workspace",
};

function showSettingsSection(section) {
  const chosen = SETTINGS_SECTIONS[section] ? section : "general";
  for (const [name, parts] of Object.entries(SETTINGS_SECTIONS)) {
    const active = name === chosen;
    const tab = el(parts.tab);
    tab.classList.toggle("active", active);
    tab.setAttribute("aria-selected", String(active));
    tab.tabIndex = active ? 0 : -1;
    el(parts.page).hidden = !active;
  }
  state.shortcutRecording = null;
  if (chosen === "appearance") {
    void loadSettingsThemes();
    renderSettingsLanguage();
  }
  if (chosen === "keyboard") renderShortcuts();
  if (chosen === "memories") void renderMemories();
}

function visibleSettingsSections() {
  return Object.entries(SETTINGS_SECTIONS)
    .filter(([, parts]) => !el(parts.tab).hidden)
    .map(([name]) => name);
}

function filterSettingsSections(query) {
  const wanted = String(query || "").trim().toLocaleLowerCase();
  let anyVisible = false;
  for (const [group, tabs] of SETTINGS_NAV) {
    let groupVisible = false;
    for (const id of tabs) {
      const tab = el(id);
      const text = `${tab.textContent || ""} ${tab.dataset.settingsKeywords || ""} ${SETTINGS_SEARCH_TERMS[id] || ""}`.toLocaleLowerCase();
      tab.hidden = Boolean(wanted) && !text.includes(wanted);
      groupVisible ||= !tab.hidden;
    }
    el(group).hidden = !groupVisible;
    anyVisible ||= groupVisible;
  }
  el("settings-search-empty").hidden = anyVisible;
  const shown = visibleSettingsSections();
  if (shown.length && !shown.some((name) => el(SETTINGS_SECTIONS[name].tab).classList.contains("active"))) {
    showSettingsSection(shown[0]);
    el(SETTINGS_SECTIONS[shown[0]].tab).tabIndex = 0;
  }
}

/// The controls the sheet shows, read from the preferences in force rather than
/// from what a click hoped to store.
function paintSettingsControls() {
  if (!el("settings-modal")) return;
  const mark = (node, on) => {
    if (!node) return;
    node.classList.toggle("active", on);
    node.setAttribute("aria-checked", String(on));
  };
  mark(el("settings-send-enter"), state.prefs.send_key === "enter");
  mark(el("settings-send-mod-enter"), state.prefs.send_key === "mod-enter");
  mark(el("settings-follow-queue"), state.prefs.follow_up === "queue");
  mark(el("settings-follow-steer"), state.prefs.follow_up === "steer");
  if (el("settings-font-value")) {
    el("settings-font-value").textContent = `${Math.round(state.prefs.font_scale * 100)}%`;
  }
  if (el("settings-terminal-font")) el("settings-terminal-font").value = state.prefs.terminal_font;
  if (el("settings-terminal-size")) {
    el("settings-terminal-size").value = String(clampTerminalFontSize(state.prefs.terminal_font_size));
  }
  if (el("settings-sidebar")) el("settings-sidebar").checked = state.prefs.sidebar_visible;
  if (el("settings-predictions")) el("settings-predictions").checked = Boolean(state.prefs.predictions);
  for (const mode of ["off", "background", "always"]) {
    const node = el(`settings-notify-${mode}`);
    if (!node) continue;
    const on = state.prefs.notifications === mode;
    node.classList.toggle("active", on);
    node.setAttribute("aria-checked", String(on));
  }
  if (el("settings-notify-approvals")) el("settings-notify-approvals").checked = state.prefs.notify_approvals;
  if (el("settings-notify-questions")) el("settings-notify-questions").checked = state.prefs.notify_questions;
  if (el("settings-keep-awake")) el("settings-keep-awake").checked = state.prefs.keep_awake;
  el("settings").setAttribute("aria-expanded", String(!el("settings-modal").hidden));
}

/// The keyboard page: every action in the table, the binding in force, and how
/// to put it back. The row being recorded says so instead of its keys.
function renderShortcuts() {
  const list = el("shortcut-list");
  if (!list) return;
  list.replaceChildren();
  for (const action of SHORTCUTS) {
    const row = document.createElement("div");
    row.className = "shortcut-row";
    row.dataset.action = action.id;
    const name = document.createElement("span");
    name.className = "shortcut-name";
    name.textContent = t(action.key, action.fallback);
    row.appendChild(name);
    const binding = document.createElement("button");
    binding.type = "button";
    binding.className = "shortcut-binding" + (state.shortcutRecording === action.id ? " recording" : "");
    binding.dataset.record = action.id;
    binding.textContent = state.shortcutRecording === action.id
      ? t("settings.pressKeys", "Press keys…")
      : formatChord(shortcutChords(action)[0] || "");
    binding.title = t("settings.recordHint", "Press the keys this action should listen for");
    binding.setAttribute("aria-label", `${t(action.key, action.fallback)}: ${binding.textContent}`);
    binding.onclick = () => {
      state.shortcutRecording = state.shortcutRecording === action.id ? null : action.id;
      renderShortcuts();
    };
    row.appendChild(binding);
    const reset = document.createElement("button");
    reset.type = "button";
    reset.className = "icon shortcut-reset";
    reset.dataset.reset = action.id;
    reset.title = t("settings.resetShortcut", "Use the shortcut this action ships with");
    reset.setAttribute("aria-label", reset.title);
    reset.innerHTML = ICONS.undo || "";
    reset.hidden = !state.prefs.shortcuts[action.id];
    reset.onclick = () => {
      state.shortcutRecording = null;
      void savePrefs({ shortcuts: { [action.id]: null } }).then(() => {
        renderShortcuts();
        announceShortcuts();
      });
    };
    row.appendChild(reset);
    list.appendChild(row);
  }
}

/// Records the key press a row is waiting for. The sheet does not take a binding
/// another action already answers to, so a recorded key is always one the
/// window can act on.
function recordShortcut(event) {
  const actionId = state.shortcutRecording;
  if (!actionId) return false;
  if (event.key === "Escape") {
    state.shortcutRecording = null;
    renderShortcuts();
    return true;
  }
  const chord = chordFromEvent(event);
  if (!chord) return false;
  const action = SHORTCUTS.find((entry) => entry.id === actionId);
  const taken = shortcutConflict(chord, actionId);
  if (taken) {
    showToast(
      t("settings.shortcutTaken", "{value0} is already {value1}", {
        value0: formatChord(chord),
        value1: t(taken.key, taken.fallback),
      }),
      "error",
    );
    return true;
  }
  state.shortcutRecording = null;
  void savePrefs({ shortcuts: { [actionId]: chord } }).then(() => {
    renderShortcuts();
    announceShortcuts();
    if (action) setStatus(t("settings.shortcutSaved", "{value0} is now {value1}", {
      value0: t(action.key, action.fallback),
      value1: formatChord(chord),
    }));
  });
  return true;
}

/// The memories page: what the agent was told to keep, read from the store the
/// memory tool writes, with the one action a reader has over an entry.
async function renderMemories() {
  const list = el("memory-list");
  if (!list) return;
  list.replaceChildren();
  let entries = [];
  try {
    entries = (await invoke("memories", { project: state.project || null })) || [];
  } catch (error) {
    const failed = document.createElement("div");
    failed.className = "dialog-empty";
    failed.textContent = t("memories.failed", "Could not read the memories: {value0}", {
      value0: error,
    });
    list.appendChild(failed);
    return;
  }
  if (!entries.length) {
    const empty = document.createElement("div");
    empty.className = "dialog-empty";
    empty.textContent = t("memories.empty", "Nothing has been remembered yet.");
    list.appendChild(empty);
    return;
  }
  for (const entry of entries) {
    const row = document.createElement("div");
    row.className = "memory-row";
    row.dataset.memory = entry.id;
    const body = document.createElement("div");
    body.className = "memory-content";
    body.textContent = entry.content;
    row.appendChild(body);
    const meta = document.createElement("div");
    meta.className = "memory-meta";
    meta.textContent = [
      String(entry.scope || "").toLowerCase(),
      ...(entry.tags || []).map((tag) => `#${tag}`),
    ]
      .filter(Boolean)
      .join(" · ");
    row.appendChild(meta);
    const forget = document.createElement("button");
    forget.type = "button";
    forget.className = "memory-forget";
    forget.dataset.forget = entry.id;
    forget.textContent = t("memories.forget", "Forget");
    forget.onclick = async () => {
      try {
        await invoke("forget_memory", { id: entry.id, project: state.project || null });
        await renderMemories();
      } catch (error) {
        showToast(t("memories.failedToForget", "Could not forget that: {value0}", {
          value0: error,
        }), "error");
      }
    };
    row.appendChild(forget);
    list.appendChild(row);
  }
}

// ---------- the command menu ----------

/// Every action the command menu offers: the window's own, each naming the
/// binding it also answers to. A row that needs a project or a thread says so by
/// doing nothing rather than by opening an empty dialog.
function commandActions() {
  const hasProject = Boolean(state.project);
  const actions = [
    { id: "new-chat", label: t("menu.newChat", "New chat"), hint: "mod+t", run: () => newChatTab() },
    { id: "new-tab", label: t("menu.newTab", "New tab"), hint: null, run: () => newChatTab() },
    { id: "reopen-tab", label: t("menu.reopenTab", "Reopen closed tab"), hint: "mod+shift+t", run: () => reopenClosedTab() },
    { id: "close-tab", label: t("menu.closeTab", "Close tab"), hint: "mod+w", run: () => closeChatTab(activeTabId) },
    { id: "toggle-sidebar", label: t("menu.toggleSidebar", "Show or hide the sidebar"), hint: "mod+b", run: () => toggleSidebar() },
    { id: "back", label: t("menu.back", "Back to the previous chat"), hint: "mod+[", run: () => navBack() },
    { id: "forward", label: t("menu.forward", "Forward to the next chat"), hint: "mod+]", run: () => navForward() },
    { id: "open-folder", label: t("menu.openFolder", "Open a folder…"), hint: "mod+o", run: () => openProjects() },
    { id: "search", label: t("menu.searchChats", "Search chats…"), hint: "mod+shift+f", run: () => openSearch() },
    { id: "panel", label: t("menu.panel", "Show or hide the panel"), hint: "mod+j", run: () => togglePanel() },
    { id: "terminal", label: t("menu.terminal", "Terminal"), hint: "ctrl+` / ctrl+~", run: () => void openTerminal() },
    { id: "split-terminal-right", label: t("settings.actionSplitTerminalRight", "Split terminal right"), hint: "mod+d", run: () => void splitTerminal("row") },
    { id: "split-terminal-down", label: t("settings.actionSplitTerminalDown", "Split terminal down"), hint: "mod+shift+d", run: () => void splitTerminal("column") },
    { id: "review", label: t("menu.review", "Review the changes"), hint: null, run: () => openRepositoryChanges() },
    { id: "find", label: t("menu.findInChat", "Find in this chat"), hint: "mod+f", run: () => openFind() },
    { id: "activity", label: t("menu.activity", "Activity…"), hint: null, run: () => openActivity() },
    { id: "archived", label: t("menu.archivedChats", "Archived chats…"), hint: null, run: () => openArchived() },
    { id: "projects", label: t("menu.projects", "Manage projects…"), hint: null, run: () => openProjectDirectory() },
    { id: "threads", label: t("menu.threads", "All threads…"), hint: null, run: () => openSessions() },
    { id: "model", label: t("menu.model", "Choose a model…"), hint: null, run: () => openModels() },
    { id: "reasoning", label: t("menu.reasoning", "Reasoning effort…"), hint: null, run: () => openReasoning() },
    { id: "permissions", label: t("menu.permissions", "Tool permissions…"), hint: null, run: () => openPermissions() },
    { id: "mcps", label: t("menu.mcps", "MCP servers…"), hint: null, run: () => openMcps() },
    { id: "theme", label: t("menu.theme", "Theme…"), hint: null, run: () => openThemes() },
    { id: "language", label: t("menu.language", "Language…"), hint: null, run: () => openLanguage() },
    { id: "settings", label: t("menu.settings", "Settings…"), hint: "mod+,", run: () => openSettings() },
    { id: "memories", label: t("menu.memories", "Memories…"), hint: null, run: () => openSettings("memories") },
    { id: "shortcuts", label: t("menu.shortcuts", "Keyboard shortcuts…"), hint: "ctrl+/", run: () => toggleHelp() },
    { id: "larger-text", label: t("menu.largerText", "Larger text"), hint: "mod+=", run: () => resizeText(1) },
    { id: "smaller-text", label: t("menu.smallerText", "Smaller text"), hint: "mod+-", run: () => resizeText(-1) },
    { id: "reset-text", label: t("menu.resetText", "Reset text size"), hint: "mod+0", run: () => resizeText(0) },
    { id: "clear-unread", label: t("menu.clearUnread", "Clear unread marks"), hint: "shift+escape", run: () => clearUnread() },
    { id: "updates", label: t("menu.checkUpdates", "Check for updates…"), hint: null, run: () => openUpdate() },
  ];
  // A row whose action needs a folder is offered only where there is one.
  return actions.filter((action) => hasProject || !["threads", "mcps", "model"].includes(action.id));
}

function commandMatches() {
  const query = String(el("command-input")?.value || "").trim().toLowerCase();
  const all = commandActions();
  if (!query) return all;
  return all.filter((action) => action.label.toLowerCase().includes(query));
}

function openCommandMenu() {
  closeOverlays("command-menu");
  el("command-menu").hidden = false;
  el("command-input").value = "";
  state.commandIndex = 0;
  renderCommandMenu();
  el("command-input").focus();
}

function closeCommandMenu() {
  if (el("command-menu").hidden) return;
  el("command-menu").hidden = true;
}

function renderCommandMenu() {
  const list = el("command-list");
  if (!list) return;
  const rows = commandMatches();
  state.commandRows = rows;
  if (state.commandIndex >= rows.length) state.commandIndex = 0;
  list.replaceChildren();
  if (!rows.length) {
    const empty = document.createElement("div");
    empty.className = "command-empty";
    empty.textContent = t("menu.empty", "No command matches that");
    list.appendChild(empty);
    return;
  }
  rows.forEach((action, index) => {
    const row = document.createElement("button");
    row.type = "button";
    row.className = "command-row" + (index === state.commandIndex ? " active" : "");
    row.setAttribute("role", "option");
    row.setAttribute("aria-selected", String(index === state.commandIndex));
    row.dataset.command = action.id;
    const label = document.createElement("span");
    label.className = "command-label";
    label.textContent = action.label;
    row.appendChild(label);
    if (action.hint) {
      const hint = document.createElement("span");
      hint.className = "command-hint";
      hint.textContent = formatChord(action.hint);
      row.appendChild(hint);
    }
    row.onclick = () => runCommand(action);
    list.appendChild(row);
  });
}

function runCommand(action) {
  closeCommandMenu();
  if (action) void action.run();
}

function walkTabs(step) {
  if (chatTabs.length < 2 || tabChangeBlocked()) return;
  const index = chatTabs.findIndex((tab) => tab.id === activeTabId);
  if (index < 0) return;
  const next = chatTabs[(index + step + chatTabs.length) % chatTabs.length];
  if (next) void activateChatTab(next.id);
}

function reopenClosedTab() {
  const closed = state.closedTabs.pop();
  if (!closed) {
    setStatus(t("tab.nothingToReopen", "No closed tab to reopen."));
    return Promise.resolve();
  }
  return newChatTab().then(async (id) => {
    const tab = chatTabs.find((entry) => entry.id === id);
    if (!tab) return;
    if (closed.session) {
      await openSession(closed.session);
    } else if (closed.draft) {
      el("prompt").value = closed.draft;
      updateSendState();
    }
  });
}

async function closeOtherTabs() {
  for (const tab of [...chatTabs]) {
    if (tab.id !== activeTabId) await closeChatTab(tab.id);
  }
}

// ---------- the add menu, search, activity and the archive ----------

state.searchRows = [];
state.searchIndex = 0;
state.searchSeq = 0;
state.searchError = "";
state.searchPhrase = "";
state.searchTimer = 0;
state.searchCategory = "all";
state.searchPending = false;
let searchChoices = [];
let searchReturnFocus = null;
state.archived = [];
state.findHits = [];
state.findIndex = -1;

// ---------- searching chats ----------

function openSearch() {
  searchReturnFocus = document.activeElement;
  state.searchCategory = "all";
  state.searchPending = false;
  clearTimeout(state.searchTimer);
  closeOverlays("search-modal");
  el("search-modal").hidden = false;
  el("search-input").value = "";
  state.searchRows = [];
  state.searchIndex = 0;
  state.searchSeq += 1;
  state.searchError = "";
  state.searchPhrase = "";
  renderSearchResults();
  el("search-input").focus();
}

function closeSearch() {
  if (el("search-modal").hidden) return;
  el("search-modal").hidden = true;
  clearTimeout(state.searchTimer);
  // A search still on its way is dropped: its answer must not repaint a box that
  // has been put away (or one reopened on another phrase).
  state.searchSeq += 1;
  searchReturnFocus?.focus();
}

/// How long the reader has to stop typing before the store is read: each scan
/// opens every stored thread, so a phrase in the middle of being typed is not
/// worth one.
const SEARCH_SETTLE_MS = 150;

/// Searches the store for a phrase, once the reader has settled on one. Every
/// keystroke would be a read of every stored thread, so the scan waits for a
/// pause, and an answer that is no longer the newest is dropped rather than
/// racing the one that is.
function searchSoon(query) {
  state.searchSeq += 1;
  const seq = state.searchSeq;
  const phrase = String(query || "").trim();
  clearTimeout(state.searchTimer);
  if (!phrase || state.searchCategory === "actions") {
    state.searchPending = false;
    state.searchPhrase = "";
    state.searchRows = [];
    state.searchIndex = 0;
    state.searchError = "";
    renderSearchResults();
    return;
  }
  state.searchRows = [];
  state.searchError = "";
  state.searchPending = true;
  renderSearchResults();
  state.searchTimer = setTimeout(() => void requestSearch(phrase, seq), SEARCH_SETTLE_MS);
}

async function requestSearch(phrase, seq) {
  if (seq !== state.searchSeq) return;
  let answer = [];
  let searchError = "";
  try {
    answer = (await invoke("search_sessions", { query: phrase })) || [];
  } catch (error) {
    answer = [];
    searchError = t("search.failed", "Could not search the stored threads: {value0}", {
      value0: error,
    });
  }
  if (seq !== state.searchSeq) return;
  state.searchPending = false;
  state.searchError = searchError;
  state.searchPhrase = phrase;
  state.searchRows = answer;
  state.searchIndex = 0;
  renderSearchResults();
}

function addSearchChoice(row, run) {
  const index = searchChoices.length;
  row.id = `search-choice-${index}`;
  row.setAttribute("role", "option");
  row.onclick = run;
  searchChoices.push({ row, run });
}

function paintSearchSelection() {
  state.searchIndex = Math.max(0, Math.min(state.searchIndex, searchChoices.length - 1));
  searchChoices.forEach(({ row }, index) => {
    row.classList.toggle("active", index === state.searchIndex);
    row.setAttribute("aria-selected", String(index === state.searchIndex));
  });
  const selected = searchChoices[state.searchIndex]?.row;
  if (selected) el("search-input").setAttribute("aria-activedescendant", selected.id);
  else el("search-input").removeAttribute("aria-activedescendant");
}

function setSearchCategory(category) {
  state.searchCategory = category;
  state.searchIndex = 0;
  searchSoon(el("search-input").value);
}

function renderSearchResults() {
  const list = el("search-results");
  if (!list) return;
  list.replaceChildren();
  searchChoices = [];
  const actions = el("search-actions");
  actions.replaceChildren();
  const query = String(el("search-input").value || "").trim().toLocaleLowerCase();
  const quickActions = [
    { label: t("menu.newChat", "New chat"), icon: ICONS.compose, shortcut: "new-tab", run: () => newChatTab() },
    { label: t("menu.openFolder", "Open folder"), icon: ICONS.folder, shortcut: "open-folder", run: () => openProjects() },
  ];
  if (state.searchCategory !== "chats") for (const action of quickActions) {
    if (query && !action.label.toLocaleLowerCase().includes(query)) continue;
    const row = document.createElement("button");
    row.type = "button";
    row.className = "command-row search-action";
    const icon = document.createElement("span");
    icon.className = "search-action-icon";
    icon.innerHTML = action.icon;
    const label = document.createElement("span");
    label.className = "command-name";
    label.textContent = action.label;
    const hint = document.createElement("span");
    hint.className = "command-hint";
    hint.textContent = formatChord(shortcutChords(SHORTCUTS.find((entry) => entry.id === action.shortcut))[0] || "");
    row.append(icon, label, hint);
    addSearchChoice(row, () => { closeSearch(); return action.run(); });
    actions.appendChild(row);
  }
  el("search-actions-heading").hidden = !searchChoices.length;
  actions.hidden = !searchChoices.length;
  const showChats = state.searchCategory !== "actions";
  el("search-chats-heading").hidden = !showChats;
  list.hidden = !showChats;
  for (const category of ["all", "actions", "chats"]) {
    const button = el(`search-category-${category}`);
    button.classList.toggle("active", category === state.searchCategory);
    button.setAttribute("aria-pressed", String(category === state.searchCategory));
  }
  if (!showChats) {
    if (!searchChoices.length) {
      const empty = document.createElement("div");
      empty.className = "command-empty";
      empty.textContent = t("search.noActions", "No matching actions.");
      actions.appendChild(empty);
      actions.hidden = false;
    }
    paintSearchSelection();
    return;
  }
  const hits = query ? state.searchRows : [...listedSessions()].sort((a, b) => (b.modified_at || b.created_at || 0) - (a.modified_at || a.created_at || 0)).slice(0, 8);
  if (state.searchError) {
    const failed = document.createElement("div");
    failed.className = "command-empty";
    failed.textContent = state.searchError;
    list.appendChild(failed);
    paintSearchSelection();
    return;
  }
  if (!hits.length) {
    const empty = document.createElement("div");
    empty.className = "command-empty";
    empty.textContent = state.searchPending ? t("search.searching", "Searching chats…") : el("search-input")?.value?.trim()
      ? t("search.empty", "No chat carries that")
      : t("search.hint", "Type a phrase from a chat, or a name");
    list.appendChild(empty);
    paintSearchSelection();
    return;
  }
  hits.forEach((hit, index) => {
    const row = document.createElement("button");
    row.type = "button";
    row.className = "command-row search-row" + (index === state.searchIndex ? " active" : "");
    row.setAttribute("role", "option");
    row.setAttribute("aria-selected", String(index === state.searchIndex));
    row.dataset.search = String(index);
    const text = document.createElement("span");
    text.className = "search-text";
    const title = document.createElement("span");
    title.className = "search-title";
    title.textContent = sessionLabel(hit);
    text.appendChild(title);
    const context = document.createElement("span");
    context.className = "search-snippet";
    context.textContent = hit.worktree || sessionProject(hit);
    text.appendChild(context);
    if (hit.snippet) {
      const snippet = document.createElement("span");
      snippet.className = "search-snippet";
      snippet.textContent = hit.snippet;
      text.appendChild(snippet);
    } else if (hit.in_title) {
      const why = document.createElement("span");
      why.className = "search-snippet";
      why.textContent = t("search.matchedName", "The chat's own name carries it");
      text.appendChild(why);
    }
    row.appendChild(text);
    if (hit.matches > 0) {
      const count = document.createElement("span");
      count.className = "command-hint";
      count.textContent = t("search.matchCount", "{count} matches", { count: hit.matches });
      row.appendChild(count);
    }
    const project = document.createElement("span");
    project.className = "search-project";
    project.textContent = projectNameOf(sessionProject(hit));
    row.appendChild(project);
    addSearchChoice(row, () => openSearchRow(hit));
    list.appendChild(row);
  });
  paintSearchSelection();
}

async function openSearchRow(hit) {
  if (!hit) return;
  const phrase = state.searchPhrase;
  closeSearch();
  const seq = state.searchSeq;
  await selectSessionFromTree(hit);
  if (seq !== state.searchSeq || state.session !== hit.id || !phrase || !hit.matches) return;
  el("find-input").value = phrase;
  state.findIndex = -1;
  openFind();
}

// ---------- activity ----------

function activityGroups() {
  const groups = [];
  const priority = { key: "activity.priority", title: t("activity.priority", "Priority"), rows: [] };
  if (state.pendingApproval || state.pendingQuestion) {
    const waiting = state.pendingApproval
      ? t("activity.waitingApproval", "A tool is waiting for your approval")
      : t("activity.waitingQuestion", "A question is waiting for your answer");
    priority.rows.push({ id: "pending", title: waiting, detail: "", open: () => closeActivity() });
  }
  const records = new Map([...threads.values()].filter((record) => record.session).map((record) => [record.session, record]));
  const today = new Date();
  today.setHours(0, 0, 0, 0);
  const yesterday = new Date(today);
  yesterday.setDate(yesterday.getDate() - 1);
  const dated = new Map();
  const sessions = [...listedSessions()].sort((a, b) => (b.modified_at || b.created_at || 0) - (a.modified_at || a.created_at || 0));
  for (const session of sessions) {
    const record = records.get(session.id);
    const row = {
      id: session.id,
      title: sessionLabel(session),
      project: projectNameOf(sessionProject(session)),
      detail: session.preview || "",
      active: session.id === state.session,
      running: Boolean(record?.busy || record?.runId),
      unread: Boolean(record?.unread),
      open: () => void selectSessionFromTree(session),
    };
    if (session.pinned || row.running || row.unread) {
      priority.rows.push(row);
      continue;
    }
    const date = new Date((session.modified_at || session.created_at || 0) * 1000);
    const key = date >= today ? "activity.today" : date >= yesterday ? "activity.yesterday" : "activity.earlier";
    if (!dated.has(key)) dated.set(key, {
      key,
      title: t(key, key === "activity.today" ? "Today" : key === "activity.yesterday" ? "Yesterday" : "Earlier"),
      rows: [],
    });
    dated.get(key).rows.push(row);
  }
  if (priority.rows.length) groups.push(priority);
  groups.push(...dated.values());
  if ([...records.values()].some((record) => record.unread) && priority.rows.length) {
    priority.action = { label: t("activity.clear", "Clear these marks"), run: () => clearUnread() };
  }
  return groups;
}

function openActivity() {
  closeCustomize();
  closeOverlays();
  if (!state.prefs.sidebar_visible) void savePrefs({ sidebar_visible: true });
  el("sidebar-projects").hidden = true;
  el("activity-panel").hidden = false;
  renderActivity();
}

function closeActivity() {
  if (el("activity-panel").hidden) return;
  el("activity-panel").hidden = true;
  el("sidebar-projects").hidden = false;
}

function renderActivity() {
  const body = el("activity-body");
  if (!body) return;
  body.replaceChildren();
  const groups = activityGroups();
  if (!groups.length) {
    const quiet = document.createElement("p");
    quiet.className = "dialog-sub";
    quiet.textContent = t("activity.quiet", "Nothing is waiting on you.");
    body.appendChild(quiet);
    return;
  }
  for (const group of groups) {
    const heading = document.createElement("h3");
    heading.className = "activity-heading";
    heading.textContent = t(group.key, group.title);
    body.appendChild(heading);
    for (const row of group.rows) {
      const button = document.createElement("button");
      button.type = "button";
      button.className = "activity-row" + (row.active ? " active" : "");
      if (row.active) button.setAttribute("aria-current", "page");
      button.dataset.activityRow = String(row.id);
      const head = document.createElement("span");
      head.className = "activity-row-head";
      const title = document.createElement("span");
      title.className = "activity-row-title";
      title.textContent = row.title;
      head.appendChild(title);
      if (row.project) {
        const project = document.createElement("span");
        project.className = "activity-row-project";
        project.textContent = row.project;
        head.appendChild(project);
      }
      if (row.running || row.unread) {
        const mark = document.createElement("span");
        mark.className = row.running ? "spinner" : "activity-unread";
        mark.setAttribute("aria-label", row.running ? t("activity.turnInFlight", "A turn is running") : t("activity.unread", "Finished while you were away"));
        head.appendChild(mark);
      }
      button.appendChild(head);
      if (row.detail) {
        const detail = document.createElement("span");
        detail.className = "activity-row-detail";
        detail.textContent = row.detail;
        button.appendChild(detail);
      }
      button.onclick = row.open;
      body.appendChild(button);
    }
    if (group.action) {
      const action = document.createElement("button");
      action.type = "button";
      action.className = "activity-clear";
      action.textContent = group.action.label;
      action.onclick = group.action.run;
      body.appendChild(action);
    }
  }
}

/// The dot on the activity filter: something the window holds or a thread that
/// is working or finished unread, which is what Activity would list.
function paintActivityDot() {
  const dot = el("activity-dot");
  if (!dot) return;
  const busy = [...threads.values()].some((record) => record.busy || record.unread);
  const waiting = Boolean(state.pendingApproval || state.pendingQuestion);
  dot.hidden = !(busy || waiting);
  el("filter-chats").classList.toggle("asking", waiting);
  paintBadge();
  if (!el("activity-panel").hidden) renderActivity();
}

// ---------- archived chats ----------

function openArchived() {
  closeOverlays("archived-modal");
  el("archived-modal").hidden = false;
  void renderArchived();
}

function closeArchived() {
  el("archived-modal").hidden = true;
}

async function renderArchived() {
  const list = el("archived-list");
  if (!list) return;
  list.replaceChildren();
  let rows = [];
  try {
    rows = (await invoke("archived_sessions")) || [];
  } catch (error) {
    const failed = document.createElement("div");
    failed.className = "dialog-empty";
    failed.textContent = t("archived.failed", "Could not read the archive: {value0}", {
      value0: error,
    });
    list.appendChild(failed);
    return;
  }
  state.archived = rows;
  if (!rows.length) {
    const empty = document.createElement("div");
    empty.className = "dialog-empty";
    empty.textContent = t("archived.empty", "Nothing archived in this window yet.");
    list.appendChild(empty);
    return;
  }
  for (const session of rows) {
    const row = document.createElement("div");
    row.className = "session-item archived-row";
    row.dataset.session = session.id;
    const open = document.createElement("button");
    open.type = "button";
    open.className = "archived-open";
    open.title = t("archived.open", "Open this chat");
    open.setAttribute("aria-label", open.title);
    const title = document.createElement("span");
    title.className = "archived-title";
    title.textContent = sessionLabel(session);
    open.appendChild(title);
    const detail = document.createElement("span");
    detail.className = "archived-detail";
    detail.textContent = `${projectNameOf(sessionProject(session))} · ${sessionAge(session.modified_at)}`;
    open.appendChild(detail);
    open.onclick = () => {
      closeArchived();
      void selectSessionFromTree(session);
    };
    row.appendChild(open);
    const restore = document.createElement("button");
    restore.type = "button";
    restore.className = "archived-restore";
    restore.dataset.unarchive = session.id;
    restore.textContent = t("archived.unarchive", "Unarchive");
    restore.onclick = async () => {
      try {
        state.sessions = await invoke("unarchive_session", { id: session.id, pinned: Boolean(session.pinned) });
        await loadSessions();
        await renderArchived();
      } catch (error) {
        showToast(t("archived.failedToUnarchive", "Could not unarchive the chat: {value0}", {
          value0: error,
        }), "error");
      }
    };
    row.appendChild(restore);
    list.appendChild(row);
  }
}

// ---------- find in chat ----------

function openFind() {
  const bar = el("find-bar");
  if (!bar) return;
  bar.hidden = false;
  const input = el("find-input");
  input.focus();
  input.select?.();
  stepFind(0);
}

function closeFind() {
  const bar = el("find-bar");
  if (!bar || bar.hidden) return;
  bar.hidden = true;
  state.findHits = [];
  state.findIndex = -1;
  // The phrase stays in the box for the next time it is opened; the count is
  // the bar's, and it goes with the bar rather than reading a stale number.
  if (el("find-count")) el("find-count").textContent = "";
  // The highlight is a selection, so putting the bar away drops it rather than
  // leaving a word marked in a conversation nobody is searching.
  window.getSelection?.()?.removeAllRanges?.();
  el("prompt").focus();
}

/// The text a search can land in: the conversation's own text nodes, which is
/// what the reader sees, in the order they see it. A DOM without a walker (the
/// stub the checks run against) has no hits rather than an error.
function* transcriptTextNodes() {
  const root = el("transcript");
  if (!root || typeof document.createTreeWalker !== "function") return;
  const walker = document.createTreeWalker(root, 4, null);
  while (walker.nextNode()) {
    const text = walker.currentNode.nodeValue;
    if (text && text.trim()) yield walker.currentNode;
  }
}

// Case conversion can expand a character (İ → i + combining dot). DOM ranges
// need offsets in the original UTF-16 text, not in its lowercase copy.
function originalFindOffset(source, folded, offset, end = false) {
  if (source.length === folded.length) return offset;
  let original = 0;
  let lower = 0;
  for (const char of source) {
    const next = lower + char.toLowerCase().length;
    if (offset < next) return original + (end && offset > lower ? char.length : 0);
    original += char.length;
    lower = next;
  }
  return original;
}

function findHits(query) {
  const needle = String(query || "").toLowerCase();
  if (!needle) return [];
  const hits = [];
  for (const node of transcriptTextNodes()) {
    const text = node.nodeValue.toLowerCase();
    let from = 0;
    while (hits.length < 500) {
      const at = text.indexOf(needle, from);
      if (at < 0) break;
      hits.push({
        node,
        start: originalFindOffset(node.nodeValue, text, at),
        end: originalFindOffset(node.nodeValue, text, at + needle.length, true),
      });
      if (hits.length >= 500) return hits;
      from = at + Math.max(1, needle.length);
    }
  }
  return hits;
}

function stepFind(step) {
  if (el("find-bar").hidden) return;
  state.findHits = findHits(el("find-input").value);
  if (!state.findHits.length) {
    state.findIndex = -1;
    paintFindCount();
    window.getSelection?.()?.removeAllRanges?.();
    return;
  }
  state.findIndex = state.findIndex < 0
    ? (step < 0 ? state.findHits.length - 1 : 0)
    : (state.findIndex + step + state.findHits.length) % state.findHits.length;
  selectFindHit();
  paintFindCount();
}

/// Marks the match on screen with the window's own selection: nothing in the
/// conversation is rewritten, so a reply still streaming is not disturbed and
/// the highlight goes away with the bar.
function selectFindHit() {
  const hit = state.findHits[state.findIndex];
  if (!hit) return;
  const selection = window.getSelection?.();
  if (selection && typeof document.createRange === "function") {
    try {
      const range = document.createRange();
      range.setStart(hit.node, hit.start);
      range.setEnd(hit.node, hit.end);
      selection.removeAllRanges();
      selection.addRange(range);
    } catch {
      // A node that has been repainted since the scan is simply not marked.
    }
  }
  hit.node.parentElement?.scrollIntoView?.({ block: "center" });
}

function paintFindCount() {
  const count = el("find-count");
  if (!count) return;
  if (!el("find-input")?.value) count.textContent = "";
  else if (!state.findHits.length) count.textContent = t("find.noMatches", "No matches");
  else {
    count.textContent = t("find.position", "{index} of {count}", {
      index: state.findIndex + 1,
      count: state.findHits.length,
    });
  }
}

// ---------- the workspace panel ----------

/// The workspace has one right-hand body, Changes, and an independent bottom
/// Terminal dock. Only one is visible at a time.
const PANEL_BODIES = {
  review: "review-modal",
  terminal: "panel-terminal",
};

state.panelTab = "review";
state.terminal = {
  panes: [],
  focused: null,
  layout: "single",
  ratio: 50,
  opening: false,
  openGeneration: 0,
  paintTimer: 0,
};

function openPanel(tab) {
  const chosen = PANEL_BODIES[tab] ? tab : "review";
  state.panelTab = chosen;
  for (const [name, body] of Object.entries(PANEL_BODIES)) el(body).hidden = name !== chosen;
  const terminalMode = chosen === "terminal";
  el("bottom-panel").classList.toggle("terminal-mode", terminalMode);
  el("terminal-resizer").hidden = !terminalMode;
  if (!terminalMode) setTerminalMaximized(false);
  el("bottom-panel").hidden = false;
  paintDockToggles();
  if (chosen === "terminal") {
    fitTerminal();
    focusedTerminalPane()?.surface.focus();
  }
}

/// Hides the panel and everything in it. The shell goes with it: a terminal the
/// reader cannot see must not keep a process alive.
function hidePanel() {
  el("bottom-panel").hidden = true;
  el("bottom-panel").classList.remove("terminal-mode");
  el("terminal-resizer").hidden = true;
  setTerminalMaximized(false);
  for (const body of Object.values(PANEL_BODIES)) el(body).hidden = true;
  void stopTerminal();
  paintDockToggles();
}

/// The panel is only there while one of its bodies is.
function syncPanel() {
  if (Object.values(PANEL_BODIES).every((body) => el(body).hidden)) {
    el("bottom-panel").hidden = true;
  }
}

function togglePanel(tab = state.panelTab) {
  if (el("bottom-panel").hidden && tab === "terminal") void openTerminal();
  else if (el("bottom-panel").hidden) openPanel(tab);
  else if (state.panelTab === tab) hidePanel();
  else openPanel(tab);
}

// ---------- a file the conversation produced ----------

state.file = { view: null, source: false };

/// Opens one file in the viewer. A markdown or HTML file is drawn and its source
/// is a toggle away; a CSV is a table; an image is the picture; anything else is
/// text.
async function openFile(path) {
  const project = runFolder() || state.project;
  if (!project) return;
  el("file-title").textContent = path;
  el("file-body").replaceChildren(reviewNote(t("file.reading", "Reading {value0}…", { value0: path })));
  el("file-mode").hidden = true;
  closeOverlays("file-modal");
  el("file-modal").hidden = false;
  let view = null;
  try {
    view = await invoke("file_view", { project, path });
  } catch (error) {
    el("file-body").replaceChildren(
      reviewNote(t("file.failed", "Could not read {value0}: {value1}", { value0: path, value1: error })),
    );
    return;
  }
  state.file = { view, source: false };
  paintFile();
  el("file-close").focus();
}

function closeFile() {
  el("file-modal").hidden = true;
  state.file = { view: null, source: false };
}

function paintFile() {
  const { view, source } = state.file;
  const body = el("file-body");
  if (!view) return;
  const toggle = el("file-mode");
  const rendered = view.kind === "markdown" || view.kind === "html";
  toggle.hidden = !rendered;
  toggle.textContent = source ? t("file.preview", "Preview") : t("file.source", "Source");
  if (view.kind === "image") {
    const image = document.createElement("img");
    image.className = "file-image";
    image.src = view.data_url;
    image.alt = view.path;
    body.replaceChildren(image);
    return;
  }
  if (view.kind === "csv" && !source) {
    body.replaceChildren(csvTable(view.text));
    return;
  }
  if (rendered && !source) {
    const preview = document.createElement("div");
    preview.className = "file-rendered";
    // Markdown through the window's own renderer, so a document reads the way a
    // reply does; an HTML file is shown as its text until a sandbox for it is
    // decided, since the page's policy draws no frames at all.
    preview.innerHTML = view.kind === "markdown" ? renderMarkdown(view.text, true) : escapeHtml(view.text);
    body.replaceChildren(preview);
    if (view.kind === "html") {
      body.appendChild(reviewNote(t("file.htmlNotRendered", "An HTML file is shown as its text: this window draws no frames.")));
    }
  } else {
    const pre = document.createElement("pre");
    pre.className = "file-source";
    pre.textContent = view.text;
    body.replaceChildren(pre);
  }
  if (view.truncated) {
    body.appendChild(reviewNote(t("file.truncated", "This file is longer than the viewer reads, so it is cut here.")));
  }
}

function csvTable(text) {
  const table = document.createElement("table");
  table.className = "file-table";
  const rows = String(text)
    .split("\n")
    .filter((line) => line.trim() !== "")
    .slice(0, 500)
    .map((line) => line.split(/,(?=(?:[^"]*"[^"]*")*[^"]*$)/).map((cell) => cell.replace(/^"|"$/g, "")));
  for (const [index, cells] of rows.entries()) {
    const row = document.createElement("tr");
    for (const cell of cells) {
      const node = document.createElement(index === 0 ? "th" : "td");
      node.textContent = cell;
      row.appendChild(node);
    }
    table.appendChild(row);
  }
  return table;
}

// ---------- the system's own notifications ----------

state.badge = -1;
state.awake = false;

/// Whether a finished turn should reach the reader through the system: never,
/// only while the window is not in front, or always. The page is the half that
/// knows whether the window is in front, so it decides and the window speaks.
function notificationsAllowed() {
  if (state.prefs.notifications === "always") return true;
  if (state.prefs.notifications !== "background") return false;
  return typeof document.hasFocus !== "function" || !document.hasFocus();
}

function notifyFinished(label) {
  if (!notificationsAllowed()) return;
  void invoke("notify", {
    title: t("notify.finishedTitle", "Oxide finished a turn"),
    body: t("notify.finishedBody", "{value0} is ready.", { value0: label }),
  }).catch(() => {});
}

/// A tool waiting for approval or a question waiting for an answer is not a
/// finished turn: it has separate switches, and it is announced whether or not
/// the window is in front, since nothing moves until it is answered.
function notifyWaiting(kind, detail) {
  const enabled =
    kind === "approval" ? state.prefs.notify_approvals : state.prefs.notify_questions;
  if (!enabled) return;
  void invoke("notify", {
    title:
      kind === "approval"
        ? t("notify.approvalTitle", "A tool is waiting for approval")
        : t("notify.questionTitle", "A question is waiting for an answer"),
    body: detail || "",
  }).catch(() => {});
}

/// The number on the dock or task bar is the threads that finished unread — the
/// same count the sidebar's dots and activity filter are showing.
function paintBadge() {
  const unread = [...threads.values()].filter((record) => record.unread).length;
  if (unread === state.badge) return;
  state.badge = unread;
  void invoke("set_badge", { count: unread }).catch(() => {});
}

/// A running turn keeps the machine awake when the reader asked it to, and lets
/// it sleep again when the turn ends.
function paintAwake() {
  const on = Boolean(state.busy && state.prefs.keep_awake);
  if (on === state.awake) return;
  state.awake = on;
  void invoke("set_awake", { on }).catch(() => {});
}

// ---------- the next message, offered as a draft ----------

state.prediction = "";

function showPrediction(text) {
  const line = el("prediction");
  if (!line) return;
  if (!text) {
    clearPrediction();
    return;
  }
  state.prediction = text;
  el("prediction-text").textContent = text;
  line.hidden = false;
}

function clearPrediction() {
  state.prediction = "";
  if (el("prediction")) el("prediction").hidden = true;
  if (el("prediction-text")) el("prediction-text").textContent = "";
}

/// Inserts the suggestion as a draft: nothing is sent, and the reader edits it
/// like their own words.
function acceptPrediction() {
  const text = state.prediction;
  if (!text) return;
  const prompt = el("prompt");
  prompt.value = prompt.value.trim() ? `${prompt.value} ${text}` : text;
  prompt.style.height = "auto";
  prompt.style.height = `${Math.min(prompt.scrollHeight, 220)}px`;
  clearPrediction();
  prompt.focus();
  prompt.setSelectionRange(prompt.value.length, prompt.value.length);
  updateSendState();
}

/// Asks for the next message after a turn ends, when the reader has asked for
/// suggestions and the thread on screen is the one that just finished.
async function maybePredict() {
  clearPrediction();
  if (!state.prefs.predictions || !state.session || !state.project) return;
  if (el("prompt").value.trim()) return;
  const project = state.project;
  const id = state.session;
  let suggestion = "";
  try {
    const answer = await invoke("predict", { project, id });
    suggestion = String(answer?.suggestion || "");
  } catch {
    // A suggestion is an offer: a provider that cannot answer is nothing to say.
    return;
  }
  if (state.session !== id || state.project !== project) return;
  if (el("prompt").value.trim()) return;
  if (el("prediction").hidden === false) return;
  showPrediction(suggestion);
}

// ---------- the terminal ----------

function terminalStyle() {
  return { fg: "", bg: "", bold: false, dim: false, underline: false, inverse: false };
}

let terminalStartingNode = null;

function clearTerminalStarting() {
  terminalStartingNode?.remove();
  terminalStartingNode = null;
  el("panel-terminal")?.removeAttribute("aria-busy");
}

/// A terminal-shaped first frame while the host starts the shell. It keeps the
/// dock from opening as an empty rectangle and leaves the real pane free to
/// replace it as soon as the PTY answers.
function terminalStartingView(project) {
  clearTerminalStarting();
  const section = document.createElement("section");
  section.className = "terminal-pane terminal-pane-starting focused";
  const surface = document.createElement("div");
  surface.className = "terminal-surface";
  surface.setAttribute("role", "status");
  surface.setAttribute("aria-live", "polite");
  const line = document.createElement("div");
  line.className = "terminal-boot-line";
  const prompt = document.createElement("span");
  prompt.className = "terminal-boot-prompt";
  prompt.innerHTML = ICONS.chevronRight;
  const spinner = document.createElement("span");
  spinner.className = "spinner";
  spinner.setAttribute("aria-hidden", "true");
  const label = document.createElement("span");
  label.textContent = t("terminal.starting", "Starting {value0}…", {
    value0: t("terminal.shell", "shell"),
  });
  line.append(prompt, spinner, label);
  const cwd = document.createElement("div");
  cwd.className = "terminal-boot-path";
  cwd.textContent = project || "";
  cwd.title = project || "";
  surface.append(line, cwd);
  section.append(surface);

  const container = el("terminal-panes");
  container.dataset.layout = "single";
  container.replaceChildren(section);
  el("terminal-context").textContent = project || "";
  el("terminal-context").title = project || "";
  el("terminal-shell").textContent = t("terminal.startingShort", "starting…");
  el("terminal-shell").title = t("terminal.startingShort", "starting…");
  el("panel-terminal").setAttribute("aria-busy", "true");
  terminalStartingNode = section;
  return section;
}

function newTerminalScreen(cols, rows) {
  return {
    cols: Math.max(20, cols),
    rows: Math.max(4, rows),
    lines: [{ cells: [] }],
    row: 0,
    col: 0,
    style: terminalStyle(),
    pending: "",
    cursorVisible: true,
    bracketedPaste: false,
  };
}

function terminalPane(id = state.terminal.focused) {
  return state.terminal.panes.find((pane) => pane.id === id) || null;
}

function focusedTerminalPane() {
  return terminalPane() || state.terminal.panes[0] || null;
}

function terminalCell(screen, col) {
  const line = screen.lines[screen.row];
  while (line.cells.length < col) line.cells.push({ ch: " ", style: terminalStyle() });
  if (!line.cells[col]) line.cells[col] = { ch: " ", style: terminalStyle() };
  return line.cells[col];
}

function ensureTerminalRow(screen, row) {
  while (screen.lines.length <= row) screen.lines.push({ cells: [] });
  return screen.lines[row];
}

function writeTerminalCell(screen, ch) {
  if (screen.col >= 400) return;
  ensureTerminalRow(screen, screen.row);
  const cell = terminalCell(screen, screen.col);
  cell.ch = ch;
  cell.style = { ...screen.style };
  screen.col += 1;
}

/// One escape sequence, from the `ESC` at `at`: how many characters it took, or
/// zero when the sequence is still arriving and the bytes wait for the rest.
function readTerminalEscape(screen, input, at) {
  const rest = input.slice(at);
  if (rest.length < 2) return 0;
  const kind = rest[1];
  if (kind === "[") {
    const match = /^\x1b\[([0-9;?]*)([A-Za-z])/.exec(rest);
    if (!match) return rest.length < 4 ? 0 : 2;
    applyTerminalCsi(screen, match[1], match[2]);
    return match[0].length;
  }
  if (kind === "]") {
    // An operating-system command (a window title): it is read to its end and
    // dropped, since nothing here draws a title.
    const end = rest.slice(2).search(/[\x07\x1b]/);
    if (end < 0) return 0;
    const tail = rest.slice(2 + end);
    return 2 + end + (tail.startsWith("\x1b\\") ? 2 : 1);
  }
  if (kind === "(" || kind === ")") return Math.min(rest.length, 3);
  return 2;
}

function applyTerminalCsi(screen, params, final) {
  const numbers = params
    .split(";")
    .map((value) => (value === "" ? 0 : Number(value)))
    .filter((value) => Number.isFinite(value));
  const first = numbers[0] ?? 0;
  if (params === "?2004" && final === "h") {
    screen.bracketedPaste = true;
    return;
  }
  if (params === "?2004" && final === "l") {
    screen.bracketedPaste = false;
    return;
  }
  if (params.includes("?25") && final === "h") {
    screen.cursorVisible = true;
    return;
  }
  if (params.includes("?25") && final === "l") {
    screen.cursorVisible = false;
    return;
  }
  if (final === "m") {
    for (let index = 0; index < numbers.length; index += 1) {
      const value = numbers[index];
      if (value === 0) screen.style = terminalStyle();
      else if (value === 1) screen.style.bold = true;
      else if (value === 2) screen.style.dim = true;
      else if (value === 22) {
        screen.style.bold = false;
        screen.style.dim = false;
      } else if (value === 4) screen.style.underline = true;
      else if (value === 24) screen.style.underline = false;
      else if (value === 7) screen.style.inverse = true;
      else if (value === 27) screen.style.inverse = false;
      else if (value === 39) screen.style.fg = "";
      else if (value === 49) screen.style.bg = "";
      else if ((value >= 30 && value <= 37) || (value >= 90 && value <= 97)) {
        screen.style.fg = String(value);
      } else if ((value >= 40 && value <= 47) || (value >= 100 && value <= 107)) {
        screen.style.bg = String(value);
      } else if (value === 38 || value === 48) {
        const target = value === 38 ? "fg" : "bg";
        if (numbers[index + 1] === 5) {
          screen.style[target] = String(256 + (numbers[index + 2] ?? 0));
          index += 2;
        } else if (numbers[index + 1] === 2) {
          screen.style[target] = `rgb(${numbers[index + 2] ?? 0},${numbers[index + 3] ?? 0},${numbers[index + 4] ?? 0})`;
          index += 4;
        }
      }
    }
    return;
  }
  if (final === "K") {
    const line = ensureTerminalRow(screen, screen.row);
    if (first === 2) line.cells = [];
    else if (first === 1) line.cells = line.cells.slice(screen.col);
    else line.cells = line.cells.slice(0, screen.col);
    return;
  }
  if (final === "J") {
    if (first === 2) {
      screen.lines = [{ cells: [] }];
      screen.row = 0;
      screen.col = 0;
    } else {
      ensureTerminalRow(screen, screen.row).cells = ensureTerminalRow(screen, screen.row).cells.slice(
        0,
        screen.col,
      );
      screen.lines = screen.lines.slice(0, screen.row + 1);
    }
    return;
  }
  if (final === "H" || final === "f") {
    screen.row = Math.max(0, (numbers[0] ?? 1) - 1);
    screen.col = Math.max(0, (numbers[1] ?? 1) - 1);
    ensureTerminalRow(screen, screen.row);
    return;
  }
  if (final === "A") screen.row = Math.max(0, screen.row - Math.max(1, first));
  else if (final === "B") {
    screen.row += Math.max(1, first);
    ensureTerminalRow(screen, screen.row);
  } else if (final === "C") screen.col += Math.max(1, first);
  else if (final === "D") screen.col = Math.max(0, screen.col - Math.max(1, first));
  else if (final === "G") screen.col = Math.max(0, first - 1);
}

/// Feeds the shell's output into the screen: the small part of a terminal's
/// language a shell's own output uses — carriage return, line feed, backspace,
/// tabs, colours, erasing and moving the cursor. A program that paints a
/// full-screen interface is outside that, and is not what this panel is for.
function terminalFeed(text, id = state.terminal.focused) {
  const pane = terminalPane(id);
  const screen = pane?.screen;
  if (!screen || !text) return;
  const input = screen.pending + text;
  screen.pending = "";
  let index = 0;
  while (index < input.length) {
    const ch = input[index];
    if (ch === "\x1b") {
      const consumed = readTerminalEscape(screen, input, index);
      if (consumed === 0) {
        screen.pending = input.slice(index);
        break;
      }
      index += consumed;
      continue;
    }
    if (ch === "\r") screen.col = 0;
    else if (ch === "\n") {
      screen.row += 1;
      screen.col = 0;
      ensureTerminalRow(screen, screen.row);
    } else if (ch === "\b") screen.col = Math.max(0, screen.col - 1);
    else if (ch === "\t") screen.col = Math.min(400, screen.col + (8 - (screen.col % 8)));
    else if (ch === "\f") {
      screen.lines = [{ cells: [] }];
      screen.row = 0;
      screen.col = 0;
    } else if (ch === "\x07") {
      // The bell has nothing to draw.
    } else if (ch >= " ") writeTerminalCell(screen, ch);
    index += 1;
  }
  // The scrollback is the panel's history, not the whole session's: a shell that
  // has printed for an hour is not kept to the byte.
  const overflow = screen.lines.length - 600;
  if (overflow > 0) {
    screen.lines.splice(0, overflow);
    screen.row = Math.max(0, screen.row - overflow);
  }
  scheduleTerminalPaint();
}

/// The sixteen colours a shell names first, then the cube and the ramp. A
/// terminal's palette is the reader's to choose in their own terminal; this is
/// the one the window draws with.
function terminalColor(name) {
  const base = [
    "#3b3b3b", "#e06c75", "#98c379", "#e5c07b", "#61afef", "#c678dd", "#56b6c2", "#d7d7d7",
    "#7b7b7b", "#ff8b93", "#b7e08f", "#ffd68a", "#8bc4ff", "#e0a2f5", "#7ad6e0", "#ffffff",
  ];
  const value = Number(name);
  if (!Number.isFinite(value)) return name;
  if (value >= 256) {
    const index = Math.max(0, Math.min(255, value - 256));
    if (index < 16) return base[index];
    if (index < 232) {
      const step = (offset) => {
        const level = Math.floor(index / offset) % 6;
        return level === 0 ? 0 : 55 + level * 40;
      };
      const r = step(36);
      const g = step(6);
      const b = step(1);
      return `rgb(${r},${g},${b})`;
    }
    const grey = 8 + (index - 232) * 10;
    return `rgb(${grey},${grey},${grey})`;
  }
  if (value >= 90 && value <= 97) return base[value - 90 + 8];
  if (value >= 30 && value <= 37) return base[value - 30];
  if (value >= 100 && value <= 107) return base[value - 100 + 8];
  if (value >= 40 && value <= 47) return base[value - 40];
  return name;
}

function scheduleTerminalPaint() {
  if (state.terminal.paintTimer) return;
  state.terminal.paintTimer = setTimeout(() => {
    state.terminal.paintTimer = 0;
    paintTerminal();
  }, 16);
}

function terminalCursor(row, pane) {
  const cursor = document.createElement("span");
  cursor.className = "terminal-cursor";
  const cell = row.cells[pane.screen.col];
  cursor.textContent = cell && cell.ch !== " " ? cell.ch : "\u00a0";
  return cursor;
}

function paintTerminalPane(pane) {
  const { screen, surface } = pane;
  if (!surface || !screen) return;
  const rows = [];
  for (const [lineIndex, line] of screen.lines.entries()) {
    const row = document.createElement("div");
    row.className = "term-line";
    let run = null;
    let text = "";
    const flush = () => {
      if (!run) return;
      const span = document.createElement("span");
      span.textContent = text;
      if (run.fg) span.style.color = terminalColor(run.fg);
      if (run.bg) span.style.backgroundColor = terminalColor(run.bg);
      if (run.bold) span.style.fontWeight = "600";
      if (run.dim) span.style.opacity = "0.7";
      if (run.underline) span.style.textDecoration = "underline";
      if (run.inverse) {
        span.style.color = run.bg ? terminalColor(run.bg) : "var(--bg)";
        span.style.backgroundColor = run.fg ? terminalColor(run.fg) : "var(--text)";
      }
      row.appendChild(span);
      run = null;
      text = "";
    };
    for (const [column, cell] of line.cells.entries()) {
      if (
        screen.cursorVisible &&
        lineIndex === screen.row &&
        column === screen.col
      ) {
        flush();
        row.appendChild(terminalCursor(line, pane));
        continue;
      }
      const key = `${cell.style.fg}|${cell.style.bg}|${cell.style.bold}|${cell.style.dim}|${cell.style.underline}|${cell.style.inverse}`;
      if (!run || run.key !== key) {
        flush();
        run = { ...cell.style, key };
      }
      text += cell.ch;
    }
    flush();
    if (
      screen.cursorVisible &&
      lineIndex === screen.row &&
      screen.col >= line.cells.length
    ) {
      row.appendChild(terminalCursor(line, pane));
    }
    rows.push(row);
  }
  surface.replaceChildren(...rows);
  surface.scrollTop = surface.scrollHeight;
}

function paintTerminal() {
  for (const pane of state.terminal.panes) paintTerminalPane(pane);
}

/// The room the panel gives the shell, in characters. A surface that has not been
/// laid out yet (the stub the checks run against) reports the default size.
function fitTerminal() {
  const fontSize = clampTerminalFontSize(state.prefs.terminal_font_size);
  for (const pane of state.terminal.panes) {
    const rect = pane.surface.getBoundingClientRect?.() || { width: 0, height: 0 };
    const cols = rect.width ? Math.max(20, Math.floor(rect.width / (fontSize * 0.6))) : pane.cols;
    const rows = rect.height ? Math.max(4, Math.floor(rect.height / (fontSize * 1.4))) : pane.rows;
    if (cols === pane.cols && rows === pane.rows) continue;
    pane.cols = cols;
    pane.rows = rows;
    pane.screen.cols = cols;
    pane.screen.rows = rows;
    void invoke("terminal_resize", { id: pane.id, cols, rows }).catch(() => {});
  }
}

let terminalDividerNode = null;

function terminalControl(label, className, icon, run) {
  const control = document.createElement("button");
  control.type = "button";
  control.className = `icon ${className}`;
  control.title = label;
  control.setAttribute("aria-label", label);
  control.innerHTML = icon;
  control.onclick = (event) => {
    event.stopPropagation();
    run();
  };
  return control;
}

function terminalPaneView(pane) {
  const section = document.createElement("section");
  section.className = "terminal-pane";
  section.dataset.terminalPane = String(pane.id);
  const head = document.createElement("header");
  head.className = "terminal-pane-head";
  const title = document.createElement("span");
  title.className = "terminal-pane-title";
  title.textContent = pane.shell;
  const cwd = document.createElement("span");
  cwd.className = "terminal-pane-cwd";
  cwd.textContent = pane.cwd;
  cwd.title = pane.cwd;
  const controls = document.createElement("div");
  controls.className = "terminal-pane-controls";
  controls.setAttribute("role", "toolbar");
  controls.setAttribute("aria-label", t("terminal.title", "Terminal"));
  const right = terminalControl(
    t("terminal.splitRight", "Split terminal right"),
    "pane-split-right",
    '<svg viewBox="0 0 24 24" width="13" height="13" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><rect x="3" y="4" width="18" height="16" rx="2"/><path d="M12 4v16"/></svg>',
    () => void splitTerminal("row", pane.id),
  );
  const down = terminalControl(
    t("terminal.splitDown", "Split terminal down"),
    "pane-split-down",
    '<svg viewBox="0 0 24 24" width="13" height="13" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><rect x="3" y="4" width="18" height="16" rx="2"/><path d="M3 12h18"/></svg>',
    () => void splitTerminal("column", pane.id),
  );
  const close = terminalControl(
    t("terminal.closePane", "Close terminal pane"),
    "pane-close",
    '<svg viewBox="0 0 24 24" width="13" height="13" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" aria-hidden="true"><path d="M18 6 6 18M6 6l12 12"/></svg>',
    () => void closeTerminalPane(pane.id),
  );
  controls.append(right, down, close);
  const surface = document.createElement("div");
  surface.className = "terminal-surface";
  surface.tabIndex = 0;
  surface.setAttribute("role", "log");
  surface.setAttribute("aria-live", "polite");
  surface.setAttribute("aria-label", t("terminal.title", "Terminal"));
  surface.addEventListener("keydown", (event) => {
    if (!terminalKey(event, pane.id)) return;
    event.preventDefault();
    event.stopPropagation();
  });
  surface.addEventListener("copy", (event) => {
    const text = terminalSelection(surface);
    if (!text) return;
    event.preventDefault();
    if (event.clipboardData?.setData) event.clipboardData.setData("text/plain", text);
    else void copyText(text);
  });
  surface.addEventListener("paste", (event) => {
    event.preventDefault();
    const text = event.clipboardData?.getData?.("text/plain");
    if (text) pasteTerminalText(text, pane.id);
    else void pasteTerminalClipboard(pane.id);
  });
  surface.addEventListener("click", () => focusTerminalPane(pane.id));
  head.append(title, cwd, controls);
  section.append(head, surface);
  section.addEventListener("pointerdown", (event) => {
    if (event.target.closest?.("button")) return;
    focusTerminalPane(pane.id);
  });
  pane.section = section;
  pane.surface = surface;
  pane.titleEl = title;
  pane.cwdEl = cwd;
  pane.splitRight = right;
  pane.splitDown = down;
}

function terminalDivider() {
  if (terminalDividerNode) return terminalDividerNode;
  const divider = document.createElement("div");
  divider.id = "terminal-divider";
  divider.className = "terminal-divider";
  divider.setAttribute("role", "separator");
  divider.setAttribute("aria-label", "Resize terminal panes");
  divider.tabIndex = -1;
  divider.addEventListener("pointerdown", (event) => {
    event.preventDefault();
    const container = el("terminal-panes");
    const rect = container.getBoundingClientRect?.() || { width: 1, height: 1 };
    const horizontal = state.terminal.layout === "row";
    divider.setPointerCapture?.(event.pointerId);
    const move = (next) => {
      const size = horizontal ? rect.width : rect.height;
      const position = horizontal ? next.clientX - rect.left : next.clientY - rect.top;
      state.terminal.ratio = Math.max(20, Math.min(80, (position / Math.max(1, size)) * 100));
      container.style.setProperty("--terminal-ratio", `${state.terminal.ratio}%`);
      fitTerminal();
    };
    const finish = (next) => {
      divider.removeEventListener("pointermove", move);
      divider.removeEventListener("pointerup", finish);
      divider.removeEventListener("pointercancel", finish);
      divider.releasePointerCapture?.(next.pointerId);
      fitTerminal();
    };
    divider.addEventListener("pointermove", move);
    divider.addEventListener("pointerup", finish);
    divider.addEventListener("pointercancel", finish);
  });
  terminalDividerNode = divider;
  return divider;
}

function renderTerminalPanes() {
  const container = el("terminal-panes");
  const focused = state.terminal.focused;
  for (const pane of state.terminal.panes) {
    pane.section.classList.toggle("focused", pane.id === focused);
    pane.splitRight.disabled = state.terminal.panes.length >= 2;
    pane.splitDown.disabled = state.terminal.panes.length >= 2;
  }
  el("terminal-split-right").disabled = state.terminal.panes.length >= 2;
  el("terminal-split-down").disabled = state.terminal.panes.length >= 2;
  const children = state.terminal.panes.map((pane) => pane.section);
  const split = state.terminal.panes.length === 2;
  if (split) {
    const divider = terminalDivider();
    divider.setAttribute(
      "aria-orientation",
      state.terminal.layout === "row" ? "vertical" : "horizontal",
    );
    children.splice(1, 0, divider);
  }
  container.replaceChildren(...children);
  container.dataset.layout = split ? state.terminal.layout : "single";
  container.style.setProperty("--terminal-ratio", `${state.terminal.ratio}%`);
  const active = focusedTerminalPane();
  if (active) {
    el("terminal-context").textContent = active.cwd;
    el("terminal-context").title = active.cwd;
    el("terminal-shell").textContent = active.shell;
    el("terminal-shell").title = active.cwd;
  } else {
    el("terminal-context").textContent = "";
    el("terminal-context").removeAttribute("title");
    el("terminal-shell").textContent = t("terminal.shell", "shell");
    el("terminal-shell").title = t("terminal.shell", "Shell");
  }
}

function focusTerminalPane(id, focus = false) {
  if (!terminalPane(id)) return;
  state.terminal.focused = id;
  renderTerminalPanes();
  const pane = focusedTerminalPane();
  if (focus) pane?.surface.focus();
  fitTerminal();
}

async function openTerminalPane(project, cols = 100, rows = 24) {
  const answer = await invoke("terminal_open", { project, cols, rows });
  const pane = {
    id: answer?.id ?? null,
    cwd: answer?.cwd || project,
    shell: answer?.shell || t("terminal.shell", "shell"),
    cols,
    rows,
    exited: false,
    screen: newTerminalScreen(cols, rows),
  };
  terminalPaneView(pane);
  state.terminal.panes.push(pane);
  state.terminal.focused = pane.id;
  return pane;
}

async function startTerminal() {
  const project = state.worktree || state.project;
  if (!project) {
    setStatus(t("composer.selectProject", "Select a project first."));
    askForProject();
    return;
  }
  if (state.terminal.opening) return;
  state.terminal.opening = true;
  const generation = ++state.terminal.openGeneration;
  terminalStartingView(project);
  try {
    const pane = await openTerminalPane(project);
    if (
      generation !== state.terminal.openGeneration ||
      el("bottom-panel").hidden ||
      state.panelTab !== "terminal"
    ) {
      state.terminal.panes = state.terminal.panes.filter((entry) => entry.id !== pane.id);
      state.terminal.focused = state.terminal.panes.at(-1)?.id ?? null;
      renderTerminalPanes();
      await invoke("terminal_close", { id: pane.id }).catch(() => {});
      return;
    }
    clearTerminalStarting();
    el("terminal-state").hidden = true;
    renderTerminalPanes();
    fitTerminal();
    paintTerminal();
  } catch (error) {
    if (generation !== state.terminal.openGeneration) return;
    clearTerminalStarting();
    el("terminal-state").hidden = false;
    el("terminal-state").textContent = t("terminal.failed", "Could not open a shell: {value0}", {
      value0: error,
    });
  } finally {
    if (generation === state.terminal.openGeneration) {
      state.terminal.opening = false;
      el("panel-terminal").removeAttribute("aria-busy");
    }
  }
}

async function stopTerminal() {
  state.terminal.openGeneration += 1;
  clearTerminalStarting();
  state.terminal.opening = false;
  const ids = state.terminal.panes.map((pane) => pane.id);
  state.terminal.panes = [];
  state.terminal.focused = null;
  state.terminal.layout = "single";
  state.terminal.ratio = 50;
  renderTerminalPanes();
  await Promise.all(ids.map((id) => invoke("terminal_close", { id }).catch(() => {})));
}

async function restartTerminal() {
  await stopTerminal();
  await startTerminal();
  focusedTerminalPane()?.surface.focus();
}

async function splitTerminal(direction = "row", source = state.terminal.focused) {
  let current = terminalPane(source);
  if (!current) {
    await openTerminal();
    current = focusedTerminalPane();
  }
  if (!current || state.terminal.panes.length >= 2) return;
  const project = current.cwd || state.worktree || state.project;
  if (!project) return;
  try {
    await openTerminalPane(project, current.cols, current.rows);
    state.terminal.layout = direction === "column" ? "column" : "row";
    el("terminal-state").hidden = true;
    renderTerminalPanes();
    fitTerminal();
    paintTerminal();
    focusedTerminalPane()?.surface.focus();
  } catch (error) {
    el("terminal-state").hidden = false;
    el("terminal-state").textContent = t("terminal.failed", "Could not open a shell: {value0}", {
      value0: error,
    });
  }
}

async function closeTerminalPane(id) {
  const pane = terminalPane(id);
  if (!pane) return;
  state.terminal.panes = state.terminal.panes.filter((entry) => entry.id !== id);
  pane.section.remove();
  await invoke("terminal_close", { id }).catch(() => {});
  state.terminal.layout = "single";
  state.terminal.ratio = 50;
  state.terminal.focused = state.terminal.panes.at(-1)?.id ?? null;
  renderTerminalPanes();
  if (!state.terminal.panes.length) {
    el("terminal-state").hidden = false;
    el("terminal-state").textContent = t("terminal.exited", "The shell has exited. Open the panel again for a new one.");
    return;
  }
  fitTerminal();
  focusedTerminalPane()?.surface.focus();
}

function clearTerminal() {
  const pane = focusedTerminalPane();
  if (!pane) return;
  pane.screen = newTerminalScreen(pane.cols, pane.rows);
  paintTerminal();
  terminalWrite("\x0c", pane.id);
  pane.surface.focus();
}

function setTerminalMaximized(on) {
  const next = Boolean(
    on && state.panelTab === "terminal" && !el("bottom-panel").hidden,
  );
  document.body.classList.toggle("terminal-maximized", next);
  el("terminal-resizer").hidden =
    next || state.panelTab !== "terminal" || el("bottom-panel").hidden;
  const button = el("terminal-maximize");
  const label = t(
    next ? "terminal.restore" : "terminal.maximize",
    next ? "Restore terminal" : "Maximize terminal",
  );
  button.setAttribute("aria-pressed", String(next));
  button.title = label;
  button.setAttribute("aria-label", label);
  fitTerminal();
}

function toggleTerminalMaximized() {
  setTerminalMaximized(!document.body.classList.contains("terminal-maximized"));
}

async function openTerminal() {
  openPanel("terminal");
  if (!state.terminal.panes.length) await startTerminal();
  focusedTerminalPane()?.surface.focus();
}

function terminalWrite(data, id = state.terminal.focused) {
  if (id == null) return;
  void invoke("terminal_write", { id, data }).catch((error) => {
    el("terminal-state").hidden = false;
    el("terminal-state").textContent = t("terminal.writeFailed", "The shell is not answering: {value0}", {
      value0: error,
    });
  });
}

/// The selected text when the selection stays inside this terminal. A selection
/// dragged into the conversation is left to the window's own copy rather than
/// being half terminal output and half chat.
function terminalSelection(surface) {
  const selection = window.getSelection?.();
  if (!selection || selection.isCollapsed || !selection.rangeCount) return "";
  const range = selection.getRangeAt(0);
  const start = range.startContainer;
  const end = range.endContainer;
  if (!surface.contains(start) || !surface.contains(end)) return "";
  return selection.toString();
}

function copyTerminalSelection(pane) {
  const text = terminalSelection(pane?.surface);
  if (!text) return false;
  void copyText(text).then((copied) => {
    if (!copied) showToast(t("ui.couldNotCopy", "Could not copy to the clipboard."), "error");
  });
  return true;
}

/// A shell enables bracketed paste when it can keep a multi-line paste on the
/// prompt until Enter. Without it, line feeds become the carriage returns the
/// shell reads as Enter.
function pasteTerminalText(text, id = state.terminal.focused) {
  const pane = terminalPane(id);
  if (!pane || !text) return;
  const normalized = String(text).replace(/\r\n?/g, "\n");
  const data = pane.screen?.bracketedPaste
    ? `\x1b[200~${normalized}\x1b[201~`
    : normalized.replace(/\n/g, "\r");
  terminalWrite(data, id);
}

async function pasteTerminalClipboard(id = state.terminal.focused) {
  if (id == null) return;
  try {
    pasteTerminalText(await readClipboardText(), id);
  } catch {
    showToast(t("terminal.pasteFailed", "Could not paste from the clipboard."), "error");
  }
}

const TERMINAL_KEYS = {
  Enter: "\r",
  Backspace: "\x7f",
  Tab: "\t",
  Escape: "\x1b",
  ArrowUp: "\x1b[A",
  ArrowDown: "\x1b[B",
  ArrowRight: "\x1b[C",
  ArrowLeft: "\x1b[D",
  Home: "\x1b[H",
  End: "\x1b[F",
  Delete: "\x1b[3~",
  PageUp: "\x1b[5~",
  PageDown: "\x1b[6~",
};

/// What a key press means to the shell. The shell does its own line editing, so
/// the window sends the keys and paints what comes back rather than keeping a
/// buffer of its own.
function terminalKey(event, id = state.terminal.focused) {
  if (id == null) return false;
  const key = String(event.key || "").toLowerCase();
  const copyChord = (event.metaKey && !event.ctrlKey && !event.shiftKey) ||
    (event.ctrlKey && event.shiftKey);
  const pasteChord = (event.metaKey && !event.ctrlKey && !event.shiftKey) ||
    (event.ctrlKey && (event.shiftKey || !isMac())) ||
    (event.shiftKey && event.key === "Insert");
  if (copyChord && key === "c") {
    copyTerminalSelection(terminalPane(id));
    return true;
  }
  if (pasteChord && key === "v") {
    void pasteTerminalClipboard(id);
    return true;
  }
  if (event.metaKey) return false;
  if (event.ctrlKey && event.key.length === 1) {
    const code = event.key.toUpperCase().charCodeAt(0);
    if (code >= 64 && code <= 95) {
      terminalWrite(String.fromCharCode(code - 64), id);
      return true;
    }
  }
  const mapped = TERMINAL_KEYS[event.key];
  if (mapped) {
    terminalWrite(mapped, id);
    return true;
  }
  if (event.key.length === 1 && !event.altKey) {
    terminalWrite(event.key, id);
    return true;
  }
  return false;
}

// ---------- wiring ----------

function toggleHelp() {
  const hidden = el("help-modal").hidden;
  closeOverlays("help-modal");
  el("help-modal").hidden = !hidden;
}

/// Keep picks and keyboard cycling in the shared configuration.
let reasoningSave = Promise.resolve();

function setReasoning(level) {
  const levels = reasoningChoices();
  if (!levels.includes(level)) return;
  state.reasoning = level;
  state.reasoningPicked = true;
  reasoningSave = reasoningSave.then(() => invoke("set_reasoning", { reasoning: level })).catch(async (error) => {
    if (state.reasoning === level) await loadInfo();
    showToast(t("ui.couldNotSaveReasoning", "Could not save reasoning: {value0}", { value0: error }), "error");
  });
  updateChips();
}

/// A level named outright — `/reasoning high` — is a pick that also puts the
/// menu away; the slider applies levels and leaves it up.
function pickReasoning(level) {
  setReasoning(level);
  closeModelMenu();
}

function cycleReasoning() {
  // Cycle the model's own levels when its listing narrowed them, so the cycle
  // never steps onto a level the picker does not offer.
  if (!hasReasoningLevels()) return;
  const levels = reasoningChoices();
  setReasoning(levels[(levels.indexOf(state.reasoning) + 1) % levels.length]);
}

async function initEvents() {
  await listen("agent-start", async (event) => {
    const payload = event.payload || {};
    const record = threadForRunId(payload.runId) || visibleThread();
    const session = payload.sessionId || null;
    const wasVisible = record === visibleThread();
    const draftKeyBefore = record.key;
    // The tab that sent the message adopts the thread it made, so a tab that
    // started a conversation is that conversation from here.
    const sendingTab = chatTabs.find((entry) => entry.draftKey === draftKeyBefore)
      || chatTabs.find((entry) => session && entry.session === session);
    // The run names the thread it made: a draft that started a conversation is
    // re-keyed with the id the core created, so the same record — and the same
    // transcript it has been painting into — is what the sidebar and every
    // later listing find by that id.
    if (session && record.key.startsWith("draft:")) {
      const key = `s:${session}`;
      const existing = threads.get(key);
      if (!existing || existing === record) {
        threads.delete(record.key);
        for (const tab of chatTabs) if (tab.draftKey === record.key) tab.draftKey = undefined;
        record.key = key;
        threads.set(key, record);
      }
    }
    if (session) record.session = session;
    record.runSession = session;
    // A turn titles its thread the moment it starts, so the header names it now
    // and the sidebar lists it — with the same label — without waiting for the
    // turn to end. The session is on disk from here, so the listing has it.
    record.runTitle = payload.title || "";
    if (session) state.heldThread = { id: session, title: record.runTitle };
    if (sendingTab && session) {
      sendingTab.draftKey = undefined;
      sendingTab.session = session;
      sendingTab.title = record.runTitle || sendingTab.title;
    }
    if (wasVisible && session) state.session = session;
    if (record === visibleThread()) {
      refreshThreadTitle();
      paintQueue();
      updateSendState();
      paintStatus();
    }
    updateRunBanner();
    paintChatTabs();
    paintRunClock();
    await loadSessions();
  });
  await listen("agent-event", (event) => {
    const payload = event.payload || {};
    const record = threadForRunId(payload.runId) || visibleThread();
    withThread(record, () => handleEvent(payload));
    paintRunClock();
    paintChatTabs();
  });
  await listen("agent-end", async (event) => {
    const payload = event.payload || {};
    const record = threadForRunId(payload.runId)
      || threadForRunId(null)
      || visibleThread();
    const visible = record === visibleThread();
    if (payload.sessionId) record.session = payload.sessionId;
    withThread(record, () => {
      finishRunClock(record);
      record.busy = false;
      record.runId = null;
      // A failure reported mid-run is the turn's outcome, so it is kept; the
      // next turn is what clears it.
      if (payload.error) {
        record.runError = t("common.errorDetail", "Error: {value0}", { value0: payload.error });
      }
      record.runProject = null;
      record.runSession = null;
      record.unread = !visible;
      renderChanges(payload);
      resetTurn();
    });
    if (visible) {
      setStatus(record.runError || idleStatus(), record.runError ? "error" : "info");
      // A turn is where the branch the composer's top row reads can change — the
      // model runs git like any other command — so the row is read again before
      // the next message is composed against it.
      loadGit();
    }
    paintRunningChrome();
    updateSendState();
    updateRunBanner();
    paintStatus();
    paintRunClock();
    paintUsage();
    await loadSessions();
    await startNextPendingSend(record);
    // The turn is over and the next one has not been queued, so the window may
    // ask what the reader would say next.
    if (visible && !payload.error) void maybePredict();
    // And the system may say it finished, if the reader asked for that and is
    // not looking at it.
    if (!payload.error) notifyFinished(record.runTitle || t("notify.untitled", "the chat"));
  });
  // The requests a thread reported are read from a forge off the listing's own
  // thread, so a state that landed after the sidebar was drawn is what this
  // listens for: the rows are listed again with the states that are known now.
  await listen("pulls-updated", async () => {
    await loadSessions();
  });
  el("review-modal").onkeydown = reviewKey;
  await listen("approval-request", (event) => showApproval(event.payload || {}));
  // The macOS menu item has no page of its own to paint into, so it asks the
  // window for the dialog the sidebar's own button opens.
  await listen("check-updates", () => openUpdate());
  await listen("chat-tab", (event) => event.payload?.action === "close" ? closeChatTab(activeTabId) : newChatTab());
  // The menu bar and the window's own buttons end in the same place: the menu
  // has no page of its own, so it asks the window for the settings sheet, the
  // command menu and the text size rather than keeping a second copy of them.
  await listen("terminal-output", (event) => {
    const payload = event.payload || {};
    const pane = terminalPane(payload.terminal);
    if (!pane) return;
    terminalFeed(String(payload.data || ""), pane.id);
  });
  await listen("terminal-exit", (event) => {
    const payload = event.payload || {};
    const pane = terminalPane(payload.terminal);
    if (!pane) return;
    pane.exited = true;
    el("terminal-state").hidden = false;
    el("terminal-state").textContent = t(
      "terminal.exited",
      "The shell has exited. Open the panel again for a new one.",
    );
  });
  await listen("toggle-panel", () => togglePanel());
  await listen("open-terminal", () => void openTerminal());
  await listen("open-settings", () => openSettings());
  await listen("command-menu", () => openCommandMenu());
  await listen("toggle-sidebar", () => toggleSidebar());
  await listen("history", (event) =>
    event.payload?.action === "forward" ? navForward() : navBack());
  await listen("zoom", (event) => {
    const action = event.payload?.action;
    resizeText(action === "reset" ? 0 : action === "in" ? 1 : -1);
  });
  // The install this process is running reports itself here, whether it is the
  // launch's own or one the window asked for, and the dialog is its surface: the
  // release that is out, the download as it happens, and the restart that runs
  // what landed. The install starts before this page does, so what it has already
  // said is asked for rather than waited on, once the listeners above are in
  // place to hear the rest.
  await listen("update-available", () => handleUpdateAvailable());
  await listen("update-progress", (event) => handleUpdateProgress(event.payload || {}));
  await listen("update-ready", (event) => handleUpdateReady(event.payload || {}));
  await listen("update-failed", (event) => handleUpdateFailed(event.payload || {}));
  await catchUpOnLaunchUpdate();
  // The launch looks for the model catalog each logged-in provider publishes,
  // which is where the window a model resolves to comes from: a catalog that
  // landed after this folder was opened repaints the model chip with the window
  // the next turn will actually use — and only that, since a full `loadInfo`
  // also restores the level the folder remembers, which is the reader's choice
  // only until they change it.
  await listen("model-catalog", () => refreshContextWindow());
  await listen("question-request", (event) => showQuestion(event.payload || {}));
  // The request timed out with nobody answering, while the run it belongs to
  // may still be going: the dialog goes away so it does not offer an answer that
  // nothing is waiting for.
  await listen("question-closed", (event) => {
    const payload = event.payload || {};
    if (state.pendingQuestion && state.pendingQuestion.id === payload.id) {
      closeQuestion();
    }
  });
}

// Create project modal state
const createProjectState = {
  folders: [],
  editing: null,
};

// The last path segment, used to default the project name to the folder's name.
function folderBasename(path) {
  const trimmed = String(path || "").replace(/[\\/]+$/, "");
  const parts = trimmed.split(/[\\/]+/);
  return parts[parts.length - 1] || trimmed;
}

function openCreateProject() {
  createProjectState.editing = null;
  createProjectState.folders = [];
  el("create-project-name").value = "";
  el("create-project-path").value = "";
  el("create-project-source-options").open = false;
  el("create-project-folders").innerHTML = "";
  renderCreateProjectFolders();
  el("create-project-source-box").classList.remove("readonly");
  el("create-project-error").hidden = true;
  el("create-project-sources").hidden = false;
  el("create-project-title").textContent = t("dialog.createProject.title", "Create project");
  el("create-project-save").textContent = t("dialog.createProject.title", "Create project");
  el("create-project-remove").hidden = true;
  el("create-project-modal").hidden = false;
  el("create-project-name").focus();
}

function openEditProject(project) {
  closeSidebarCards();
  createProjectState.editing = project;
  createProjectState.folders = [project.path];
  el("create-project-name").value = project.name;
  el("create-project-path").value = "";
  renderCreateProjectFolders();
  el("create-project-error").hidden = true;
  // A registered project is one source folder in the desktop model. Editing
  // changes its display name only, so the folder is listed without the add and
  // remove controls whose changes the command cannot persist.
  el("create-project-sources").hidden = false;
  el("create-project-source-box").classList.add("readonly");
  el("create-project-title").textContent = t("ui.editProject", "Edit project");
  el("create-project-save").textContent = t("common.save", "Save");
  el("create-project-remove").hidden = false;
  el("create-project-remove").textContent = t("dialog.createProject.removeLocal", "Remove local project");
  el("create-project-modal").hidden = false;
  el("create-project-name").focus();
}

async function addCreateProjectFolder() {
  try {
    const path = (await invoke("pick_folder")) || "";
    if (path && !createProjectState.folders.includes(path)) {
      createProjectState.folders.push(path);
      renderCreateProjectFolders();
      const nameInput = el("create-project-name");
      if (!nameInput.value.trim()) nameInput.value = folderBasename(path);
    }
  } catch (error) {
    showCreateProjectError(t("ui.couldNotOpenFolderChooser", "Could not open folder chooser: {value0}", { value0: error }));
  }
}

/// A path typed by hand, so a folder can be added even when the platform
/// chooser cannot be seen — a window without focus leaves the panel behind and
/// the list must not depend on it.
function addCreateProjectTypedPath() {
  const input = el("create-project-path");
  const path = input.value.trim();
  if (!path) return;
  if (createProjectState.folders.includes(path)) {
    showCreateProjectError(t("ui.thatFolderIsAlreadyInTheList", "That folder is already in the list"));
    return;
  }
  createProjectState.folders.push(path);
  input.value = "";
  showCreateProjectError("");
  renderCreateProjectFolders();
  const nameInput = el("create-project-name");
  if (!nameInput.value.trim()) nameInput.value = folderBasename(path);
}

function removeCreateProjectFolder(path) {
  createProjectState.folders = createProjectState.folders.filter((f) => f !== path);
  renderCreateProjectFolders();
}

function renderCreateProjectFolders() {
  const container = el("create-project-folders");
  container.innerHTML = "";
  for (const folder of createProjectState.folders) {
    const item = document.createElement("div");
    item.className = "folder-item";
    item.innerHTML = ICONS.folder;
    const pathEl = document.createElement("span");
    pathEl.className = "folder-item-path";
    pathEl.textContent = folderBasename(folder);
    pathEl.title = folder;
    const removeBtn = document.createElement("button");
    removeBtn.type = "button";
    removeBtn.className = "folder-item-remove";
    removeBtn.title = t("common.remove", "Remove");
    removeBtn.setAttribute("aria-label", `${t("common.remove", "Remove")} ${folderBasename(folder)}`);
    removeBtn.innerHTML = ICONS.close;
    removeBtn.onclick = () => removeCreateProjectFolder(folder);
    item.appendChild(pathEl);
    item.appendChild(removeBtn);
    container.appendChild(item);
  }
  const has = createProjectState.folders.length > 0;
  el("create-project-source-box").classList.toggle("has-folders", has);
  el("create-project-add-row").hidden = !has;
  el("create-project-where-label").textContent = t("dialog.createProject.thisComputer", "This computer");
  el("create-project-where").hidden = !has;
}

function showCreateProjectError(message) {
  const box = el("create-project-error");
  if (!box) return;
  box.textContent = message || "";
  box.hidden = !message;
}

async function saveCreateProject() {
  if (createProjectState.folders.length === 0) {
    showCreateProjectError(t("ui.atLeastOneSourceFolderIsRequired", "At least one source folder is required"));
    return;
  }
  const nameInput = el("create-project-name");
  let name = nameInput.value.trim();
  if (!name) {
    name = folderBasename(createProjectState.folders[0]);
    nameInput.value = name;
  }
  if (!name) {
    showCreateProjectError(t("ui.projectNameIsRequired", "Project name is required"));
    return;
  }
  try {
    if (createProjectState.editing) {
      state.projects = await invoke("update_project", {
        id: createProjectState.editing.id,
        name,
      });
      if (state.project === createProjectState.editing.path) {
        state.projectName = name;
        updateProjectChip();
      }
      el("create-project-modal").hidden = true;
      renderProjectsTree();
      return;
    }
    const result = await invoke("create_project", {
      name,
      folders: createProjectState.folders,
    });
    state.projects = result.projects;
    el("create-project-modal").hidden = true;
    renderProjectsTree(); // Update tree view instead of dropdown
    const added = state.projects.find((project) => project.id === result.added);
    if (added) {
      selectProject(added);
    }
  } catch (error) {
    showCreateProjectError(t("ui.couldNotCreateProject", "Could not create project: {value0}", { value0: error }));
  }
}

/// Drag the divider to resize the sidebar; double-click restores the default.
/// The width is remembered so the layout survives a relaunch.
function initSidebarResize() {
  const sidebar = document.querySelector(".sidebar");
  const handle = el("sidebar-resizer");
  if (!sidebar || !handle) return;
  const MIN = 160;
  const STEP = 16;
  const STORAGE_KEY = "oxide.sidebarWidth";
  // Keep a comfortable reading column for the transcript.
  const maxWidth = () => Math.max(MIN, window.innerWidth - 475);
  const clamp = (width) => Math.max(MIN, Math.min(maxWidth(), Math.round(width)));
  const apply = (width, persist) => {
    const next = clamp(width);
    sidebar.style.width = `${next}px`;
    document.body.style.setProperty("--sidebar-width", `${next}px`);
    // The separator advertises a resize affordance, so keep its value in sync
    // for screen readers.
    handle.setAttribute("aria-valuenow", String(next));
    handle.setAttribute("aria-valuemax", String(maxWidth()));
    if (persist) localStorage.setItem(STORAGE_KEY, String(next));
    paintDockToggles();
  };

  const saved = Number(localStorage.getItem(STORAGE_KEY) || 0);
  apply(saved > 0 ? saved : sidebar.getBoundingClientRect().width, false);

  handle.addEventListener("pointerdown", (event) => {
    event.preventDefault();
    handle.setPointerCapture(event.pointerId);
    document.body.classList.add("resizing");
    const startX = event.clientX;
    const startWidth = sidebar.getBoundingClientRect().width;
    const onMove = (move) => apply(startWidth + (move.clientX - startX), false);
    const onUp = (up) => {
      handle.removeEventListener("pointermove", onMove);
      handle.removeEventListener("pointerup", onUp);
      handle.releasePointerCapture?.(up.pointerId);
      document.body.classList.remove("resizing");
      apply(sidebar.getBoundingClientRect().width, true);
    };
    handle.addEventListener("pointermove", onMove);
    handle.addEventListener("pointerup", onUp);
  });

  // Keyboard path for the same resize, since the separator is focusable.
  handle.addEventListener("keydown", (event) => {
    const step = event.shiftKey ? STEP * 4 : STEP;
    const width = sidebar.getBoundingClientRect().width;
    let next;
    if (event.key === "ArrowLeft") next = width - step;
    else if (event.key === "ArrowRight") next = width + step;
    else if (event.key === "Home") next = MIN;
    else if (event.key === "End") next = maxWidth();
    else return;
    event.preventDefault();
    apply(next, true);
  });

  handle.addEventListener("dblclick", () => {
    localStorage.removeItem(STORAGE_KEY);
    sidebar.style.width = "";
    document.body.style.removeProperty("--sidebar-width");
    const current = sidebar.getBoundingClientRect().width;
    paintDockToggles();
    handle.setAttribute("aria-valuenow", String(Math.round(current)));
    handle.setAttribute("aria-valuemax", String(maxWidth()));
  });

  window.addEventListener("resize", () => {
    apply(sidebar.getBoundingClientRect().width, false);
  });
}

function dockHasRoom() {
  const sidebar = document.querySelector(".sidebar")?.getBoundingClientRect().width || 268;
  return window.innerWidth - (state.prefs.sidebar_visible ? sidebar + 7 : 0) - 48 >= 1100;
}

function initPanelResize() {
  const handle = el("panel-resizer");
  const storageKey = "oxide.workspaceWidth";
  const DEFAULT = 360;
  const MIN = 300;
  const MAX = 560;
  let width = DEFAULT;
  const apply = (value, persist = false) => {
    width = Math.max(MIN, Math.min(MAX, Number.isFinite(value) ? value : DEFAULT));
    document.body.style.setProperty("--workspace-width", `${width}px`);
    handle.setAttribute("aria-valuemin", String(MIN));
    handle.setAttribute("aria-valuemax", String(MAX));
    handle.setAttribute("aria-valuenow", String(Math.round(width)));
    if (persist) localStorage.setItem(storageKey, String(width));
    fitTerminal();
  };
  const saved = Number(localStorage.getItem(storageKey) || 0);
  apply(saved >= MIN ? saved : DEFAULT);
  handle.addEventListener("pointerdown", (event) => {
    event.preventDefault();
    const start = width;
    handle.setPointerCapture(event.pointerId);
    document.body.classList.add("resizing");
    const move = (next) => apply(start - (next.clientX - event.clientX));
    const finish = (next) => {
      handle.removeEventListener("pointermove", move);
      handle.removeEventListener("pointerup", finish);
      handle.removeEventListener("pointercancel", finish);
      handle.releasePointerCapture?.(next.pointerId);
      document.body.classList.remove("resizing");
      apply(width, true);
    };
    handle.addEventListener("pointermove", move);
    handle.addEventListener("pointerup", finish);
    handle.addEventListener("pointercancel", finish);
  });
  handle.addEventListener("keydown", (event) => {
    const step = event.shiftKey ? 40 : 16;
    if (event.key === "ArrowLeft") apply(width + step, true);
    else if (event.key === "ArrowRight") apply(width - step, true);
    else if (event.key === "Home") apply(MIN, true);
    else if (event.key === "End") apply(MAX, true);
    else return;
    event.preventDefault();
  });
  handle.addEventListener("dblclick", () => {
    localStorage.removeItem(storageKey);
    apply(DEFAULT);
  });
}

/// The conversation/terminal boundary follows the pointer and the keyboard.
/// Its height belongs to the window rather than either front-end, so Electron
/// and Tauri restore the same split.
function initTerminalResize() {
  const handle = el("terminal-resizer");
  const panel = el("bottom-panel");
  if (!handle || !panel) return;
  const storageKey = "oxide.terminalHeight";
  const MIN = 180;
  const STEP = 16;
  const defaultHeight = () =>
    Math.max(MIN, Math.min(440, Math.round(window.innerHeight * 0.46)));
  const maxHeight = () => Math.max(MIN, Math.min(Math.round(window.innerHeight * 0.7), window.innerHeight - 150));
  const clamp = (height) => Math.max(MIN, Math.min(maxHeight(), Math.round(height)));
  let height = defaultHeight();
  const apply = (value, persist = false) => {
    height = clamp(Number.isFinite(value) ? value : defaultHeight());
    document.body.style.setProperty("--terminal-height", `${height}px`);
    handle.setAttribute("aria-valuemin", String(MIN));
    handle.setAttribute("aria-valuemax", String(maxHeight()));
    handle.setAttribute("aria-valuenow", String(height));
    if (persist) localStorage.setItem(storageKey, String(height));
    fitTerminal();
  };
  const saved = Number(localStorage.getItem(storageKey) || 0);
  apply(saved >= MIN ? saved : defaultHeight());

  handle.addEventListener("pointerdown", (event) => {
    event.preventDefault();
    const startY = event.clientY;
    const startHeight = height;
    handle.setPointerCapture?.(event.pointerId);
    document.body.classList.add("resizing");
    const move = (next) => apply(startHeight - (next.clientY - startY));
    const finish = (next) => {
      handle.removeEventListener("pointermove", move);
      handle.removeEventListener("pointerup", finish);
      handle.removeEventListener("pointercancel", finish);
      handle.releasePointerCapture?.(next.pointerId);
      document.body.classList.remove("resizing");
      apply(height, true);
    };
    handle.addEventListener("pointermove", move);
    handle.addEventListener("pointerup", finish);
    handle.addEventListener("pointercancel", finish);
  });

  handle.addEventListener("keydown", (event) => {
    const step = event.shiftKey ? STEP * 4 : STEP;
    if (event.key === "ArrowUp") apply(height + step, true);
    else if (event.key === "ArrowDown") apply(height - step, true);
    else if (event.key === "Home") apply(MIN, true);
    else if (event.key === "End") apply(maxHeight(), true);
    else return;
    event.preventDefault();
  });

  handle.addEventListener("dblclick", () => {
    localStorage.removeItem(storageKey);
    apply(defaultHeight());
  });

  window.addEventListener("resize", () => apply(height));
}

/// How long a scrollbar stays drawn after the last scroll of its box, in
/// milliseconds: long enough to be read while the wheel is still spinning down,
/// short enough that the bar is away by the time the reader has moved on.
const SCROLLBAR_LINGER = 900;
/// The window's scrollbars are overlay bars: one is drawn while its box is being
/// scrolled and taken away again when that stops, so a bar is never a permanent
/// seam down the side of what it scrolls. A scrollbar belongs to the element it
/// scrolls, so the element that scrolled is the one marked — and since a scroll
/// does not bubble, the listener is registered for the capture phase, which is
/// one listener for every list in the window rather than one per list.
function initOverlayScrollbars() {
  const timers = new WeakMap();
  document.addEventListener(
    "scroll",
    (event) => {
      const scrolled = event.target === document ? document.documentElement : event.target;
      if (!scrolled?.classList) return;
      scrolled.classList.add("scrolling");
      clearTimeout(timers.get(scrolled));
      timers.set(
        scrolled,
        setTimeout(() => {
          timers.delete(scrolled);
          scrolled.classList.remove("scrolling");
        }, SCROLLBAR_LINGER),
      );
    },
    { capture: true, passive: true },
  );
}

const EDITABLE_INPUT_TYPES = new Set([
  "text",
  "search",
  "email",
  "url",
  "tel",
  "password",
  "number",
  "date",
  "datetime-local",
  "month",
  "time",
  "week",
]);

/// Whether `node` is a text editor of its own. WebKit can spend the first press
/// after editing only moving focus to a button, withholding its click until the
/// next press; that press is kept off the focus default so the same native click
/// completes, and the click WebKit omits is supplied on the next task instead.
function isTextEditor(node) {
  if (!node) return false;
  if (node.isContentEditable || node.tagName === "TEXTAREA") return true;
  return (
    node.tagName === "INPUT" &&
    EDITABLE_INPUT_TYPES.has(String(node.type || "text").toLowerCase())
  );
}

/// The innermost control a press belongs to. Buttons and links are controls by
/// their element kind; a native radio or checkbox resolves through its label to
/// the input it operates, and the page's rows become controls when it gives them
/// an `onclick` handler.
function pressedControl(target) {
  for (let node = target; node && node !== document.body; node = node.parentNode) {
    if (node.tagName === "INPUT" && ["checkbox", "radio"].includes(node.type)) {
      return node;
    }
    if (
      node.tagName === "LABEL" &&
      node.control?.tagName === "INPUT" &&
      ["checkbox", "radio"].includes(node.control.type)
    ) {
      return node.control;
    }
    if (node.tagName === "BUTTON" || node.tagName === "A" || node.onclick) return node;
  }
  return null;
}

/// Whether a control belongs to the list that completes the box being typed in —
/// a row of the `@` reference list or of the command palette. Taking one splices
/// into the editor the caret is in, so it is the one kind of control that leaves
/// the caret where it is.
function completesEditor(control) {
  return Boolean(control.closest(".palette-item"));
}

/// The editor WebKit ended immediately before dispatching the press that ended
/// it. Some macOS WebKit versions update `activeElement` before the page sees
/// `mousedown`, so reading `activeElement` alone misses that press and lets the
/// webview spend it ending an editing session that is already over. The memory
/// lasts only for the current task, so a later, unrelated press stays native.
let justBlurredEditor = null;
function rememberBlurredEditor(event) {
  const editor = event.target;
  if (!isTextEditor(editor)) return;
  justBlurredEditor = editor;
  setTimeout(() => {
    if (justBlurredEditor === editor) justBlurredEditor = null;
  }, 0);
}

/// The control the current editing-session press landed on, and whether the
/// webview delivered its native click. The native click stays authoritative:
/// one is supplied on the next task only when the webview omitted it, which is
/// what some macOS WebKit versions do when the press ends an editing session.
let editorControlPress = null;
function finishEditorControlPress(event) {
  // Only the primary button arms the press, so only its release finishes it: a
  // secondary release while the primary is still held must not run the
  // fallback ahead of the primary release, which would activate the control
  // twice (once supplied here, once by the primary press's native click).
  if (event.button !== 0) return;
  const press = editorControlPress;
  if (!press) return;
  if (pressedControl(event.target) !== press.control) {
    editorControlPress = null;
    return;
  }
  // A native click follows mouseup before the next task. Give WebKit that
  // chance first, then supply the click only when the webview omitted it, so a
  // control never answers two clicks and a press dragged off stays cancelled.
  setTimeout(() => {
    if (editorControlPress !== press) return;
    editorControlPress = null;
    if (!press.clicked && !press.control.disabled) press.control.click();
  }, 0);
}

function init() {
  document.body.classList.toggle("platform-mac", /Mac/.test(window.navigator?.platform || ""));
  paintNavigation();
  el("nav-back").onclick = navBack;
  el("nav-forward").onclick = navForward;
  el("sidebar-toggle").onclick = toggleSidebar;
  el("window-commands").onclick = openCommandMenu;
  el("rail-home").onclick = () => { closeCustomize(); void newChatFromSidebar(); };
  for (const [control, [id]] of Object.entries(RAIL_PANELS)) {
    el(control).setAttribute("aria-controls", id);
    el(control).setAttribute("aria-expanded", "false");
    el(control).onclick = () => toggleRailPanel(control);
    if (typeof MutationObserver !== "undefined") {
      new MutationObserver(() => {
        const panel = el(id);
        if (panel.hidden && panel.classList.contains("rail-panel") && panel.contains(document.activeElement)) {
          el(control).focus();
        }
        syncRailPanels();
      }).observe(el(id), { attributes: true, attributeFilter: ["hidden"] });
    }
  }
  for (const [id, icon] of Object.entries({
    "customize-plugins": "puzzle", "customize-skills": "book", "customize-mcps": "globe",
    "plugins-refresh": "refresh", "plugins-add": "plus",
    "skills-refresh": "refresh",
  })) {
    const control = el(id);
    const label = document.createElement("span");
    label.textContent = control.textContent;
    control.innerHTML = ICONS[icon];
    if (id !== "plugins-refresh") control.appendChild(label);
  }
  el("rail-projects").onclick = () => {
    if (el("projects-page").hidden) openProjectDirectory();
    else { closeProjectDirectory(); el("rail-projects").focus(); }
  };
  el("projects-filter").oninput = renderProjectDirectory;
  el("projects-sort").onclick = () => { projectsOldestFirst = !projectsOldestFirst; renderProjectDirectory(); };
  el("projects-page-create").onclick = openCreateProject;
  el("rail-update").onclick = openUpdateNotice;
  el("rail-customize").onclick = () => openCustomize();
  for (const view of ["plugins", "skills", "mcps"]) el(`customize-${view}`).onclick = () => openCustomize(view);
  el("plugin-search").oninput = renderPlugins;
  el("plugins-refresh").onclick = loadPluginCatalog;
  el("skills-refresh").onclick = () => openCustomize("skills");
  el("plugins-add").onclick = () => {
    el("marketplace-form").hidden = false;
    el("plugins-add").setAttribute("aria-expanded", "true");
    el("marketplace-source").focus();
  };
  el("marketplace-cancel").onclick = () => {
    el("marketplace-form").hidden = true;
    el("plugins-add").setAttribute("aria-expanded", "false");
    el("plugins-add").focus();
  };
  el("marketplace-form").onsubmit = (event) => {
    event.preventDefault();
    const source = el("marketplace-source").value.trim();
    if (source) void mutatePlugin("add_marketplace", source);
  };
  installMarkdownInput(
    el("prompt"),
    (text) => renderMarkdown(text, true),
    () => !atOpen() && !state.paletteOpen && !state.prediction,
  );
  initSidebarResize();
  initPanelResize();
  initTerminalResize();
  const unifiedReview = localStorage.getItem("oxide.reviewUnified") === "true";
  el("review-modal").classList.toggle("unified", unifiedReview);
  el("review-layout").setAttribute("aria-pressed", String(unifiedReview));
  el("review-layout").onclick = () => {
    const unified = el("review-modal").classList.toggle("unified");
    el("review-layout").setAttribute("aria-pressed", String(unified));
    localStorage.setItem("oxide.reviewUnified", String(unified));
  };
  initChatTabs();
  initMessageNavigator();
  initJumpLatest();
  initOverlayScrollbars();
  document.addEventListener("click", (event) => {
    const tools = el("composer-tools");
    if (!event.target?.closest?.("#composer-tools summary")) tools.open = false;
  });
  document.addEventListener("keydown", (event) => {
    const tools = el("composer-tools");
    if (event.key === "Escape" && tools.open) {
      tools.open = false;
      tools.querySelector("summary")?.focus();
      event.preventDefault();
      event.stopImmediatePropagation?.();
    }
  });
  // The language is the core's to resolve, so the page asks before it paints
  // anything of its own; the markup it was authored in stands until then.
  void loadLanguage();
  el("projects-collapse-icon").innerHTML = ICONS.caret;
  el("projects-collapse").onclick = () => setProjectsCollapsed(!state.projectsCollapsed);
  el("projects-options").onclick = () => openOrganizeMenu(el("projects-options"));
  const focusButton = el("filter-chats");
  focusButton.onclick = () => toggleActiveFocus();
  focusButton.addEventListener("mouseenter", () => showFocusTip(focusButton));
  focusButton.addEventListener("focus", () => {
    if (focusButton.matches(":focus-visible")) showFocusTip(focusButton);
  });
  focusButton.addEventListener("mouseleave", hideFocusTip);
  focusButton.addEventListener("blur", hideFocusTip);
  paintActiveFocus();
  const createBtnTree = el("create-project-btn-tree");
  if (createBtnTree) createBtnTree.onclick = openCreateProject;
  const newChatBtn = el("new-chat");
  if (newChatBtn) newChatBtn.onclick = newChatFromSidebar;
  el("project").onclick = openProjects;
  el("projects-add").onclick = () => {
    el("projects-modal").hidden = true;
    openCreateProject();
  };
  el("projects-close").onclick = () => (el("projects-modal").hidden = true);
  el("create-project-add-folder").onclick = addCreateProjectFolder;
  el("create-project-add-row").onclick = addCreateProjectFolder;
  el("create-project-cancel").onclick = () => (el("create-project-modal").hidden = true);
  el("create-project-close").onclick = () => el("create-project-cancel").click();
  el("create-project-save").onclick = saveCreateProject;
  el("create-project-remove").onclick = () => {
    const project = createProjectState.editing;
    if (!project) return;
    el("create-project-modal").hidden = true;
    removeProject(project);
  };

  el("model-menu").addEventListener("keydown", (event) => {
    if (event.key === "Tab") {
      closeModelMenu();
      event.stopPropagation();
      return;
    }
    if (!modelPickerShown() || (event.key !== "ArrowDown" && event.key !== "ArrowUp")) return;
    event.preventDefault();
    const rows = [...el("model-list").children].filter((row) => row.tagName === "BUTTON");
    const index = rows.indexOf(document.activeElement);
    const step = event.key === "ArrowDown" ? 1 : -1;
    rows[(index + step + rows.length) % rows.length]?.focus();
  });
  el("effort-slider").addEventListener("keydown", (event) => {
    if (stepEffort(event.key)) event.preventDefault();
  });
  document.addEventListener("pointerup", () => {
    effortDragging = false;
  });
  el("effort-reset").onclick = () => chooseEffort("auto");
  el("model-page").onclick = () => showModelPicker();
  el("models-back").innerHTML = ICONS.back;
  el("models-back").onclick = () => {
    showModelSummary();
    el("model-page").focus();
  };
  document.addEventListener("click", (event) => {
    if (event.target?.closest?.(".settings-dropdown, .setting-chip")) return;
    closeModelMenu(false);
  });
  window.addEventListener("resize", positionSettingsDropdowns);
  el("model").onclick = () => (modelMenuOpen() ? closeModelMenu() : openModelMenu());
  el("settings-language").onchange = async () => {
    const select = el("settings-language");
    select.disabled = true;
    await switchLanguage(select.value);
    renderSettingsLanguage();
    select.disabled = false;
  };
  el("language-close").onclick = closeLanguage;
  el("permissions").onclick = openPermissions;
  el("help").onclick = toggleHelp;
  el("connect").onclick = openConnect;
  el("settings").onclick = () => (el("settings-modal").hidden ? openSettings() : closeSettings());
  el("settings-search").oninput = () => filterSettingsSections(el("settings-search").value);
  for (const [section, parts] of Object.entries(SETTINGS_SECTIONS)) {
    el(parts.tab).onclick = () => showSettingsSection(section);
    el(parts.tab).addEventListener("keydown", (event) => {
      const names = visibleSettingsSections();
      const index = names.indexOf(section);
      if (index < 0) return;
      const next = event.key === "Home" ? 0 : event.key === "End" ? names.length - 1
        : event.key === "ArrowDown" ? (index + 1) % names.length
        : event.key === "ArrowUp" ? (index + names.length - 1) % names.length : null;
      if (next === null) return;
      event.preventDefault();
      showSettingsSection(names[next]);
      el(SETTINGS_SECTIONS[names[next]].tab).focus();
    });
  }
  for (const [id, view] of [["settings-open-mcps", "mcps"], ["settings-open-plugins", "plugins"], ["settings-open-skills", "skills"]]) {
    el(id).onclick = () => void openCustomize(view);
  }
  el("settings-send-enter").onclick = () => savePrefs({ send_key: "enter" });
  el("settings-send-mod-enter").onclick = () => savePrefs({ send_key: "mod-enter" });
  el("settings-follow-queue").onclick = () => savePrefs({ follow_up: "queue" });
  el("settings-follow-steer").onclick = () => savePrefs({ follow_up: "steer" });
  el("settings-font-smaller").onclick = () => resizeText(-1);
  el("settings-font-reset").onclick = () => resizeText(0);
  el("settings-font-larger").onclick = () => resizeText(1);
  el("settings-terminal-font").onchange = () => {
    void savePrefs({ terminal_font: el("settings-terminal-font").value.trim() });
  };
  el("settings-terminal-size").onchange = () => {
    void savePrefs({
      terminal_font_size: clampTerminalFontSize(el("settings-terminal-size").value),
    });
  };
  el("settings-sidebar").onchange = (event) =>
    savePrefs({ sidebar_visible: Boolean(event.target?.checked) });
  el("settings-predictions").onchange = (event) => {
    void savePrefs({ predictions: Boolean(event.target?.checked) });
    if (!event.target?.checked) clearPrediction();
  };
  for (const mode of ["off", "background", "always"]) {
    const node = el(`settings-notify-${mode}`);
    if (node) node.onclick = () => void savePrefs({ notifications: mode });
  }
  el("settings-notify-approvals").onchange = (event) =>
    savePrefs({ notify_approvals: Boolean(event.target?.checked) });
  el("settings-notify-questions").onchange = (event) =>
    savePrefs({ notify_questions: Boolean(event.target?.checked) });
  el("settings-keep-awake").onchange = (event) => {
    void savePrefs({ keep_awake: Boolean(event.target?.checked) });
    paintAwake();
  };
  el("command-input").addEventListener("input", () => {
    state.commandIndex = 0;
    renderCommandMenu();
  });
  el("command-menu").onclick = (event) => {
    // The box is the dialog; a click on the field around it puts it away.
    if (event.target === el("command-menu")) closeCommandMenu();
  };
  el("search-modal").onclick = (event) => {
    if (event.target === el("search-modal")) closeSearch();
  };
  el("search-chats").onclick = openSearch;
  for (const category of ["all", "actions", "chats"]) {
    el(`search-category-${category}`).onclick = () => setSearchCategory(category);
  }
  el("search-input").addEventListener("keydown", (event) => {
    if (event.key === "ArrowDown" || event.key === "ArrowUp") {
      event.preventDefault();
      state.searchIndex = (state.searchIndex + (event.key === "ArrowDown" ? 1 : -1) + searchChoices.length) % (searchChoices.length || 1);
      paintSearchSelection();
      searchChoices[state.searchIndex]?.row.scrollIntoView?.({ block: "nearest" });
    } else if (event.key === "Enter") {
      event.preventDefault();
      void searchChoices[state.searchIndex]?.run();
    }
  });
  el("search-input").addEventListener("input", () => {
    state.searchIndex = 0;
    searchSoon(el("search-input").value);
  });
  el("archived-close").onclick = closeArchived;
  // The find bar's own keys: Enter walks forward, Shift+Enter back, and Escape
  // puts the bar away without the conversation losing its place.
  el("find-input").addEventListener("keydown", (event) => {
    if (event.key === "Enter") {
      event.preventDefault();
      stepFind(event.shiftKey ? -1 : 1);
    } else if (event.key === "Escape") {
      event.preventDefault();
      closeFind();
    }
  });
  el("find-input").addEventListener("input", () => {
    state.findIndex = -1;
    stepFind(1);
  });
  el("find-next").onclick = () => stepFind(1);
  el("find-prev").onclick = () => stepFind(-1);
  el("find-close").onclick = closeFind;
  el("file-close").onclick = closeFile;
  el("file-mode").onclick = () => {
    state.file.source = !state.file.source;
    paintFile();
  };
  el("review-stage-all").onclick = () => void shapeChange("stage-all");
  el("review-unstage-all").onclick = () => void shapeChange("unstage-all");
  el("terminal-new").onclick = () => void restartTerminal();
  el("terminal-split-right").onclick = () => void splitTerminal("row");
  el("terminal-split-down").onclick = () => void splitTerminal("column");
  el("terminal-clear").onclick = clearTerminal;
  el("terminal-maximize").onclick = toggleTerminalMaximized;
  el("panel-close").onclick = hidePanel;
  window.addEventListener("resize", () => {
    if (!el("panel-terminal").hidden) fitTerminal();
  });
  // The preferences are read before anything else asks what the window should
  // look like, since the sidebar and the scale are drawn from them.
  void loadPrefs().then(() => {
    announceShortcuts();
    paintSettingsControls();
  });

  el("send").onclick = () => send(false);
  el("live-changes").onclick = openRepositoryChanges;
  el("queue-list").onclick = (event) => {
    const menu = event.target.closest?.(".queued-more");
    if (menu) {
      openQueueMenu(menu, Number(menu.dataset.menu));
      return;
    }
    const button = event.target.closest?.("[data-action]");
    if (!button) return;
    void queueAction(
      button.dataset.action,
      Number(button.dataset.index ?? -1),
      Number(button.dataset.direction ?? 0),
    );
  };
  el("queue-menu").onclick = (event) => {
    const button = event.target.closest?.("[data-action]");
    if (!button) return;
    closeQueueMenu();
    void queueAction(
      button.dataset.action,
      Number(button.dataset.index ?? -1),
      Number(button.dataset.direction ?? 0),
    );
  };
  let draggedQueueIndex = -1;
  el("queue-list").ondragstart = (event) => {
    const row = event.target.closest?.(".queued-message");
    draggedQueueIndex = row ? Number(row.dataset.index) : -1;
  };
  el("queue-list").ondragover = (event) => event.preventDefault();
  el("queue-list").ondrop = (event) => {
    event.preventDefault();
    const row = event.target.closest?.(".queued-message");
    const target = row ? Number(row.dataset.index) : -1;
    if (draggedQueueIndex >= 0 && target >= 0 && target !== draggedQueueIndex) {
      void queueAction("move", draggedQueueIndex, target - draggedQueueIndex);
    }
    draggedQueueIndex = -1;
  };
  el("stop").onclick = stop;
  // The header's own line about the turn that is running while another thread is
  // on screen: the click opens that thread, so the Queue or Steer it takes is in
  // reach again.
  el("run-banner").onclick = openRun;
  el("approval-once").onclick = () => answerApproval("once");
  el("approval-always").onclick = () => answerApproval("always");
  el("approval-deny").onclick = () => answerApproval("deny");
  el("question-submit").onclick = () => questionNext();
  el("question-back").onclick = () => showQuestionStep((state.pendingQuestion?.index ?? 0) - 1);
  el("question-dismiss").onclick = () => answerQuestion(true);
  el("login-cancel").onclick = () => (el("connect-modal").hidden = true);
  el("login-save").onclick = saveConnect;
  el("provider-filter").addEventListener("input", renderProviders);
  el("model-filter").addEventListener("input", renderModels);
  el("models-refresh").onclick = () => showModelPicker(true);
  el("themes-close").onclick = () => (el("themes-modal").hidden = true);
  el("permissions-close").onclick = () => (el("permissions-modal").hidden = true);
  el("permissions-clear").onclick = clearApprovals;
  el("trust").onclick = () => {
    if (!state.project) { setStatus(t("composer.selectProject", "Select a project first.")); askForProject(); return; }
    if (state.trust) showTrust(state.trust);
  };
  el("agents").onclick = openAgents;
  el("agents-close").onclick = () => { el("agents-modal").hidden = true; el("agents").focus(); };
  el("trust-allow").onclick = () => answerTrust(true);
  el("trust-deny").onclick = () => answerTrust(false);
  el("confirm-cancel").onclick = () => resolveConfirm(false);
  el("confirm-ok").onclick = () => resolveConfirm(true);
  el("confirm-alt").onclick = () => confirmAlt && resolveConfirm(confirmAlt.value);
  el("rename-cancel").onclick = () => resolveRename(null);
  el("rename-save").onclick = () => resolveRename(el("rename-input").value.trim());
  el("rename-input").addEventListener("keydown", (event) => {
    if (event.key === "Enter") {
      event.preventDefault();
      resolveRename(el("rename-input").value.trim());
    }
  });
  el("mcps-close").onclick = closeCustomize;
  el("mcps-refresh").onclick = () => loadMcps();
  el("worktree").onclick = openWorktrees;
  el("worktrees-close").onclick = () => closeWorktrees();
  el("worktrees-refresh").onclick = () => loadWorktrees();
  el("worktree-create").onclick = createWorktree;
  el("worktree-branch").onclick = createWorktreeBranch;
  el("worktree-handoff").onclick = handoffSession;
  el("worktree-name").addEventListener("keydown", (event) => {
    if (event.key !== "Enter") return;
    event.preventDefault();
    createWorktree();
  });
  // Every step's own way out, which is the step it is on: Dismiss and Later put
  // the dialog away without touching the install, Download in background does
  // the same for a download that is still running, and Cancel is the one that
  // stops it.
  const closeUpdate = () => (el("update-modal").hidden = true);
  el("update-close").onclick = closeUpdate;
  el("update-later").onclick = closeUpdate;
  el("update-background").onclick = closeUpdate;
  el("update-page").onclick = openReleaseNotes;
  el("update-install").onclick = installUpdate;
  el("update-download").onclick = downloadUpdate;
  el("update-cancel").onclick = cancelUpdate;
  el("update-restart").onclick = restartApp;
  el("sessions-close").onclick = () => (el("sessions-modal").hidden = true);
  el("sessions-new").onclick = () => {
    el("sessions-modal").hidden = true;
    newChat();
  };
  el("create-project-add-path").onclick = addCreateProjectTypedPath;
  el("create-project-path").addEventListener("keydown", (event) => {
    if (event.key === "Enter") {
      event.preventDefault();
      addCreateProjectTypedPath();
    }
  });
  el("help-close").onclick = () => (el("help-modal").hidden = true);
  el("review-close").onclick = () => { closeReview(); el("changes-toggle").focus(); };
  el("changes-toggle").onclick = () => {
    if (!el("review-modal").hidden) closeReview();
    else openRepositoryChanges();
  };
  el("details-toggle").onclick = () => {
    if (!el("bottom-panel").hidden) return openProjectDetails();
    state.detailsOpen = !state.detailsOpen;
    paintDockToggles();
  };
  el("details-more").onclick = () => {
    const project = state.projects.find((project) => project.path === state.project);
    if (project) showProjectActions(project, listedSessions().filter((session) => sessionProject(session) === project.path), el("details-more"), el("details-more"));
  };
  el("details-changes").onclick = openRepositoryChanges;
  el("details-add-source").onclick = openSources;
  el("details-view-sources").onclick = openSources;
  el("sources-close").onclick = () => { el("sources-modal").hidden = true; el("details-view-sources").focus(); };
  el("sources-manage").onclick = openMcps;
  el("sources-attach").onclick = () => { el("sources-modal").hidden = true; el("attach-input").click(); };
  el("details-changes-icon").innerHTML = ICONS.changes;
  el("review-repository-icon").innerHTML = ICONS.branch;
  el("details-repository-icon").innerHTML = ICONS.folder;
  el("details-sources-icon").innerHTML = ICONS.globe;
  el("details-view-icon").innerHTML = ICONS.caret;
  el("sources-attach-icon").innerHTML = ICONS.plus;
  el("sources-manage-icon").innerHTML = ICONS.power;
  el("review-scope").onchange = changeReviewScope;
  el("review-refresh").onclick = changeReviewScope;

  // WebKit can spend the first press after editing only moving focus to a
  // control, withholding its click until the next press. That focus default is
  // kept off mousedown so the browser completes that same native click, reading
  // the editor WebKit may have already ended through the `focusout` it
  // dispatched just before the press. When a version still omits the click — or
  // ends the editing session before the page sees the press at all — the next
  // task supplies the one click that was withheld, and never a second one.
  // Question choices use labels around their native controls and need the same
  // treatment. Keyboard focus and activation remain untouched.
  document.addEventListener("focusout", rememberBlurredEditor, true);
  document.addEventListener(
    "mousedown",
    (event) => {
      if (event.button !== 0) return;
      const editor = isTextEditor(document.activeElement)
        ? document.activeElement
        : justBlurredEditor;
      justBlurredEditor = null;
      if (!editor) return;
      const control = pressedControl(event.target);
      if (!control) return;
      event.preventDefault();
      // The press landed on a control rather than in the box the caret is in, so
      // the box gives the caret up with it: keeping it as first responder would
      // leave the reader typing into a box they have moved on from. The rows
      // that complete that box are the exception, and a control that takes the
      // caret itself — a picker focusing its own filter — does so after this
      // press, so its own `focus` is the one that stands.
      if (!completesEditor(control)) editor.blur();
      editorControlPress = { control, clicked: false };
    },
    true,
  );
  document.addEventListener(
    "click",
    (event) => {
      if (editorControlPress && pressedControl(event.target) === editorControlPress.control) {
        editorControlPress.clicked = true;
      }
    },
    true,
  );
  document.addEventListener("mouseup", finishEditorControlPress, true);
  // The reader comes back to the window from a terminal where they may have
  // switched branch: the row's own facts are read again, since nothing here told
  // this window that they changed.
  window.addEventListener("focus", () => {
    loadGit();
    if (!state.busy) reasoningSave.then(() => loadInfo());
  });
  window.addEventListener("blur", () => {
    editorControlPress = null;
    justBlurredEditor = null;
  });

  // The renderer cannot navigate to a remote page, so a link click opens the
  // platform browser through the app instead of reloading the app window.
  document.addEventListener("click", (event) => {
    const target = event.target;
    const inCard = target?.closest?.(".sidebar-floating-card");
    const inTrigger = sidebarCardTrigger && (target === sidebarCardTrigger || sidebarCardTrigger.contains?.(target));
    if (!inCard && !inTrigger) closeSidebarCards();
    const link = event.target?.closest?.("a[href]") || null;
    if (!link) return;
    event.preventDefault();
    invoke("open_url", { url: link.getAttribute("href") }).catch((error) =>
      showToast(t("ui.couldNotOpenLink", "Could not open link: {value0}", { value0: error }), "error"),
    );
  });

  const onPromptInput = () => {
    el("prompt").style.height = "auto";
    el("prompt").style.height = `${Math.min(el("prompt").scrollHeight, 220)}px`;
    updateSendState();
    requestAt();
    // Typing `/` at the start of a message opens the palette; anything with a
    // space in it is arguments, so the menu closes and lets the text through.
    if (el("prompt").value.startsWith("/") && !/\s/.test(el("prompt").value.slice(1))) {
      if (!state.paletteOpen) {
        state.paletteIndex = 0;
        refreshPaletteEntries().then(openPalette);
      } else {
        state.paletteIndex = 0;
        renderPalette();
      }
    } else {
      closePalette();
    }
  };
  el("prompt").addEventListener("input", (event) => {
    if (event.isComposing) return;
    onPromptInput();
  });
  // The last input of an input-method session arrives while it is still
  // composing, so the commit is what sizes the box and enables Send.
  el("prompt").addEventListener("compositionend", () => onPromptInput());
  // The caret moves without the value changing, which a reference the caret has
  // been moved into needs. While the list is up the arrows walk it instead, so
  // the click is what re-asks from there.
  el("prompt").addEventListener("keyup", (event) => {
    if (!atOpen() && CARET_KEYS.has(event.key)) requestAt();
  });
  el("prompt").addEventListener("click", () => requestAt());
  el("prompt").addEventListener("paste", (event) => {
    // A pasted image becomes an attachment; a text paste keeps its default
    // behavior. The platform's own paste shortcut (⌘V / Ctrl+V) triggers this.
    const items = event.clipboardData?.items || [];
    const files = [];
    for (const item of items) {
      if (item.kind === "file" && item.type.startsWith("image/")) {
        const file = item.getAsFile();
        if (file) files.push(file);
      }
    }
    if (files.length) {
      event.preventDefault();
      addAttachmentFiles(files, { fallbackToClipboard: true });
    }
  });
  el("attach").onclick = () => el("attach-input").click();
  el("attach-input").addEventListener("change", async (event) => {
    await addAttachmentFiles(Array.from(event.target.files || []));
    event.target.value = "";
  });
  el("image-view-close").onclick = closeImage;
  el("image-modal").onclick = (event) => {
    if (event.target === el("image-modal")) closeImage();
  };

  // A dialog is dismissed by a press on the room around it, not only by its own
  // button: the backdrop is the app, and a press there means "not this". Each
  // one answers through the button that already means close, cancel or dismiss,
  // so the teardown a dialog does — a promise resolved as cancelled, a field
  // cleared, the caret handed back — is the same either way. A dialog whose only
  // buttons are decisions (the approval a tool waits on, the trust question a
  // folder waits on) has no such button and so no backdrop to press: a stray
  // press must not be an answer.
  const BACKDROP_DISMISS = {
    "agents-modal": "agents-close",
    "sources-modal": "sources-close",
    "confirm-modal": "confirm-cancel",
    "connect-modal": "login-cancel",
    "create-project-modal": "create-project-cancel",
    "help-modal": "help-close",
    "language-modal": "language-close",
    "permissions-modal": "permissions-close",
    "question": "question-dismiss",
    "rename-modal": "rename-cancel",
    "archived-modal": "archived-close",
    "file-modal": "file-close",
    "themes-modal": "themes-close",
    "update-modal": "update-close",
  };
  document.addEventListener("click", (event) => {
    const pressed = event.target?.id ? BACKDROP_DISMISS[event.target.id] : null;
    if (pressed) el(pressed).click();
  });

  // The three listings that open above the composer have no backdrop of their
  // own: what is outside them is the rest of the window, so a press anywhere
  // else closes them. It is taken on the press rather than on its click, since
  // the click of the control that opens one is still to come — closing there
  // would shut the listing in the same breath it was asked for.
  document.addEventListener("mousedown", (event) => {
    const target = event.target;
    if (!target || typeof target.id !== "string") return;
    for (const [control, [id]] of Object.entries(RAIL_PANELS)) {
      const panel = el(id);
      if (panel.hidden || !panel.classList.contains("rail-panel") || panel.contains(target) || target.closest?.(".navigation-rail")) continue;
      if (id === "search-modal") closeSearch();
      else if (id === "command-menu") closeCommandMenu();
      else panel.hidden = true;
      el(control).setAttribute("aria-expanded", "false");
    }
    for (const id of ["projects-modal", "worktrees-modal", "sessions-modal"]) {
      const box = el(id);
      if (box.hidden || box.contains(target) || target.closest?.(".navigation-rail")) continue;
      box.hidden = true;
    }
    // The worktree chip says whether its listing is up, and this is the one
    // closing that does not go through the control itself.
    el("worktree").setAttribute("aria-expanded", String(!el("worktrees-modal").hidden));
    syncRailPanels();
  });
  el("image-modal").addEventListener("keydown", (event) => {
    if (event.key !== "Tab") return;
    event.preventDefault();
    el("image-view-close").focus();
  });
  el("prompt").addEventListener("input", () => clearPrediction());
  el("prediction").onclick = () => acceptPrediction();
  el("prompt").addEventListener("keydown", (event) => {
    if (event.isComposing) return;
    if (event.defaultPrevented) return;
    if (atKey(event)) {
      event.preventDefault();
      return;
    }
    if (paletteKey(event)) {
      event.preventDefault();
      return;
    }
    if (event.key === "Tab" && !event.shiftKey && state.prediction) {
      event.preventDefault();
      acceptPrediction();
      return;
    }
    if (event.key === "ArrowUp" && !event.isComposing && !event.metaKey && !event.ctrlKey && !event.altKey && !event.shiftKey && el("prompt").value === "") {
      const users = [...el("transcript").querySelectorAll(".msg.user")];
      const previous = lastInputs.get(inputHistoryKey()) || users.at(-1)?.dataset.prompt;
      if (previous) {
        event.preventDefault();
        el("prompt").value = previous;
        el("prompt").setSelectionRange(previous.length, previous.length);
        el("prompt").style.height = "auto";
        el("prompt").style.height = `${Math.min(el("prompt").scrollHeight, 220)}px`;
        updateSendState();
      }
      return;
    }
    if (event.key === "Escape") {
      closePalette();
      return;
    }
    if (event.key === "Enter" && (event.metaKey || event.ctrlKey)) {
      event.preventDefault();
      clearPrediction();
      send(Boolean(event.shiftKey) && state.busy);
      return;
    }
    if (event.key === "Enter" && !event.shiftKey) {
      // Which key sends is the window's own preference: with ⌘Enter sending,
      // Enter is the newline Shift+Enter is in the other mode.
      if (state.prefs.send_key === "mod-enter") return;
      event.preventDefault();
      clearPrediction();
      send(false);
    }
  });

  document.addEventListener("keydown", (event) => {
    // A key pressed into a row that is waiting for one is that row's binding,
    // not an action: the sheet says what it heard instead of doing it.
    if (recordShortcut(event)) {
      event.preventDefault();
      return;
    }
    if (event.defaultPrevented) return;
    if (event.key === "Escape") {
      // The catalog is the menu's second page: Escape steps back to the
      // summary first, the way the Tauri window's does.
      if (modelPickerShown()) {
        event.preventDefault();
        showModelSummary();
        return;
      }
      const railControl = Object.entries(RAIL_PANELS).find(([, [id]]) => !el(id).hidden && el(id).classList.contains("rail-panel"))?.[0];
      el("message-preview").hidden = true;
      const modalOpen = OVERLAYS.some((id) => !el(id).hidden);
      closeSidebarCards();
      closeImage();
      closeModelMenu();
      closeSettings();
      closeCommandMenu();
      closeActivity();
      closeArchived();
      closeSearch();
      closeFind();
      closeFile();
      if (!el("confirm-modal").hidden) resolveConfirm(false);
      if (!el("rename-modal").hidden) resolveRename(null);
      // A question dismissed with Escape is answered as unanswered rather than
      // hidden, so the turn continues instead of waiting out the timeout.
      if (!el("question").hidden) answerQuestion(true);
      closeOverlays();
      if (railControl) el(railControl).focus();
      if (!modalOpen) {
        if (!el("projects-page").hidden) { closeProjectDirectory(); el("rail-projects").focus(); }
        closeReview();
      }
      return;
    }
    if (event.key === "Tab" && event.shiftKey) {
      if (event.target?.closest?.(".changes-pane, .details-pane")) return;
      // Shift+Tab belongs to the model menu while it is up, which Tab closes,
      // rather than cycling the levels under a reader who is choosing one.
      if (modelMenuOpen()) return;
      event.preventDefault();
      cycleReasoning();
      return;
    }
    // The command menu takes the arrows and Enter while it is up, and its input
    // takes everything else as a search.
    if (!el("command-menu").hidden) {
      if (event.key === "ArrowDown" || event.key === "ArrowUp") {
        event.preventDefault();
        const rows = state.commandRows;
        if (rows.length) {
          const step = event.key === "ArrowDown" ? 1 : -1;
          state.commandIndex = (state.commandIndex + step + rows.length) % rows.length;
          renderCommandMenu();
        }
        return;
      }
      if (event.key === "Enter") {
        event.preventDefault();
        runCommand(state.commandRows[state.commandIndex]);
        return;
      }
    }
    const action = matchShortcut(event);
    if (action) {
      event.preventDefault();
      void action.run();
      return;
    }
    // The numbered rows are the tree's, not the table's: they follow the order
    // the sidebar lists, which changes as threads arrive.
    if ((event.metaKey || event.ctrlKey) && event.key >= "1" && event.key <= "9") {
      const session = orderedSessions()[Number(event.key) - 1];
      if (session) {
        event.preventDefault();
        selectSessionFromTree(session);
      }
    }
  });

  updateChips();
  // The window starts idle: the badge's own icon is what says so, with the word
  // it stands for in its tooltip.
  setStatus(idleStatus());
  updateSendState();
  renderWelcome();
  paintDockToggles();
  const eventsReady = initEvents();
  loadTheme();
  // The sidebar is loaded rather than opened on: a project is not selected for
  // the reader on the way in, so the window starts with nothing open — the home
  // transcript, the composer and the project chip that takes a folder.
  loadProjects();
  return eventsReady;
}

Promise.all([init(), tauriListener])
  .then(async () => {
    pageEventsReady = true;
    drainPendingHostMessages();
    if (nativeTauri?.core?.invoke) {
      await nativeTauri.core.invoke("oxide_message", {
        message: JSON.stringify({
          type: "request",
          id: nextRequest++,
          method: "oxide_invoke",
          params: { command: "native_ready", args: {} },
        }),
      });
      drainPendingHostMessages();
    }
  })
  .catch((error) => {
    console.error("Oxide could not start its window:", error);
  });

// ============================================================================
// CodeX-style tree view rendering
// ============================================================================

// Flattened project/session order, matching how the tree renders them, so
// `⌘1`…`⌘9` select the thread standing at that place in the sidebar.
function orderedSessions() {
  if (state.sidebarView === "list") return arrangedSessions(listedSessions());
  const byProject = new Map();
  for (const project of state.projects) byProject.set(project.path, []);
  for (const session of listedSessions()) {
    const list = byProject.get(sessionProject(session));
    if (list) list.push(session);
  }
  const ordered = [];
  for (const project of state.projects) {
    ordered.push(...arrangedSessions(byProject.get(project.path) || []));
  }
  return ordered;
}

/// The project whose row is in the air, and the row it would land on. A drop is
/// a move of the whole row — the project and the threads under it — rather than
/// of a thread, which is why this is one id and not a list.
let draggedProjectId = null;
let draggedSessionId = null;
let draggedSessionProject = "";
let dropMarkedRow = null;

/// The projects in the order a drop would leave them: the dragged row taken out
/// and put back beside the row it was dropped on, above or below that row's own
/// middle. `null` when the row is the one being dragged, when either is no
/// longer in the list, or when the drop would leave the order as it is — so a
/// drag that ends where it started writes nothing.
function reorderedProjects(projects, fromId, toId, after) {
  if (!fromId || fromId === toId) return null;
  const moved = projects.find((project) => project.id === fromId);
  const rest = projects.filter((project) => project.id !== fromId);
  const at = rest.findIndex((project) => project.id === toId);
  if (!moved || at < 0) return null;
  rest.splice(after ? at + 1 : at, 0, moved);
  if (rest.every((project, index) => project.id === projects[index].id)) return null;
  return rest;
}

/// Where a drop on this row would land: below its own middle, or above it. It is
/// `null` when nothing is being dragged and when the row is the one in the air,
/// which is the row that cannot be dropped onto itself.
function dropAfter(event, row) {
  if (!draggedProjectId || draggedProjectId === row.dataset.project) return null;
  const box = row.getBoundingClientRect();
  return event.clientY > box.top + box.height / 2;
}

function sessionDropAfter(event, row) {
  if (
    state.chatSort !== "manual" ||
    !draggedSessionId ||
    draggedSessionId === row.dataset.session ||
    (state.sidebarView !== "list" && draggedSessionProject !== row.dataset.project)
  ) {
    return null;
  }
  const box = row.getBoundingClientRect();
  return event.clientY > box.top + box.height / 2;
}

/// The line the row would land on, which is the only mark a drag leaves: the
/// sidebar is not rebuilt while a row is in the air, so the mark is kept as the
/// row it was put on rather than looked up again.
function markDrop(row, after) {
  clearDropMark();
  dropMarkedRow = row;
  row.classList.add(after ? "drop-after" : "drop-before");
}

function clearDropMark() {
  if (!dropMarkedRow) return;
  dropMarkedRow.classList.remove("drop-before", "drop-after");
  dropMarkedRow = null;
}

/// Whether a node is the row itself or one of the things it is made of: a drag
/// across a row crosses its own children, and each of those is a `dragleave` of
/// the row around it.
function isInside(node, root) {
  for (let each = node; each; each = each.parentNode) {
    if (each === root) return true;
  }
  return false;
}

function clearDropMarkOf(row) {
  if (dropMarkedRow !== row) return;
  clearDropMark();
}

/// Puts a dragged project where it was dropped, and the threads under it with
/// it: the sidebar is painted from the new order at once — a drop that waited on
/// the disk would show the row snapping back and forth — and the app is what
/// makes that order the sidebar's own. A call that fails puts the order it had
/// back and says why, rather than leaving the window showing an arrangement that
/// was never written.
async function dropProject(fromId, toId, after) {
  const next = reorderedProjects(state.projects, fromId, toId, after);
  if (!next) return;
  const before = state.projects;
  state.projects = next;
  renderProjectsTree();
  try {
    state.projects = await invoke("reorder_projects", {
      ids: next.map((project) => project.id),
    });
  } catch (error) {
    state.projects = before;
    showToast(t("ui.couldNotReorderProjects", "Could not reorder projects: {value0}", { value0: error }), "error");
  }
  renderProjectsTree();
}

function dropSession(fromId, toId, after) {
  if (!fromId || !toId || fromId === toId) return;
  const order = state.manualChatOrder.length
    ? [...state.manualChatOrder]
    : updatedChatOrder(listedSessions());
  const fromIndex = order.indexOf(fromId);
  if (fromIndex < 0) return;
  order.splice(fromIndex, 1);
  const targetIndex = order.indexOf(toId);
  if (targetIndex < 0) return;
  order.splice(after ? targetIndex + 1 : targetIndex, 0, fromId);
  saveManualChatOrder(order);
  renderProjectsTree();
}

let sidebarCardTrigger = null;
let sidebarCardHoverTimer = null;
let sidebarCardReturnFocus = false;
let sidebarCardOrigin = "";

function cancelSidebarCardHover() {
  if (sidebarCardHoverTimer !== null) {
    clearTimeout(sidebarCardHoverTimer);
    sidebarCardHoverTimer = null;
  }
}

function closeSidebarCards(except = null) {
  cancelSidebarCardHover();
  closeSidebarSubmenu();
  for (const card of document.body.querySelectorAll(".sidebar-floating-card")) {
    if (card !== except) card.remove();
  }
  if (except) return;
  const trigger = sidebarCardTrigger;
  const returnFocus = sidebarCardReturnFocus;
  sidebarCardTrigger = null;
  sidebarCardReturnFocus = false;
  sidebarCardOrigin = "";
  if (trigger) {
    trigger.setAttribute("aria-expanded", "false");
    // Closing a menu returns the keyboard to the control that opened it. An
    // action may immediately open a dialog or repaint the row; those operations
    // move focus on from here themselves.
    if (returnFocus && trigger.parentNode) trigger.focus();
  }
}

function placeSidebarCard(card, row) {
  document.body.appendChild(card);
  const rect = row.getBoundingClientRect();
  const width = card.offsetWidth || 292;
  card.style.left = `${Math.min(rect.right + 8, window.innerWidth - width - 10)}px`;
  card.style.top = `${Math.max(10, Math.min(rect.top, window.innerHeight - (card.offsetHeight || 150) - 10))}px`;
}

function projectActivity(project, sessions) {
  const active = state.busy && state.runProject === project.path ? 1 : 0;
  return t(sessions.length === 1 ? "sidebar.taskCountOne" : "sidebar.taskCount", sessions.length === 1 ? "{count} task" : "{count} tasks", { count: sessions.length }) +
    (active ? ` · ${t("sidebar.activeCount", "{count} active", { count: active })}` : "");
}

function showProjectActions(project, sessions, row, trigger, focusMenu = true) {
  closeSidebarCards();
  sidebarCardTrigger = trigger;
  sidebarCardReturnFocus = focusMenu;
  sidebarCardOrigin = focusMenu ? "click" : "hover";
  trigger.setAttribute("aria-expanded", "true");
  const card = document.createElement("div");
  card.id = "project-actions-menu";
  card.className = "sidebar-floating-card project-actions-card";
  card.setAttribute("role", "menu");
  card.setAttribute("aria-label", t("ui.projectActions", "{value0} project actions", { value0: project.name }));
  // A hover card remains under the pointer when it crosses the narrow gap from
  // the sidebar. Once the pointer leaves both surfaces it closes; a card opened
  // with the ellipsis stays up until the menu is dismissed.
  card.addEventListener("mouseenter", cancelSidebarCardHover);
  card.addEventListener("mouseleave", () => {
    if (sidebarCardOrigin !== "hover") return;
    sidebarCardHoverTimer = setTimeout(closeSidebarCards, 120);
  });
  const icon = (drawing, className = "") => {
    const node = document.createElement("span");
    node.className = className;
    node.innerHTML = drawing;
    return node;
  };
  const head = document.createElement("div");
  head.className = "sidebar-card-head";
  head.setAttribute("role", "presentation");
  const heading = document.createElement("strong");
  heading.textContent = project.name;
  head.append(icon(ICONS.folder, "sidebar-card-icon"), heading);
  const pin = document.createElement("button");
  pin.type = "button";
  pin.className = "sidebar-card-pin";
  pin.title = project.pinned ? t("ui.unpinProject", "Unpin project") : t("ui.pinProject", "Pin project");
  pin.setAttribute("aria-label", pin.title);
  pin.setAttribute("role", "menuitem");
  pin.innerHTML = ICONS.pin;
  const meta = document.createElement("div");
  meta.className = "sidebar-card-meta";
  meta.setAttribute("role", "presentation");
  const metaText = document.createElement("span");
  metaText.textContent = projectActivity(project, sessions);
  meta.append(icon(ICONS.chat), metaText);
  const path = document.createElement("div");
  path.className = "sidebar-card-row sidebar-card-path";
  path.setAttribute("role", "presentation");
  const pathText = document.createElement("span");
  pathText.textContent = project.display_path || project.path;
  path.append(icon(ICONS.folder), pathText);
  const action = (className, drawing, label) => {
    const button = document.createElement("button");
    button.type = "button";
    button.className = `sidebar-card-row sidebar-card-action ${className}`;
    button.setAttribute("role", "menuitem");
    const text = document.createElement("span");
    text.textContent = label;
    button.append(icon(drawing), text);
    return button;
  };
  const edit = project.registered ? action("edit-project", ICONS.gear, t("ui.editProject", "Edit project")) : null;
  const remove = project.registered ? null : action("remove-project", ICONS.close, t("dialog.createProject.remove", "Remove project"));
  card.append(head, pin, meta, path);
  if (edit) card.append(edit);
  if (remove) card.append(remove);
  const menuItems = [pin, ...(edit ? [edit] : []), ...(remove ? [remove] : [])];
  pin.onclick = async () => {
    try {
      state.projects = await invoke("set_project_pinned", { id: project.id, pinned: !project.pinned });
      closeSidebarCards();
      renderProjectsTree();
    } catch (error) {
      showToast(t("ui.couldNotProject", "Could not {value0} project: {value1}", { value0: project.pinned ? t("ui.unpin", "unpin") : t("ui.pin", "pin"), value1: error }), "error");
    }
  };
  if (edit) edit.onclick = () => openEditProject(project);
  if (remove) {
    remove.onclick = () => {
      closeSidebarCards();
      removeProject(project);
    };
  }
  card.addEventListener("keydown", (event) => {
    const index = menuItems.indexOf(document.activeElement);
    let next = null;
    if (event.key === "ArrowDown") {
      next = menuItems[(index + 1 + menuItems.length) % menuItems.length];
    } else if (event.key === "ArrowUp") {
      next = menuItems[(index - 1 + menuItems.length) % menuItems.length];
    } else if (event.key === "Home") next = menuItems[0];
    else if (event.key === "End") next = menuItems[menuItems.length - 1];
    else if (event.key === "Escape") {
      event.preventDefault();
      event.stopPropagation();
      closeSidebarCards();
      return;
    } else if (event.key === "Tab") {
      closeSidebarCards();
      return;
    } else {
      return;
    }
    event.preventDefault();
    next?.focus();
  });
  placeSidebarCard(card, row);
  if (focusMenu) pin.focus();
}

async function showThreadDetails(session, row) {
  closeSidebarCards();
  const card = document.createElement("div");
  card.className = "sidebar-floating-card thread-details-card";
  const head = document.createElement("div");
  head.className = "thread-detail-head";
  const title = document.createElement("div");
  title.className = "thread-detail-title";
  title.textContent = sessionLabel(session);
  const computer = document.createElement("span");
  computer.className = "thread-detail-computer";
  computer.innerHTML = ICONS.computer;
  const age = document.createElement("span");
  age.className = "thread-detail-age";
  age.textContent = sessionAge(session.modified_at || session.created_at);
  head.append(title, computer, age);
  const project = document.createElement("div");
  project.className = "thread-detail-project";
  const folder = document.createElement("span");
  folder.innerHTML = ICONS.folder;
  const projectName = document.createElement("span");
  projectName.textContent = projectNameOf(sessionProject(session));
  project.append(folder, projectName);
  card.append(head, project);
  if (session.worktree) {
    const checkout = document.createElement("div");
    checkout.className = "thread-detail-worktree";
    const checkoutIcon = document.createElement("span");
    checkoutIcon.innerHTML = ICONS.worktree;
    const checkoutName = document.createElement("span");
    checkoutName.textContent = session.worktree;
    checkout.append(checkoutIcon, checkoutName);
    card.appendChild(checkout);
  }
  placeSidebarCard(card, row);
  try {
    const info = await invoke("git_info", { project: session.cwd });
    if (!card.parentNode || !info?.repo) return;
    const branchName = info.branch || info.detached;
    if (!branchName) return;
    const branch = document.createElement("div");
    branch.className = "thread-detail-branch";
    const branchIcon = document.createElement("span");
    branchIcon.innerHTML = ICONS.branch;
    const branchText = document.createElement("span");
    branchText.textContent = branchName;
    branch.append(branchIcon, branchText);
    card.appendChild(branch);
    placeSidebarCard(card, row);
  } catch {
    // A folder outside a repository has no branch row, like Codex.
  }
}

async function pinSession(session) {
  try {
    state.sessions = await invoke("set_session_pinned", { id: session.id, pinned: !session.pinned });
    renderProjectsTree();
    repaintWelcome();
  } catch (error) {
    showToast(t("ui.couldNotThread", "Could not {value0} thread: {value1}", { value0: session.pinned ? t("ui.unpin", "unpin") : t("ui.pin", "pin"), value1: error }), "error");
  }
}

async function archiveSession(session) {
  if (!session || session.unstored) return;
  // A turn's own thread cannot be archived from anywhere: its reply would have
  // nowhere to land, and the files it is writing are still its own.
  if (turnInThread(session.id)) {
    showToast(t("ui.aTurnIsRunningStopItBeforeArchivingThisThread", "A turn is running; stop it before archiving this thread."));
    return;
  }
  try {
    state.sessions = await invoke("archive_session", { id: session.id });
    for (let index = chatTabs.length - 1; index >= 0; index--) {
      if (chatTabs[index].id !== activeTabId && chatTabs[index].session === session.id) chatTabs.splice(index, 1);
    }
    paintChatTabs();
    if (state.session === session.id) resetTranscript();
    updateRunBanner();
    closeSidebarCards();
    renderProjectsTree();
    repaintWelcome();
    showToast(t("ui.chatArchived", "Chat archived"), "info", {
      label: t("ui.undo", "Undo"),
      run: async () => {
        state.sessions = await invoke("unarchive_session", { id: session.id, pinned: Boolean(session.pinned) });
        renderProjectsTree();
        repaintWelcome();
      },
    });
  } catch (error) {
    showToast(t("ui.couldNotArchiveThread", "Could not archive thread: {value0}", { value0: error }), "error");
  }
}

function sidebarPref(key, fallback) {
  try {
    return localStorage.getItem(`oxide.sidebar.${key}`) || fallback;
  } catch {
    return fallback;
  }
}

function setSidebarPref(key, value) {
  state[key] = value;
  try {
    localStorage.setItem(`oxide.sidebar.${key}`, value);
  } catch {}
}

function saveManualChatOrder(order) {
  state.manualChatOrder = order;
  try {
    localStorage.setItem("oxide.sidebar.manualChatOrder", JSON.stringify(order));
  } catch {}
}

function updatedChatOrder(sessions) {
  return [...sessions]
    .sort((a, b) => Number(b.modified_at || b.created_at || 0) - Number(a.modified_at || a.created_at || 0))
    .map((session) => session.id);
}

function manualOrderFor(sessions) {
  const original = new Map(sessions.map((session, index) => [session.id, index]));
  const position = new Map(
    state.manualChatOrder.map((id, index) => [id, index]),
  );
  return [...sessions].sort((a, b) => {
    const aIndex = position.get(a.id);
    const bIndex = position.get(b.id);
    if (aIndex !== undefined && bIndex !== undefined) return aIndex - bIndex;
    if (aIndex !== undefined) return -1;
    if (bIndex !== undefined) return 1;
    return (original.get(a.id) || 0) - (original.get(b.id) || 0);
  });
}

/// Threads in the order and subset the sidebar's own controls ask for.
function arrangedSessions(sessions) {
  let rows = sessions;
  if (state.chatFilter === "running") rows = rows.filter((session) => turnInThread(session.id));
  else if (state.chatFilter === "pulls") rows = rows.filter((session) => pullOf(session));
  if (state.chatSort === "manual") return manualOrderFor(rows);
  const field = state.chatSort === "created" ? "created_at" : "modified_at";
  return [...rows].sort((a, b) => Number(b[field] || 0) - Number(a[field] || 0));
}

function sidebarMenuItem(
  label,
  { trail = "", checked = false, hasPopup = false, onclick },
) {
  const item = document.createElement("button");
  item.type = "button";
  item.className = "sidebar-menu-item";
  item.setAttribute("role", checked ? "menuitemradio" : "menuitem");
  if (checked) item.setAttribute("aria-checked", "true");
  if (hasPopup) {
    item.setAttribute("aria-haspopup", "menu");
    item.setAttribute("aria-expanded", "false");
  }
  const text = document.createElement("span");
  text.textContent = label;
  item.append(text);
  const mark = document.createElement("span");
  mark.className = "sidebar-menu-trail";
  mark.innerHTML = trail || (checked ? ICONS.check : "");
  item.append(mark);
  item.onclick = (event) => {
    event.stopPropagation();
    onclick(item);
  };
  return item;
}

let sidebarSubmenuCard = null;
let sidebarSubmenuParent = null;

function closeSidebarSubmenu() {
  sidebarSubmenuCard?.remove();
  sidebarSubmenuParent?.setAttribute("aria-expanded", "false");
  sidebarSubmenuCard = null;
  sidebarSubmenuParent = null;
}

function showSidebarSubmenu(parent, items, rootCard) {
  if (sidebarSubmenuParent === parent && sidebarSubmenuCard) {
    closeSidebarSubmenu();
    return;
  }
  closeSidebarSubmenu();
  sidebarSubmenuParent = parent;
  parent.setAttribute("aria-expanded", "true");
  const card = document.createElement("div");
  card.className = "sidebar-floating-card sidebar-submenu-card";
  card.setAttribute("role", "menu");
  card.append(...items);
  document.body.appendChild(card);
  const parentRect = parent.getBoundingClientRect();
  const rootRect = rootCard.getBoundingClientRect();
  const width = card.offsetWidth || 200;
  const left = rootRect.right + 5;
  card.style.left = `${left + width <= window.innerWidth - 10 ? left : Math.max(8, rootRect.left - width - 5)}px`;
  card.style.top = `${Math.max(8, parentRect.top - 5)}px`;
  card.addEventListener("keydown", (event) => {
    if (event.key === "Escape") closeSidebarCards();
  });
  card.querySelector("button")?.focus();
  sidebarSubmenuCard = card;
}

/// A small menu under a sidebar button. A row that carries its own choices
/// opens them beside the menu rather than replacing the list the reader is in.
function showSidebarMenu(trigger, build) {
  if (trigger.getAttribute("aria-expanded") === "true") {
    closeSidebarCards();
    return;
  }
  closeSidebarCards();
  sidebarCardTrigger = trigger;
  sidebarCardReturnFocus = true;
  sidebarCardOrigin = "click";
  trigger.setAttribute("aria-expanded", "true");
  const card = document.createElement("div");
  card.className = "sidebar-floating-card sidebar-menu-card";
  card.setAttribute("role", "menu");
  const close = closeSidebarCards;
  const openSubmenu = (parent, items) => showSidebarSubmenu(parent, items, card);
  card.append(...build(openSubmenu, close));
  document.body.appendChild(card);
  const rect = trigger.getBoundingClientRect();
  card.style.left = `${Math.max(8, Math.min(rect.left, window.innerWidth - card.offsetWidth - 10))}px`;
  card.style.top = `${rect.bottom + 6}px`;
  card.addEventListener("keydown", (event) => {
    if (event.key === "Escape") closeSidebarCards();
  });
  card.querySelector("button")?.focus();
}

function openOrganizeMenu(trigger) {
  showSidebarMenu(trigger, (openSubmenu, close) => {
    const chevron = ICONS.chevronRight;
    const root = () => [
      sidebarMenuItem(t("sidebar.organizeSidebar", "Organize sidebar"), {
        trail: chevron,
        hasPopup: true,
        onclick: (item) => openSubmenu(item, organize()),
      }),
      sidebarMenuItem(t("sidebar.sortChatsBy", "Sort chats by"), {
        trail: chevron,
        hasPopup: true,
        onclick: (item) => openSubmenu(item, sort()),
      }),
    ];
    const organize = () => [
      ...[
        ["project", t("sidebar.byProject", "By project")],
        ["list", t("sidebar.inOneList", "In one list")],
      ].map(([value, label]) =>
        sidebarMenuItem(label, {
          checked: state.sidebarView === value,
          onclick: () => {
            setSidebarPref("sidebarView", value);
            close();
            renderProjectsTree();
          },
        }),
      ),
      sidebarMenuItem(t("sidebar.expandAll", "Expand all projects"), {
        onclick: () => {
          state.collapsedSidebarProjects.clear();
          close();
          renderProjectsTree();
        },
      }),
      sidebarMenuItem(t("sidebar.collapseAll", "Collapse all projects"), {
        onclick: () => {
          for (const project of state.projects || []) state.collapsedSidebarProjects.add(project.id);
          close();
          renderProjectsTree();
        },
      }),
    ];
    const sort = () =>
      [
        ["updated", t("sidebar.sortUpdated", "Last updated")],
        ["manual", t("sidebar.sortManual", "Manual")],
      ].map(([value, label]) =>
        sidebarMenuItem(label, {
          checked: state.chatSort === value,
          onclick: () => {
            if (value === "manual" && state.chatSort !== "manual") {
              saveManualChatOrder(updatedChatOrder(listedSessions()));
            }
            setSidebarPref("chatSort", value);
            close();
            renderProjectsTree();
          },
        }),
      );
    return root();
  });
}

function focusLabel() {
  return state.chatFilter === "running"
    ? t("sidebar.showAllChats", "Show all chats")
    : t("sidebar.focusActive", "Focus on active chats");
}

function paintActiveFocus() {
  const button = el("filter-chats");
  if (!button) return;
  const on = state.chatFilter === "running";
  button.classList.toggle("active", on);
  button.setAttribute("aria-pressed", String(on));
  button.setAttribute("aria-label", focusLabel());
  button.removeAttribute("title");
}

function toggleActiveFocus() {
  hideFocusTip();
  state.chatFilter = state.chatFilter === "running" ? "all" : "running";
  paintActiveFocus();
  renderProjectsTree();
}

let focusTip = null;

function hideFocusTip() {
  focusTip?.remove();
  focusTip = null;
}

/// The words sit in a tip of the window's own so the chord can be drawn beside
/// them as keys; a native tooltip is one string.
function showFocusTip(button) {
  hideFocusTip();
  const action = SHORTCUTS.find((entry) => entry.id === "focus-active");
  const tip = document.createElement("div");
  tip.className = "sidebar-tip";
  tip.setAttribute("role", "tooltip");
  const text = document.createElement("span");
  text.textContent = focusLabel();
  const chord = document.createElement("kbd");
  chord.textContent = formatChord(shortcutChords(action)[0] || "");
  tip.append(text, chord);
  document.body.appendChild(tip);
  const rect = button.getBoundingClientRect();
  tip.style.left = `${Math.max(8, Math.min(rect.left - 20, window.innerWidth - tip.offsetWidth - 8))}px`;
  tip.style.top = `${rect.bottom + 6}px`;
  focusTip = tip;
}

function setProjectsCollapsed(collapsed) {
  state.projectsCollapsed = collapsed;
  el("sidebar-projects").classList.toggle("projects-collapsed", collapsed);
  el("projects-collapse").setAttribute("aria-expanded", String(!collapsed));
}

async function renderProjectsTree() {
  if (!el("projects-page").hidden) renderProjectDirectory();
  const container = el("projects-tree");
  if (!container) return;
  closeSidebarCards();
  if (!el("activity-panel").hidden) renderActivity();
  
  container.innerHTML = "";
  
  if (!state.projects || !state.projects.length) {
    container.innerHTML = `<div class="empty" style="margin:12px; font-size:12px; color:var(--faint);">${escapeHtml(t("desktop.empty.sidebar", "No projects yet. Add a folder with the + above."))}</div>`;
    return;
  }
  
  // Group sessions by project
  const sessionsByProject = {};
  const visibleProjects = state.projects;

  for (const project of visibleProjects) {
    sessionsByProject[project.path] = [];
  }
  
  for (const session of listedSessions()) {
    // A thread in a worktree is the project's own thread: the host names the
    // project it was made from, so it is listed under that project rather than
    // under a row of its own per checkout.
    const project = sessionProject(session);
    if (sessionsByProject[project]) {
      sessionsByProject[project].push(session);
    }
  }
  
  for (const [projectIndex, project] of (state.sidebarView === "list" ? [] : visibleProjects).entries()) {
    const projectGroup = document.createElement("div");
    projectGroup.className = "project-group";
    projectGroup.setAttribute("role", "group");
    
    // Project header/button
    const projectItem = document.createElement("div");
    projectItem.className = "project-item" + (project.path === state.project ? " active" : "");
    projectItem.setAttribute("role", "treeitem");
    projectItem.setAttribute("tabindex", "0");
    
    const icon = document.createElement("div");
    icon.className = "icon";
    icon.innerHTML = ICONS.folder;

    const collapsed = state.collapsedSidebarProjects.has(project.id);
    projectItem.setAttribute("aria-expanded", String(!collapsed));
    projectItem.setAttribute(
      "aria-label",
      t("ui.toggleProjectChats", "Expand or collapse chats in {value0}", { value0: project.name }),
    );
    projectItem.appendChild(icon);
    
    const name = document.createElement("div");
    name.className = "name";
    name.textContent = project.name;
    projectItem.appendChild(name);
    
    const count = document.createElement("div");
    count.className = "count";
    const sessionsForProject = arrangedSessions(sessionsByProject[project.path] || []);
    count.textContent = sessionsForProject.length;
    projectItem.appendChild(count);

    const moreBtn = document.createElement("button");
    moreBtn.type = "button";
    moreBtn.className = "row-more";
    moreBtn.title = t("ui.projectOptionsFor", "Project options for {value0}", { value0: project.name });
    moreBtn.setAttribute("aria-label", moreBtn.title);
    moreBtn.setAttribute("aria-haspopup", "menu");
    moreBtn.setAttribute("aria-controls", "project-actions-menu");
    moreBtn.setAttribute("aria-expanded", "false");
    moreBtn.innerHTML = ICONS.more;
    moreBtn.onclick = (event) => {
      event.stopPropagation();
      if (moreBtn.getAttribute("aria-expanded") === "true") {
        // The hover preview is already the right card. Clicking the ellipsis
        // promotes it to the persistent keyboard menu instead of making the
        // first click appear to close the control it was meant to open.
        if (sidebarCardOrigin === "hover") {
          cancelSidebarCardHover();
          sidebarCardOrigin = "click";
          sidebarCardReturnFocus = true;
          document.body.querySelector(".project-actions-card .sidebar-card-pin")?.focus();
          return;
        }
        closeSidebarCards();
        return;
      }
      showProjectActions(project, sessionsForProject, projectItem, moreBtn);
    };
    projectItem.appendChild(moreBtn);

    const newTaskBtn = document.createElement("button");
    newTaskBtn.type = "button";
    newTaskBtn.className = "row-add";
    newTaskBtn.title = t("ui.newTaskIn", "New task in {value0}", { value0: project.name });
    newTaskBtn.setAttribute("aria-label", newTaskBtn.title);
    newTaskBtn.innerHTML = ICONS.compose;
    newTaskBtn.onclick = (event) => {
      event.stopPropagation();
      newTaskIn(project);
    };
    projectItem.appendChild(newTaskBtn);

    // Codex previews a project's summary from the row itself. The short delay
    // keeps an incidental pass down the sidebar quiet, and the preview never
    // takes focus away from the composer or a keyboard-held row.
    projectItem.addEventListener("mouseenter", () => {
      cancelSidebarCardHover();
      if (draggedProjectId || sidebarCardOrigin === "click" || sidebarCardTrigger === moreBtn) return;
      sidebarCardHoverTimer = setTimeout(
        () => showProjectActions(project, sessionsForProject, projectItem, moreBtn, false),
        280,
      );
    });
    projectItem.addEventListener("mouseleave", (event) => {
      cancelSidebarCardHover();
      if (isInside(event.relatedTarget, projectItem)) return;
      if (sidebarCardTrigger === moreBtn && sidebarCardOrigin === "hover") {
        sidebarCardHoverTimer = setTimeout(closeSidebarCards, 120);
      }
    });

    // The row is what the arrangement is dragged by: the whole row moves — the
    // project and the threads under it — and the app writes the order down.
    projectItem.dataset.project = project.id;
    projectItem.draggable = true;
    projectItem.addEventListener("dragstart", (event) => {
      draggedProjectId = project.id;
      projectItem.classList.add("dragging");
      // A drag with nothing on the clipboard is one some browsers refuse to
      // start at all, so the row puts its own name there.
      event.dataTransfer?.setData("text/plain", project.name);
      if (event.dataTransfer) event.dataTransfer.effectAllowed = "move";
    });
    projectItem.addEventListener("dragend", () => {
      draggedProjectId = null;
      clearDropMark();
      projectItem.classList.remove("dragging");
    });
    projectItem.addEventListener("dragover", (event) => {
      const after = dropAfter(event, projectItem);
      if (after === null) return;
      // This is what says the row will take a drop at all.
      event.preventDefault();
      if (event.dataTransfer) event.dataTransfer.dropEffect = "move";
      markDrop(projectItem, after);
    });
    projectItem.addEventListener("dragleave", (event) => {
      // A row is made of children — its glyph, its name, its count, its own
      // buttons — and crossing from one to the next is a `dragleave` of the row
      // they belong to, so the line comes off only when the pointer has really
      // left the row.
      if (isInside(event.relatedTarget, projectItem)) return;
      clearDropMarkOf(projectItem);
    });
    projectItem.addEventListener("drop", (event) => {
      const after = dropAfter(event, projectItem);
      clearDropMark();
      if (after === null) return;
      event.preventDefault();
      dropProject(draggedProjectId, project.id, after);
    });
    
    projectGroup.appendChild(projectItem);
    
    // Threads for this project, grouped under its row.
    const sessionsContainer = document.createElement("div");
    sessionsContainer.className = "project-sessions";
    sessionsContainer.id = `project-sessions-${projectIndex}`;
    sessionsContainer.setAttribute("role", "group");
    sessionsContainer.hidden = collapsed;
    projectItem.setAttribute("aria-controls", sessionsContainer.id);
    const toggleProject = () => {
      const folded = !state.collapsedSidebarProjects.has(project.id);
      if (folded) state.collapsedSidebarProjects.add(project.id);
      else state.collapsedSidebarProjects.delete(project.id);
      sessionsContainer.hidden = folded;
      projectItem.setAttribute("aria-expanded", String(!folded));
    };
    projectItem.onclick = toggleProject;
    projectItem.onkeydown = (event) => {
      if (event.target !== projectItem) return;
      const folded = state.collapsedSidebarProjects.has(project.id);
      if (event.key === "Enter" || event.key === " ") {
        event.preventDefault();
        toggleProject();
      } else if (event.key === "ArrowRight" && folded) {
        event.preventDefault();
        toggleProject();
      } else if (event.key === "ArrowLeft" && !folded) {
        event.preventDefault();
        toggleProject();
      }
    };

    const expanded = state.expandedSidebarProjects.has(project.id);
    const visibleSessions = expanded
      ? sessionsForProject
      : sessionsForProject.slice(0, SIDEBAR_THREAD_LIMIT);

    if (!sessionsForProject.length) {
      const empty = document.createElement("div");
      empty.className = "project-empty";
      empty.textContent = t("sidebar.noChats", "No chats");
      sessionsContainer.appendChild(empty);
    }

    for (const session of visibleSessions) {
      // The thread a turn is running in says so on its own row, wherever the
      // reader is: the transcript they are looking at may be another thread's,
      // so this is what tells them which conversation is still working.
      const running = turnInThread(session.id);
      const sessionItem = document.createElement("div");
      sessionItem.className =
        "session-item" +
        (session.id === state.session ? " active" : "") +
        (running ? " running" : "");
      sessionItem.dataset.session = session.id;
      sessionItem.dataset.project = project.path;
      sessionItem.draggable = state.chatSort === "manual";
      sessionItem.addEventListener("dragstart", (event) => {
        if (state.chatSort !== "manual") return;
        draggedSessionId = session.id;
        draggedSessionProject = project.path;
        sessionItem.classList.add("dragging");
        event.dataTransfer?.setData("text/plain", sessionLabel(session));
        if (event.dataTransfer) event.dataTransfer.effectAllowed = "move";
      });
      sessionItem.addEventListener("dragend", () => {
        draggedSessionId = null;
        draggedSessionProject = "";
        clearDropMark();
        sessionItem.classList.remove("dragging");
      });
      sessionItem.addEventListener("dragover", (event) => {
        const after = sessionDropAfter(event, sessionItem);
        if (after === null) return;
        event.preventDefault();
        if (event.dataTransfer) event.dataTransfer.dropEffect = "move";
        markDrop(sessionItem, after);
      });
      sessionItem.addEventListener("dragleave", (event) => {
        if (isInside(event.relatedTarget, sessionItem)) return;
        clearDropMarkOf(sessionItem);
      });
      sessionItem.addEventListener("drop", (event) => {
        const after = sessionDropAfter(event, sessionItem);
        clearDropMark();
        if (after === null) return;
        event.preventDefault();
        dropSession(draggedSessionId, session.id, after);
      });

      const label = sessionLabel(session);
      // A thread that runs in a worktree says so on its row: the project's own
      // folder and its checkouts hold different files, and two threads of one
      // project are only readable apart if the row says which is which.
      if (session.worktree) {
        const checkout = document.createElement("span");
        checkout.className = "session-worktree";
        checkout.title = t("ui.runsInTheWorktree", "Runs in the worktree {value0}", { value0: session.worktree });
        checkout.setAttribute("aria-hidden", "true");
        checkout.innerHTML = ICONS.worktree;
        sessionItem.appendChild(checkout);
      }
      const sessionName = document.createElement("div");
      sessionName.className = "name";
      sessionName.textContent = label;
      sessionItem.title = `${
        session.worktree ? `${label} — worktree ${session.worktree}` : label
      }${running ? " — a turn is running" : ""}`;
      sessionItem.appendChild(sessionName);

      // A thread that opened a pull request carries it on its row, in the state
      // the forge last reported: the row is the one place the reader can see a
      // conversation's request without opening it, and the mark is the link.
      const pull = pullOf(session);
      if (pull) {
        sessionItem.classList.add("has-pull");
        if (!running) sessionItem.appendChild(pullButton(pull));
      }

      if (running) {
        const mark = document.createElement("span");
        mark.className = "session-run";
        mark.title = t("ui.aTurnIsRunningInThisThread", "A turn is running in this thread");
        mark.setAttribute("aria-hidden", "true");
        const spinner = document.createElement("span");
        spinner.className = "spinner";
        mark.appendChild(spinner);
        sessionItem.appendChild(mark);
      }

      // A thread the store has not written has no file to remove, so its row
      // carries no ✕: leaving it is what starting a new thread does.
      if (!session.unstored) {
        const pinBtn = document.createElement("button");
        pinBtn.type = "button";
        pinBtn.className = "row-pin" + (session.pinned ? " active" : "");
        pinBtn.title = session.pinned ? t("ui.unpinThread", "Unpin thread") : t("ui.pinThread", "Pin thread");
        pinBtn.setAttribute("aria-label", pinBtn.title);
        pinBtn.innerHTML = ICONS.pin;
        pinBtn.onclick = (event) => {
          event.stopPropagation();
          pinSession(session);
        };
        sessionItem.appendChild(pinBtn);

        const archiveBtn = document.createElement("button");
        archiveBtn.type = "button";
        archiveBtn.className = "row-archive";
        archiveBtn.title = t("ui.archiveThread", "Archive thread");
        archiveBtn.setAttribute("aria-label", archiveBtn.title);
        archiveBtn.innerHTML = ICONS.archive;
        archiveBtn.onclick = (event) => {
          event.stopPropagation();
          archiveSession(session);
        };
        sessionItem.appendChild(archiveBtn);
      }

      sessionItem.onclick = (event) => {
        if (event?.metaKey || event?.ctrlKey) openSessionTab(session);
        else selectSessionFromTree(session);
      };
      sessionItem.onauxclick = (event) => {
        if (event.button === 1) { event.preventDefault(); openSessionTab(session); }
      };
      sessionItem.addEventListener("mouseenter", () => showThreadDetails(session, sessionItem));
      sessionItem.addEventListener("mouseleave", closeSidebarCards);
      sessionItem.addEventListener("focusin", () => showThreadDetails(session, sessionItem));
      sessionItem.addEventListener("focusout", closeSidebarCards);

      sessionsContainer.appendChild(sessionItem);
    }

    if (!expanded && sessionsForProject.length > SIDEBAR_THREAD_LIMIT) {
      const more = document.createElement("button");
      more.type = "button";
      more.className = "sidebar-show-more project-show-more";
      more.textContent = t("sidebar.showMore", "Show more");
      more.title = t("ui.showMoreChatsIn", "Show {value0} more chats in {value1}", { value0: sessionsForProject.length - SIDEBAR_THREAD_LIMIT, value1: project.name });
      more.onclick = (event) => {
        event.stopPropagation();
        state.expandedSidebarProjects.add(project.id);
        renderProjectsTree();
      };
      sessionsContainer.appendChild(more);
    }

    projectGroup.appendChild(sessionsContainer);
    
    container.appendChild(projectGroup);
  }

  // Codex keeps a project-independent way back to recent work below the
  // project tree. These rows intentionally repeat chats that are also visible
  // under a project: one view is structural, the other is chronological.
  const recentSection = document.createElement("section");
  recentSection.className = "sidebar-recents";
  const recentTitle = document.createElement("div");
  recentTitle.className = "sidebar-section-title";
  recentTitle.textContent = t("sidebar.recents", "Recents");
  recentSection.appendChild(recentTitle);

  const recentSessions = state.sidebarView === "list"
    ? arrangedSessions(listedSessions())
    : [...listedSessions()]
      .sort((a, b) => (b.modified_at || b.created_at || 0) - (a.modified_at || a.created_at || 0))
      .slice(0, SIDEBAR_RECENT_LIMIT);
  for (const session of recentSessions) {
    const row = document.createElement("button");
    row.type = "button";
    row.className =
      "recent-session-item" +
      (session.id === state.session ? " active" : "") +
      (turnInThread(session.id) ? " running" : "");
    row.dataset.session = session.id;
    row.dataset.project = sessionProject(session);
    row.draggable = state.chatSort === "manual";
    row.addEventListener("dragstart", (event) => {
      if (state.chatSort !== "manual") return;
      draggedSessionId = session.id;
      draggedSessionProject = sessionProject(session);
      row.classList.add("dragging");
      event.dataTransfer?.setData("text/plain", sessionLabel(session));
      if (event.dataTransfer) event.dataTransfer.effectAllowed = "move";
    });
    row.addEventListener("dragend", () => {
      draggedSessionId = null;
      draggedSessionProject = "";
      clearDropMark();
      row.classList.remove("dragging");
    });
    row.addEventListener("dragover", (event) => {
      const after = sessionDropAfter(event, row);
      if (after === null) return;
      event.preventDefault();
      if (event.dataTransfer) event.dataTransfer.dropEffect = "move";
      markDrop(row, after);
    });
    row.addEventListener("dragleave", (event) => {
      if (isInside(event.relatedTarget, row)) return;
      clearDropMarkOf(row);
    });
    row.addEventListener("drop", (event) => {
      const after = sessionDropAfter(event, row);
      clearDropMark();
      if (after === null) return;
      event.preventDefault();
      dropSession(draggedSessionId, session.id, after);
    });
    const name = document.createElement("div");
    name.className = "name";
    name.textContent = sessionLabel(session);
    row.title = `${sessionLabel(session)} — ${
      session.worktree ? `worktree ${session.worktree}` : projectNameOf(sessionProject(session))
    }`;
    row.appendChild(name);
    const pull = pullOf(session);
    if (pull) {
      const mark = document.createElement("span");
      mark.className = `recent-pull state-${pull.state || "unknown"}`;
      mark.title = pullTitle(pull);
      mark.setAttribute("aria-hidden", "true");
      mark.innerHTML = ICONS.pull;
      row.appendChild(mark);
    }
    if (turnInThread(session.id)) {
      const spinner = document.createElement("span");
      spinner.className = "spinner";
      spinner.setAttribute("aria-hidden", "true");
      row.appendChild(spinner);
    }
    row.onclick = () => selectSessionFromTree(session);
    recentSection.appendChild(row);
  }
  container.appendChild(recentSection);
}

async function openSessionTab(session) {
  const existing = chatTabs.find((tab) => tab.session === session.id);
  if (existing) return activateChatTab(existing.id);
  if (!await newChatTab()) return;
  return selectSessionFromTree(session);
}

async function selectSessionFromTree(session) {
  closeCustomize();
  if (!session) return;
  const project = sessionProject(session);
  const projectOwner = state.projects.find((candidate) => candidate.path === project);
  if (projectOwner && state.collapsedSidebarProjects.delete(projectOwner.id)) renderProjectsTree();
  const existing = chatTabs.find((tab) => tab.session === session.id && tab.id !== activeTabId);
  if (existing) return activateChatTab(existing.id);

  const projectSessions = listedSessions().filter((candidate) => sessionProject(candidate) === project);
  if (projectSessions.findIndex((candidate) => candidate.id === session.id) >= SIDEBAR_THREAD_LIMIT) {
    const owner = state.projects.find((candidate) => candidate.path === project);
    if (owner) state.expandedSidebarProjects.add(owner.id);
  }

  // The thread on screen whose store entry has not been written yet is the one
  // already being shown, with its title already in the header: there is no file
  // to read back, and asking for one would clear the title the row stands in
  // under. Selecting it leaves the window as it is — while a row standing in for
  // a turn that is running in another thread is the way back to it, which is
  // what the header's strip does too.
  if (session.unstored) {
    if (session.id !== state.session) await openRun();
    return;
  }

  // Switch to the session's project first if different, so `session_messages`
  // is queried against the right project and the composer is enabled.
  const owner = state.projects.find((p) => p.path === sessionProject(session));
  if (owner && owner.path !== state.project) await selectProject(owner);

  window.history.replaceState({}, "", `?session=${session.id}`);
  await openSession(session);
}

function clearSelectedProject() {
  // The folder is going away from the window, so a turn running in it keeps its
  // thread like any other conversation the reader leaves: it goes on writing
  // into its own record, and the strip is the way back.
  parkThread();
  state.project = null;
  state.projectName = "";
  state.session = null;
  state.trust = null;
  state.worktree = null;
  state.worktrees = [];
  // Nothing is open, so there is no repository to name and no window to measure
  // a request against: the row keeps the folder chip, which is the way into one.
  state.git = null;
  el("project-meta").textContent = "";
  el("trust-modal").hidden = true;
  updateTrustButton();
  updateChips();
  setStatus(idleStatus());
  paintComposerFacts();
  resetTranscript();
}

// True when nothing is selected or the selected project is still in the tree.
function selectedProjectStillListed() {
  return !state.project || state.projects.some((project) => project.path === state.project);
}

async function removeProject(project) {
  // Any turn in the folder would have its transcript and its files moved under
  // it, whichever thread it belongs to.
  const busyInFolder = [...threads.values()].some((record) => {
    if (!record.busy) return false;
    const folder = record.worktree || record.project || record.runProject;
    return folder === project.path;
  });
  if (busyInFolder || (state.busy && (state.runProject || state.project) === project.path)) {
    showToast(t("ui.aTurnIsRunningStopItBeforeRemovingThisProject", "A turn is running; stop it before removing this project."));
    return;
  }

  try {
    const choice = await confirmDialog(
      t("dialog.removeProject.title", "Remove {value0}?", { value0: project.name }),
      t("dialog.removeProject.message", "This only removes the project from the app. Files on your computer and existing chats won't be deleted."),
      t("dialog.createProject.remove", "Remove project"),
    );
    if (!choice) return;
    state.projects = await invoke("remove_project", { id: project.id });
    if (!selectedProjectStillListed()) {
      clearSelectedProject();
      loadTheme();
    }
    await loadSessions();
    renderProjectsTree();
  } catch (error) {
    showToast(t("ui.couldNotRemoveProject", "Could not remove project: {value0}", { value0: error }), "error");
  }
}
