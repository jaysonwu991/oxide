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
  folder: glyph(
    `<path d="M20.2 19.4a1.8 1.8 0 0 0 1.8-1.8v-8a1.8 1.8 0 0 0-1.8-1.8h-7.9a1.8 1.8 0 0 1-1.5-.8l-.9-1.4a1.8 1.8 0 0 0-1.5-.8H3.8A1.8 1.8 0 0 0 2 6.6v11a1.8 1.8 0 0 0 1.8 1.8Z" ${STROKE_24}/>`,
  ),
  file: glyph(
    `<path d="M14 3H7a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h10a2 2 0 0 0 2-2V8z" ${STROKE_24}/>` +
      `<path d="M14 3v5h5" ${STROKE_24}/>`,
  ),
  close: glyph(`<path d="M18 6 6 18M6 6l12 12" ${STROKE_24}/>`),
  plus: glyph('<path d="M8 3.6v8.8M3.6 8h8.8" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round"/>', 16),
  check: glyph('<path d="m5.6 8 1.5 1.5 3.5-3.5" ' + STROKE_16 + '/>', 16),
  caret: glyph(`<path d="M5.5 8.6 12 14.8l6.5-6.2" ${STROKE_24}/>`),
  power: glyph(
    '<path d="M18.36 6.64a9 9 0 1 1-12.73 0" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round"/>' +
      '<path d="M12 2v10" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round"/>',
  ),
  mark: '<svg viewBox="0 0 24 24" aria-hidden="true"><path d="M12 2.4 21.6 12 12 21.6 2.4 12Z" fill="currentColor"/></svg>',
};

/// The check a chosen row wears, or nothing at all on a row that is not the one
/// in use: a picker repaints its rows in place, so what it draws is replaced
/// rather than added to.
function setCheck(node, on) {
  node.innerHTML = on ? ICONS.check : "";
}

const state = {
  projects: [],
  project: null,
  projectName: "",
  session: null,
  sessions: [],
  busy: false,
  runId: null,
  // New context sent during a run waits by default. Steering is a deliberate
  // choice because it changes the work already in progress.
  busyMessageMode: "queue",
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
  reasoning: "auto",
  // The levels the active model advertised, when its listing carried them. The
  // picker shows these instead of the full set, so it never offers a level the
  // model would clamp away.
  reasoningLevels: null,
  contextWindow: 0,
  providers: [],
  // The provider the connect dialog has selected, kept by name rather than by
  // row: the list is filtered as the reader searches, so a row's position says
  // nothing about which provider is chosen.
  providerName: "",
  models: null,
  pendingApproval: null,
  pendingQuestion: null,
  trust: null,
  // What the footer's usage line is showing, which is the open thread's own
  // totals rather than the window's: a run in another thread carries them while
  // its thread is parked (see `parkRun`).
  usage: null,
  currentAssistant: null,
  tools: [],
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
  if (mins < 1) return "just now";
  if (mins < 60) return `${mins}m ago`;
  const hours = Math.round(mins / 60);
  if (hours < 24) return `${hours}h ago`;
  const days = Math.round(hours / 24);
  if (days < 30) return `${days}d ago`;
  return date.toLocaleDateString(undefined, { month: "short", day: "numeric" });
}

function baseName(path) {
  const trimmed = String(path).replace(/[\\/]+$/, "");
  return trimmed.split(/[\\/]/).pop() || trimmed;
}

function setStatus(text) {
  el("status-text").textContent = text;
}

/// The totals a `usage` event carries, as the footer draws them. The provider
/// reports `input` as the uncached prompt, and the cached prefix still occupies
/// the window, so the two are carried as the raw `prompt`: the percentage is a
/// measure of a window, and which window that is is a question about where the
/// totals are painted rather than about where the event arrived (see `setUsage`).
function usageTotals(event) {
  const usage = event.usage || {};
  return {
    input: usage.input,
    output: usage.output,
    cacheRead: usage.cacheRead,
    cacheWrite: usage.cacheWrite,
    cost: usage.cost,
    prompt: (usage.input || 0) + (usage.cacheRead || 0) + (usage.cacheWrite || 0),
  };
}

function setUsage({
  input = 0,
  output = 0,
  cacheRead = 0,
  cacheWrite = 0,
  cost = 0,
  prompt = 0,
  contextPct = null,
}) {
  // The window the percentage is a fraction of is the one in force where the
  // totals are drawn, not the one that happened to be open when the event
  // arrived: a run in another folder counts its tokens while the reader is
  // looking at a project with a window of its own — and its totals come back to
  // the folder they belong to, since the strip opens the run's own first.
  if (contextPct == null && prompt > 0 && state.contextWindow > 0) {
    contextPct = (prompt / state.contextWindow) * 100;
  }
  // What the footer is showing, held so a thread the reader leaves can take its
  // own totals with it and paint them back on return.
  state.usage = { input, output, cacheRead, cacheWrite, cost, prompt, contextPct };
  const bits = [`↑ ${input} ↓ ${output}`];
  if (cacheRead || cacheWrite) bits.push(`R ${cacheRead} W ${cacheWrite}`);
  if (cost) bits.push(`$${Number(cost).toFixed(4)}`);
  if (contextPct != null && state.contextWindow) bits.push(`ctx ${contextPct.toFixed(0)}%`);
  el("usage").textContent = bits.join(" · ");
}

function setThreadTitle(text) {
  el("thread-title").textContent = text || "";
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
      // window instead of through `session_messages` or `delete_session`.
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

function inline(text) {
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
  s = s.replace(/`([^`]+)`/g, (_, code) => stash(`<code>${escapeHtml(code)}</code>`));
  s = s.replace(/\[([^\]]+)\]\(([^)\s]+)\)/g, (_, label, href) =>
    stash(anchor(href, escapeHtml(label))),
  );
  s = s.replace(/<(https?:\/\/[^\s<>]+)>/g, (_, url) => stash(anchor(url)));
  s = s.replace(/\bhttps?:\/\/[^\s<>"'`]+/g, (url) => autolink(url, stash));
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

function tableHtml(header, rows) {
  const head = header.map((cell) => `<th>${inline(cell)}</th>`).join("");
  const body = rows
    .map((row) => `<tr>${row.map((cell) => `<td>${inline(cell)}</td>`).join("")}</tr>`)
    .join("");
  return `<table><thead><tr>${head}</tr></thead><tbody>${body}</tbody></table>`;
}

