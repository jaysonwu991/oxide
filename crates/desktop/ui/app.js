// Oxide desktop front-end. Talks to the Rust core over Tauri commands; the
// project/session/config data is the same store the CLI uses.

const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

const el = (id) => document.getElementById(id);

const REASONING = ["auto", "off", "low", "medium", "high"];

const SUGGESTIONS = [
  "Explain this codebase and its architecture.",
  "Find and fix the highest-priority bug in this repository.",
  "Add tests for the most important untested code path.",
  "Review the working tree changes and summarize the risks.",
];

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
  // What the running turn titled itself: the header names the thread by this
  // until the sidebar's own listing carries it.
  runTitle: "",
  reasoning: "auto",
  contextWindow: 0,
  providers: [],
  providerIndex: 0,
  models: null,
  pendingApproval: null,
  pendingQuestion: null,
  trust: null,
  currentAssistant: null,
  tools: [],
  currentThinking: null,
  attachments: [],
  mcps: null,
  // Why the last read of the servers or of the threads failed, kept until the
  // next read answers so the listing's own render paints it — a failure painted
  // straight from the await lands under a press like any other repaint.
  mcpError: "",
  sessionsError: "",
  palette: [],
  paletteIndex: 0,
  paletteOpen: false,
  // One card per finished turn that changed something, in the order the turns
  // ran, so a review opened from an older card still reads its own files.
  changes: [],
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

function setUsage({
  input = 0,
  output = 0,
  cacheRead = 0,
  cacheWrite = 0,
  cost = 0,
  contextPct = null,
}) {
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
/// thread this window is in standing in for itself until the store has it.
/// `all_sessions` lists what is already on disk, and a thread that was just
/// started has not written its first entry yet, so a new thread used to be
/// missing from every one of those lists until its first turn ended — the one
/// thread the reader was looking at. The entry drops out again as soon as the
/// store lists that id, since a row is keyed by it.
function listedSessions() {
  const listed = state.sessions || [];
  if (!state.session || !state.project) return listed;
  if (listed.some((session) => session.id === state.session)) return listed;
  // The timestamps are the store's own unit — Unix seconds, which `sessionAge`
  // subtracts from `Date.now() / 1000` — rather than milliseconds, which would
  // read as a thread written in the future and be reported as `just now` for as
  // long as the store has not written it.
  const seconds = Math.floor(Date.now() / 1000);
  return [
    {
      id: state.session,
      name: state.runTitle || null,
      cwd: state.project,
      created_at: seconds,
      modified_at: seconds,
      message_count: 0,
      preview: "",
      path: "",
      // A thread the store has not written has no file, so the commands that read
      // one off disk have nothing to answer for it: its rows are handled in the
      // window instead of through `session_messages` or `delete_session`.
      unstored: true,
    },
    ...listed,
  ];
}

function sessionById(id) {
  if (!id) return null;
  return listedSessions().find((session) => session.id === id) || null;
}

/// The header names the thread on screen — the one that was resumed, or the one
/// this window started — by what the sidebar's own listing calls it, so the two
/// never disagree; until that listing has it, the run's own title does.
function refreshThreadTitle() {
  const session = sessionById(state.session);
  setThreadTitle(session ? sessionLabel(session) : state.runTitle);
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

function renderWelcome() {
  const project = state.project;
  const suggestions = project
    ? `<div class="suggestions">${SUGGESTIONS.map(
        (text) => `<button data-prompt="${escapeHtml(text)}">${escapeHtml(text)}</button>`,
      ).join("")}</div>`
    : "";
  el("transcript").innerHTML = `
    <div class="welcome">
      <div class="welcome-mark">◆</div>
      <h1>${project ? "What should we build?" : "Select a project"}</h1>
      <p>${project ? escapeHtml(state.project) : "Add or pick a project on the left to get started."}</p>
      ${suggestions}
    </div>`;
  el("transcript").querySelectorAll(".suggestions button").forEach((button) => {
    button.onclick = () => {
      el("prompt").value = button.dataset.prompt;
      el("prompt").focus();
      updateSendState();
    };
  });
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
  } catch (error) {
    setStatus(`Failed to load projects: ${error}`);
  }
}

/// Opens the app on the first project the sidebar shows — a registered folder,
/// most recently opened first, then one discovered from a session — rather than
/// on nothing. The composer belongs to a project: with none selected the box
/// stays disabled, and the path behind it would resolve against whatever
/// directory the app was launched in ($HOME on one platform, `/` on another),
/// which is not a folder the user picked. With no project at all the empty
/// state stays, since there is nothing to run in.
async function openDefaultProject() {
  await loadProjects();
  if (state.project) return;
  const [first] = state.projects || [];
  if (first) await selectProject(first);
}

async function selectProject(project) {
  state.project = project.path;
  state.projectName = project.name;
  state.session = null;
  state.trust = null;
  clearAttachments();
  el("prompt").disabled = false;
  el("trust-modal").hidden = true;
  updateTrustButton();
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
  el("model").textContent = info.model;
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
  button.title = state.trust && state.trust.trusted
    ? "Project trusted · click to review"
    : "Project resources are not trusted";
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
  el("reasoning").textContent = state.reasoning;
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
    return "";
  } catch (error) {
    setStatus(`Failed to load threads: ${error}`);
    state.sessionsError = String(error);
    return String(error);
  }
}

