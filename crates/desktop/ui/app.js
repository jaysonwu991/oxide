// Oxide desktop front-end. The window is Electron's own renderer and the page
// is sandboxed against it: the project/session/config stores it reads are the
// CLI's, reached through the app's commands rather than the filesystem.
//
// A command is performed by name: one packet carries the name and the arguments,
// and its answer carries that command's own value or the reason it failed.
// Everything a run has to say while one is in flight arrives as a packet of its
// own, and a handler is given the event's own `payload`.
//
// Both directions use the names this page has always read, which
// `electron/preload.ts` installs before the page runs: a packet goes out on the
// user bridge, which the preload hands to the window
// (`electron/main.ts`), and the window answers through
// `window.__electrobun.receiveMessageFromHost`, which the preload leaves
// filling a queue until a page takes it over — this page does, since it speaks
// the wire itself. A window whose preload installs no user bridge still has the
// event one, which carries the packet as the `host-message` event it was written
// as; the page reads the same packet off either.
const userBridge = window.__electrobunHostBridge;
const eventBridge = window.__electrobunSendToHost;

function hostSend(packet) {
  if (userBridge) return userBridge.postMessage(JSON.stringify(packet));
  if (eventBridge) return eventBridge(packet);
  throw new Error("This window has no channel to the app");
}

const pending = new Map();
const listeners = new Map();
let nextRequest = 1;

// The preload sets the namespace up, and the page fills it in rather than
// loading a typed RPC layer this window does not use. A page the preload ran in
// a sandboxed mode leaves nothing to fill in.
if (!window.__electrobun) window.__electrobun = {};

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
  for (const packet of queued) receiveMessageFromHost(packet);
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

const REASONING = ["auto", "off", "minimal", "low", "medium", "high", "xhigh", "max"];

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

// The sidebar follows Codex's disclosure model: enough of every group to be
// useful at a glance, with an explicit expansion instead of one unbounded wall
// of projects and chats. Expansions live only for this window; the underlying
// project order and complete session list are unchanged.
const SIDEBAR_PROJECT_LIMIT = 5;
const SIDEBAR_THREAD_LIMIT = 5;
const SIDEBAR_RECENT_LIMIT = 5;

// ---------- glyphs ----------

/// Every icon the window draws: inline SVG stroked with `currentColor`, so a
/// control's glyph is the same drawing on every machine. A character is not one
/// — a `✕`, a `📁` or a `＋` is whatever shape and size the machine's own font
/// gives it, which is how the composer's stroked paperclip ended up beside an
/// emoji folder. The box and the weight are the panel's own two — a 24-unit box
/// at 1.8 for a glyph that fills its button, a 16-unit one at 1.5–1.6 for the
/// marks that sit on a text baseline — and the paths this window shares with the
/// VS Code panel are the panel's verbatim, held to them by `check-app.mjs`.
function glyph(paths, box = 24) {
  return `<svg viewBox="0 0 ${box} ${box}" aria-hidden="true">${paths}</svg>`;
}

// The two boxes the app's own drawings are made in, at the weight the panel
// draws each in: a 24-unit box for a glyph that fills its button, a 16-unit one
// at the panel's own 1.5 for the plus it shares with it.
const STROKE_24 =
  'fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"';
const STROKE_16 =
  'fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"';

/// Sizing is the sheet's, next to the box each glyph is drawn in.
const ICONS = {
  // A project's folder and a document the webview cannot paint: the two drawn
  // for this window alone, in the composer's own style.
  globe: glyph(`<circle cx="12" cy="12" r="9" ${STROKE_24}/><path d="M3 12h18M12 3c5 5 5 13 0 18-5-5-5-13 0-18Z" ${STROKE_24}/>`),
  folder: glyph(
    `<path d="M20.2 19.4a1.8 1.8 0 0 0 1.8-1.8v-8a1.8 1.8 0 0 0-1.8-1.8h-7.9a1.8 1.8 0 0 1-1.5-.8l-.9-1.4a1.8 1.8 0 0 0-1.5-.8H3.8A1.8 1.8 0 0 0 2 6.6v11a1.8 1.8 0 0 0 1.8 1.8Z" ${STROKE_24}/>`,
  ),
  file: glyph(
    `<path d="M14 3H7a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h10a2 2 0 0 0 2-2V8z" ${STROKE_24}/>` +
      `<path d="M14 3v5h5" ${STROKE_24}/>`,
  ),
  close: glyph(`<path d="M18 6 6 18M6 6l12 12" ${STROKE_24}/>`),
  plus: glyph('<path d="M8 3.6v8.8M3.6 8h8.8" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round"/>', 16),
  more: glyph('<path d="M5 12h.01M12 12h.01M19 12h.01" ' + STROKE_24 + '/>'),
  compose: glyph('<path d="M12 20h9M16.5 3.5a2.1 2.1 0 0 1 3 3L8 18l-4 1 1-4Z" ' + STROKE_24 + '/>'),
  changes: glyph('<rect x="4" y="3" width="16" height="18" rx="3" ' + STROKE_24 + '/><path d="M8 9h8M12 5v8M8 17h8" ' + STROKE_24 + '/>'),
  undo: glyph('<path d="m9 4-5 5 5 5M4 9h10a6 6 0 0 1 0 12h-3" ' + STROKE_24 + '/>'),
  pin: glyph('<path d="m14 4 6 6-3 1-4 4-1 5-3-6-5-3 5-1 4-4Z" ' + STROKE_24 + '/>'),
  archive: glyph('<path d="M4 8h16M5 8v11h14V8M3 4h18v4H3ZM9 12h6" ' + STROKE_24 + '/>'),
  chat: glyph('<path d="M21 15a4 4 0 0 1-4 4H8l-5 3 1.5-5A7 7 0 0 1 3 13V8a4 4 0 0 1 4-4h10a4 4 0 0 1 4 4Z" ' + STROKE_24 + '/>'),
  computer: glyph('<rect x="3" y="4" width="18" height="13" rx="2" ' + STROKE_24 + '/><path d="M8 21h8M12 17v4" ' + STROKE_24 + '/>'),
  branch: glyph('<circle cx="4" cy="3" r="1.5" ' + STROKE_16 + '/><circle cx="4" cy="13" r="1.5" ' + STROKE_16 + '/><circle cx="12" cy="5" r="1.5" ' + STROKE_16 + '/><path d="M4 4.5v7M5.5 11c4 0 6.5-1.5 6.5-4.5" ' + STROKE_16 + '/>', 16),
  // A worktree: a checkout of this project beside it, on its own branch. Drawn
  // in the composer's own box, so the row's chip and a thread's badge are the
  // same mark.
  worktree: glyph(
    '<rect x="1.9" y="3.1" width="7.2" height="8.4" rx="1.5" fill="none" stroke="currentColor" stroke-width="1.3"/>' +
      '<path d="M11.2 4.9h1.1a1.5 1.5 0 0 1 1.5 1.5v4.2a1.5 1.5 0 0 1-1.5 1.5h-1.1" fill="none" stroke="currentColor" stroke-width="1.3" stroke-linecap="round"/>' +
      '<path d="M3.8 6.2h3.4M3.8 8.5h2.3" fill="none" stroke="currentColor" stroke-width="1.3" stroke-linecap="round"/>',
    16,
  ),
  gear: glyph('<circle cx="12" cy="12" r="3" ' + STROKE_24 + '/><path d="M19.4 15a1.7 1.7 0 0 0 .3 1.9l.1.1-2.8 2.8-.1-.1a1.7 1.7 0 0 0-1.9-.3 1.7 1.7 0 0 0-1 1.6v.2h-4V21a1.7 1.7 0 0 0-1-1.6 1.7 1.7 0 0 0-1.9.3l-.1.1L4.2 17l.1-.1a1.7 1.7 0 0 0 .3-1.9A1.7 1.7 0 0 0 3 14H2.8v-4H3a1.7 1.7 0 0 0 1.6-1 1.7 1.7 0 0 0-.3-1.9L4.2 7 7 4.2l.1.1a1.7 1.7 0 0 0 1.9.3A1.7 1.7 0 0 0 10 3V2.8h4V3a1.7 1.7 0 0 0 1 1.6 1.7 1.7 0 0 0 1.9-.3l.1-.1L19.8 7l-.1.1a1.7 1.7 0 0 0-.3 1.9 1.7 1.7 0 0 0 1.6 1h.2v4H21a1.7 1.7 0 0 0-1.6 1Z" ' + STROKE_24 + '/>'),
  check: glyph('<path d="m5.6 8 1.5 1.5 3.5-3.5" ' + STROKE_16 + '/>', 16),
  caret: glyph(`<path d="M5.5 8.6 12 14.8l6.5-6.2" ${STROKE_24}/>`),
  power: glyph(
    '<path d="M18.36 6.64a9 9 0 1 1-12.73 0" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round"/>' +
      '<path d="M12 2v10" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round"/>',
  ),
  mark: '<svg viewBox="0 0 24 24" aria-hidden="true"><path d="M12 2.4 21.6 12 12 21.6 2.4 12Z" fill="currentColor"/></svg>',
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

function savedFollowUpBehavior() {
  try {
    return window.localStorage?.getItem("oxide.followUpBehavior") === "steer" ? "steer" : "queue";
  } catch {
    return "queue";
  }
}

const state = {
  projects: [],
  project: null,
  projectName: "",
  session: null,
  sessions: [],
  sidebarProjectsExpanded: false,
  expandedSidebarProjects: new Set(),
  busy: false,
  runId: null,
  // New context sent during a run waits by default. Steering is a deliberate
  // choice because it changes the work already in progress.
  followUpBehavior: savedFollowUpBehavior(),
  // Messages rejected at the exact instant a run finishes. Their composed
  // payloads start against the same session, in submission order, as each
  // preceding turn ends — with the thread and folder they were typed in, since
  // the window may have moved on to another one by the time they go.
  pendingSends: [],
  // The thread the running turn belongs to, held apart from the one on screen:
  // the reader may open another thread to read while it works, and the run keeps
  // its own — the sidebar row for it, the header's banner and the refusal to
  // send anywhere else are all read from here.
  runSession: null,
  // The folder that turn is running in, which is not the one on screen once the
  // reader has opened another project's thread: the composer refuses to send
  // anywhere else, and the way back to the run's thread is resolved against it.
  runProject: null,
  // The thread the message that started the running turn was composed in, and
  // `null` while no send is in flight. The run's own `agent-start` compares it
  // with the thread on screen: a reader who has opened another conversation in
  // the meantime is reading that one, and the run's output belongs to the thread
  // they left.
  sendView: null,
  // What the running turn titled itself: the only name the thread it is in has
  // until the sidebar's own listing carries it.
  runTitle: "",
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
  contextWindow: 0,
  catalogWindow: 0,
  currentProvider: "",
  contextCorrection: null,
  // Whether the credential is a plan rather than a metered key, which the usage
  // line marks ` (sub)` the way the terminal footer does.
  subscription: false,
  // What the window's own status is saying, and which state its icon wears when
  // no turn is running: read by `paintStatus`. The words are dropped while it is
  // the idle placeholder, so the badge is the icon alone.
  statusText: "",
  statusLevel: "info",
  runStartedAt: 0,
  runElapsed: 0,
  runClockProject: null,
  runClockSession: null,
  // The repository the row names: what `git_info` answered for the folder on
  // screen, read when the folder changes, when a turn ends and when the reader
  // comes back to the window.
  git: null,
  // The worktrees of the project on screen, and the one the next turn runs in.
  // A worktree is another checkout of the project on its own branch, so a
  // conversation's work stays out of the folder the reader has open; `null` is
  // that folder itself.
  worktrees: [],
  worktree: null,
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
  // What the composer's usage line is showing, which is the open thread's own
  // totals rather than the window's: a run in another thread carries them while
  // its thread is parked (see `parkRun`).
  usage: null,
  currentAssistant: null,
  tools: [],
  // One repository listing feeds every summary and the repository review.
  repositoryChanges: null,
  detailsOpen: true,
  currentThinking: null,
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
  // One card per finished turn that changed something, in the order the turns
  // ran, so a review opened from an older card still reads its own files.
  changes: [],
  // The thread a turn is running in while the reader is reading another one: its
  // transcript leaves the screen with the run — the nodes painted so far, the
  // cards of its finished turns and the totals in the footer — so the run keeps
  // writing into its own thread and the reply is whole when the reader comes
  // back, instead of being re-read from a store that is a step behind.
  parked: null,
};

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
    : state.statusText === t("composer.working", "Working…") ? "composer.working" : null;
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
  } catch (error) {
    setStatus(String(error), "error");
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
  if (!el("reasoning-modal").hidden) renderReasoning();
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
  const busy = Boolean(state.busy || state.sendView);
  const level = busy ? "busy" : state.statusLevel;
  const badge = el("status");
  badge.dataset.level = level;
  const text = state.statusText || "";
  for (const id of ["send", "stop"]) {
    el(id).dataset.level = level;
    el(id).setAttribute("aria-description", text || IDLE_STATUS);
  }
  el("status-text").textContent = text;
  el("status-text").hidden = text === idleStatus();
  badge.hidden = busy || !text || text === idleStatus();
  const stopHint = t("composer.stopHint", "Stop the running turn");
  el("stop").title = `${stopHint}${text && text !== idleStatus() ? ` · ${text}` : ""}`;
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

function paintRunClock() {
  const active = state.busy && viewingRun();
  const belongsHere =
    state.runClockProject === state.project && state.runClockSession === state.session;
  const visible = state.runStartedAt > 0 && (active || (!state.busy && belongsHere));
  const clock = el("run-time");
  clock.hidden = !visible;
  if (!visible) return;
  const elapsed = active ? Date.now() - state.runStartedAt : state.runElapsed;
  el("run-time-text").textContent = t(active ? "run.workingFor" : "run.workedFor", active ? "Working for {duration}" : "Worked for {duration}", { duration: runDuration(elapsed) });
}

function startRunClock() {
  if (state.runStartedAt) return;
  state.runStartedAt = Date.now();
  state.runElapsed = 0;
  state.runClockProject = state.runProject || state.project;
  state.runClockSession = state.sendView?.session ?? state.session;
  paintRunClock();
  if (!runClockTimer) runClockTimer = setInterval(paintRunClock, 1000);
}

function finishRunClock() {
  if (state.runStartedAt) state.runElapsed = Math.max(0, Date.now() - state.runStartedAt);
  if (runClockTimer) clearInterval(runClockTimer);
  runClockTimer = 0;
}

/// The word the window is idle under: the words are dropped for it and the icon
/// speaks, since a row that says `Ready` is a row saying nothing.
const IDLE_STATUS = t("composer.ready", "Ready");

/// The idle word in the language on screen, which is what the status row
/// compares against before it decides the row has nothing to say.
const idleStatus = () => t("composer.ready", IDLE_STATUS);

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
    prompt,
    cacheHitRate,
  };
}