function renderMarkdown(text) {
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
      i += 1;
      const lang = fence[1] ? ` data-lang="${escapeHtml(fence[1])}"` : "";
      out.push(`<pre${lang}><code>${highlight(code.join("\n"), fence[1])}</code></pre>`);
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
      out.push(`<h${level}>${inline(heading[2])}</h${level}>`);
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
      out.push(`<blockquote>${renderMarkdown(quote.join("\n"))}</blockquote>`);
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
      out.push(`<li class="task">${mark} ${inline(task[2])}</li>`);
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
      out.push(`<li>${inline(bullet[1])}</li>`);
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
      out.push(`<li>${inline(ordered[1])}</li>`);
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
      out.push(tableHtml(header, rows));
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
    out.push(`<p>${inline(para.join(" "))}</p>`);
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
  title.textContent = "What should we build?";
  const hint = document.createElement("p");
  hint.textContent = project
    ? project
    : (state.projects || []).length
      ? "Pick a folder with the project chip below, or open a recent thread."
      : "Add a folder to run in — the project chip below, or the + beside Projects.";
  welcome.append(mark, title, hint);
  if (project) {
    const suggestions = document.createElement("div");
    suggestions.className = "suggestions";
    for (const text of SUGGESTIONS) {
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
    .filter((session) => !session.unstored && listed.has(session.cwd))
    .slice(0, MAX_RECENT_THREADS);
}

function recentList(threads) {
  const box = document.createElement("div");
  box.className = "recent";
  const label = document.createElement("div");
  label.className = "recent-label";
  label.textContent = "Recent threads";
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
    meta.textContent = [projectNameOf(session.cwd), sessionAge(session.modified_at)]
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
    setStatus(`Failed to load projects: ${error}`);
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
    box.innerHTML = '<div class="dialog-empty">No projects yet. Add a folder to run in.</div>';
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
  state.project = project.path;
  state.projectName = project.name;
  state.session = null;
  state.trust = null;
  state.pendingSends = [];
  if (wasOpen) clearAttachments();
  el("projects-modal").hidden = true;
  el("trust-modal").hidden = true;
  updateTrustButton();
  updateChips();
  setStatus("Ready");
  renderProjectsTree(); // Update tree view instead of dropdown
  resetTranscript();
  await Promise.all([loadInfo(), loadSessions(), loadTheme()]);
}

async function loadInfo() {
  if (!state.project) return;
  const project = state.project;
  try {
    const info = await invoke("project_info", { project });
    // The selection can change while the request is in flight; a late reply
    // must not replace the new project's trust state or open its dialog.
    if (project !== state.project) return;
    state.reasoning = info.reasoning;
    state.reasoningLevels =
      Array.isArray(info.reasoningLevels) && info.reasoningLevels.length
        ? ["auto", ...info.reasoningLevels]
        : null;
    state.contextWindow = info.contextWindow || 0;
    state.trust = info.trust || null;
    updateChips();
    renderProjectMeta(info);
    updateTrustButton();
    if (state.trust && state.trust.awaiting) showTrust(state.trust);
  } catch (error) {
    if (project !== state.project) return;
    el("project-meta").textContent = String(error);
    el("project-meta").hidden = false;
  }
}

/// The model the composer's own chip names, and the little the header adds: the
/// provider is already that chip, so only what the user has to act on — a
/// missing key, project resources left off — is written beside the title.
function renderProjectMeta(info) {
  const window = state.contextWindow ? ` · ${formatTokens(state.contextWindow)}` : "";
  labelControl("model", `model: ${info.model}${window}`, "Switch model");
  const notes = [];
  if (!info.hasKey) notes.push("no API key");
  if (info.trust && info.trust.required && !info.trust.trusted) {
    notes.push("project resources off");
  }
  el("project-meta").textContent = notes.join(" · ");
  el("project-meta").hidden = notes.length === 0;
}

function updateTrustButton() {
  const button = el("trust");
  if (!button) return;
  const required = Boolean(state.project && state.trust && state.trust.required);
  button.hidden = !required;
  button.classList.toggle("active", Boolean(state.trust && state.trust.trusted));
  const label = state.trust && state.trust.trusted
    ? "Project trusted · click to review"
    : "Project resources are not trusted";
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
  try {
    const info = await invoke("set_project_trust", { project: state.project, trusted });
    state.trust = info.trust || null;
    renderProjectMeta(info);
    updateTrustButton();
    setStatus(
      trusted
        ? "Project trusted · project resources loaded"
        : "Project resources left untrusted",
    );
  } catch (error) {
    setStatus(`Trust update failed: ${error}`);
  }
}

function updateChips() {
  labelControl("reasoning", `thinking: ${state.reasoning}`, "Choose the reasoning level (Shift+Tab cycles)");
  updateProjectChip();
  // A folder switch resets the transcript, which is where the header's own
  // banner is decided.
  updateRunBanner();
}

/// The composer's project chip names the folder the message goes to, and asks for
/// one while there is none: a project is not selected for the reader on the way
/// in, so the chip is where they pick it. It keeps its words rather than hiding
/// them in a tooltip, since with nothing open it reads as the action it is.
function updateProjectChip() {
  const open = Boolean(state.project);
  const name = open ? projectNameOf(state.project) : "Choose a project";
  el("project-name").textContent = name;
  el("project").classList.toggle("empty", !open);
  el("project").title = open
    ? `project: ${name}\nSwitch the folder this window runs in`
    : "Choose a project\nPick the folder this window runs in";
  el("project").setAttribute("aria-label", open ? `project: ${name}` : "Choose a project");
}

/// An icon-only control names the value it holds where a text chip used to show
/// it — the tooltip and the accessible name — the way the VS Code panel's own
/// chips carry it, so the composer's row is the same chrome in both front-ends.
function labelControl(id, label, hint) {
  const chip = el(id);
  chip.title = hint ? `${label}\n${hint}` : label;
  chip.setAttribute("aria-label", label);
}

/// A context window in the compact shape the panel's chip uses for it, so the
/// same number reads the same way in both front-ends.
function formatTokens(count) {
  if (count < 1000) return String(count);
  if (count < 1000000) return `${(count / 1000).toFixed(1)}k`;
  return `${(count / 1000000).toFixed(2)}M`;
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
    setStatus(`Failed to load threads: ${error}`);
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
async function openSession(session) {
  // Leaving the thread a turn is running in parks its transcript with the run:
  // what it has painted stays, and its next step goes on painting into it.
  parkRun();
  state.session = session.id;
  // Coming back to a thread the run parked: its own transcript is the thread —
  // with everything the run did while the reader was away, including the reply
  // the store has not been told about yet — so it is handed back rather than
  // read again from a listing that is a step behind.
  if (restoreParked(session.id)) {
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
  closeReview();
  updateRunBanner();
  try {
    const data = await invoke("session_messages", { project: state.project, id: session.id });
    const transcript = el("transcript");
    transcript.innerHTML = "";
    renderStoredTranscript(transcript, data.messages);
    scrollDown();
    setThreadTitle(sessionLabel(session));
    if (data.usage) {
      setUsage({
        input: data.usage.input,
        output: data.usage.output,
        cacheRead: data.usage.cacheRead,
        cacheWrite: data.usage.cacheWrite,
        cost: data.usage.cost,
      });
    }
  } catch (error) {
    setStatus(`Failed to open thread: ${error}`);
  }
  loadSessions();
}

function resetTranscript() {
  state.session = null;
  // The transcript is not the running turn's own any more, so its banner says
  // where that turn is — if it is not the one this welcome belongs to.
  state.changes = [];
  closeReview();
  el("transcript").innerHTML = "";
  el("usage").textContent = "";
  state.usage = null;
  setThreadTitle("");
  renderWelcome();
  updateRunBanner();
}

/// A turn belongs to this window's own process — its tools write files and its
/// stream is read here — so anything that would replace what is on screen, or the
/// process itself, is refused while one runs.
function busyRefusal(what) {
  if (!state.busy) return false;
  setStatus(`A turn is running; stop it before ${what}.`);
  return true;
}

/// A running turn owns the thread it is on: clearing the transcript under it
/// would paint the rest of its events into a chat that no longer exists, and the
/// session id it is about to report would be dropped with the queued messages.
/// Every path that starts a fresh thread asks here first, as the VS Code panel
/// refuses its own new chat while a turn is live.
function canStartNewChat() {
  return !busyRefusal("starting a new thread");
}

function newChat() {
  if (!canStartNewChat()) return;
  state.pendingSends = [];
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
  const label = speakerLabel(kind);
  const body = document.createElement("div");
  body.className = "body";
  body.innerHTML = kind === "assistant" ? renderMarkdown(text) : escapeHtml(text);
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
    label.textContent = "you";
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
function scrollDown(node = el("transcript")) {
  node.scrollTop = node.scrollHeight;
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

/// The composer's action while idle is Send. During a run it holds Stop until
/// there is new context, then shows Send beside an explicit Queue/Steer choice.
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
  const hasBusyMessage = state.busy && hasText && here;
  const mode = el("busy-message-mode");
  el("composer").classList.toggle("filled", hasText);
  el("send").classList.toggle("enabled", hasText);
  el("send").disabled = !hasText;
  el("send").hidden = state.busy && (!hasText || !here);
  el("stop").hidden = !state.busy || (hasText && here);
  mode.hidden = !hasBusyMessage;
  mode.textContent = state.busyMessageMode === "steer" ? "Steer" : "Queue";
  mode.title = state.busyMessageMode === "steer"
    ? "Steer the active response; click to queue instead"
    : "Queue as the next turn after the current response; click to steer instead";
  mode.setAttribute("aria-label", mode.title);
  el("send").title = hasBusyMessage
    ? state.busyMessageMode === "steer"
      ? "Steer the active response (Enter)"
      : "Queue as the next turn (Enter)"
    : "Send (Enter)";
  // The accessible name follows the action the button now performs, the way the
  // panel's own corner does: a screen reader hears Queue or Steer, not the
  // label the button was built with.
  el("send").setAttribute(
    "aria-label",
    hasBusyMessage ? (state.busyMessageMode === "steer" ? "Steer" : "Queue") : "Send",
  );
}

function toggleBusyMessageMode() {
  state.busyMessageMode = state.busyMessageMode === "queue" ? "steer" : "queue";
  updateSendState();
}

function setBusy() {
  state.busy = true;
  state.busyMessageMode = "queue";
  updateSendState();
}

function setIdle() {
  state.busy = false;
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
    return state.sendView.session === state.session && state.sendView.project === state.project;
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
  return title || (listed ? sessionLabel(listed) : "") || "another thread";
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
  transcript.scrollTop = transcript.scrollHeight;
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
      ? `A turn is running in “${label}”`
      : `A turn finished in “${label}”`;
    el("run-banner-text").textContent = text;
    banner.title = `Open “${label}”`;
    banner.setAttribute("aria-label", `${text}. Open it.`);
  }
  updateSendState();
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
    setStatus("Already attached");
    return false;
  }
  if (state.attachments.length >= 8) {
    setStatus("At most 8 attachments per message");
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
        `${file.name} is ${formatBytes(file.size)}; the attachment limit is ${formatBytes(MAX_ATTACHMENT_BYTES)}`,
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
      setStatus(`Could not read ${file.name}: ${error}`);
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
    remove.title = "Remove attachment";
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

function openImage(dataUrl, name = "Attachment preview") {
  imageReturnFocus = document.activeElement || null;
  el("image-view-img").src = dataUrl;
  el("image-view-img").alt = name || "Attachment preview";
  el("image-view-img").title = name || "Attachment preview";
  el("image-view-name").textContent = name || "Attachment preview";
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
  button.title = "Open preview";
  button.setAttribute("aria-label", name ? `Open ${name}` : "Open image preview");
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

async function send(followUp = false) {
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
    setStatus("Select a project first.");
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
      setStatus(`A turn is running in “${runThreadLabel()}”; open it to queue or steer, or stop it.`);
      return;
    }
    // An explicit shortcut can always queue; the ordinary Send action follows
    // the visible choice beside it.
    followUp = followUp || state.busyMessageMode === "queue";
    textarea.value = "";
    clearAttachments();
    clearWelcome();
    el("transcript").appendChild(
      bubble("user", prompt, attachments),
    );
    scrollDown();
    const accepted = await invoke("steer_run", {
      runId: state.runId,
      message: prompt,
      followUp,
      attachments: attachments.length ? attachments : null,
    });
    state.busyMessageMode = "queue";
    updateSendState();
    if (!accepted) {
      // A message that arrived as the run finished is the next turn of the
      // thread and folder it was typed in, not of whatever is on screen by then.
      state.pendingSends.push({
        prompt,
        attachments,
        project: state.project,
        session: state.session,
      });
      setStatus("The response finished; starting this as the next turn…");
      // The end event may have beaten the command reply to the renderer.
      await startNextPendingSend();
      return;
    }
    setStatus(followUp ? "Queued as the next turn." : "Steering the active response…");
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
  const project = target?.project || state.project;
  if (!project) return;
  const session = target ? target.session : state.session;
  const here = project === state.project && session === state.session;
  state.sendView = { session, project };
  // The run's own thread and folder, held apart from the ones on screen: the
  // reader may open another thread while it works, and the run keeps writing into
  // this thread of this folder.
  state.runProject = project;
  if (here) {
    clearWelcome();
    if (showBubble) {
      el("prompt").value = "";
      clearAttachments();
      el("transcript").appendChild(bubble("user", prompt, attachments));
      scrollDown();
    }
  }
  resetTurn();
  setBusy();
  setStatus("Working…");
  try {
    state.runId = await invoke("send_prompt", {
      project,
      prompt,
      // No thread on screen means the reader is starting one: `new` has the core
      // create it. `latest` would append this message to whichever thread was
      // used last, which is a thread they never chose — and one the sidebar
      // would go on listing unchanged.
      session: session || "new",
      reasoning: state.reasoning,
      attachments: attachments.length ? attachments : null,
    });
  } catch (error) {
    setStatus(`Error: ${error}`);
    setIdle();
  }
}

async function startNextPendingSend() {
  if (state.busy || state.pendingSends.length === 0) return;
  const pending = state.pendingSends.shift();
  await startPrompt(pending.prompt, pending.attachments, false, pending);
}

async function stop() {
  if (state.runId == null) return;
  await invoke("cancel_run", { runId: state.runId });
  setStatus("Stopping…");
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
  assistant.body.innerHTML = renderMarkdown(assistant.text);
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
  const raw = String(
    args.command || args.path || args.pattern || args.query || args.url || args.prompt || "",
  )
    .replace(/\s+/g, " ")
    .trim();
  let text = raw;
  if (name === "bash" && raw) {
    const parts = raw
      .split(/\s*(?:&&|\|\||;)\s*/)
      .map((part) => part.trim())
      .filter(Boolean);
    text = parts.find((part) => !CMD_NOISE.test(part)) || parts[parts.length - 1] || raw;
  }
  return {
    text: text.length > 96 ? `${text.slice(0, 96)}…` : text,
    title: raw || text,
  };
}

/// The first `lines` of `text`, plus how many were dropped, for the collapsed
/// card preview.
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

/// Marks a card finished. A call that changed a file reads as the one line its
/// header builds — the path, and how many lines moved — and keeps its own diff
/// for the reader who expands the card: the turn's changes are listed together
/// by the change card, so the same diff is not painted twice.
function finishTool(tool, output, { isError = false, diff = null, elapsed = 0 } = {}) {
  if (tool.timer) {
    clearInterval(tool.timer);
    tool.timer = null;
  }
  tool.done = true;
  tool.full = output || "";
  tool.block.classList.remove("running");
  if (isError) tool.block.classList.add("error");
  const counts = diff ? diffCounts(diff.text) : "";
  tool.tstate.textContent = `${isError ? "✖" : "✔"}${counts ? ` ${counts}` : ""}${
    elapsed ? ` ${formatDuration(elapsed)}` : ""
  }`;
  if (diff) {
    tool.diffEl = diffBlock(diff);
    tool.diffEl.hidden = true;
    tool.block.appendChild(tool.diffEl);
  }
  tool.inline = Boolean(diff);
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
  if (tool.expanded) {
    tool.pre.hidden = false;
    tool.pre.textContent = tool.full;
    tool.hint.hidden = true;
    tool.block.classList.add("expanded");
    markToolToggle(tool, true);
    return;
  }
  tool.block.classList.remove("expanded");
  if (tool.inline) {
    tool.pre.hidden = true;
    tool.hint.hidden = true;
    markToolToggle(tool, true);
    return;
  }
  tool.pre.hidden = false;
  const { text, more } = previewText(tool.full, 3);
  tool.pre.textContent = text;
  tool.hint.hidden = more === 0;
  tool.hint.textContent = `⋯ ${more} more line${more === 1 ? "" : "s"} · click to expand`;
  markToolToggle(tool, more > 0);
}

/// A card's two handles are buttons only while it has output to fold: once
/// marked, they are reachable with Tab and toggle with Enter or Space, and a
/// card that shows everything it has stays the text it looks like — which is
/// what `foldable` draws, the caret and the pointer both. A card can only gain
/// output to fold, so the marks are never taken back.
function markToolToggle(tool, expandable) {
  if (expandable) tool.block.classList.add("foldable");
  for (const toggle of [tool.head, tool.hint]) {
    toggle.tabIndex = expandable ? 0 : -1;
    if (!expandable) continue;
    toggle.setAttribute("role", "button");
    toggle.setAttribute("aria-expanded", tool.expanded ? "true" : "false");
  }
}

function toggleTool(tool) {
  if (!tool.done) return;
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

// Rows a card shows before the rest are folded behind a "+N more files" button.
const CHANGES_VISIBLE = 5;

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
    collapsed: false,
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
}

/// `+12 −3`, shared by a card's header and its rows. A side with nothing to
/// count is left out, as the CLI's own listing spells it, so a turn that only
/// removed lines reads as `−3` rather than `+0 −3`.
function statsHtml(added, removed) {
  const add = added ? `<span class="stats-add">+${added}</span>` : "";
  const del = removed ? `<span class="stats-del">−${removed}</span>` : "";
  return add + del;
}

function statusBadge(status) {
  const key = CHANGE_LETTERS[status] ? status : "modified";
  return `<span class="change-status ${key}" title="${key}">${CHANGE_LETTERS[key]}</span>`;
}

/// The diff a row opens: the same preview the tool cards paint, without their
/// path header, since the row above it already names the file.
function changeDiffHtml(file) {
  if (file.binary) return `<div class="diff"><pre>Binary file — no text diff.</pre></div>`;
  if (!file.diff) return `<div class="diff"><pre>No textual changes.</pre></div>`;
  return `<div class="diff"><pre>${diffLines(file.diff)}</pre></div>`;
}

function changeRow(file) {
  const row = document.createElement("button");
  row.type = "button";
  row.className = "change-row";
  row.innerHTML =
    statusBadge(file.status) +
    `<span class="change-path">${escapeHtml(file.path)}</span>` +
    `<span class="change-stats">${file.binary ? "binary" : statsHtml(file.added, file.removed) || "no line changes"}</span>` +
    `<span class="change-chev">${ICONS.caret}</span>`;
  row.onclick = () => toggleChangeDiff(row, file);
  return row;
}

/// A row swaps its own diff in and out, so reading one file does not disturb
/// the rest of the listing.
function toggleChangeDiff(row, file) {
  if (row.diffPanel) {
    row.diffPanel.remove();
    row.diffPanel = null;
    row.classList.remove("open");
    return;
  }
  const panel = document.createElement("div");
  panel.className = "change-diff";
  panel.innerHTML = changeDiffHtml(file);
  row.after(panel);
  row.diffPanel = panel;
  row.classList.add("open");
}

function changesCard(card) {
  const block = document.createElement("div");
  block.className = "changes";

  const head = document.createElement("div");
  head.className = "changes-head";
  const icon = document.createElement("span");
  icon.className = "changes-icon";
  icon.setAttribute("aria-hidden", "true");
  icon.innerHTML = ICONS.file;
  const caret = document.createElement("span");
  caret.className = "changes-caret";
  caret.innerHTML = ICONS.caret;
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
  review.className = "ghost small";
  review.textContent = "Review";
  review.onclick = (event) => {
    event.stopPropagation();
    openReview(card);
  };
  const undo = document.createElement("button");
  undo.type = "button";
  undo.className = "ghost small";
  undo.textContent = "Undo";
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

  head.append(icon, caret, title, total, note, actions);
  head.onclick = () => {
    card.collapsed = !card.collapsed;
    paintChanges(card);
  };
  block.append(head, list, more);

  card.block = block;
  card.list = list;
  card.more = more;
  card.note = note;
  card.undo = undo;
  card.title = title;
  card.total = total;
  for (const file of card.files) {
    const row = changeRow(file);
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
  const count = card.files.length;
  card.title.textContent = `Edited ${count} file${count === 1 ? "" : "s"}`;
  card.total.innerHTML = statsHtml(card.added, card.removed);
  // An undo restores the state before its own turn, so it is only offered while
  // that turn is the newest one of its own thread: anything made after it would
  // go too.
  const list = card.changes || state.changes;
  card.undo.hidden = card.undone || list[list.length - 1] !== card;
  card.block.classList.toggle("collapsed", card.collapsed);
  const shown = card.all ? card.rows : card.rows.slice(0, CHANGES_VISIBLE);
  const visible = new Set(shown);
  for (const entry of card.rows) entry.row.hidden = !visible.has(entry);
  const hidden = count - shown.length;
  card.more.hidden = card.collapsed || (hidden <= 0 && !card.all);
  card.more.textContent = card.all ? "Show less" : `Show ${hidden} more file${hidden === 1 ? "" : "s"}`;
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
    "Undo changes",
    `Put ${count} file${count === 1 ? "" : "s"} back to how ${count === 1 ? "it was" : "they were"} before this turn?`,
    "Undo changes",
  );
  if (!ok) return;
  try {
    await invoke("undo_turn", {
      project: card.project || state.project,
      baseline: card.baseline,
      after: card.after,
    });
  } catch (error) {
    setStatus(`Could not undo: ${error}`);
    return;
  }
  card.undone = true;
  card.note.textContent = "Undone";
  card.note.hidden = false;
  card.undo.hidden = true;
  setStatus("Put the turn's files back");
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

function openReview(card) {
  reviewState = { card, index: 0, sides: new Map(), expanded: new Set() };
  el("review-title").textContent = card.title.textContent;
  el("review-total").innerHTML = statsHtml(card.added, card.removed);
  closeOverlays("review-modal");
  el("review-modal").hidden = false;
  paintReview();
  el("review-close").focus();
}

function closeReview() {
  reviewState = null;
  el("review-modal").hidden = true;
}

/// Paints the card's own listing and the file the review is showing.
function paintReview() {
  if (!reviewState) return;
  const { card } = reviewState;
  const list = el("review-files");
  list.innerHTML = "";
  card.files.forEach((file, index) => {
    const row = changeRow(file);
    row.classList.toggle("active", index === reviewState.index);
    row.onclick = () => {
      reviewState.index = index;
      paintReview();
    };
    list.appendChild(row);
  });
  paintReviewFile(card.files[reviewState.index]);
}

/// Paints one file's two sides. They come from the project's shadow snapshot —
/// the state the run found is nowhere on disk — so a file reads as a whole diff,
/// both sides with their own line numbers, rather than as the few hunks a row's
/// preview keeps. The answer is kept per path, and one for a file the reader has
/// already walked off is dropped rather than painted under the wrong name.
function paintReviewFile(file) {
  if (!reviewState) return;
  const target = el("review-diff");
  const showing = () =>
    reviewState && reviewState.card.files[reviewState.index] === file;
  const sides = reviewState.sides.get(file.path);
  if (sides !== undefined) {
    target.replaceChildren(reviewFileHead(file), ...reviewBody(file, sides));
    scrollReview();
    return;
  }
  target.replaceChildren(reviewFileHead(file), reviewNote("Reading the change⋯"));
  invoke("change_sides", {
    project: reviewState.card.project,
    baseline: reviewState.card.baseline,
    path: file.path,
  })
    .then((read) => {
      if (!showing()) return;
      reviewState.sides.set(file.path, read);
      paintReviewFile(file);
    })
    .catch((error) => {
      if (!showing()) return;
      // A failure is kept as its own answer, so walking away and back reports
      // it rather than asking again in a loop.
      const sides = { failed: String(error) };
      reviewState.sides.set(file.path, sides);
      paintReviewFile(file);
    });
}

function reviewFileHead(file) {
  const head = document.createElement("div");
  head.className = "review-file";
  // The same words a card's row uses, so a binary or mode-only file reads the
  // same in both listings rather than as an empty column.
  const stats = file.binary
    ? "binary"
    : statsHtml(file.added, file.removed) || "no line changes";
  head.innerHTML =
    statusBadge(file.status) +
    `<span class="change-path">${escapeHtml(file.path)}</span>` +
    `<span class="change-stats">${stats}</span>`;
  return head;
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
  if (sides.binary) return [reviewNote("Binary file — no text diff.")];
  if (sides.omitted) return [reviewNote("File too large to diff line by line.")];
  const lines = sides.lines || [];
  if (!lines.length) return [reviewNote("No textual changes.")];
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
  bar.textContent = `${hidden} unmodified line${hidden === 1 ? "" : "s"}`;
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

/// Opens a file on its first change rather than on whatever its top happens to
/// be, since a long file would otherwise show the reader its unchanged head.
function scrollReview() {
  const row = el("review-diff").querySelector?.(".split-row.add, .split-row.remove");
  if (row && row.scrollIntoView) row.scrollIntoView({ block: "center" });
}

/// Walks the review's files, the way a changes view does.
function walkReview(step) {
  if (!reviewState) return;
  const count = reviewState.card.files.length;
  reviewState.index = (reviewState.index + step + count) % count;
  paintReview();
}

function reviewKey(event) {
  if (!reviewState) return;
  const step = event.key === "ArrowDown" ? 1 : event.key === "ArrowUp" ? -1 : 0;
  if (!step) return;
  event.preventDefault();
  walkReview(step);
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
  if (event.runId != null) state.runId = event.runId;
  // A delta, a tool card, a retry that drops the attempt it replaced: each writes
  // into the thread the run is in, which is the one on screen or the one parked
  // with the run — `runTranscript` picks it. The footer is the exception: the
  // totals it shows are the read thread's own, so the run's wait with its thread.
  if (event.type === FOOTER_EVENT && !viewingRun()) {
    const parked = state.parked;
    const totals = usageTotals(event);
    if (parked && parked.session === state.runSession) parked.usage = totals;
    else setUsage(totals);
    return;
  }
  switch (event.type) {
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
      setStatus(`Retrying (${event.attempt}/${event.maxAttempts})…`);
      break;
    case "usage": {
      setUsage(usageTotals(event));
      break;
    }
    case "compaction":
      setStatus(`Compacted ${event.summarized} earlier messages`);
      break;
    case "error":
      setStatus(`Error: ${event.message}`);
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
    label.textContent = "Type your own answer";
    free.placeholder = "Type your answer…";
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
    free.placeholder = options.length ? "Or type your own answer…" : "Type your answer…";
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
  el("question-step").textContent = `${pending.index + 1} of ${total} questions`;
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
      ? "Select all that apply"
      : "Select one answer"
    : "";
  hint.hidden = !hint.textContent;
  el("question-back").hidden = pending.index === 0;
  el("question-submit").textContent = pending.index + 1 < total ? "Next" : "Submit";
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
  "create-project-modal",
  "projects-modal",
  "mcps-modal",
  "models-modal",
  "reasoning-modal",
  "themes-modal",
  "permissions-modal",
  "trust-modal",
  "confirm-modal",
  "rename-modal",
  "image-modal",
  "sessions-modal",
  "review-modal",
  "update-modal",
  "help-modal",
];

function closeOverlays(except) {
  for (const id of OVERLAYS) {
    if (id !== except) el(id).hidden = true;
  }
  // The review's state is its card and the file it is showing, so closing the
  // panel drops it along with the markup rather than leaving it half-open.
  if (except !== "review-modal") reviewState = null;
  // The palette lives above the composer rather than in an overlay, but any
  // dialog that opens takes the keyboard with it.
  if (except !== "command-palette") closePalette();
}

// macOS's WKWebView does not implement `window.confirm`/`window.prompt`, so
// destructive and rename actions go through these in-app dialogs instead.
let confirmResolution = null;
// What the open confirm dialog offers beside Confirm, so its second button is
// wired once rather than per dialog.
let confirmAlt = null;
let renameResolution = null;

function confirmDialog(title, message, confirmLabel = "Confirm", alt = null) {
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

function promptDialog(value) {
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
      '<div class="empty" style="margin:8px 0">No provider matches that search.</div>';
    renderLoginFields();
    return;
  }
  for (const provider of rows) {
    const row = document.createElement("button");
    row.type = "button";
    row.className = "provider" + (provider.name === state.providerName ? " active" : "");
    const marks = [];
    if (provider.active) marks.push("in use");
    if (provider.stored) marks.push("stored");
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
    el("login-key").placeholder = "Paste your API key";
    return;
  }
  el("login-key").placeholder = provider.stored
    ? "Leave empty to reuse the stored key"
    : "Paste your API key";
  if (provider.local) {
    note.textContent = `${provider.label} is a server on this machine — no key needed.`;
  } else if (keyless) {
    note.textContent = `${provider.label} authorizes with a credential this machine already has — no key needed.`;
  }
}

async function saveConnect() {
  const provider = state.providers.find((one) => one.name === state.providerName);
  if (!provider) {
    setStatus("Choose a provider first.");
    return;
  }
  try {
    await invoke("login", {
      provider: provider.name,
      key: el("login-key").value || null,
      model: el("login-model").value || null,
      baseUrl: el("login-url").value || null,
    });
    el("connect-modal").hidden = true;
    el("login-key").value = "";
    await loadInfo();
    setStatus(`Connected ${provider.label}`);
  } catch (error) {
    setStatus(`Login failed: ${error}`);
  }
}

// ---------- models ----------

async function openModels() {
  // The catalog is read from this project's config, and the request would
  // otherwise resolve against the directory the app was launched in — the same
  // reason the message box asks for a folder before it sends anything.
  if (!state.project) {
    setStatus("Select a project first.");
    askForProject();
    return;
  }
  closeOverlays("models-modal");
  el("models-modal").hidden = false;
  el("model-list").innerHTML = '<div class="empty" style="margin:14px">Loading…</div>';
  el("model-filter").value = "";
  try {
    state.models = await invoke("list_models", { project: state.project });
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
    header.textContent = provider.provider + (provider.active ? " (active)" : "");
    box.appendChild(header);
    for (const model of models) {
      const row = document.createElement("button");
      row.className = "model" + (model === provider.current ? " active" : "");
      row.textContent = model;
      row.onclick = async () => {
        await invoke("set_model", { provider: provider.provider, model });
        el("models-modal").hidden = true;
        await loadInfo();
      };
      box.appendChild(row);
    }
  }
  if (!box.children.length) {
    box.innerHTML = '<div class="empty" style="margin:14px">No models found.</div>';
  }
}

// ---------- reasoning ----------

// Where the keyboard was before the picker took it, the way the full-size
// preview keeps its own.
let reasoningReturnFocus = null;

/// The picker the thinking chip opens: the level a turn thinks at is a choice
/// rather than a step, and the panel's own chip offers the same list. A level
/// is applied to the turns this window sends, the way the chip's cycle applied
/// it — `updateChips` is what says which one it is now.
function openReasoning() {
  if (!state.project) {
    setStatus("Select a project first.");
    return Promise.resolve();
  }
  reasoningReturnFocus = document.activeElement || null;
  closeOverlays("reasoning-modal");
  renderReasoning();
  el("reasoning-modal").hidden = false;
  focusReasoning();
  // A cold model cache has no advertised levels yet; ask the host, which warms
  // it from the provider's listing, and repaint the open picker with the model's
  // own levels. The built-in set stands until that answer arrives.
  if (state.reasoningLevels) return Promise.resolve();
  return invoke("reasoning_levels", { project: state.project })
    .then((info) => {
      if (info && Array.isArray(info.reasoningLevels) && info.reasoningLevels.length) {
        state.reasoningLevels = ["auto", ...info.reasoningLevels];
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

/// Tab walks the picker's own rows and its Close: the dialog is `aria-modal`, so
/// nothing behind it is reachable while it is up.
function stepReasoningFocus(step) {
  const controls = [...el("reasoning-list").children, el("reasoning-close")];
  const at = controls.indexOf(document.activeElement);
  controls[at === -1 ? 0 : (at + step + controls.length) % controls.length].focus();
}

/// Closing hands the keyboard back to whatever opened the picker. Every path
/// goes through here — a row, the Close button, or Escape — so a reader who
/// chose a level is not left at the top of the page behind the dialog.
function closeReasoning() {
  if (el("reasoning-modal").hidden) return;
  el("reasoning-modal").hidden = true;
  const target = reasoningReturnFocus;
  reasoningReturnFocus = null;
  target?.focus?.();
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
    name.textContent = level;
    row.appendChild(name);

    const hint = document.createElement("span");
    hint.className = "reasoning-option-detail";
    hint.textContent = REASONING_HINTS[level] || "";
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
  box.innerHTML = '<div class="theme-empty">Loading themes…</div>';
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
    setStatus(`Themes failed: ${error}`);
  }
}

const THEME_SWATCH_SLOTS = ["background", "panel", "accent", "text", "success", "tool"];

function renderThemes(entries, current) {
  const box = el("theme-list");
  box.innerHTML = "";
  if (!entries.length) {
    box.innerHTML = '<div class="theme-empty">No themes available.</div>';
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
    setStatus(`Theme failed: ${error}`);
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
      '<div class="empty" style="margin:6px">Select a project to review its tool approvals.</div>';
    return;
  }
  el("permissions-clear").disabled = false;
  const list = await invoke("list_approvals", { project: state.project });
  box.innerHTML = "";
  if (!list.length) {
    box.innerHTML = '<div class="empty" style="margin:6px">No saved approvals.</div>';
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
/// An install in flight. The CLI replaces itself on disk, and two of them
/// running over one binary is the one thing this dialog must not allow.
let installingUpdate = false;
/// The check the dialog is waiting on. The menu item and the sidebar button ask
/// the same question and either may be pressed again before the first answer
/// arrives, so every request takes a number: a reply for a number the dialog has
/// moved past is dropped rather than painted over the newer answer — which would
/// show a release, or a failure, from a question nobody is waiting on.
let updateProbe = 0;
/// The install this dialog has finished, if any. The window is still the build
/// that started, so a check compares against that older version and keeps
/// resolving the release now on disk; while the install is remembered, a check
/// that answers with that release reports what happened instead of offering the
/// same install again. Held in an object so a front-end check can put the dialog
/// back to the state a fresh window is in.
const installedUpdate = { answer: null };
/// The install a launch performs on its own, as the shell reported it: null
/// while there is none, `{ stage, version }` while one works, `{ answer }` once a
/// release is in place. The app keeps itself current the way its other front-ends
/// check for one, so the window is told rather than asking.
let launchUpdate = null;
/// Whether the reader has put this launch's row away. Kept apart from the
/// install's own state, since what the row says goes on arriving — and a row that
/// came back on the next stage would be one the reader cannot dismiss, which is
/// not what a dismissal means.
const launchDismissed = { yes: false };

/// Checks the app's own release train (`desktop-v*`) and offers to install it.
/// The resolution is `oxide_core::updates`, shared with the terminal, so the
/// release this window offers is a release of this app and not of the CLI — and
/// when this installation is one the app may replace (a bundle it can write to,
/// an AppImage, an installation this app's own installer made) the dialog
/// installs it in place.
async function openUpdate() {
  el("update-title").textContent = "Updates";
  el("update-note").textContent = "";
  el("update-body").innerHTML = checkingLine();
  el("update-notes").hidden = true;
  el("update-install").hidden = true;
  el("update-install").disabled = true;
  el("update-restart").hidden = true;
  closeOverlays("update-modal");
  el("update-modal").hidden = false;
  // A fresh check replaces the release the last one resolved, so a check that
  // fails does not leave a Release notes button pointing at the old release.
  updateCheck = null;
  const probe = ++updateProbe;
  const answer = await readUpdate();
  // The dialog may have been closed while the check was in flight, and a
  // repaint here would put it back; a check started since — the menu and the
  // button pressed one after the other — owns the dialog now.
  if (probe !== updateProbe || el("update-modal").hidden) return;
  if (answer.failed) paintUpdateFailure("Could not check for updates", answer.failed);
  else paintUpdate(answer.check, "");
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
  return '<div class="update-line">Checking the newest release…</div>';
}

/// Paints what the check found. `headline` replaces the title when the dialog is
/// reporting an install that just landed rather than a release that is out.
function paintUpdate(check, headline) {
  updateCheck = check || null;
  const box = el("update-body");
  if (!check) {
    el("update-title").textContent = "Updates";
    el("update-note").textContent = "";
    box.innerHTML = "";
    el("update-notes").hidden = true;
    el("update-install").hidden = true;
    el("update-restart").hidden = true;
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
  el("update-title").textContent =
    headline ||
    (check.updateAvailable
      ? `Oxide ${check.latest} is available`
      : `Oxide ${check.current} is up to date`);
  el("update-note").textContent = check.updateAvailable
    ? "Installing downloads the release for this machine and puts it in this app's place. The running app keeps running until it is quit and opened again."
    : "";
  const lines = [
    `Current ${mono(check.current)}`,
    `Latest ${mono(check.tag)}`,
  ];
  if (check.installation || check.path) {
    lines.push(
      `<span class="update-path">${escapeHtml(check.installation || "unknown")} \u00b7 ${escapeHtml(check.path || "")}</span>`,
    );
  }
  if (check.asset && check.asset.name) {
    lines.push(`Download ${escapeHtml(check.asset.name)}`);
  }
  if (check.advice) lines.push(`<span class="update-advice">${escapeHtml(check.advice)}</span>`);
  box.innerHTML = lines.map((line) => `<div class="update-line">${line}</div>`).join("");
  const notes = el("update-notes");
  notes.hidden = !check.releaseUrl;
  const install = el("update-install");
  // Only the app itself decides whether it can replace this installation: a
  // copy in a system directory, a distribution's package or a checkout's build
  // cannot be written over, and the advice line above is the way to install
  // instead.
  install.hidden = !(check.updateAvailable && check.installable);
  install.disabled = false;
  install.textContent = check.updateAvailable ? `Install ${check.latest}` : "Install";
  // A release that is only offered has not been installed, so there is nothing
  // to restart into yet.
  el("update-restart").hidden = true;
}

/// A failure to check or to install: the CLI's own words, which say which of the
/// two it was — a release lookup that could not reach GitHub, or a download that
/// did not verify.
function paintUpdateFailure(title, text) {
  el("update-title").textContent = title;
  el("update-note").textContent =
    title === "Could not check for updates"
      ? "The check asks GitHub for the newest release, so a machine with no network — or a request the network refuses — says so here."
      : "";
  el("update-body").innerHTML = `<div class="update-error">${escapeHtml(text)}</div>`;
  el("update-notes").hidden = updateCheck === null || !updateCheck.releaseUrl;
  el("update-install").hidden = true;
  el("update-restart").hidden = true;
}

/// Installs the release the dialog offered: the app downloads the artifact its
/// own release train publishes for this platform, checks it against the digest
/// that release carries, and puts it in this installation's place.
async function installUpdate() {
  if (installingUpdate) return;
  // The install owns the dialog until it reports, and it is what says whether
  // one is running at all: the click that asks for a second install while this
  // one works answers with this one's result. A check asked for meanwhile — the
  // sidebar button is still there — is a look at the installation as it was
  // when the check started, which the install's own report outranks.
  const probe = ++updateProbe;
  installingUpdate = true;
  const install = el("update-install");
  install.disabled = true;
  install.textContent = "Installing…";
  el("update-body").innerHTML =
    '<div class="update-line">Downloading the release for this machine and putting it in place…</div>';
  let answer;
  try {
    answer = await invoke("install_update");
  } catch (error) {
    answer = { ok: false, text: String(error) };
  }
  installingUpdate = false;
  if (!answer || answer.ok !== true) {
    if (probe !== updateProbe || el("update-modal").hidden) return;
    paintUpdateFailure(
      "Could not install the update",
      (answer && answer.text) || "The install did not finish.",
    );
    return;
  }
  // The install owns the dialog from here, whatever a check started while it
  // worked has to say: that check asked about the build running when it
  // started, and the release it would offer is the one now in place.
  installedUpdate.answer = answer;
  if (el("update-modal").hidden) return;
  paintInstalled(answer);
}

/// Reports an install that landed. The version named is the one the install
/// itself resolved and put on disk rather than the one that was offered a
/// download ago, since a release published in between is the one that went on.
/// The running app is still the build that started, so the note says what to do
/// about that instead of claiming this window is already the new version.
function paintInstalled(answer) {
  // A Windows installer is a program this app starts and cannot wait on: it
  // asks for elevation and for Oxide to be closed, and the user may cancel it.
  // What is known here is that it is running, so that is what the dialog says
  // rather than a version it cannot claim is in place.
  const pending = answer.pending === true;
  el("update-title").textContent = pending
    ? `The Oxide ${answer.version} installer is running`
    : `Oxide ${answer.version} is installed`;
  el("update-note").textContent = pending
    ? "Finish the installer, then open Oxide again to run the new version."
    : "Restart Oxide to run the new version. The release is in place; the app running here is still the one that started.";
  const lines = [];
  if (answer.tag) lines.push(`Release ${mono(answer.tag)}`);
  if (answer.asset) lines.push(`Downloaded ${escapeHtml(answer.asset)}`);
  if (answer.path) lines.push(`<span class="update-path">${escapeHtml(answer.path)}</span>`);
  if (answer.text) lines.push(`<span class="update-advice">${escapeHtml(answer.text)}</span>`);
  el("update-body").innerHTML = lines.map((line) => `<div class="update-line">${line}</div>`).join("");
  el("update-notes").hidden = !updateCheck || !updateCheck.releaseUrl;
  el("update-install").hidden = true;
  // Only a release this app put in place is one to restart into: a Windows
  // installer owns what happens next, and the app it asks to be closed is this
  // one.
  el("update-restart").hidden = pending;
}

// ---------- the launch's own update ----------

/// The launch looked for a release of this app and is installing one without
/// being asked, so the window hears about it rather than asking: each step
/// arrives as a stage, and what is left when it lands is the restart that runs
/// it.
function handleUpdateProgress(payload) {
  launchUpdate = {
    stage: (payload && payload.stage) || "",
    version: (payload && payload.version) || "",
  };
  paintLaunchUpdate();
}

function handleUpdateReady(answer) {
  // The dialog reports an install this window performed from the same place a
  // click's own install is remembered: a check that resolves the release now on
  // disk reports the install rather than offering it a second time.
  installedUpdate.answer = answer;
  launchUpdate = { answer };
  paintLaunchUpdate();
  // A dialog the reader opened while this install ran was asked about the build
  // that started — the release it resolved is the one now on disk — so its own
  // Install row would put the same release there twice. The install is the newest
  // word on the dialog too, and it is repaired rather than left offering it.
  if (!el("update-modal").hidden) paintInstalled(answer);
}

/// A launch's own install that could not finish. Nobody asked for it, so it is
/// a line rather than a dialog — and asking again is what the window's own Check
/// for Updates… is for.
function handleUpdateFailed(payload) {
  launchUpdate = null;
  paintLaunchUpdate();
  setStatus(`Could not install the update: ${(payload && payload.message) || "unknown error"}`);
}

/// Paints where the launch's own install got to, or what it left to do. A
/// release installed in the background is not the app that is running — the
/// process is still the build that started — so the row ends in the restart that
/// runs it.
function paintLaunchUpdate() {
  const banner = el("update-banner");
  if (!launchUpdate || launchDismissed.yes) {
    banner.hidden = true;
    return;
  }
  const answer = launchUpdate.answer;
  const stage =
    {
      checking: "Looking for",
      downloading: "Downloading",
      verifying: "Verifying",
      installing: "Installing",
    }[launchUpdate.stage] || "Installing";
  el("update-banner-text").textContent = answer
    ? `Oxide ${answer.version} is installed.`
    : launchUpdate.version
      ? `${stage} Oxide ${launchUpdate.version}…`
      : "Looking for a new release…";
  el("update-banner-restart").hidden = !answer;
  banner.hidden = false;
}

/// The row is the window's own, so it can be put away without stopping what it
/// names: the install goes on either way, and a release already in place is
/// offered again by Check for Updates…, which reports the install rather than
/// offering to repeat it. The dismissal lasts the rest of the launch, since a row
/// that returned with the next stage is one the reader cannot put away.
function dismissLaunchUpdate() {
  launchDismissed.yes = true;
  launchUpdate = null;
  paintLaunchUpdate();
}

/// Asks what the launch's own install has already said.
///
/// The install starts before this page does, so its first steps were emitted into
/// a window with nothing listening; the newest of them is kept beside the app's
/// state for exactly this window, and the report the restart hangs on is not one
/// to miss. What it answers is painted through the same handlers the events go to,
/// since it is the event it would have heard.
function catchUpOnLaunchUpdate() {
  return invoke("launch_update")
    .then((heard) => {
      if (!heard || !heard.event) return;
      const payload = heard.payload || {};
      if (heard.event === "update-progress") handleUpdateProgress(payload);
      else if (heard.event === "update-ready") handleUpdateReady(payload);
      else if (heard.event === "update-failed") handleUpdateFailed(payload);
    })
    .catch(() => {});
}

/// Restarts Oxide, which is what runs a release an install has put in place: the
/// process running is still the build that started, so only a new one is the new
/// version. A turn is work this process owns, so a restart mid-turn is refused
/// the way starting a new thread is.
function restartApp() {
  if (busyRefusal("restarting Oxide")) return;
  invoke("restart_app").catch((error) => setStatus(`Could not restart Oxide: ${error}`));
}

/// Opens the release the dialog is showing in the platform browser, the way a
/// link in a reply is opened.
function openReleaseNotes() {
  if (!updateCheck || !updateCheck.releaseUrl) return;
  invoke("open_url", { url: updateCheck.releaseUrl }).catch((error) =>
    setStatus(`Could not open link: ${error}`),
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
  el("mcp-list").innerHTML = '<div class="mcp-empty">Checking servers…</div>';
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

function renderMcps() {
  const box = el("mcp-list");
  box.innerHTML = "";
  if (state.mcpError) {
    box.innerHTML = `<div class="mcp-empty">Could not list MCP servers: ${escapeHtml(state.mcpError)}</div>`;
    return;
  }
  const servers = state.mcps || [];
  if (!servers.length) {
    box.innerHTML =
      '<div class="mcp-empty">No MCP servers configured. Add one with <code>oxide mcp add</code>, or a <code>.mcp.json</code> in the project.</div>';
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
    status.textContent = server.status;
    const toggle = document.createElement("button");
    toggle.type = "button";
    // Icon-only: the listing is a row per server, so the switch is sized to the
    // row and its meaning is in the tooltip and the accessible name.
    toggle.className = `icon mcp-toggle${server.enabled ? " on" : ""}`;
    toggle.innerHTML = ICONS.power;
    toggle.title = server.enabled
      ? `Disable ${server.name}: turn it off in the file that defines it`
      : `Enable ${server.name}: turn it back on`;
    toggle.setAttribute("aria-label", toggle.title);
    toggle.onclick = () => toggleMcp(server, toggle);
    head.append(name, status, toggle);

    const detail = document.createElement("div");
    detail.className = "mcp-detail";
    detail.textContent = [server.transport, server.detail].filter(Boolean).join(" · ");
    const source = document.createElement("div");
    source.className = "mcp-source";
    source.textContent = `source: ${server.source}`;

    row.append(head, detail, source);
    box.appendChild(row);
  }
}

async function toggleMcp(server, button) {
  if (!state.project) {
    setStatus("Select a project first.");
    return;
  }
  button.disabled = true;
  setStatus(`${server.enabled ? "Turning off" : "Turning on"} ${server.name}…`);
  try {
    state.mcps = await invoke("set_mcp_server", {
      project: state.project || "",
      name: server.name,
      enabled: !server.enabled,
    });
    state.mcpError = "";
    renderMcps();
    setStatus("Ready");
  } catch (error) {
    button.disabled = false;
    setStatus(`Could not change ${server.name}: ${error}`);
  }
}

// ---------- sessions ----------

/// The threads stored for this project, newest first: what `/session` opens,
/// and the same list the sidebar draws beside the project. The terminal's
/// `/session` picker and the VS Code panel's dialog show the same sessions, so a
/// thread started in one front-end is reachable from the others.
async function openSessions() {
  if (!state.project) {
    setStatus("Select a project first.");
    return;
  }
  closeOverlays("sessions-modal");
  el("sessions-modal").hidden = false;
  el("sessions-list").innerHTML = '<div class="dialog-empty">Loading threads…</div>';
  await loadSessions();
  renderSessions();
}

function renderSessions() {
  const box = el("sessions-list");
  box.innerHTML = "";
  // The store could not be read, so the empty listing that would be painted
  // from no threads is not a fact about this project.
  if (state.sessionsError) {
    box.innerHTML = `<div class="dialog-empty">Could not read this project's threads: ${escapeHtml(state.sessionsError)}</div>`;
    return;
  }
  const threads = listedSessions().filter((session) => session.cwd === state.project);
  if (!threads.length) {
    box.innerHTML =
      '<div class="dialog-empty">No threads for this project yet. Send a message to start one.</div>';
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
    const meta = [sessionAge(session.modified_at), sessionMessages(session.message_count)]
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
      ? `${meta ? `${name} — ${meta} · ` : `${name} — `}a turn is running`
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
  if (elapsed < 60) return "just now";
  if (elapsed < 3600) return `${Math.floor(elapsed / 60)}m ago`;
  if (elapsed < 86400) return `${Math.floor(elapsed / 3600)}h ago`;
  return `${Math.floor(elapsed / 86400)}d ago`;
}

function sessionMessages(count) {
  if (count === undefined || count === null) return "";
  return `${count} message${Number(count) === 1 ? "" : "s"}`;
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
        setStatus("Select a project first.");
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
    case "permissions":
      await openPermissions();
      return true;
    case "trust":
      if (state.trust) showTrust(state.trust);
      else setStatus("Nothing to decide — this project needs no trust decision.");
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
            ? `Name a provider: /logout ${stored.map((entry) => entry.name).join("|")}`
            : "No provider is connected.",
        );
        return true;
      }
      const ok = await confirmDialog(
        "Sign out",
        `Forget the stored credential for ${target.label}?`,
        "Sign out",
      );
      if (!ok) return true;
      await invoke("logout", { provider: target.name });
      await loadInfo();
      setStatus(`Signed out of ${target.label}`);
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
        setStatus(`Reasoning must be one of ${(state.reasoningLevels || REASONING).join(", ")}.`);
      }
      return true;
    case "attach":
      el("attach-input").click();
      return true;
    case "usage":
      setStatus(el("usage").textContent ? `This chat: ${el("usage").textContent}` : "No usage reported yet.");
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
        setStatus(`/${builtin.name} is the ${elsewhere.join(" and ")}'s command.`);
        return true;
      }
      setStatus(`/${builtin.name} is not available in the desktop app yet.`);
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
        ? `${row.label} — a folder in this project`
        : `${row.label} — referenced in this message`;
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
  desktop: "desktop app",
  panel: "VS Code panel",
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
    list.innerHTML = '<div class="palette-empty">No matching command.</div>';
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

/// The level a turn is given, whether it was picked from the dialog or walked to
/// with the keyboard. `--reasoning` is sent per turn rather than stored, so this
/// only has to agree with the chip — and a level set from anywhere but the
/// picker's own row leaves the listing, whose mark would be out of date.
function setReasoning(level) {
  state.reasoning = level;
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
  setReasoning(REASONING[(REASONING.indexOf(state.reasoning) + 1) % REASONING.length]);
}

async function initEvents() {
  await listen("agent-start", async (event) => {
    const payload = event.payload || {};
    if (payload.runId != null) state.runId = payload.runId;
    // The run belongs to the thread it started in, whichever thread the reader
    // has opened since: `state.session` follows the transcript, so the run's own
    // id is kept apart from it and the sidebar and the header can name it.
    state.runSession = payload.sessionId || null;
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
    if (sent == null || (state.session === sent.session && state.project === sent.project)) {
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
    setStatus("Ready");
    renderChanges(event.payload || {});
    resetTurn();
    await loadSessions();
    await startNextPendingSend();
  });
  el("review-modal").onkeydown = reviewKey;
  await listen("approval-request", (event) => showApproval(event.payload || {}));
  // The macOS menu item has no page of its own to paint into, so it asks the
  // window for the dialog the sidebar's own button opens.
  await listen("check-updates", () => openUpdate());
  // The launch installs a release on its own, so these are the window's side of
  // an install nobody in the window asked for — and the install starts before
  // this page does, so what it has already said is asked for rather than waited
  // on, once the listeners above are in place to hear the rest.
  await listen("update-progress", (event) => handleUpdateProgress(event.payload || {}));
  await listen("update-ready", (event) => handleUpdateReady(event.payload || {}));
  await listen("update-failed", (event) => handleUpdateFailed(event.payload || {}));
  await catchUpOnLaunchUpdate();
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
};

// The last path segment, used to default the project name to the folder's name.
function folderBasename(path) {
  const trimmed = String(path || "").replace(/[\\/]+$/, "");
  const parts = trimmed.split(/[\\/]+/);
  return parts[parts.length - 1] || trimmed;
}

function openCreateProject() {
  createProjectState.folders = [];
  el("create-project-name").value = "";
  el("create-project-path").value = "";
  el("create-project-folders").innerHTML = "";
  el("create-project-error").hidden = true;
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
    showCreateProjectError(`Could not open folder chooser: ${error}`);
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
    showCreateProjectError("That folder is already in the list");
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
    removeBtn.textContent = "Remove";
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
    showCreateProjectError("At least one source folder is required");
    return;
  }
  const nameInput = el("create-project-name");
  let name = nameInput.value.trim();
  if (!name) {
    name = folderBasename(createProjectState.folders[0]);
    nameInput.value = name;
  }
  if (!name) {
    showCreateProjectError("Project name is required");
    return;
  }
  try {
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
    showCreateProjectError(`Could not create project: ${error}`);
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
    handle.setAttribute("aria-valuenow", String(Math.round(current)));
    handle.setAttribute("aria-valuemax", String(maxWidth()));
  });

  window.addEventListener("resize", () => {
    apply(sidebar.getBoundingClientRect().width, false);
  });
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
  initSidebarResize();
  initOverlayScrollbars();
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

  el("reasoning").onclick = openReasoning;
  el("reasoning-close").onclick = closeReasoning;
  el("reasoning-modal").addEventListener("keydown", (event) => {
    if (event.key !== "Tab") return;
    event.preventDefault();
    stepReasoningFocus(event.shiftKey ? -1 : 1);
  });
  el("model").onclick = openModels;
  el("theme").onclick = openThemes;
  el("permissions").onclick = openPermissions;
  el("help").onclick = toggleHelp;
  el("connect").onclick = openConnect;

  el("send").onclick = () => send(false);
  el("busy-message-mode").onclick = toggleBusyMessageMode;
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
  el("models-close").onclick = () => (el("models-modal").hidden = true);
  el("model-filter").addEventListener("input", renderModels);
  el("themes-close").onclick = () => (el("themes-modal").hidden = true);
  el("permissions-close").onclick = () => (el("permissions-modal").hidden = true);
  el("permissions-clear").onclick = clearApprovals;
  el("trust").onclick = () => state.trust && showTrust(state.trust);
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
  el("update").onclick = openUpdate;
  el("update-close").onclick = () => (el("update-modal").hidden = true);
  el("update-notes").onclick = openReleaseNotes;
  el("update-install").onclick = installUpdate;
  el("update-restart").onclick = restartApp;
  el("update-banner-restart").onclick = restartApp;
  el("update-banner-dismiss").onclick = dismissLaunchUpdate;
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
  el("review-close").onclick = closeReview;
  el("review-prev").onclick = () => walkReview(-1);
  el("review-next").onclick = () => walkReview(1);

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
  window.addEventListener("blur", () => {
    editorControlPress = null;
    justBlurredEditor = null;
  });

  // The renderer cannot navigate to a remote page, so a link click opens the
  // platform browser through the app instead of reloading the app window.
  document.addEventListener("click", (event) => {
    const link = event.target?.closest?.("a[href]") || null;
    if (!link) return;
    event.preventDefault();
    invoke("open_url", { url: link.getAttribute("href") }).catch((error) =>
      setStatus(`Could not open link: ${error}`),
    );
  });

  el("prompt").addEventListener("input", () => {
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
  el("image-modal").addEventListener("keydown", (event) => {
    if (event.key !== "Tab") return;
    event.preventDefault();
    el("image-view-close").focus();
  });
  el("prompt").addEventListener("keydown", (event) => {
    if (atKey(event)) {
      event.preventDefault();
      return;
    }
    if (paletteKey(event)) {
      event.preventDefault();
      return;
    }
    if (event.key === "Escape") {
      closePalette();
      return;
    }
    if (event.key === "Enter" && !event.shiftKey) {
      event.preventDefault();
      send(event.altKey);
    }
  });

  document.addEventListener("keydown", (event) => {
    if (event.key === "Escape") {
      closeImage();
      closeReasoning();
      if (!el("confirm-modal").hidden) resolveConfirm(false);
      if (!el("rename-modal").hidden) resolveRename(null);
      // A question dismissed with Escape is answered as unanswered rather than
      // hidden, so the turn continues instead of waiting out the timeout.
      if (!el("question").hidden) answerQuestion(true);
      closeOverlays();
      return;
    }
    if (event.key === "Tab" && event.shiftKey) {
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
  updateSendState();
  renderWelcome();
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
    const list = byProject.get(session.cwd);
    if (list) list.push(session);
  }
  const ordered = [];
  for (const project of state.projects) ordered.push(...(byProject.get(project.path) || []));
  return ordered;
}

async function renderProjectsTree() {
  const container = el("projects-tree");
  if (!container) return;
  
  container.innerHTML = "";
  
  if (!state.projects || !state.projects.length) {
    container.innerHTML = '<div class="empty" style="margin:12px; font-size:12px; color:var(--faint);">No projects yet. Add a folder with the + above.</div>';
    return;
  }
  
  // Group sessions by project
  const sessionsByProject = {};
  for (const project of state.projects) {
    sessionsByProject[project.path] = [];
  }
  
  for (const session of listedSessions()) {
    if (sessionsByProject[session.cwd]) {
      sessionsByProject[session.cwd].push(session);
    }
  }
  
  for (const project of state.projects) {
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

    const newTaskBtn = document.createElement("button");
    newTaskBtn.type = "button";
    newTaskBtn.className = "row-add";
    newTaskBtn.title = `New task in ${project.name}`;
    newTaskBtn.innerHTML = ICONS.plus;
    newTaskBtn.onclick = (event) => {
      event.stopPropagation();
      newTaskIn(project);
    };
    projectItem.appendChild(newTaskBtn);

    const removeProjectBtn = document.createElement("button");
    removeProjectBtn.type = "button";
    removeProjectBtn.className = "row-remove";
    removeProjectBtn.title = project.registered
      ? "Remove or delete project…"
      : "Delete project…";
    removeProjectBtn.innerHTML = ICONS.close;
    removeProjectBtn.onclick = (event) => {
      event.stopPropagation();
      removeProject(project);
    };
    projectItem.appendChild(removeProjectBtn);
    
    projectItem.onclick = () => {
      selectProject(project);
    };
    
    projectGroup.appendChild(projectItem);
    
    // Threads for this project, grouped under its row.
    const sessionsContainer = document.createElement("div");
    sessionsContainer.className = "project-sessions";

    for (const session of sessionsForProject) {
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
      const sessionName = document.createElement("div");
      sessionName.className = "name";
      sessionName.textContent = label;
      sessionItem.title = running ? `${label} — a turn is running` : label;
      sessionItem.appendChild(sessionName);

      if (running) {
        const mark = document.createElement("span");
        mark.className = "session-run";
        mark.title = "A turn is running in this thread";
        mark.setAttribute("aria-hidden", "true");
        const spinner = document.createElement("span");
        spinner.className = "spinner";
        mark.appendChild(spinner);
        sessionItem.appendChild(mark);
      }

      // A thread the store has not written has no file to remove, so its row
      // carries no ✕: leaving it is what starting a new thread does.
      if (!session.unstored) {
        const removeSessionBtn = document.createElement("button");
        removeSessionBtn.type = "button";
        removeSessionBtn.className = "row-remove";
        removeSessionBtn.title = "Delete thread";
        removeSessionBtn.innerHTML = ICONS.close;
        removeSessionBtn.onclick = (event) => {
          event.stopPropagation();
          removeSession(session);
        };
        sessionItem.appendChild(removeSessionBtn);
      }

      sessionItem.onclick = () => {
        selectSessionFromTree(session);
      };

      sessionsContainer.appendChild(sessionItem);
    }

    projectGroup.appendChild(sessionsContainer);
    
    container.appendChild(projectGroup);
  }
}

async function selectSessionFromTree(session) {
  if (!session) return;

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
  const project = state.projects.find((p) => p.path === session.cwd);
  if (project && project.path !== state.project) await selectProject(project);

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
  el("project-meta").textContent = "";
  el("trust-modal").hidden = true;
  updateTrustButton();
  updateChips();
  setStatus("Ready");
  resetTranscript();
}

// True when nothing is selected or the selected project is still in the tree.
function selectedProjectStillListed() {
  return !state.project || state.projects.some((project) => project.path === state.project);
}

// After a deletion, reload the tree and drop the selection if its project no
// longer exists, so the composer cannot start a turn against a removed row.
async function refreshAfterRemoval() {
  await loadProjects();
  if (!selectedProjectStillListed()) {
    clearSelectedProject();
    loadTheme();
  }
}

async function removeProject(project) {
  const threads = (state.sessions || []).filter((session) => session.cwd === project.path);
  const count = threads.length;

  if (project.registered) {
    // "Remove" and "delete its threads" are different actions, so the dialog
    // makes the choice explicit instead of guessing from the row.
    const choice = await confirmDialog(
      "Remove project",
      count
        ? `Remove “${project.name}” from the sidebar, or also delete its ${count} thread${count === 1 ? "" : "s"}?\n\nRemoving keeps the threads on disk, so adding the folder again brings them back.`
        : `Remove “${project.name}” from the sidebar?`,
      "Remove project",
      count
        ? { label: `Delete ${count} thread${count === 1 ? "" : "s"}`, value: "delete-threads" }
        : null,
    );
    if (!choice) return;
    if (choice === "delete-threads") {
      for (const session of threads) {
        await invoke("delete_session", { project: session.cwd, id: session.id });
      }
    }
    state.projects = await invoke("remove_project", { id: project.id });
    if (!selectedProjectStillListed()) {
      clearSelectedProject();
      loadTheme();
    }
    renderProjectsTree();
    return;
  }

  if (!count) return;
  const ok = await confirmDialog(
    "Delete project",
    `“${project.name}” only exists because of its ${count} thread${count === 1 ? "" : "s"}. Deleting removes the project and ${count === 1 ? "that thread" : "those threads"} permanently.`,
    "Delete project",
  );
  if (!ok) return;
  for (const session of threads) {
    await invoke("delete_session", { project: session.cwd, id: session.id });
  }
  await refreshAfterRemoval();
}

async function removeSession(session) {
  // Nothing on disk to remove for a thread the store has not written yet.
  if (!session || session.unstored) return;
  // A running turn appends to this thread's file as it works, so deleting it
  // here would leave the process writing into a file that is gone — and the
  // thread it is running in may be one the reader is not looking at, which is
  // the run's own to keep until it stops.
  if (state.busy && (state.session === session.id || state.runSession === session.id)) {
    setStatus("A turn is running; stop it before deleting this thread.");
    return;
  }
  const label = session.name || session.preview || session.id.slice(0, 8);
  const ok = await confirmDialog(
    "Delete thread",
    `Delete “${label}” permanently?\n\nThe project and its other threads are not affected.`,
    "Delete thread",
  );
  if (!ok) return;
  await invoke("delete_session", { project: session.cwd, id: session.id });
  // A thread the run left parked is gone from the store, so its transcript goes
  // too — the strip that was the way back to it has nothing to open.
  if (state.parked?.session === session.id) state.parked = null;
  if (state.session === session.id) resetTranscript();
  updateRunBanner();
  await refreshAfterRemoval();
}