async function openSession(session) {
  state.session = session.id;
  state.runTitle = "";
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
  state.runTitle = "";
  state.changes = [];
  closeReview();
  el("transcript").innerHTML = "";
  el("usage").textContent = "";
  setThreadTitle("");
  renderWelcome();
}

function newChat() {
  resetTranscript();
  clearAttachments();
  loadSessions();
  el("prompt").focus();
}

/// Starts a task in `project`, selecting it first when it is not the active one.
/// The composer belongs to a project, so a task always lands in a real one.
async function newTaskIn(project) {
  if (project && project.path !== state.project) {
    await selectProject(project);
  }
  newChat();
}

// ---------- chat ----------

function bubble(kind, text, attachments = []) {
  const wrap = document.createElement("div");
  wrap.className = `msg ${kind}`;
  const label = document.createElement("div");
  label.className = "label";
  label.textContent = kind === "user" ? "you" : "◆ Oxide";
  const body = document.createElement("div");
  body.className = "body";
  body.innerHTML = kind === "assistant" ? renderMarkdown(text) : escapeHtml(text);
  const images = attachments.filter((attachment) => isImageAttachment(attachment.dataUrl));
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

function scrollDown() {
  const transcript = el("transcript");
  transcript.scrollTop = transcript.scrollHeight;
}

function resetTurn() {
  for (const tool of state.tools) {
    if (tool.timer) clearInterval(tool.timer);
  }
  state.currentAssistant = null;
  state.tools = [];
  state.currentThinking = null;
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
  const hasBusyMessage = state.busy && hasText;
  const mode = el("busy-message-mode");
  el("send").classList.toggle("enabled", hasText);
  el("send").disabled = !hasText;
  el("send").hidden = state.busy && !hasText;
  el("stop").hidden = !state.busy || hasText;
  mode.hidden = !hasBusyMessage;
  mode.textContent = state.busyMessageMode === "steer" ? "Steer" : "Queue";
  mode.title = state.busyMessageMode === "steer"
    ? "Steer the active response; click to queue instead"
    : "Queue for after the current response; click to steer instead";
  mode.setAttribute("aria-label", mode.title);
  el("send").title = hasBusyMessage
    ? state.busyMessageMode === "steer"
      ? "Steer the active response (Enter)"
      : "Queue for after the current response (Enter)"
    : "Send (Enter)";
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
  updateSendState();
}

// ---------- attachments ----------

/// `data:<mime>;base64,...` — the only shape the clipboard and FileReader give
/// us, and the shape the backend turns back into a media part.
function dataUrlMime(dataUrl) {
  const match = /^data:([^;,]+)[;,]/.exec(dataUrl || "");
  return match ? match[1].toLowerCase() : "";
}

/// The core's own limit (`media::MAX_ATTACHMENT_BYTES`) and the types a
/// provider takes and this webview can paint, so an over-large file is refused
/// before it is read into a data URL and a format nothing can draw never
/// becomes a thumbnail the browser cannot render.
const MAX_ATTACHMENT_BYTES = 20 * 1024 * 1024;
const ATTACHABLE_MIMES = [
  "image/png",
  "image/jpeg",
  "image/gif",
  "image/webp",
  "image/bmp",
  "application/pdf",
];

function formatBytes(bytes) {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${Math.round(bytes / 1024)} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}

function isImageAttachment(dataUrl) {
  return dataUrlMime(dataUrl).startsWith("image/");
}

function addAttachment(name, dataUrl) {
  const mime = dataUrlMime(dataUrl);
  if (!ATTACHABLE_MIMES.includes(mime)) {
    setStatus("Only PNG, JPEG, GIF, WebP, BMP and PDF can be attached");
    return false;
  }
  if (state.attachments.some((attachment) => attachment.dataUrl === dataUrl)) {
    setStatus("Already attached");
    return false;
  }
  if (state.attachments.length >= 8) {
    setStatus("At most 8 attachments per message");
    return false;
  }
  state.attachments.push({
    name: name || (mime === "application/pdf" ? "document.pdf" : "image"),
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

async function addAttachmentFiles(files) {
  for (const file of files) {
    if (file.size > MAX_ATTACHMENT_BYTES) {
      setStatus(
        `${file.name} is ${formatBytes(file.size)}; the attachment limit is ${formatBytes(MAX_ATTACHMENT_BYTES)}`,
      );
      continue;
    }
    // A blob the browser has typed says what it is before anything is read, so
    // a format nothing here can paint (a TIFF, a HEIC) is refused rather than
    // read into a data URL first; one it has not typed is left to the data URL
    // it turns into.
    const declared = (file.type || "").toLowerCase();
    if (declared && !ATTACHABLE_MIMES.includes(declared)) {
      setStatus(`Cannot attach ${file.name}: ${declared} is not one of PNG, JPEG, GIF, WebP, BMP and PDF`);
      continue;
    }
    try {
      let dataUrl = await readFileAsDataUrl(file);
      const mime = dataUrlMime(dataUrl);
      if (mime.startsWith("image/")) {
        dataUrl = await resizeImageDataUrl(dataUrl, mime);
      }
      addAttachment(file.name, dataUrl);
    } catch (error) {
      setStatus(`Could not read ${file.name}: ${error}`);
    }
  }
}

function renderAttachments() {
  const box = el("attachments");
  box.innerHTML = "";
  box.hidden = state.attachments.length === 0;
  state.attachments.forEach((attachment, index) => {
    const chip = document.createElement("div");
    chip.className = "attachment";
    if (isImageAttachment(attachment.dataUrl)) {
      chip.appendChild(openableImage(attachment.dataUrl, attachment.name));
    } else {
      const icon = document.createElement("div");
      icon.className = "att-file";
      icon.textContent = "📄";
      chip.appendChild(icon);
    }
    const label = document.createElement("div");
    label.className = "att-name";
    label.textContent = attachment.name;
    chip.appendChild(label);
    const remove = document.createElement("button");
    remove.className = "att-remove";
    remove.textContent = "×";
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

  if (state.busy) {
    if (state.runId == null) return;
    // An explicit shortcut can always queue; the ordinary Send action follows
    // the visible choice beside it.
    followUp = followUp || state.busyMessageMode === "queue";
    textarea.value = "";
    clearAttachments();
    clearWelcome();
    el("transcript").appendChild(
      bubble("user", prompt + (followUp ? "  (follow-up)" : ""), attachments),
    );
    scrollDown();
    await invoke("steer_run", {
      runId: state.runId,
      message: prompt,
      followUp,
      attachments: attachments.length ? attachments : null,
    });
    state.busyMessageMode = "queue";
    updateSendState();
    setStatus(followUp ? "Queued for the next response." : "Steering the active response…");
    return;
  }

  if (!state.project) return;
  textarea.value = "";
  clearAttachments();
  clearWelcome();
  el("transcript").appendChild(bubble("user", prompt, attachments));
  scrollDown();
  resetTurn();
  setBusy();
  setStatus("Working…");
  try {
    state.runId = await invoke("send_prompt", {
      project: state.project,
      prompt,
      // No thread on screen means the reader is starting one: `new` has the core
      // create it. `latest` would append this message to whichever thread was
      // used last, which is a thread they never chose — and one the sidebar
      // would go on listing unchanged.
      session: state.session || "new",
      reasoning: state.reasoning,
      attachments: attachments.length ? attachments : null,
    });
  } catch (error) {
    setStatus(`Error: ${error}`);
    setIdle();
  }
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
  const label = document.createElement("div");
  label.className = "label";
  label.textContent = "◆ Oxide";
  const body = document.createElement("div");
  body.className = "body";
  wrap.append(label, body);
  el("transcript").appendChild(wrap);
  state.currentAssistant = { wrap, body, text: "" };
  return state.currentAssistant;
}

function appendText(delta) {
  const assistant = ensureAssistant();
  assistant.text += delta;
  assistant.body.innerHTML = renderMarkdown(assistant.text);
  scrollDown();
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
  if (!state.currentThinking) {
    const block = document.createElement("div");
    block.className = "thinking";
    block.textContent = "✦ ";
    el("transcript").appendChild(block);
    state.currentThinking = block;
  }
  state.currentThinking.textContent += delta;
  scrollDown();
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
  head.append(tname, targ, tstate);
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
  el("transcript").appendChild(tool.block);
  state.tools.push(tool);
  startToolTimer(tool);
  scrollDown();
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
/// card that shows everything it has stays the text it looks like. A card can
/// only gain output to fold, so the marks are never taken back.
function markToolToggle(tool, expandable) {
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
        const tool = createToolCard(call.name || "tool", call.arguments);
        finishTool(tool, output, { isError: output.startsWith("error:") });
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
  // A turn that ran in another project — the window switched while it was still
  // going — is not this project's work: its card would sit under the wrong
  // transcript and its Undo would reach into the wrong work tree, so the payload
  // is dropped rather than painted.
  const project = payload.project || state.project;
  if (project !== state.project) return;
  const card = {
    project,
    // The state this turn left behind, which its own Undo checks the work tree
    // still holds (see `undoChanges`).
    after: payload.after || null,
    files: changes.files,
    added: changes.added || 0,
    removed: changes.removed || 0,
    baseline: payload.baseline || null,
    collapsed: false,
    all: false,
    undone: false,
    rows: [],
  };
  // Only the newest turn can be put back: an older card's baseline is the state
  // before *that* turn, so restoring it would take every change made since with
  // it, including the turns whose cards follow it.
  state.changes.push(card);
  for (const older of state.changes) {
    if (older !== card) paintChanges(older);
  }
  el("transcript").appendChild(changesCard(card));
  scrollDown();
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
    `<span class="change-chev">▸</span>`;
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
  icon.innerHTML =
    '<svg viewBox="0 0 24 24" width="14" height="14" aria-hidden="true">' +
    '<path d="M14 3H7a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h10a2 2 0 0 0 2-2V8z" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"/>' +
    '<path d="M14 3v5h5" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"/></svg>';
  const caret = document.createElement("span");
  caret.className = "changes-caret";
  caret.textContent = "▾";
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
  // that turn is the newest one: anything made after it would go too.
  card.undo.hidden = card.undone || state.changes[state.changes.length - 1] !== card;
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
      split.appendChild(splitRow(lines[index]));
      index += 1;
      continue;
    }
    let end = index;
    while (end < lines.length && lines[end].kind === "context") end += 1;
    const open = reviewState.expanded.has(`${file.path}:${index}`);
    if (open || end - index <= REVIEW_CONTEXT * 2 + 1) {
      for (let at = index; at < end; at += 1) split.appendChild(splitRow(lines[at]));
    } else {
      for (let at = index; at < index + REVIEW_CONTEXT; at += 1) {
        split.appendChild(splitRow(lines[at]));
      }
      split.appendChild(unmodifiedBar(file, index, end - index - REVIEW_CONTEXT * 2));
      for (let at = end - REVIEW_CONTEXT; at < end; at += 1) {
        split.appendChild(splitRow(lines[at]));
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
function splitRow(line) {
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
        scrollDown();
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
        scrollDown();
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
      const usage = event.usage || {};
      // The provider reports `input` as the uncached prompt; the cached prefix
      // still occupies the window, so count it toward the context percentage.
      const prompt = (usage.input || 0) + (usage.cacheRead || 0) + (usage.cacheWrite || 0);
      const pct = state.contextWindow ? (prompt / state.contextWindow) * 100 : null;
      setUsage({
        input: usage.input,
        output: usage.output,
        cacheRead: usage.cacheRead,
        cacheWrite: usage.cacheWrite,
        cost: usage.cost,
        contextPct: pct,
      });
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
  "mcps-modal",
  "models-modal",
  "themes-modal",
  "permissions-modal",
  "trust-modal",
  "confirm-modal",
  "rename-modal",
  "image-modal",
  "sessions-modal",
  "review-modal",
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
  state.providerIndex = Math.max(
    0,
    state.providers.findIndex((provider) => provider.stored),
  );
  renderProviders();
  closeOverlays("connect-modal");
  el("connect-modal").hidden = false;
}

function renderProviders() {
  const box = el("provider-list");
  box.innerHTML = "";
  state.providers.forEach((provider, index) => {
    const row = document.createElement("button");
    row.type = "button";
    row.className = "provider" + (index === state.providerIndex ? " active" : "");
    row.innerHTML =
      `<span class="p-name">${escapeHtml(provider.label)}</span>` +
      `<span class="p-desc">${escapeHtml(provider.description)}</span>` +
      (provider.stored ? '<span class="badge">stored</span>' : "");
    row.onclick = () => {
      state.providerIndex = index;
      renderProviders();
    };
    box.appendChild(row);
  });
}

async function saveConnect() {
  const provider = state.providers[state.providerIndex];
  if (!provider) return;
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
  if (!state.project) return;
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
    check.textContent = name === current ? "✓" : "";
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
      if (check) check.textContent = isActive ? "✓" : "";
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

// ---------- MCP servers ----------

/// The one icon the app builds in JavaScript: the power switch beside a server,
/// which no character renders the same way everywhere. The popover's Recheck and
/// Close are in `index.html`, and every other button is words.
const ICONS = {
  power:
    '<svg viewBox="0 0 24 24" width="14" height="14" aria-hidden="true"><path d="M18.36 6.64a9 9 0 1 1-12.73 0" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round"/><path d="M12 2v10" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round"/></svg>',
};

/// The servers this project loads, with the state the core reports: the same
/// listing `oxide mcp list` prints and the terminal's `/mcps` shows.
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
  if (pressed) {
    heldRepaints.push(renderMcps);
    return;
  }
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

/// The threads stored for this project, newest first: what `/sessions` opens,
/// and the same list the sidebar draws beside the project. The terminal's
/// `/resume` picker and the VS Code panel's dialog show the same sessions, so a
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
  if (pressed) {
    heldRepaints.push(renderSessions);
    return;
  }
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
    const row = document.createElement("button");
    row.type = "button";
    row.className = "session-row" + (session.id === state.session ? " active" : "");

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
    // The strings, not the elements they were written into: a tooltip built
    // from a node reads as `[object HTMLDivElement]`.
    row.title = meta ? `${name} — ${meta}` : name;
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

/// Alias → the name the built-ins dispatch on, mirroring
/// `oxide_core::commands`.
const SLASH_ALIASES = {
  mcps: "mcp",
  approvals: "permissions",
  access: "trust",
  thinking: "reasoning",
  sessions: "session",
  clear: "new",
  cost: "usage",
  login: "connect",
};

function slashName(raw) {
  const name = String(raw || "").replace(/^\//, "").toLowerCase();
  return SLASH_ALIASES[name] || name;
}

/// The built-ins this app performs itself. A name that is not here is sent on as
/// a prompt, so a project command, prompt template or skill still reaches the
/// agent through the CLI's own resolution.
async function runSlashCommand(text) {
  const parts = String(text).trim().split(/\s+/);
  const name = slashName(parts[0]);
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
        const index = state.providers.findIndex(
          (provider) =>
            provider.name === args.toLowerCase() ||
            provider.label.toLowerCase() === args.toLowerCase(),
        );
        if (index >= 0) {
          state.providerIndex = index;
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
      if (!state.project) {
        setStatus("Select a project first.");
        return true;
      }
      newChat();
      return true;
    case "reasoning":
      if (!args) {
        cycleReasoning();
      } else if (REASONING.includes(args)) {
        state.reasoning = args;
        updateChips();
      } else {
        setStatus(`Reasoning must be one of ${REASONING.join(", ")}.`);
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
      // rather than sending `/logout` to the model as a prompt.
      const builtin = state.palette.find((entry) => entry.kind === "client" && entry.name === name);
      if (!builtin) return false;
      setStatus(`/${builtin.name} is not available in the desktop app yet.`);
      return true;
    }
  }
}

// ---------- `@path` completion ----------

// The rows the host offered for the reference at the caret, the range of the
// value they replace, which row is highlighted, and the sequence number the
// answer belongs to. The host decides the token and the rows (`oxide_core::at`,
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

/// Asks the host what the caret is sitting in. Nothing is asked of a value with
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

/// Takes a row: the reference is replaced by what the host said it stands for,
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
  // A folder opens its own list; a file's reference is done, which the host
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
    state.palette = await invoke("list_commands", { project: state.project || "" });
  } catch (error) {
    state.palette = state.palette || [];
  }
}

function paletteMatches() {
  const text = el("prompt").value;
  if (!text.startsWith("/")) return null;
  const query = text.slice(1).toLowerCase();
  if (/\s/.test(query)) return null;
  return state.palette.filter(
    (entry) =>
      entry.name.toLowerCase().includes(query) ||
      entry.aliases.some((alias) => alias.toLowerCase().includes(query)),
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
    runSlashCommand(`/${entry.name}`);
    return;
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

function cycleReasoning() {
  state.reasoning = REASONING[(REASONING.indexOf(state.reasoning) + 1) % REASONING.length];
  updateChips();
}

async function initEvents() {
  await listen("agent-start", async (event) => {
    const payload = event.payload || {};
    if (payload.runId != null) state.runId = payload.runId;
    state.session = payload.sessionId || null;
    // A turn titles its thread the moment it starts, so the header names it now
    // and the sidebar lists it — with the same label — without waiting for the
    // turn to end. The session is on disk from here, so the listing has it.
    state.runTitle = payload.title || "";
    resetTurn();
    refreshThreadTitle();
    await loadSessions();
  });
  await listen("agent-event", (event) => handleEvent(event.payload || {}));
  await listen("agent-end", async (event) => {
    setIdle();
    setStatus("Ready");
    renderChanges(event.payload || {});
    resetTurn();
    await loadSessions();
  });
  el("review-modal").onkeydown = reviewKey;
  await listen("approval-request", (event) => showApproval(event.payload || {}));
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

// A repaint that replaces the row under the pointer before the press is over
// costs that press its click: the down landed on the row that was there and the
// up on the row that replaced it, so the webview dispatches no click at all and
// the control the press landed on never answers — the user clicks a second time.
// The sidebar is rebuilt from the read a *previous* click started, which lands
// exactly while the next press is already in flight, so a repaint asked for
// mid-press is held and run once the press is over.
let pressed = false;
let heldRepaints = [];
let controlPress = null;

/// The innermost control a press belongs to. Buttons and links are controls by
/// their element kind; native choices resolve through their label to the input
/// they operate, and the page's rows become controls when it gives them an
/// `onclick` handler.
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

function finishControlPress(event) {
  if (event.button !== 0) return;
  const press = controlPress;
  if (!press) return;
  if (pressedControl(event.target) !== press.control) {
    controlPress = null;
    return;
  }
  // A native click follows mouseup before the next task. Give the webview that
  // chance first, then supply the click when the webview omits one. Keyboard
  // activation remains native, and a press dragged away still cancels.
  setTimeout(() => {
    if (controlPress === press) controlPress = null;
    if (!press.clicked && !press.control.disabled) press.control.click();
  }, 0);
}

function endPress() {
  if (!pressed) return;
  pressed = false;
  const held = heldRepaints;
  heldRepaints = [];
  // A press dispatches its click after this handler returns, so the held repaint
  // waits for the next task: running it here would replace the row the click is
  // about to be delivered to.
  if (held.length) setTimeout(() => held.forEach((paint) => paint()), 0);
}

function init() {
  initSidebarResize();
  const createBtnTree = el("create-project-btn-tree");
  if (createBtnTree) createBtnTree.onclick = openCreateProject;
  el("create-project-add-folder").onclick = addCreateProjectFolder;
  el("create-project-cancel").onclick = () => (el("create-project-modal").hidden = true);
  el("create-project-save").onclick = saveCreateProject;

  el("reasoning").onclick = cycleReasoning;
  el("model").onclick = openModels;
  el("theme").onclick = openThemes;
  el("permissions").onclick = openPermissions;
  el("help").onclick = toggleHelp;
  el("connect").onclick = openConnect;

  el("send").onclick = () => send(false);
  el("busy-message-mode").onclick = toggleBusyMessageMode;
  el("stop").onclick = stop;
  el("approval-once").onclick = () => answerApproval("once");
  el("approval-always").onclick = () => answerApproval("always");
  el("approval-deny").onclick = () => answerApproval("deny");
  el("question-submit").onclick = () => questionNext();
  el("question-back").onclick = () => showQuestionStep((state.pendingQuestion?.index ?? 0) - 1);
  el("question-dismiss").onclick = () => answerQuestion(true);
  el("login-cancel").onclick = () => (el("connect-modal").hidden = true);
  el("login-save").onclick = saveConnect;
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

  // Treat a primary press that starts and ends on the same control as exactly
  // one activation. Native click stays authoritative when the webview emits it;
  // the next task supplies one only when it did not. This path deliberately
  // ignores focus and platform: fields keep their normal browser behavior, and
  // every actionable control follows the same activation contract.
  document.addEventListener(
    "mousedown",
    (event) => {
      if (event.button !== 0) return;
      const control = pressedControl(event.target);
      if (!control) return;
      controlPress = { control, clicked: false };
    },
    true,
  );
  document.addEventListener(
    "click",
    (event) => {
      if (controlPress && pressedControl(event.target) === controlPress.control) {
        controlPress.clicked = true;
      }
    },
    true,
  );
  document.addEventListener("mouseup", finishControlPress, true);

  // What the held repaints key on: the press is over once the pointer is
  // released, whether or not it was released inside the window.
  document.addEventListener(
    "mousedown",
    () => {
      pressed = true;
    },
    true,
  );
  document.addEventListener("mouseup", endPress, true);
  window.addEventListener("blur", () => {
    controlPress = null;
    endPress();
  });

  // The webview cannot navigate to a remote page, so a link click opens the
  // platform browser through the host instead of reloading the app window.
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
      addAttachmentFiles(files);
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
      if (!el("confirm-modal").hidden) resolveConfirm(false);
      if (!el("rename-modal").hidden) resolveRename(null);
      // A question dismissed with Escape is answered as unanswered rather than
      // hidden, so the turn continues instead of waiting out the timeout.
      if (!el("question").hidden) answerQuestion(true);
      closeOverlays();
      return;
    }
    if (event.key === "Tab" && event.shiftKey) {
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
  openDefaultProject();
}

init();

// ============================================================================
// CodeX-style tree view rendering
// ============================================================================

// Flattened project/session order, matching how the tree renders them, so
// `⌘1`…`⌘9` select the session carrying that label.
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
  if (pressed) {
    heldRepaints.push(renderProjectsTree);
    return;
  }
  const container = el("projects-tree");
  if (!container) return;
  
  container.innerHTML = "";
  
  if (!state.projects || !state.projects.length) {
    container.innerHTML = '<div class="empty" style="margin:12px; font-size:12px; color:var(--faint);">No projects yet. Click "+ New" to add one.</div>';
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
  
  const shortcutFor = new Map(
    orderedSessions()
      .slice(0, 9)
      .map((session, index) => [session.id, index + 1]),
  );
  
  for (const project of state.projects) {
    const projectGroup = document.createElement("div");
    projectGroup.className = "project-group";
    
    // Project header/button
    const projectItem = document.createElement("div");
    projectItem.className = "project-item" + (project.path === state.project ? " active" : "");
    
    const icon = document.createElement("div");
    icon.className = "icon";
    icon.textContent = "📁";
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
    newTaskBtn.textContent = "＋";
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
    removeProjectBtn.textContent = "✕";
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
      const sessionItem = document.createElement("div");
      sessionItem.className = "session-item" + (session.id === state.session ? " active" : "");

      const label = sessionLabel(session);
      const sessionName = document.createElement("div");
      sessionName.className = "name";
      sessionName.textContent = label;
      sessionItem.title = label;
      sessionItem.appendChild(sessionName);

      const shortcutNumber = shortcutFor.get(session.id);
      if (shortcutNumber) {
        const shortcut = document.createElement("div");
        shortcut.className = "shortcut";
        shortcut.textContent = "⌘" + shortcutNumber;
        sessionItem.appendChild(shortcut);
      }

      // A thread the store has not written has no file to remove, so its row
      // carries no ✕: leaving it is what starting a new thread does.
      if (!session.unstored) {
        const removeSessionBtn = document.createElement("button");
        removeSessionBtn.type = "button";
        removeSessionBtn.className = "row-remove";
        removeSessionBtn.title = "Delete thread";
        removeSessionBtn.textContent = "✕";
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
  // under. Selecting it leaves the window as it is.
  if (session.unstored) return;

  // Switch to the session's project first if different, so `session_messages`
  // is queried against the right project and the composer is enabled.
  const project = state.projects.find((p) => p.path === session.cwd);
  if (project && project.path !== state.project) {
    await selectProject(project);
  }

  window.history.replaceState({}, "", `?session=${session.id}`);
  await openSession(session);
}

function clearSelectedProject() {
  state.project = null;
  state.projectName = "";
  state.session = null;
  state.trust = null;
  el("prompt").disabled = true;
  el("project-meta").textContent = "";
  el("trust-modal").hidden = true;
  updateTrustButton();
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
  // here would leave the process writing into a file that is gone.
  if (state.busy && state.session === session.id) {
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
  if (state.session === session.id) resetTranscript();
  await refreshAfterRemoval();
}