/// A step of the thread the reader is in, added to what the window is showing.
function addUsage(event) {
  setUsage(addTotals(state.usage, event));
}

function setUsage({
  input = 0,
  output = 0,
  cacheRead = 0,
  cacheWrite = 0,
  cost = 0,
  prompt = 0,
  cacheHitRate = null,
}) {
  // What the thread has spent, held so one the reader leaves can take its own
  // totals with it and paint them back on return. The percentage is not kept
  // beside them: it is a fraction of the window in force where they are drawn
  // (see `usagePercent`), so a folder switched under a parked run cannot label
  // its thread with the window of the folder the reader left.
  state.usage = { input, output, cacheRead, cacheWrite, cost, prompt, cacheHitRate };
  paintUsage();
}

/// The usage line, drawn the way the terminal footer draws its own: the tokens
/// the chat has spent, the cache it read and wrote, the hit rate the provider
/// reported, and the spend — marked ` (sub)` for a provider whose credential is
/// a plan rather than a metered key, where the price table's number is what the
/// plan would have billed. Every segment is left out instead of drawn as a zero,
/// and the context reading is the ring on the row above rather than a second word
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
  el("usage").title = bits.join(" · ");
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
  tab.snapshot = {
    project: state.project, projectName: state.projectName, session: state.session, trust: state.trust,
    worktree: state.worktree, worktrees: state.worktrees, agent: state.agent,
    changes: state.changes, repositoryChanges: state.repositoryChanges,
    usage: state.usage, contextWindow: state.contextWindow, contextCorrection: state.contextCorrection,
    draft: el("prompt").value, attachments: [...state.attachments],
    selectionStart: el("prompt").selectionStart, selectionEnd: el("prompt").selectionEnd,
    inputScrollTop: el("prompt").scrollTop, undo: el("prompt").captureHistory?.(),
    nodes: [...el("transcript").children], scrollTop: el("transcript").scrollTop,
    followLatest: el("transcript").followLatest !== false,
  };
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
    const running = state.busy && (tab.session != null
      ? tab.session === state.runSession : state.sendView?.tabId === tab.id);
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
    parkRun();
    clearPaintIdle();
    activeTabId = id;
    const saved = tab.snapshot;
    if (saved) {
      for (const key of ["project", "projectName", "session", "trust", "worktree", "worktrees", "agent", "changes", "repositoryChanges", "contextWindow", "contextCorrection"]) state[key] = saved[key];
      el("transcript").replaceChildren(...saved.nodes);
      const ownsParked = tab.session != null
        ? state.parked?.session === tab.session
        : state.parked?.pending && state.sendView?.tabId === tab.id;
      if (!ownsParked || !restoreParked(tab.session)) setUsage(saved.usage || {});
      el("transcript").scrollTop = saved.scrollTop || 0;
      el("transcript").followLatest = saved.followLatest;
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
  if (chatTabs.length === 1) {
    await newChatTab();
  } else if (activeTabId === id) {
    await activateChatTab(chatTabs[index - 1]?.id ?? chatTabs[index + 1].id);
  }
  chatTabs.splice(chatTabs.findIndex((tab) => tab.id === id), 1);
  paintChatTabs();
}

function initChatTabs() {
  paintChatTabs();
  el("new-tab").onclick = newChatTab;
  document.addEventListener("keydown", (event) => {
    if (event.defaultPrevented || state.handingOff) return;
    const key = event.key.toLowerCase();
    if ((event.metaKey || event.ctrlKey) && key === "t" && !event.shiftKey && !event.altKey) {
      event.preventDefault(); newChatTab();
    } else if ((event.metaKey || event.ctrlKey) && key === "w" && !event.shiftKey && !event.altKey) {
      event.preventDefault(); closeChatTab(activeTabId);
    } else if (event.ctrlKey && key === "tab") {
      event.preventDefault();
      const index = chatTabs.findIndex((tab) => tab.id === activeTabId);
      activateChatTab(chatTabs[(index + (event.shiftKey ? -1 : 1) + chatTabs.length) % chatTabs.length].id);
    }
  });
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
  for (const id of [state.session, state.runSession, state.parked?.session]) {
    // The thread the run is in — or the one a finished turn left parked — lives in
    // a folder of its own, which is not the one the store was read for once the
    // reader has opened another project's thread: a row standing in for it carries
    // that folder, so opening the row goes back to its own work rather than asking
    // the wrong project for a thread it has not got.
    const own = id != null && (id === state.runSession || state.parked?.session === id);
    const cwd = own ? transcriptProject(id) : state.project;
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
  return heldTitle(id) || null;
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

function anchor(href, label) {
  const text = label == null ? escapeHtml(href) : label;
  return `<a href="${escapeHtml(href)}" target="_blank" rel="noreferrer">${text}</a>`;
}

// A bare URL often ends a sentence, so trailing punctuation stays outside the
// link and an unbalanced closing bracket is handed back to the surrounding text.
function autolink(url, stash) {
  let href = url.replace(/[.,;:!?]+$/, "");
  let trail = url.slice(href.length);
  while (href.endsWith(")") && href.split("(").length < href.split(")").length) {
    href = href.slice(0, -1);
    trail = ")" + trail;
  }
  return href ? stash(anchor(href)) + trail : url;
}

// Link labels can contain balanced brackets, as issue keys in PR titles do.
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
      const destination = /^\(([^)\s]+)\)/.exec(text.slice(i + 1));
      if (!destination) continue;
      output += text.slice(copied, start) + stash(anchor(
        destination[1], escapeHtml(text.slice(start + 1, i)),
      ));
      i += destination[0].length;
      copied = i + 1;
      starts.length = 0;
    }
  }
  return output + text.slice(copied);
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
  if (references) {
    for (const [start, end] of referenceRanges(s).reverse()) {
      const reference = s.slice(start, end);
      if (reference.includes("\u0000")) continue;
      s = s.slice(0, start) + stash(`<span class="file-reference">${escapeHtml(reference)}</span>`) + s.slice(end);
    }
  }
  s = inlineLinks(s, stash);
  s = s.replace(/<(https?:\/\/[^\s<>]+)>/g, (_, url) => stash(anchor(url)));
  s = s.replace(/\bhttps?:\/\/[^\s<>"'`]+/g, (url) => autolink(url, stash));
  if (hardBreaks) s = s.replace(/ {2,}\n|\\\n/g, () => stash("<br>")).replace(/\n/g, " ");
  s = escapeHtml(s);
  s = s.replace(/\*\*([^*]+)\*\*/g, "<strong>$1</strong>");
  s = s.replace(/(^|[^*])\*([^*\n]+)\*/g, "$1<em>$2</em>");
  s = s.replace(/~~([^~]+)~~/g, "<del>$1</del>");
  s = s.replace(/\u0000(\d+)\u0000/g, (_, i) => stashed[Number(i)]);
  return s;
}

const BLOCK_START = /^\s*(```|#{1,6}\s|[-*+]\s|\d+[.)]\s|>|\|)/;

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
  if (row.endsWith("|")) row = row.slice(0, -1);
  return row.split("|").map((cell) => cell.trim());
}

function isTableSeparator(line) {
  return /^\s*\|?\s*:?-{1,}:?\s*(\|\s*:?-{1,}:?\s*)*\|?\s*$/.test(line) && line.includes("-");
}

function tableHtml(header, rows, references = false) {
  const head = header.map((cell) => `<th>${inline(cell, references)}</th>`).join("");
  const body = rows
    .map((row) => `<tr>${row.map((cell) => `<td>${inline(cell, references)}</td>`).join("")}</tr>`)
    .join("");
  return `<table><thead><tr>${head}</tr></thead><tbody>${body}</tbody></table>`;
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
  let list = null;

  const closeList = () => {
    if (list) {
      out.push(`</${list}>`);
      list = null;
    }
  };

  while (i < lines.length) {
    const line = lines[i];

    const fence = line.match(/^\s*```([\w+-]*)\s*$/);
    if (fence) {
      closeList();
      const code = [];
      i += 1;
      while (i < lines.length && !/^\s*```\s*$/.test(lines[i])) {
        code.push(lines[i]);
        i += 1;
      }
      const complete = i < lines.length && fence[1].toLowerCase() === "mermaid" && !softBreaks;
      i += 1;
      const lang = fence[1] ? ` data-lang="${escapeHtml(fence[1].toLowerCase())}"` : "";
      out.push(`<pre${lang}${complete ? ' data-complete="true"' : ""}><code>${highlight(code.join("\n"), fence[1])}</code></pre>`);
      continue;
    }

    if (/^\s*(-{3,}|\*{3,}|_{3,})\s*$/.test(line)) {
      closeList();
      out.push("<hr />");
      i += 1;
      continue;
    }

    const heading = line.match(/^(#{1,6})\s+(.*)$/);
    if (heading) {
      closeList();
      const level = heading[1].length;
      out.push(`<h${level}>${inline(heading[2], softBreaks)}</h${level}>`);
      i += 1;
      continue;
    }

    if (/^\s*>\s?/.test(line)) {
      closeList();
      const quote = [];
      while (i < lines.length && /^\s*>\s?/.test(lines[i])) {
        quote.push(lines[i].replace(/^\s*>\s?/, ""));
        i += 1;
      }
      out.push(`<blockquote>${renderMarkdown(quote.join("\n"), softBreaks)}</blockquote>`);
      continue;
    }

    const task = line.match(/^\s*[-*+]\s+\[([ xX])\]\s+(.*)$/);
    if (task) {
      if (list !== "ul") {
        closeList();
        out.push('<ul class="tasks">');
        list = "ul";
      }
      const mark = task[1].toLowerCase() === "x" ? "☑" : "☐";
      out.push(`<li class="task">${mark} ${inline(task[2], softBreaks)}</li>`);
      i += 1;
      continue;
    }

    const bullet = line.match(/^\s*[-*+]\s+(.*)$/);
    if (bullet) {
      if (list !== "ul") {
        closeList();
        out.push("<ul>");
        list = "ul";
      }
      out.push(`<li>${inline(bullet[1], softBreaks)}</li>`);
      i += 1;
      continue;
    }

    const ordered = line.match(/^\s*\d+[.)]\s+(.*)$/);
    if (ordered) {
      if (list !== "ol") {
        closeList();
        out.push("<ol>");
        list = "ol";
      }
      out.push(`<li>${inline(ordered[1], softBreaks)}</li>`);
      i += 1;
      continue;
    }

    if (line.includes("|") && i + 1 < lines.length && isTableSeparator(lines[i + 1])) {
      closeList();
      const header = splitRow(line);
      const rows = [];
      i += 2;
      while (i < lines.length && lines[i].includes("|") && lines[i].trim() !== "") {
        rows.push(splitRow(lines[i]));
        i += 1;
      }
      out.push(tableHtml(header, rows, softBreaks));
      continue;
    }

    if (line.trim() === "") {
      closeList();
      i += 1;
      continue;
    }

    closeList();
    const para = [line];
    i += 1;
    while (i < lines.length && lines[i].trim() !== "" && !BLOCK_START.test(lines[i])) {
      para.push(lines[i]);
      i += 1;
    }
    out.push(`<p>${softBreaks ? para.map((line) => inline(line, true)).join("<br>") : inline(para.join("\n"), false, true)}</p>`);
  }
  closeList();
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
  // A running turn keeps the folder it started in — its run id is that folder's
  // thread — so the window moving to another project takes the run's thread with
  // it rather than stopping it: the transcript it is writing into is parked (see
  // `parkRun`), the header's strip and the sidebar row the run's own mark keep
  // saying where it is, and the composer refuses to send into a folder the run is
  // not in. The reader can look at another project while it works.
  parkRun();
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
  // A project reached through Recents or the picker must not become selected
  // behind the five-row disclosure. Once the reader explicitly opens it, keep
  // the project visible in the structural list too.
  const sidebarIndex = state.projects.findIndex((candidate) => candidate.id === project.id);
  if (sidebarIndex >= SIDEBAR_PROJECT_LIMIT) state.sidebarProjectsExpanded = true;
  state.session = null;
  state.trust = null;
  // The checkout belonged to the folder being left: a worktree of one project
  // is not a folder of another, so the next message starts in this project's
  // own tree until the reader picks one of its worktrees.
  state.worktree = null;
  state.worktrees = [];
  paintWorktreeChip();
  // Follow-ups belong to the parked run, not the folder being viewed. Keep
  // them while the reader looks at another project; `paintQueue` hides them
  // until the run's own thread is opened again.
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
  // than carried over from the folder being left. The context reading is painted
  // with the rest of the row by the reset, and again when the folder's own window
  // arrives.
  loadGit();
  await Promise.all([loadInfo(), loadSessions(), loadTheme(), loadWorktrees()]);
}

function correctionWindow(correction = state.contextCorrection) {
  return correction && correction.project === state.project && correction.session === state.session &&
    correction.provider === state.currentProvider && correction.model === state.currentModel
    ? correction.tokens : null;
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
    state.supportsReasoning = info.supportsReasoning;
    state.reasoning = info.supportsReasoning === false ? "auto" : info.reasoning;
    state.reasoningPicked = Boolean(changedModel) || info.supportsReasoning === false;
    state.reasoningLevels = info.supportsReasoning === false
      ? ["auto"]
      : Array.isArray(info.reasoningLevels)
        ? ["auto", ...info.reasoningLevels]
        : null;
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
  el("model-value").textContent = info.model;
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

function updateChips() {
  el("model-value").textContent = state.currentModel || t("composer.model", "Model");
  labelControl("model", state.currentModel ? t("ui.model", "model: {value0}{value1}", { value0: state.currentModel, value1: state.contextWindow ? ` · ${formatTokens(state.contextWindow)}` : "" }) : t("composer.model", "Model"), t("ui.switchModel", "Switch model"));
  labelControl("reasoning", t("ui.thinking", "thinking: {value0}", { value0: t(`reasoning.${state.reasoning}`, state.reasoning) }), t("ui.chooseTheReasoningLevelShiftTabCycles", "Choose the reasoning level (Shift+Tab cycles)"));
  el("reasoning").hidden = state.supportsReasoning === false;
  el("reasoning-value").textContent = reasoningLabel(state.reasoning);
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
/// composer's top row, whose remaining auto margin belongs to the context ring.
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

/// The composer's own top row, above the message box: the branch the folder is on
/// and the context reading the row ends with — an arc filled to the percent the
/// thread's totals come to, with that percent spelled out beside it and the tokens
/// and the window behind it in its tooltip. Nothing but the folder chip is shown
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
  // The percent the arc is filled to, beside it: the number and the arc are one
  // reading, and a thread that has spent nothing yet has none to spell out.
  el("context-percent").textContent = percent == null ? "" : `${percent.toFixed(0)}%`;
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
    state.parked = null;
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

/// Opens a stored thread for reading. A turn may be running in another one while
/// this happens — the transcript is the reader's, so the run's own output is not
/// painted into it (see `viewingRun`) — and a card a turn left for this thread
/// while the reader was elsewhere is painted with it.
let sessionRead = 0;
async function openSession(session) {
  if (state.handingOff && !state.applyingHandoff) return;
  const read = ++sessionRead;
  const ownerTab = activeChatTab();
  if (ownerTab) ownerTab.pendingSession = session;
  const tabId = activeTabId;
  // Leaving the thread a turn is running in parks its transcript with the run:
  // what it has painted stays, and its next step goes on painting into it.
  parkRun();
  state.session = session.id;
  setThreadTitle(sessionLabel(session));
  // The thread runs where its own header says it does: a thread in a worktree
  // is read back from, and continued in, that checkout rather than in the
  // folder the reader has open.
  state.worktree = worktreePathFor(session.cwd);
  paintWorktreeChip();
  // Coming back to a thread the run parked: its own transcript is the thread —
  // with everything the run did while the reader was away, including the reply
  // the store has not been told about yet — so it is handed back rather than
  // read again from a listing that is a step behind.
  if (restoreParked(session.id)) {
    if (ownerTab) delete ownerTab.pendingSession;
    closeReview();
    setThreadTitle(sessionLabel(session));
    updateRunBanner();
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
    if (data.usage) {
      setUsage({
        input: data.usage.input,
        output: data.usage.output,
        cacheRead: data.usage.cacheRead,
        cacheWrite: data.usage.cacheWrite,
        cost: data.usage.cost,
        // The prompt of the newest recorded request is the context the thread was
        // last run with, so a thread opened again draws its context reading — the
        // arc and the percent beside it — from that rather than from nothing until
        // the next turn runs, and the hit rate the store kept is what the totals
        // line's `CH` shows.
        prompt: data.usage.contextTokens || 0,
        cacheHitRate: data.usage.cacheHitRate ?? null,
      });
    }
  } catch (error) {
    showToast(t("ui.failedToOpenThread", "Failed to open thread: {value0}", { value0: error }), "error");
  }
  loadSessions();
}

function resetTranscript() {
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
  if (!canStartNewChat()) return;
  if (state.project) {
    newChat();
    return;
  }
  askForProject();
}

// ---------- chat ----------

function bubble(kind, text, attachments = []) {
  const wrap = document.createElement("div");
  wrap.className = `msg ${kind}`;
  wrap.chatAttachments = attachments;
  const label = speakerLabel(kind);
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
  wrap.append(label, body);
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

// One rendered, editable surface. The value/selection facade keeps completion,
// recall and sending on Markdown while the browser edits the rendered nodes.
function installMarkdownInput(editor, render) {
  let source = "";
  let painted = "";
  let composing = false;
  let savedStart = 0;
  let savedEnd = 0;
  const history = [""];
  let historyIndex = 0;
  const marker = "\uE000oxide-caret\uE001";

  function markdown(node, point = null) {
    if (node.nodeType === 3) {
      const raw = node.nodeValue || "";
      // Preserve typed newlines when read() strips paragraph wrapper newlines.
      const text = node.parentNode?.hasAttribute?.("data-editor-tail") ? raw.replace(/\n/g, "\uE002") : raw;
      return point?.node === node ? text.slice(0, point.offset) + marker + text.slice(point.offset) : text;
    }
    const children = Array.from(node.childNodes || []);
    const parts = children.map((child, i) =>
      (point?.node === node && point.offset === i ? marker : "") + markdown(child, point),
    );
    let text = parts.reduce((all, part) => {
      if ((node === editor || node.tagName === "BLOCKQUOTE") && /\n$/.test(all) && /^\n/.test(part)) {
        return all.replace(/\n+$/, "") + "\n\n" + part.replace(/^\n+/, "");
      }
      return all + part;
    }, "");
    if (point?.node === node && point.offset === children.length) text += marker;
    const tag = node.tagName;
    if (node.hasAttribute?.("data-editor-padding")) return "";
    if (tag === "BR") return "\n";
    if (tag === "STRONG" || tag === "B") return `**${text}**`;
    if (tag === "EM" || tag === "I") return `*${text}*`;
    if (tag === "DEL" || tag === "S") return `~~${text}~~`;
    if (tag === "PRE") return `\n\n\`\`\`${node.dataset.lang || ""}\n${text}\n\`\`\`\n\n`;
    if (tag === "CODE" && node.parentNode?.tagName !== "PRE") return `\`${text}\``;
    if (tag === "A") {
      const url = node.getAttribute("href") || "";
      return text.replace(marker, "") === url ? text : `[${text}](${url})`;
    }
    if (/^H[1-6]$/.test(tag)) return `\n\n${"#".repeat(Number(tag[1]))} ${text}\n\n`;
    if (tag === "BLOCKQUOTE") return `\n\n${text.trim().split("\n").map((line) => `> ${line}`).join("\n")}\n\n`;
    if (tag === "LI") {
      const index = Array.from(node.parentNode.children).indexOf(node) + 1;
      const prefix = node.parentNode.tagName === "OL" ? `${index}. ` : "- ";
      return prefix + text.replace(/^☑ /, "[x] ").replace(/^☐ /, "[ ] ") + "\n";
    }
    if (tag === "UL" || tag === "OL") return `\n\n${text}\n`;
    if (tag === "HR") return "\n\n---\n\n";
    if (tag === "TH" || tag === "TD") return text + " | ";
    if (tag === "TR") return "| " + text + "\n";
    if (tag === "THEAD") return text + "| " + Array(children[0]?.children.length || 1).fill("---").join(" | ") + " |\n";
    if (tag === "TABLE") return `\n\n${text}\n\n`;
    if (tag === "P") return `\n\n${text}\n\n`;
    if (tag === "DIV" && node !== editor) return `\n${text || "\n"}`;
    return text;
  }

  function read(point = null) {
    return markdown(editor, point).replace(/\u00a0/g, " ").replace(/^\n+|\n+$/g, "").replace(/\uE002/g, "\n");
  }

  function selectionOffset(end) {
    const selection = window.getSelection?.();
    if (!selection?.rangeCount || !editor.contains(selection.anchorNode)) return end ? savedEnd : savedStart;
    const range = selection.getRangeAt(0);
    const node = end ? range.endContainer : range.startContainer;
    const offset = end ? range.endOffset : range.startOffset;
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
    const whitespace = source.match(/\s*$/)[0];
    const body = source.slice(0, source.length - whitespace.length);
    editor.innerHTML = body ? render(body) : "";
    if (source && document.createTextNode) {
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
  editor.addEventListener("beforeinput", (event) => {
    if (composing || event.isComposing || !["insertText", "insertLineBreak"].includes(event.inputType)) return;
    if (event.inputType === "insertText" && event.data === " ") leaveInlineCode(1);
    // Applying text to the Markdown model avoids inheriting a code/bold span
    // when the caret is immediately after its closing delimiter.
    event.preventDefault();
    const start = selectionOffset(false);
    const end = selectionOffset(true);
    const text = event.inputType === "insertLineBreak" ? "\n" : event.data || "";
    source = source.slice(0, start) + text + source.slice(end);
    remember();
    paint(start + text.length);
    editor.dispatchEvent(new Event("input", { bubbles: true }));
  });
  editor.addEventListener("compositionstart", () => { composing = true; });
  editor.addEventListener("compositionend", () => { composing = false; commit(); editor.dispatchEvent(new Event("input", { bubbles: true })); });
  editor.addEventListener("input", commit);
  editor.addEventListener("keydown", (event) => {
    if (event.isComposing || composing) return;
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
  // The corner's own actions belong to the thread this composer is showing. A
  // turn running in another thread is not this transcript's, so a message typed
  // here would be steered into that run and its reply would land in a
  // conversation that is not on screen: the corner offers Stop alone until the
  // reader is back in it (the header's strip is the way there), and the typed
  // text is kept rather than sent.
  const here = viewingRun();
  const busyHere = state.busy && here;
  el("composer").classList.toggle("filled", hasText);
  el("send").classList.toggle("enabled", hasText);
  el("send").disabled = !hasText;
  el("send").hidden = state.busy && (!hasText || !here);
  el("stop").hidden = !state.busy || (hasText && here);
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
  state.followUpBehavior = mode === "steer" ? "steer" : "queue";
  try {
    window.localStorage?.setItem("oxide.followUpBehavior", state.followUpBehavior);
  } catch {
    // The preference still applies for this window when storage is unavailable.
  }
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
  list.hidden = state.pendingSends.length === 0 || (state.busy && !viewingRun());
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
  document.body.classList.toggle("running", state.busy && viewingRun());
  paintLiveChanges();
  paintDockToggles();
  paintChatTabs();
}

function setBusy() {
  const starting = !state.busy;
  state.busy = true;
  if (starting) {
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
  finishRunClock();
  state.busy = false;
  paintRunningChrome();
  state.runId = null;
  // The turn is over, so the thread it was in is nobody's running thread: what
  // the window shows is the reader's own thread again, wherever that is. The
  // run's folder is dropped with it, since nothing is running in it any more —
  // a thread it left parked stays parked, which is the strip's way back.
  state.runSession = null;
  state.runProject = null;
  state.runTitle = "";
  // And no send is in flight any more, so the next run cannot be judged by the
  // view this one was composed in.
  state.sendView = null;
  updateSendState();
  updateRunBanner();
  paintStatus();
  paintRunClock();
}

/// Whether the thread the transcript is showing is the one the turn on the window
/// is running in. A reader who opened another thread while it works is reading
/// that one, and the composer refuses to send into it — the run's own output
/// belongs to the thread they left, which is parked with the run (see `parkRun`)
/// rather than painted under this one. A turn with no thread recorded for it is
/// not known to be anywhere else, so the transcript on screen is where it is
/// read.
function viewingRun() {
  if (state.runSession != null) return state.runSession === state.session;
  // A turn that has not reported itself yet has no id of its own, and the thread
  // the message being started was composed in is where it is running — so the same
  // question is asked of that one. It matters: between the send and `agent-start`
  // the reader can open another thread, and a message typed there would be steered
  // into a run whose reply belongs to a conversation that is not on screen.
  if (state.sendView != null) {
    return (!state.sendView.tabId || state.sendView.tabId === activeTabId) && state.sendView.session === state.session && state.sendView.project === state.project;
  }
  return true;
}

/// The thread the header's strip names: the one a turn is running in, and — once
/// that turn has ended while the reader was elsewhere — the one its transcript is
/// still parked for, since what it painted is waiting there to be read.
function bannerThread() {
  return state.runSession ?? state.parked?.session ?? null;
}

/// The thread the strip names, by the label the sidebar's own row carries for it.
function runThreadLabel() {
  const id = bannerThread();
  const listed = id ? sessionById(id) : null;
  const title = id === state.runSession ? state.runTitle : state.parked?.title;
  return title || (listed ? sessionLabel(listed) : "") || t("ui.anotherThread", "another thread");
}

/// The transcript the run's own output is painted into: the one on screen while
/// the reader is in its thread, and the parked one while they are reading
/// another. The run writes into its own thread either way — a delta, a tool card,
/// the card of a turn that changed files — so nothing it does while the reader is
/// away is lost or painted under the thread they moved to.
function runTranscript(session) {
  if (session == null) return el("transcript");
  if (state.parked?.session === session) return state.parked.node;
  // A run's thread is parked by whichever door moved the reader off it — under
  // the run's id, or under the thread a message being started was composed in
  // while the run has not said which one that is — so what is left to answer here
  // is the node: the parked transcript, or the one on screen when the thread
  // being painted is the reader's own.
  return state.parked?.session === session ? state.parked.node : el("transcript");
}

/// The finished turns whose cards a thread holds. Each thread keeps its own, so
/// the Undo an older card drops is the newest card *of that thread* rather than
/// of whichever conversation happens to be on screen.
function changesFor(session) {
  if (session != null && state.parked?.session === session) return state.parked.changes;
  return state.changes;
}

/// The folder a thread's transcript belongs to: the run's own while it is in
/// flight, the one the park recorded once it is over — the window's folder for
/// everything else. A run that ended has no folder of its own any more, but its
/// parked transcript is still that folder's, and the strip is the way back into
/// it.
function transcriptProject(session) {
  if (session != null && state.parked?.session === session) {
    return state.parked.project || state.runProject || state.project;
  }
  if (state.runSession != null && session === state.runSession) return state.runProject || state.project;
  return state.project;
}

/// Parks the thread a turn is running in when the reader is reading another one:
/// the transcript on screen goes with the run when it is the run's own — every
/// node painted into it so far, the cards of its finished turns and the totals in
/// the footer — and an empty one is opened for the run when the reader had
/// already moved on. The run goes on writing into that node, so the thread is
/// whole when the reader comes back to it, including the reply the store has not
/// been told about yet. One thread is parked at a time: a turn that ended has
/// written its thread to the store, so a second park is free to let the first go.
function parkRun() {
  // The run's own thread. A turn that has not reported itself yet has no id, and
  // the message being started is what names the thread it is in: the one it was
  // composed in, where its own bubble is already painted and where the run's
  // first output will land. A message that starts a thread of its own has no id
  // at all — the park is keyed by nothing and `agent-start` re-keys it with the
  // id the run created, which is why the park says it is still waiting for one.
  const session = state.runSession ?? state.sendView?.session ?? null;
  const starting = state.runSession == null && state.sendView != null;
  if (!starting && session == null) return;
  if (state.parked?.session === session && (!starting || state.parked.pending)) return;
  const node = document.createElement("div");
  node.className = "conversation";
  node.followLatest = el("transcript").followLatest;
  if (state.session === session) node.append(...el("transcript").children);
  state.parked = {
    session,
    // Whether this park is still waiting for the turn to say which thread it is
    // in — the case `agent-start` finishes.
    pending: starting,
    node,
    changes: state.changes,
    // The footer describes the thread being read, so the run's totals wait here
    // while the reader is away.
    usage: state.usage,
    contextWindow: state.contextWindow,
    contextCorrection: state.contextCorrection,
    title: state.runTitle,
    // And the folder it belongs to travels with it: the turn may end while the
    // reader is in another project, and the strip still opens this thread's own
    // folder rather than whichever one the window happens to be showing. Every
    // door parks before it moves the view, so a message still being started is
    // recorded under the folder it was composed in.
    project: transcriptProject(session),
  };
  state.changes = [];
}

/// Hands a parked thread back to the screen — the run has been painting into it
/// the whole time, so the reply is whole rather than re-read from a store that is
/// a step behind — and answers whether it had it.
function restoreParked(session) {
  if (state.parked?.session !== session) return false;
  const parked = state.parked;
  state.parked = null;
  state.changes = parked.changes;
  const transcript = el("transcript");
  transcript.replaceChildren(...parked.node.children);
  scrollDown(transcript, true);
  state.contextCorrection = parked.contextCorrection || null;
  state.contextWindow = correctionWindow() ?? state.catalogWindow;
  setUsage(parked.usage || {});
  return true;
}

/// The header says where the turn on the window is running while the reader is
/// looking at another thread, and the whole strip is the way back to it: the
/// rest of the window — the transcript, the composer, the send refusal — is
/// about the thread being read, so one line has to say what is still going and
/// which conversation it belongs to. A turn that ends while the reader is away
/// leaves the strip up, saying what it left behind, since its transcript is still
/// parked and nowhere else on screen. The composer's corner reads the same
/// question — off the run's own thread it offers Stop alone — so the two are
/// repainted together, and every path that moves the view goes through here.
function updateRunBanner() {
  const banner = el("run-banner");
  if (!banner) return;
  const id = bannerThread();
  const away = Boolean(id && id !== state.session);
  banner.hidden = !away;
  if (away) {
    const label = runThreadLabel();
    const text = state.runSession
      ? t("ui.aTurnIsRunningIn", "A turn is running in “{value0}”", { value0: label })
      : t("ui.aTurnFinishedIn", "A turn finished in “{value0}”", { value0: label });
    el("run-banner-text").textContent = text;
    banner.title = t("ui.open", "Open “{value0}”", { value0: label });
    banner.setAttribute("aria-label", t("ui.openIt", "{value0}. Open it.", { value0: text }));
  }
  paintQueue();
  paintRunningChrome();
  updateSendState();
  paintRunClock();
}

/// The banner's click: the thread the running turn is in — or the one a finished
/// turn left parked — opened for reading, so the Queue or Steer it takes is in
/// reach again. The run's own folder is where its thread lives, so a reader who
/// moved to another project is taken back there first. It is the same door the
/// sidebar's own row for that thread is.
async function openRun() {
  const id = bannerThread();
  if (!id || state.session === id) return;
  // The thread lives in a folder of its own — the run's while it is in flight,
  // the one its park recorded once it is over — which is not the one the store
  // was read for once the reader has opened another project's thread.
  const folder = transcriptProject(id);
  if (folder && folder !== state.project) {
    const project = state.projects.find((entry) => entry.path === folder);
    if (project) await selectProject(project);
  }
  // A thread written in a worktree keeps that checkout: the window may have moved
  // to another thread's folder while it ran, and coming back to it is coming back
  // to the files its next message belongs to.
  if (folder) {
    state.worktree = folder === state.project ? null : folder;
    paintWorktreeChip();
  }
  const listed = sessionById(id);
  if (!listed) return;
  if (!listed.unstored) {
    await selectSessionFromTree(listed);
    return;
  }
  // A thread the store has not listed yet is one this window is still holding:
  // the run wrote its header as it started, so the listing is a moment behind,
  // and what it has painted so far is not somewhere to read back from. Its own
  // parked transcript is, when there is one — the run may have gone on painting
  // into it while the reader was away.
  state.session = id;
  if (restoreParked(id)) {
    closeReview();
    refreshThreadTitle();
    updateRunBanner();
    el("prompt").focus();
    return;
  }
  state.changes = [];
  closeReview();
  if (!el("details-panel").hidden) refreshProjectDetails();
  el("transcript").innerHTML = "";
  clearPaintIdle();
  refreshThreadTitle();
  updateRunBanner();
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
    if (state.runId == null) return;
    // The run owns the thread it is in: a message typed into another thread's
    // transcript would be steered into that run while its reply has nowhere here
    // to land. The header names the thread it is in, and opening it is where a
    // Queue or a Steer belongs.
    if (!viewingRun()) {
      setStatus(t("ui.aTurnIsRunningInOpenItToQueueOrSteerOrStopIt", "A turn is running in “{value0}”; open it to queue or steer, or stop it.", { value0: runThreadLabel() }));
      return;
    }
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

/// Starts a turn. `target` is the thread and folder a message was composed in
/// when it is not the one on screen — a message that arrived as the previous run
/// finished, started once that run is really over — and either way the view it
/// was sent from is recorded, so the run's own `agent-start` can tell whether the
/// reader is still looking at the thread that message was typed in. The folder is
/// recorded with it: a message that starts a thread of its own has no id for the
/// comparison, and two folders both have none.
async function startPrompt(prompt, attachments, showBubble = true, target = null) {
  const project = target?.project || runFolder();
  if (!project) return;
  const session = target ? target.session : state.session;
  const here = project === runFolder() && session === state.session;
  const tabId = here ? activeTabId : chatTabs.find((tab) => tab.session === session)?.id;
  state.sendView = { session, project, tabId };
  // The run's own thread and folder, held apart from the ones on screen: the
  // reader may open another thread while it works, and the run keeps writing into
  // this thread of this folder.
  state.runProject = project;
  if (here) {
    clearWelcome();
    if (showBubble) {
      lastInputs.set(inputHistoryKey(), el("prompt").value);
      el("prompt").value = "";
      clearAttachments();
      el("transcript").appendChild(bubble("user", prompt, attachments));
      scrollDown(el("transcript"), true);
    }
  }
  resetTurn();
  setBusy();
  setStatus(t("composer.working", "Working…"));
  try {
    await reasoningSave;
    state.runId = await invoke("send_prompt", {
      project,
      prompt,
      // No thread on screen means the reader is starting one: `new` has the core
      // create it. `latest` would append this message to whichever thread was
      // used last, which is a thread they never chose — and one the sidebar
      // would go on listing unchanged.
      session: session || "new",
      agent: state.agent || null,
      reasoning: state.reasoningPicked ? state.reasoning : null,
      attachments: attachments.length ? attachments : null,
    });
  } catch (error) {
    setStatus(t("common.errorDetail", "Error: {value0}", { value0: error }), "error");
    setIdle();
  }
}

async function startNextPendingSend() {
  if (state.busy || state.pendingSends.length === 0) return;
  const pending = state.pendingSends.shift();
  paintQueue();
  await startPrompt(pending.prompt, pending.attachments, pending.showBubble !== false, pending);
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
    if (!state.busy || state.runId == null || !viewingRun()) return;
    const pending = state.pendingSends[index];
    const accepted = await invoke("steer_run", {
      runId: state.runId,
      message: pending.prompt,
      followUp: false,
      attachments: pending.attachments.length ? pending.attachments : null,
    });
    if (!accepted) return;
    state.pendingSends.splice(index, 1);
    el("transcript").appendChild(bubble("user", pending.prompt, pending.attachments));
    scrollDown(el("transcript"), true);
    setStatus(t("composer.steering", "Steering the active response…"));
  }
  paintQueue();
  updateSendState();
}

async function stop() {
  if (state.runId == null) return;
  await invoke("cancel_run", { runId: state.runId });
  setStatus(t("composer.stopping", "Stopping…"));
}

function ensureAssistant() {
  if (state.currentAssistant) return state.currentAssistant;
  const wrap = document.createElement("div");
  wrap.className = "msg assistant";
  const label = speakerLabel("assistant");
  const body = document.createElement("div");
  body.className = "body";
  wrap.append(label, body);
  runTranscript(state.runSession).appendChild(wrap);
  state.currentAssistant = { wrap, body, text: "" };
  return state.currentAssistant;
}

function appendText(delta) {
  const assistant = ensureAssistant();
  assistant.text += delta;
  paintMarkdown(assistant.body, assistant.text);
  scrollDown(runTranscript(state.runSession));
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
  const transcript = runTranscript(state.runSession);
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

/// The one-line subject a tool card shows. Shell calls drop the boilerplate
/// (the PATH export every desktop shell needs, the `cd` prefix, `echo` labels)
/// and keep the first real statement; other tools show their subject.
function toolSummary(name, argsJson) {
  let args = {};
  try {
    args = JSON.parse(argsJson || "{}");
  } catch (error) {
    args = {};
  }
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
    text: text.length > 96 ? `${text.slice(0, 96)}…` : text,
    title: title || text,
  };
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

/// Builds one tool card. `startTool` wires it for a live call; the stored
/// transcript calls it too so a reopened thread reads the same way.
function createToolCard(name, args) {
  const block = document.createElement("div");
  block.chatTool = { name, args };
  block.className = "tool running";
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
  tname.textContent = name;
  const targ = document.createElement("span");
  targ.className = "targ";
  const summary = toolSummary(name, args);
  targ.textContent = summary.text;
  if (summary.title) head.title = summary.title;
  const tstate = document.createElement("span");
  tstate.className = "tstate";
  tstate.innerHTML = '<span class="spinner"></span>running';
  head.append(caret, tname, targ, tstate);
  const pre = document.createElement("pre");
  pre.className = "tbody";
  const hint = document.createElement("div");
  hint.className = "thint";
  hint.hidden = true;
  const tool = {
    name,
    block,
    head,
    pre,
    hint,
    tstate,
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
  const transcript = runTranscript(state.runSession);
  transcript.appendChild(tool.block);
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
  return Boolean(runFolder() && (state.session || (state.busy && viewingRun())));
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
function finishTool(tool, output, { isError = false, diff = null, elapsed = 0 } = {}) {
  // A call that landed may have written a file, which is what the dock's own
  // Changes row is read out of (see `scheduleProjectChanges`).
  scheduleProjectChanges();
  if (tool.timer) {
    clearInterval(tool.timer);
    tool.timer = null;
  }
  tool.done = true;
  tool.full = output || "";
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
    : `${isError ? "✖" : "✔"}${counts ? ` ${counts}` : ""}${
        elapsed ? ` ${formatDuration(elapsed)}` : ""
      }`;
  if (diff) {
    tool.diffEl = diffBlock(diff);
    tool.diffEl.hidden = true;
    tool.block.appendChild(tool.diffEl);
  }
  tool.inline = Boolean(diff);
  tool.isError = isError;
  paintTool(tool);
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
  const results = new Map();
  for (const message of messages) {
    if (message.role === "tool" && message.toolCallId) {
      results.set(message.toolCallId, message.content || "");
    }
  }
  for (const message of messages) {
    if (message.role === "user") {
      container.appendChild(bubble("user", message.content, message.attachments || []));
    } else if (message.role === "assistant") {
      if (message.content) {
        container.appendChild(bubble("assistant", message.content, []));
      }
      for (const call of message.toolCalls || []) {
        const output = results.get(call.id) || "";
        const isError = output.startsWith("error:");
        const displayOutput = isError ? output.replace(/^error:\s*/, "") : output;
        const tool = createToolCard(call.name || "tool", call.arguments);
        finishTool(tool, displayOutput, { isError });
        container.appendChild(tool.block);
      }
    }
  }
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
  // is reading when it lands: that thread's transcript is the one on screen, or
  // the one parked with the run, and its own folder travels with the card so an
  // Undo reaches the work tree it really changed. `runTranscript` answers with
  // that thread's transcript — parking the run's thread if the reader moved on
  // before the turn started there — and `changesFor` with its cards, so an older
  // turn's Undo is dropped against the newest card *of that thread*.
  const session = payload.sessionId || (state.runSession ?? state.session);
  const transcript = runTranscript(session);
  const list = changesFor(session);
  // The transcript it lands in belongs to one folder — the thread's own, which is
  // the window's only when that thread is this project's. A turn the frame places
  // in another folder leaves no card here: its Undo would reach into a work tree
  // this transcript is not about.
  const folder = transcriptProject(session);
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
  if (project === runFolder() && !el("details-panel").hidden) refreshProjectDetails();
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
  const undoIcon = document.createElement("span");
  undoIcon.setAttribute("aria-hidden", "true");
  undoIcon.innerHTML = ICONS.undo;
  undo.appendChild(undoIcon);
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
  card.undo.innerHTML = escapeHtml(t("ui.undo", "Undo")) + ICONS.undo;
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
  el("review-modal").hidden = false;
  el("review-scope").value = "turn";
  paintDockToggles();
  paintReview();
  el("review-files").querySelectorAll(".change-row")[index]?.scrollIntoView?.({ block: "nearest" });
  el("review-close").focus();
}

function closeReview() {
  reviewState = null;
  el("review-modal").hidden = true;
  return paintDockToggles();
}

/// Paints the card's own listing and the file the review is showing.
function paintReview() {
  if (!reviewState) return;
  const { card } = reviewState;
  el("review-title").textContent = t("ui.changes", "Changes");
  el("review-total").innerHTML = statsHtml(card.added, card.removed);
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
    if (reviewState.openFiles.has(file.path)) {
      const body = document.createElement("div");
      body.className = "review-inline-diff";
      body.id = `review-file-${index}`;
      list.appendChild(body);
      paintReviewFile(file, body);
    }
  });
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
  const wasHidden = panel.hidden;
  panel.hidden = !state.detailsOpen || !hasChat() || !el("review-modal").hidden || !dockHasRoom();
  el("details-toggle").hidden = !hasChat();
  el("changes-toggle").hidden = !hasChat();
  paintLiveChanges();
  el("changes-toggle").setAttribute("aria-expanded", String(!el("review-modal").hidden));
  el("details-toggle").setAttribute("aria-expanded", String(!panel.hidden));
  if (wasHidden && !panel.hidden) return refreshProjectDetails();
}

function openProjectDetails() {
  state.detailsOpen = true;
  return closeReview();
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
  el("details-stats").innerHTML = changes?.project === project ? statsHtml(changes.added, changes.removed) : "";
  el("details-changes").disabled = !project;
  el("details-more").disabled = !project;
  el("details-branch").hidden = !state.git?.repo || !project;
  el("details-branch").innerHTML = ICONS.branch + `<span>${escapeHtml(project ? state.git?.branch || state.git?.detached || "" : "")}</span>`;
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
      sources.set("web", { kind: "web", name: t("ui.webSearch", "Web search") });
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
    icon.innerHTML = source.kind === "mcp" ? ICONS.power : source.kind === "web" ? ICONS.globe : ICONS.file;
    row.appendChild(icon);
  }
  const name = document.createElement("span");
  name.textContent = source.name;
  row.appendChild(name);
  if (source.server) {
    const status = document.createElement("span");
    status.className = "source-state";
    status.textContent = mcpStatusText(source.server);
    row.appendChild(status);
  }
  row.onclick = () => {
    if (attachment && isPaintableImage(attachment.dataUrl)) openImage(attachment.dataUrl, attachment.name);
    else if (source.kind === "mcp") openMcps();
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
  el("review-modal").hidden = false;
  el("review-scope").value = "repository";
  el("review-title").textContent = t("ui.changes", "Changes");
  el("review-total").innerHTML = "";
  el("review-files").replaceChildren(reviewNote(t("ui.readingChanges", "Reading changes…")));
  paintDockToggles();
  const project = runFolder();
  reviewState = { card: { repository: true, project, files: [] } };
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

/// The events that write into the thread a turn is in. The transcript events go
/// there wherever the reader is — the thread is parked while they read another,
/// never dropped (see `runTranscript`) — while the usage line that reads out that
/// conversation's own totals belongs to the thread being read and waits for its
/// own thread to be opened again. Everything else — the status line, a compaction
/// note — is the window's readout of the run and is painted wherever the reader
/// happens to be.
const FOOTER_EVENT = "usage";

/// The still-running card a tool event belongs to. Results are emitted in call
/// order, so the oldest running card with a matching name is the right one.
function activeTool(name) {
  return (
    state.tools.find((tool) => !tool.done && tool.name === name) ||
    state.tools.find((tool) => !tool.done)
  );
}

function handleEvent(event) {
  updateTranscriptFollow(runTranscript(state.runSession));
  if (event.runId != null) state.runId = event.runId;
  // A delta, a tool card, a retry that drops the attempt it replaced: each writes
  // into the thread the run is in, which is the one on screen or the one parked
  // with the run — `runTranscript` picks it. The footer is the exception: the
  // totals it shows are the read thread's own, so the run's wait with its thread.
  if (event.type === FOOTER_EVENT && !viewingRun()) {
    const parked = state.parked;
    // A run's step belongs to the thread it is in: while that thread is parked,
    // its own totals wait with it rather than being painted under the thread
    // being read.
    if (parked && parked.session === state.runSession) {
      parked.usage = addTotals(parked.usage, event);
      return;
    }
    addUsage(event);
    return;
  }
  switch (event.type) {
    case "session_title": {
      const title = String(event.title || "").trim();
      if (!title) break;
      state.runTitle = title;
      for (const tab of chatTabs) if (tab.session === state.runSession) tab.title = title;
      paintChatTabs();
      if (state.heldThread?.id === state.runSession) state.heldThread.title = title;
      if (state.parked?.session === state.runSession) state.parked.title = title;
      refreshThreadTitle();
      updateRunBanner();
      loadSessions();
      break;
    }
    case "message_update": {
      const inner = event.assistantMessageEvent || {};
      if (inner.type === "text_delta") appendText(inner.delta || "");
      else if (inner.type === "thinking_delta") appendThinking(inner.delta || "");
      break;
    }
    case "tool_call":
      startTool(event.toolName || "tool", event.arguments);
      break;
    case "tool_execution_update": {
      const tool = activeTool(event.toolName);
      if (tool) {
        tool.live += event.partialResult || "";
        tool.pre.textContent = tool.live;
        scrollDown(runTranscript(state.runSession));
      }
      break;
    }
    case "tool_execution_end": {
      const tool = activeTool(event.toolName);
      if (tool) {
        finishTool(tool, event.result || tool.live, {
          isError: event.isError,
          diff: event.diff,
          elapsed: Math.max(0, Math.round(performance.now() - tool.started)),
        });
        scrollDown(runTranscript(state.runSession));
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
      setStatus(t("ui.retrying", "Retrying ({value0}/{value1})…", { value0: event.attempt, value1: event.maxAttempts }));
      break;
    case "context_window": {
      if (!(event.tokens > 0) || !event.provider || !event.model) break;
      const correction = { tokens: event.tokens, provider: event.provider, model: event.model,
        project: viewingRun() ? runFolder() : state.parked?.project, session: state.runSession ?? state.session };
      if (viewingRun()) {
        state.contextCorrection = correction;
        state.contextWindow = correctionWindow() ?? state.catalogWindow;
        paintComposerFacts();
      } else if (state.parked?.session === state.runSession) {
        state.parked.contextCorrection = correction;
        state.parked.contextWindow = event.tokens;
      }
      break;
    }
    case "usage": {
      addUsage(event);
      break;
    }
    case "compaction":
      setStatus(t("ui.compactedEarlierMessages", "Compacted {value0} earlier messages", { value0: event.summarized }));
      break;
    case "error":
      setStatus(t("common.errorDetail", "Error: {value0}", { value0: event.message }), "error");
      break;
    default:
      break;
  }
}

// ---------- approvals ----------

function showApproval(request) {
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
  if (!questions.length) return;
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
  "mcps-modal",
  "worktrees-modal",
  "models-modal",
  "reasoning-modal",
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
];

function closeOverlays(except) {
  for (const id of OVERLAYS) {
    if (id !== except) el(id).hidden = true;
  }
  // The review's state is its card and the file it is showing, so closing the
  // panel drops it along with the markup rather than leaving it half-open.

  // The palette lives above the composer rather than in an overlay, but any
  // dialog that opens takes the keyboard with it.
  if (except !== "command-palette") closePalette();
  syncSettingsDropdowns();
}

const SETTINGS_DROPDOWNS = { "models-modal": "model", "reasoning-modal": "reasoning" };

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

function closeModels(restoreFocus = true) {
  if (el("models-modal").hidden) return;
  el("models-modal").hidden = true;
  syncSettingsDropdowns();
  if (restoreFocus) el("model").focus();
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
      el("model-value").textContent = outcome.model;
    }
    showToast(t("provider.connected", "Connected {value0}", { value0: provider.label }));
  } catch (error) {
    showToast(t("ui.loginFailed", "Login failed: {value0}", { value0: error }), "error");
  } finally {
    dismissConnecting();
  }
}

// ---------- models ----------

async function openModels() {
  // The catalog is read from this project's config, and the request would
  // otherwise resolve against the directory the app was launched in — the same
  // reason the message box asks for a folder before it sends anything.
  if (!state.project) {
    setStatus(t("composer.selectProject", "Select a project first."));
    askForProject();
    return;
  }
  closeOverlays("models-modal");
  el("models-modal").hidden = false;
  syncSettingsDropdowns();
  positionSettingsDropdowns();
  el("model-list").innerHTML = `<div class="empty" style="margin:14px">${escapeHtml(t("desktop.empty.loadingModels", "Loading\u2026"))}</div>`;
  el("model-filter").value = "";
  el("model-filter").focus();
  const project = state.project;
  try {
    const models = await invoke("list_models", { project });
    if (state.project !== project || el("models-modal").hidden) return;
    state.models = models;
    renderModels();
  } catch (error) {
    el("model-list").innerHTML = `<div class="empty" style="margin:14px">${escapeHtml(String(error))}</div>`;
  }
}

function renderModels() {
  const box = el("model-list");
  box.innerHTML = "";
  if (!state.models) return;
  const filter = el("model-filter").value.trim().toLowerCase();
  for (const provider of state.models.providers) {
    const models = provider.models.filter((model) => !filter || model.toLowerCase().includes(filter));
    if (!models.length) continue;
    const header = document.createElement("div");
    header.className = "model-provider";
    header.textContent = provider.provider + (provider.active ? t("ui.active", " (active)") : "");
    box.appendChild(header);
    for (const model of models) {
      const row = document.createElement("button");
      const current = provider.active && model === provider.current;
      row.className = "model" + (current ? " active" : "");
      row.setAttribute("aria-pressed", String(Boolean(current)));
      row.textContent = model;
      row.onclick = async () => {
        await reasoningSave;
        await invoke("set_model", { provider: provider.provider, model });
        closeModels();
        await loadInfo();
      };
      box.appendChild(row);
    }
  }
  if (!box.children.length) {
    box.innerHTML = `<div class="empty" style="margin:14px">${escapeHtml(t("desktop.empty.models", "No models found."))}</div>`;
  }
}

// ---------- reasoning ----------

// Where the keyboard was before the picker took it, the way the full-size
// preview keeps its own.
let reasoningReturnFocus = null;
// The newest read of the model's levels. A response that arrives after the
// project or model moved on is dropped rather than painting its levels over the
// new selection's.
let reasoningEpoch = 0;

/// The picker the thinking chip opens: the level a turn thinks at is a choice
/// rather than a step, and the panel's own chip offers the same list. A level
/// is applied to the turns this window sends, the way the chip's cycle applied
/// it — `updateChips` is what says which one it is now.
function openReasoning() {
  if (!state.project) {
    setStatus(t("composer.selectProject", "Select a project first."));
    return Promise.resolve();
  }
  reasoningReturnFocus = document.activeElement || null;
  closeOverlays("reasoning-modal");
  renderReasoning();
  el("reasoning-modal").hidden = false;
  syncSettingsDropdowns();
  positionSettingsDropdowns();
  focusReasoning();
  return ensureReasoningLevels();
}

function ensureReasoningLevels() {
  // A cold model cache has no advertised levels yet; ask the host, which warms
  // it from the provider's listing, and repaint the open picker with the model's
  // own levels. The built-in set stands until that answer arrives, and a late
  // answer for a project or model the reader has left is dropped.
  if (!state.project || state.reasoningLevels) return Promise.resolve();
  const epoch = ++reasoningEpoch;
  const project = state.project;
  return invoke("reasoning_levels", { project })
    .then((info) => {
      if (epoch !== reasoningEpoch || state.project !== project) return;
      if (info) {
        state.supportsReasoning = info.supportsReasoning;
        state.reasoningLevels = info.supportsReasoning === false
          ? ["auto"]
          : Array.isArray(info.reasoningLevels) ? ["auto", ...info.reasoningLevels] : null;
        if (state.reasoningLevels && !state.reasoningLevels.includes(state.reasoning)) {
          state.reasoning = "auto";
          state.reasoningPicked = true;
        }
        updateChips();
        if (!el("reasoning-modal").hidden) {
          renderReasoning();
          focusReasoning();
        }
      }
    })
    .catch(() => {
      // Keep the built-in list.
    });
}

/// The keyboard starts on the level in use — the row a reader who opened the
/// picker is looking for — rather than staying where it was, which is what a
/// dialog that never moved focus left behind the overlay.
function focusReasoning() {
  const rows = [...el("reasoning-list").children];
  const target =
    rows.find((row) => row.classList.contains("active")) || rows[0] || el("reasoning-close");
  target.focus();
}

/// Arrow keys walk the dropdown's rows and Close button.
function stepReasoningFocus(step) {
  const controls = [...el("reasoning-list").children, el("reasoning-close")];
  const at = controls.indexOf(document.activeElement);
  controls[at === -1 ? 0 : (at + step + controls.length) % controls.length].focus();
}

/// Closing hands the keyboard back to whatever opened the picker. Every path
/// goes through here — a row, the Close button, or Escape — so a reader who
/// chose a level is not left at the top of the page behind the dialog.
function closeReasoning(restoreFocus = true) {
  if (el("reasoning-modal").hidden) return;
  el("reasoning-modal").hidden = true;
  syncSettingsDropdowns();
  const target = reasoningReturnFocus;
  reasoningReturnFocus = null;
  if (restoreFocus) target?.focus?.();
}

function renderReasoning() {
  const box = el("reasoning-list");
  box.innerHTML = "";
  const levels = state.reasoningLevels || REASONING;
  for (const level of levels) {
    const current = level === state.reasoning;
    const row = document.createElement("button");
    row.type = "button";
    row.className = "reasoning-option" + (current ? " active" : "");
    // The glyph and the color say which level is in use; a screen reader is told
    // the same thing in a word it can read out.
    row.setAttribute("aria-pressed", String(current));

    const name = document.createElement("span");
    name.className = "reasoning-option-name";
    name.textContent = t(`reasoning.${level}`, level);
    row.appendChild(name);

    const hint = document.createElement("span");
    hint.className = "reasoning-option-detail";
    hint.textContent = t(`reasoning.hint.${level}`, REASONING_HINTS[level] || "");
    row.appendChild(hint);

    const check = document.createElement("span");
    check.className = "reasoning-option-check";
    setCheck(check, current);
    check.setAttribute("aria-hidden", "true");
    row.appendChild(check);

    row.onclick = () => pickReasoning(level);
    box.appendChild(row);
  }
}

// ---------- themes ----------

function themeProject() {
  return state.project || "";
}

async function openThemes() {
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
  const modal = el("language-modal");
  const open = modal.hidden;
  closeOverlays("language-modal");
  modal.hidden = !open;
  el("language").setAttribute("aria-expanded", String(open));
  if (open) renderLanguageRows(null);
}

function closeLanguage() {
  el("language-modal").hidden = true;
  el("language").setAttribute("aria-expanded", "false");
}

const THEME_SWATCH_SLOTS = ["background", "panel", "accent", "text", "success", "tool"];

function renderThemes(entries, current) {
  const box = el("theme-list");
  box.innerHTML = "";
  if (!entries.length) {
    box.innerHTML = `<div class="theme-empty">${escapeHtml(t("desktop.empty.themes", "No themes available."))}</div>`;
    return;
  }
  for (const { name, colors } of entries) {
    const row = document.createElement("button");
    row.type = "button";
    row.className = "theme-option" + (name === current ? " active" : "");

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

    row.onclick = () => selectTheme(name, row);
    box.appendChild(row);
  }
}

async function selectTheme(name, row) {
  try {
    const result = await invoke("set_theme", { project: themeProject(), name });
    applyTheme(result.colors);
    const box = el("theme-list");
    for (const node of box.querySelectorAll(".theme-option")) {
      const isActive = node === row;
      node.classList.toggle("active", isActive);
      const check = node.querySelector(".theme-option-check");
      if (check) setCheck(check, isActive);
    }
  } catch (error) {
    showToast(t("ui.themeFailed", "Theme failed: {value0}", { value0: error }), "error");
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
  updateHead(
    available
      ? t("dialog.update.available", "Update Available (v{version})", {
          version: check.latest,
        })
      : t("dialog.update.current", "Oxide {version} is up to date", {
          version: check.current,
        }),
    available
      ? t("ui.installingDownloadsTheReleaseForThisMachineAndPutsItInThisAppSPlace", "Installing downloads the release for this machine and puts it in this app's place. ") +
          t("update.keepsRunning", "The app keeps running until it is restarted.")
      : "",
  );
  paintFacts(check);
  paintChangelog(check);
  paintProgressBar(null);
  // Only the app itself decides whether it can replace this installation: a
  // copy in a system directory, a distribution's package or a checkout's build
  // cannot be written over, and the advice line above is the way to install
  // instead. The release page is where that copy is installed from, and where
  // the notes are when the release carried none of its own.
  const install = available && check.installable === true;
  updateButtons({
    "update-close": true,
    "update-page": Boolean(check.releaseUrl) && (!install || !check.notes),
    "update-install": install,
  });
  if (available) el("update-close").textContent = t("common.dismiss", "Dismiss");
  // The button says what it does rather than naming the release again: the
  // dialog's own title already carries the version it is about.
  if (install) el("update-install").textContent = t("dialog.update.install", "Update");
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

/// Stops the install the dialog is watching. The reader asked for it, so nothing
/// is put in place and nothing is reported as having gone wrong: the next check
/// offers the release again.
function cancelUpdate() {
  const token = updateInstall && updateInstall.token;
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
/// left to do about it and the window keeps no row of its own for it.
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

// ---------- MCP servers ----------

/// The servers this project loads, with the state the core reports: the same
/// listing `oxide mcp list` prints and the terminal's `/mcp` shows.
async function openMcps() {
  closeOverlays("mcps-modal");
  el("mcps-modal").hidden = false;
  el("mcp-list").innerHTML = `<div class="mcp-empty">${escapeHtml(t("desktop.empty.checkingServers", "Checking servers\u2026"))}</div>`;
  await loadMcps();
}

async function loadMcps() {
  try {
    state.mcps = await invoke("mcp_servers", { project: state.project || "" });
    state.mcpError = "";
  } catch (error) {
    state.mcpError = String(error);
  }
  renderMcps();
}

function mcpStatusText(server) {
  const keys = { connected: "ui.connected", "needs-auth": "ui.needsAuth", "needs-trust": "ui.needsTrust", disabled: "ui.disabled" };
  if (server.state === "error" && server.status?.startsWith("Error: ")) return t("common.errorDetail", "Error: {value0}", { value0: server.status.slice(7) });
  return t(keys[server.state], server.status || server.state || "");
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
    head.append(name, status, toggle);

    const detail = document.createElement("div");
    detail.className = "mcp-detail";
    detail.textContent = [server.transport, server.detail].filter(Boolean).join(" · ");
    const source = document.createElement("div");
    source.className = "mcp-source";
    source.textContent = t("ui.source", "source: {value0}", { value0: server.source });

    row.append(head, detail, source);
    box.appendChild(row);
  }
}

async function toggleMcp(server, button) {
  if (!state.project) {
    setStatus(t("composer.selectProject", "Select a project first."));
    return;
  }
  button.disabled = true;
  const enabled = !server.enabled;
  const dismissProgress = showToast(`${enabled ? "Turning on" : "Turning off"} ${server.name}…`);
  try {
    state.mcps = await invoke("set_mcp_server", {
      project: state.project || "",
      name: server.name,
      enabled,
    });
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
    const running = Boolean(state.runSession) && session.id === state.runSession;
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
    case "model":
      await openModels();
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
      } else if ((state.reasoningLevels || REASONING).includes(args)) {
        pickReasoning(args);
      } else {
        setStatus(t("ui.reasoningMustBeOneOf", "Reasoning must be one of {value0}.", { value0: (state.reasoningLevels || REASONING).join(", ") }));
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
async function requestAt() {
  const prompt = el("prompt");
  if (!prompt || !prompt.value.includes("@") || !state.project) {
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
  const rows = (answer && answer.rows) || [];
  if (!rows.length) {
    closeAt();
    return;
  }
  atStart = answer.start;
  atEnd = answer.end;
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
  if ((event.key === "Enter" || event.key === "Tab") && matches.length) {
    runPaletteEntry(matches[state.paletteIndex] || matches[0]);
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
  const levels = state.reasoningLevels || REASONING;
  if (!levels.includes(level)) return;
  state.reasoning = level;
  state.reasoningPicked = true;
  reasoningSave = reasoningSave.then(() => invoke("set_reasoning", { reasoning: level })).catch(async (error) => {
    if (state.reasoning === level) await loadInfo();
    showToast(t("ui.couldNotSaveReasoning", "Could not save reasoning: {value0}", { value0: error }), "error");
  });
  updateChips();
  // A level set while the picker is up — `Ctrl+R`, or a row — moves the mark, so
  // the dialog never shows the level it has just left. The list is rebuilt to do
  // it, which is why the keyboard is put back on the level in use: a rebuild
  // leaves focus on a row that is no longer in the document.
  if (!el("reasoning-modal").hidden) {
    renderReasoning();
    focusReasoning();
  }
}

/// A pick is the one thing that both applies a level and puts the picker away.
function pickReasoning(level) {
  setReasoning(level);
  closeReasoning();
}

function cycleReasoning() {
  // Cycle the model's own levels when its listing narrowed them, so the cycle
  // never steps onto a level the picker does not offer.
  const levels = state.reasoningLevels || REASONING;
  setReasoning(levels[(levels.indexOf(state.reasoning) + 1) % levels.length]);
}

async function initEvents() {
  await listen("agent-start", async (event) => {
    const payload = event.payload || {};
    if (payload.runId != null) state.runId = payload.runId;
    // The run belongs to the thread it started in, whichever thread the reader
    // has opened since: `state.session` follows the transcript, so the run's own
    // id is kept apart from it and the sidebar and the header can name it.
    state.runSession = payload.sessionId || null;
    state.runClockSession = payload.sessionId || state.runClockSession;
    // A turn titles its thread the moment it starts, so the header names it now
    // and the sidebar lists it — with the same label — without waiting for the
    // turn to end. The session is on disk from here, so the listing has it.
    state.runTitle = payload.title || "";
    state.heldThread = payload.sessionId ? { id: payload.sessionId, title: state.runTitle } : null;
    // A park the reader made while this turn was still starting was keyed by the
    // thread the message was composed in — or by nothing at all, when it started a
    // thread of its own: the run's own id is what the way back to it is keyed by
    // from here, and what the strip, the sidebar row and the composer read.
    if (state.parked?.pending) {
      state.parked.session = payload.sessionId || null;
      state.parked.pending = false;
    }
    // The thread this run reports is adopted by the view only while the reader
    // is still on the thread the message that started it was composed in: one
    // sent from a thread with nothing in it yet has no id to compare, and a
    // reader who opened another conversation — or another folder, which has none
    // of its own — since is reading that one.
    const sent = state.sendView;
    if (sent?.tabId) {
      const tab = chatTabs.find((entry) => entry.id === sent.tabId);
      if (tab) {
        tab.session = payload.sessionId || null;
        tab.title = payload.title || tab.title;
        if (tab.snapshot) tab.snapshot.session = tab.session;
      }
      paintChatTabs();
    }
    if (sent == null || ((!sent.tabId || sent.tabId === activeTabId) && state.session === sent.session && state.project === sent.project)) {
      state.session = payload.sessionId || null;
    }
    state.sendView = null;
    resetTurn();
    refreshThreadTitle();
    updateRunBanner();
    await loadSessions();
  });
  await listen("agent-event", (event) => handleEvent(event.payload || {}));
  await listen("agent-end", async (event) => {
    setIdle();
    setStatus(idleStatus());
    renderChanges(event.payload || {});
    resetTurn();
    // A turn is where the branch the composer's top row reads can change — the
    // model runs git like any other command — so the row is read again before
    // the next message is composed against it.
    loadGit();
    await loadSessions();
    await startNextPendingSend();
  });
  el("review-modal").onkeydown = reviewKey;
  await listen("approval-request", (event) => showApproval(event.payload || {}));
  // The macOS menu item has no page of its own to paint into, so it asks the
  // window for the dialog the sidebar's own button opens.
  await listen("check-updates", () => openUpdate());
  await listen("chat-tab", (event) => event.payload?.action === "close" ? closeChatTab(activeTabId) : newChatTab());
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
  el("create-project-folders").innerHTML = "";
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
  // changes its display name only; do not offer add/remove folder controls whose
  // changes the command cannot persist.
  el("create-project-sources").hidden = true;
  el("create-project-title").textContent = t("ui.editProject", "Edit project");
  el("create-project-save").textContent = t("common.save", "Save");
  el("create-project-remove").hidden = false;
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
    const pathEl = document.createElement("span");
    pathEl.className = "folder-item-path";
    pathEl.textContent = folder;
    const removeBtn = document.createElement("button");
    removeBtn.className = "folder-item-remove ghost small";
    removeBtn.textContent = t("common.remove", "Remove");
    removeBtn.onclick = () => removeCreateProjectFolder(folder);
    item.appendChild(pathEl);
    item.appendChild(removeBtn);
    container.appendChild(item);
  }
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
  const maxWidth = () => Math.max(MIN, window.innerWidth - 420);
  const clamp = (width) => Math.max(MIN, Math.min(maxWidth(), Math.round(width)));
  const apply = (width, persist) => {
    const next = clamp(width);
    sidebar.style.width = `${next}px`;
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
  return window.innerWidth - sidebar - 7 >= 1100;
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
  installMarkdownInput(el("prompt"), (text) => renderMarkdown(text, true));
  initSidebarResize();
  initChatTabs();
  initMessageNavigator();
  initJumpLatest();
  initOverlayScrollbars();
  // The language is the core's to resolve, so the page asks before it paints
  // anything of its own; the markup it was authored in stands until then.
  void loadLanguage();
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
  el("create-project-cancel").onclick = () => (el("create-project-modal").hidden = true);
  el("create-project-save").onclick = saveCreateProject;
  el("create-project-remove").onclick = () => {
    const project = createProjectState.editing;
    if (!project) return;
    el("create-project-modal").hidden = true;
    removeProject(project);
  };

  el("reasoning").onclick = () => el("reasoning-modal").hidden ? openReasoning() : closeReasoning();
  el("reasoning-close").onclick = closeReasoning;
  el("reasoning-modal").addEventListener("keydown", (event) => {
    if (event.key === "Tab") {
      closeReasoning();
      event.stopPropagation();
    } else if (event.key === "ArrowDown" || event.key === "ArrowUp") {
      event.preventDefault();
      stepReasoningFocus(event.key === "ArrowDown" ? 1 : -1);
    }
  });
  el("models-modal").addEventListener("keydown", (event) => {
    if (event.key === "Tab") return closeModels();
    if (event.key !== "ArrowDown" && event.key !== "ArrowUp") return;
    event.preventDefault();
    const rows = [...el("model-list").children].filter((row) => row.tagName === "BUTTON");
    const index = rows.indexOf(document.activeElement);
    const step = event.key === "ArrowDown" ? 1 : -1;
    rows[(index + step + rows.length) % rows.length]?.focus();
  });
  document.addEventListener("click", (event) => {
    if (event.target?.closest?.(".settings-dropdown, .setting-chip")) return;
    closeModels(false);
    closeReasoning(false);
  });
  window.addEventListener("resize", positionSettingsDropdowns);
  el("model").onclick = () => el("models-modal").hidden ? openModels() : closeModels();
  el("theme").onclick = openThemes;
  el("language").onclick = openLanguage;
  el("language-close").onclick = closeLanguage;
  el("permissions").onclick = openPermissions;
  el("help").onclick = toggleHelp;
  el("connect").onclick = openConnect;

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
  el("models-close").onclick = () => closeModels();
  el("model-filter").addEventListener("input", renderModels);
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
  el("mcps-close").onclick = () => (el("mcps-modal").hidden = true);
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
  el("update").onclick = openUpdate;
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
    if (!el("review-modal").hidden) return openProjectDetails();
    state.detailsOpen = !state.detailsOpen;
    paintDockToggles();
  };
  el("details-more").onclick = () => {
    const project = state.projects.find((project) => project.path === state.project);
    if (project) showProjectActions(project, listedSessions().filter((session) => sessionProject(session) === project.path), el("details-more"), el("details-more"));
  };
  el("details-changes").onclick = openRepositoryChanges;
  el("details-add-source").onclick = () => el("attach-input").click();
  el("details-view-sources").onclick = openSources;
  el("sources-close").onclick = () => { el("sources-modal").hidden = true; el("details-view-sources").focus(); };
  el("sources-manage").onclick = openMcps;
  el("details-changes-icon").innerHTML = ICONS.file;
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
    if (!event.target?.closest?.(".sidebar-floating-card,.row-more")) closeSidebarCards();
    const link = event.target?.closest?.("a[href]") || null;
    if (!link) return;
    event.preventDefault();
    invoke("open_url", { url: link.getAttribute("href") }).catch((error) =>
      showToast(t("ui.couldNotOpenLink", "Could not open link: {value0}", { value0: error }), "error"),
    );
  });

  el("prompt").addEventListener("input", (event) => {
    if (event.isComposing) return;
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
  });
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
    for (const id of ["projects-modal", "mcps-modal", "worktrees-modal", "sessions-modal"]) {
      const box = el(id);
      if (box.hidden || box.contains(target)) continue;
      box.hidden = true;
    }
    // The worktree chip says whether its listing is up, and this is the one
    // closing that does not go through the control itself.
    el("worktree").setAttribute("aria-expanded", String(!el("worktrees-modal").hidden));
  });
  el("image-modal").addEventListener("keydown", (event) => {
    if (event.key !== "Tab") return;
    event.preventDefault();
    el("image-view-close").focus();
  });
  el("prompt").addEventListener("keydown", (event) => {
    if (event.isComposing) return;
    if (atKey(event)) {
      event.preventDefault();
      return;
    }
    if (paletteKey(event)) {
      event.preventDefault();
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
    if (event.key === "Enter" && state.busy && event.shiftKey && (event.metaKey || event.ctrlKey)) {
      event.preventDefault();
      send(true);
      return;
    }
    if (event.key === "Enter" && !event.shiftKey) {
      event.preventDefault();
      send(false);
    }
  });

  document.addEventListener("keydown", (event) => {
    if (event.defaultPrevented) return;
    if (event.key === "Escape") {
      el("message-preview").hidden = true;
      const modalOpen = OVERLAYS.some((id) => !el(id).hidden);
      closeSidebarCards();
      closeImage();
      closeReasoning();
      closeModels();
      if (!el("confirm-modal").hidden) resolveConfirm(false);
      if (!el("rename-modal").hidden) resolveRename(null);
      // A question dismissed with Escape is answered as unanswered rather than
      // hidden, so the turn continues instead of waiting out the timeout.
      if (!el("question").hidden) answerQuestion(true);
      closeOverlays();
      if (!modalOpen) {
        closeReview();
      }
      return;
    }
    if (event.key === "Tab" && event.shiftKey) {
      if (event.target?.closest?.(".changes-pane, .details-pane")) return;
      // Shift+Tab belongs to the picker while it is up: it walks the rows, which
      // is what the dialog's own handler does with it, rather than cycling the
      // levels under a reader who is choosing one.
      if (!el("reasoning-modal").hidden) return;
      event.preventDefault();
      cycleReasoning();
      return;
    }
    if (!event.ctrlKey && !event.metaKey) return;
    if (event.key === "r") {
      event.preventDefault();
      cycleReasoning();
    } else if (event.key === "k") {
      event.preventDefault();
      openModels();
    } else if (event.key === "/") {
      event.preventDefault();
      toggleHelp();
    } else if (event.key >= "1" && event.key <= "9") {
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
  initEvents();
  loadTheme();
  // The sidebar is loaded rather than opened on: a project is not selected for
  // the reader on the way in, so the window starts with nothing open — the home
  // transcript, the composer and the project chip that takes a folder.
  loadProjects();
}

init();
setTimeout(drainPendingHostMessages, 0);

// ============================================================================
// CodeX-style tree view rendering
// ============================================================================

// Flattened project/session order, matching how the tree renders them, so
// `⌘1`…`⌘9` select the thread standing at that place in the sidebar.
function orderedSessions() {
  const byProject = new Map();
  for (const project of state.projects) byProject.set(project.path, []);
  for (const session of listedSessions()) {
    const list = byProject.get(sessionProject(session));
    if (list) list.push(session);
  }
  const ordered = [];
  for (const project of state.projects) ordered.push(...(byProject.get(project.path) || []));
  return ordered;
}

/// The project whose row is in the air, and the row it would land on. A drop is
/// a move of the whole row — the project and the threads under it — rather than
/// of a thread, which is why this is one id and not a list.
let draggedProjectId = null;
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
  if (state.busy && (state.session === session.id || state.runSession === session.id)) {
    showToast(t("ui.aTurnIsRunningStopItBeforeArchivingThisThread", "A turn is running; stop it before archiving this thread."));
    return;
  }
  try {
    state.sessions = await invoke("archive_session", { id: session.id });
    for (let index = chatTabs.length - 1; index >= 0; index--) {
      if (chatTabs[index].id !== activeTabId && chatTabs[index].session === session.id) chatTabs.splice(index, 1);
    }
    paintChatTabs();
    if (state.parked?.session === session.id) state.parked = null;
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

async function renderProjectsTree() {
  const container = el("projects-tree");
  if (!container) return;
  closeSidebarCards();
  
  container.innerHTML = "";
  
  if (!state.projects || !state.projects.length) {
    container.innerHTML = `<div class="empty" style="margin:12px; font-size:12px; color:var(--faint);">${escapeHtml(t("desktop.empty.sidebar", "No projects yet. Add a folder with the + above."))}</div>`;
    return;
  }
  
  // Group sessions by project
  const sessionsByProject = {};
  const visibleProjects = state.sidebarProjectsExpanded
    ? state.projects
    : state.projects.slice(0, SIDEBAR_PROJECT_LIMIT);

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
  
  for (const project of visibleProjects) {
    const projectGroup = document.createElement("div");
    projectGroup.className = "project-group";
    
    // Project header/button
    const projectItem = document.createElement("div");
    projectItem.className = "project-item" + (project.path === state.project ? " active" : "");
    
    const icon = document.createElement("div");
    icon.className = "icon";
    icon.innerHTML = ICONS.folder;
    projectItem.appendChild(icon);
    
    const name = document.createElement("div");
    name.className = "name";
    name.textContent = project.name;
    projectItem.appendChild(name);
    
    const count = document.createElement("div");
    count.className = "count";
    const sessionsForProject = sessionsByProject[project.path] || [];
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

    projectItem.onclick = () => {
      selectProject(project);
    };

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
      const running = Boolean(state.runSession) && session.id === state.runSession;
      const sessionItem = document.createElement("div");
      sessionItem.className =
        "session-item" +
        (session.id === state.session ? " active" : "") +
        (running ? " running" : "");

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

  if (!state.sidebarProjectsExpanded && state.projects.length > SIDEBAR_PROJECT_LIMIT) {
    const moreProjects = document.createElement("button");
    moreProjects.type = "button";
    moreProjects.className = "sidebar-show-more projects-show-more";
    moreProjects.textContent = t("sidebar.showMore", "Show more");
    moreProjects.title = t("ui.showMoreProjects", "Show {value0} more projects", { value0: state.projects.length - SIDEBAR_PROJECT_LIMIT });
    moreProjects.onclick = () => {
      state.sidebarProjectsExpanded = true;
      renderProjectsTree();
    };
    container.appendChild(moreProjects);
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

  const recentSessions = [...listedSessions()]
    .sort((a, b) => (b.modified_at || b.created_at || 0) - (a.modified_at || a.created_at || 0))
    .slice(0, SIDEBAR_RECENT_LIMIT);
  for (const session of recentSessions) {
    const row = document.createElement("button");
    row.type = "button";
    row.className =
      "recent-session-item" +
      (session.id === state.session ? " active" : "") +
      (session.id === state.runSession ? " running" : "");
    const name = document.createElement("div");
    name.className = "name";
    name.textContent = sessionLabel(session);
    row.title = `${sessionLabel(session)} — ${
      session.worktree ? `worktree ${session.worktree}` : projectNameOf(sessionProject(session))
    }`;
    row.appendChild(name);
    if (session.id === state.runSession) {
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
  if (!session) return;
  const existing = chatTabs.find((tab) => tab.session === session.id && tab.id !== activeTabId);
  if (existing) return activateChatTab(existing.id);

  const project = sessionProject(session);
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
  // The folder is going away from the window, so a turn running in it parks its
  // transcript like any other thread the reader leaves: it keeps its thread and
  // goes on writing into it, and the strip is the way back.
  parkRun();
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
  if (state.busy && (state.runProject || state.project) === project.path) {
    showToast(t("ui.aTurnIsRunningStopItBeforeRemovingThisProject", "A turn is running; stop it before removing this project."));
    return;
  }

  try {
    const choice = await confirmDialog(
      t("dialog.createProject.remove", "Remove project"),
      t("ui.removeFromTheSidebarItsSessionsStayOnDiskUseOxideSessionsDeleteFromTheProjectFol", "Remove “{value0}” from the sidebar?\n\nIts sessions stay on disk. Use oxide sessions delete from the project folder to delete sessions permanently.", { value0: project.name }),
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
