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
  reasoning: "auto",
  contextWindow: 0,
  providers: [],
  providerIndex: 0,
  models: null,
  pendingApproval: null,
  trust: null,
  currentAssistant: null,
  tools: [],
  currentThinking: null,
  attachments: [],
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
  el("thread-title").textContent = text || "New task";
}

// ---------- markdown ----------

function inline(text) {
  let s = escapeHtml(text);
  s = s.replace(/`([^`]+)`/g, (_, code) => `<code>${code}</code>`);
  s = s.replace(/\[([^\]]+)\]\(([^)\s]+)\)/g, '<a href="$2" target="_blank" rel="noreferrer">$1</a>');
  s = s.replace(/\*\*([^*]+)\*\*/g, "<strong>$1</strong>");
  s = s.replace(/(^|[^*])\*([^*\n]+)\*/g, "$1<em>$2</em>");
  s = s.replace(/~~([^~]+)~~/g, "<del>$1</del>");
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

function renderDiff(diff) {
  const body = String(diff.text || "")
    .split("\n")
    .map((line) => {
      let cls = "";
      if (line.startsWith("@@")) cls = "hunk";
      else if (line.startsWith("+")) cls = "add";
      else if (line.startsWith("-")) cls = "del";
      return `<span class="dline ${cls}">${escapeHtml(line)}</span>`;
    })
    .join("");
  return `<div class="diff"><div class="diff-path">${escapeHtml(diff.path || "diff")}</div><pre>${body}</pre></div>`;
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
  }
}

function renderProjectMeta(info) {
  const off =
    info.trust && info.trust.required && !info.trust.trusted ? " · project resources off" : "";
  el("model").textContent = info.model;
  el("project-meta").textContent =
    info.provider + (info.hasKey ? "" : " · no API key") + off;
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

async function loadSessions() {
  try {
    state.sessions = await invoke("all_sessions");
    renderProjectsTree();
  } catch (error) {
    setStatus(`Failed to load threads: ${error}`);
  }
}

async function openSession(session) {
  state.session = session.id;
  try {
    const data = await invoke("session_messages", { project: state.project, id: session.id });
    const transcript = el("transcript");
    transcript.innerHTML = "";
    renderStoredTranscript(transcript, data.messages);
    scrollDown();
    setThreadTitle(session.name || session.preview || session.id.slice(0, 8));
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
  el("transcript").innerHTML = "";
  el("usage").textContent = "";
  setThreadTitle("New task");
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
  state.currentAssistant = null;
  state.tools = [];
  state.currentThinking = null;
}

function updateSendState() {
  const hasText =
    el("prompt").value.trim().length > 0 || state.attachments.length > 0;
  el("send").classList.toggle("enabled", hasText);
  el("send").disabled = !hasText;
}

function setBusy() {
  state.busy = true;
  el("stop").hidden = false;
  el("send").title = "Steer (Enter)";
}

function setIdle() {
  state.busy = false;
  state.runId = null;
  el("stop").hidden = true;
  el("send").title = "Send (Enter)";
  updateSendState();
}

// ---------- attachments ----------

/// `data:<mime>;base64,...` — the only shape the clipboard and FileReader give
/// us, and the shape the backend turns back into a media part.
function dataUrlMime(dataUrl) {
  const match = /^data:([^;,]+)[;,]/.exec(dataUrl || "");
  return match ? match[1] : "";
}

function isImageAttachment(dataUrl) {
  return dataUrlMime(dataUrl).startsWith("image/");
}

function addAttachment(name, dataUrl) {
  const mime = dataUrlMime(dataUrl);
  if (!mime.startsWith("image/") && mime !== "application/pdf") return false;
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

function openImage(dataUrl) {
  el("image-view-img").src = dataUrl;
  closeOverlays("image-modal");
  el("image-modal").hidden = false;
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
  button.appendChild(img);
  button.onclick = () => openImage(dataUrl);
  return button;
}

async function send(followUp = false) {
  const textarea = el("prompt");
  const prompt = textarea.value.trim();
  const attachments = attachmentPayload();
  if (!prompt && attachments.length === 0) return;

  if (state.busy) {
    if (state.runId == null) return;
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
      session: state.session || "latest",
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
    pre,
    hint,
    tstate,
    done: false,
    full: "",
    live: "",
    expanded: false,
    started: performance.now(),
  };
  head.onclick = () => toggleTool(tool);
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
  scrollDown();
  return tool;
}

/// Marks a card finished and paints the collapsed preview (or the full result
/// when it carries a diff). Shared by the live stream and the stored transcript.
function finishTool(tool, output, { isError = false, diff = null, elapsed = 0 } = {}) {
  tool.done = true;
  tool.full = output || "";
  tool.expanded = Boolean(diff);
  tool.block.classList.remove("running");
  if (isError) tool.block.classList.add("error");
  tool.tstate.textContent = `${isError ? "✖" : "✔"}${elapsed ? ` ${formatDuration(elapsed)}` : ""}`;
  if (diff) tool.block.insertAdjacentHTML("beforeend", renderDiff(diff));
  paintTool(tool);
}

/// Collapsed cards show the first lines of output plus how much was hidden;
/// clicking swaps in the full result.
function paintTool(tool) {
  if (!tool.done) {
    tool.pre.textContent = tool.live;
    return;
  }
  if (tool.expanded) {
    tool.pre.textContent = tool.full;
    tool.hint.hidden = true;
    tool.block.classList.add("expanded");
    return;
  }
  const { text, more } = previewText(tool.full, 3);
  tool.pre.textContent = text;
  tool.hint.hidden = more === 0;
  tool.hint.textContent = `⋯ ${more} more line${more === 1 ? "" : "s"} · click to expand`;
  tool.block.classList.remove("expanded");
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
    case "auto_retry_start":
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

// ---------- providers ----------

const OVERLAYS = [
  "approval",
  "connect-modal",
  "create-project-modal",
  "models-modal",
  "themes-modal",
  "permissions-modal",
  "trust-modal",
  "confirm-modal",
  "rename-modal",
  "image-modal",
  "help-modal",
];

function closeOverlays(except) {
  for (const id of OVERLAYS) {
    if (id !== except) el(id).hidden = true;
  }
}

// macOS's WKWebView does not implement `window.confirm`/`window.prompt`, so
// destructive and rename actions go through these in-app dialogs instead.
let confirmResolution = null;
let renameResolution = null;

function confirmDialog(title, message, confirmLabel = "Confirm", alt = null) {
  el("confirm-title").textContent = title;
  el("confirm-message").textContent = message;
  el("confirm-ok").textContent = confirmLabel;
  const altButton = el("confirm-alt");
  altButton.hidden = !alt;
  altButton.textContent = alt ? alt.label : "";
  altButton.onclick = alt ? () => resolveConfirm(alt.value) : null;
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
  await listen("agent-start", (event) => {
    const payload = event.payload || {};
    if (payload.runId != null) state.runId = payload.runId;
    if (payload.sessionId) state.session = payload.sessionId;
    resetTurn();
  });
  await listen("agent-event", (event) => handleEvent(event.payload || {}));
  await listen("agent-end", async () => {
    setIdle();
    setStatus("Ready");
    resetTurn();
    await loadSessions();
  });
  await listen("approval-request", (event) => showApproval(event.payload || {}));
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
  const STORAGE_KEY = "oxide.sidebarWidth";
  // Keep a comfortable reading column for the transcript.
  const maxWidth = () => Math.max(MIN, window.innerWidth - 420);
  const clamp = (width) => Math.max(MIN, Math.min(maxWidth(), Math.round(width)));
  const apply = (width, persist) => {
    sidebar.style.width = `${clamp(width)}px`;
    if (persist) localStorage.setItem(STORAGE_KEY, sidebar.style.width);
  };

  const saved = Number(localStorage.getItem(STORAGE_KEY) || 0);
  if (saved > 0) apply(saved, false);

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

  handle.addEventListener("dblclick", () => {
    localStorage.removeItem(STORAGE_KEY);
    sidebar.style.width = "";
  });

  window.addEventListener("resize", () => {
    if (sidebar.style.width) apply(sidebar.getBoundingClientRect().width, false);
  });
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
  el("stop").onclick = stop;
  el("approval-once").onclick = () => answerApproval("once");
  el("approval-always").onclick = () => answerApproval("always");
  el("approval-deny").onclick = () => answerApproval("deny");
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
  el("rename-cancel").onclick = () => resolveRename(null);
  el("rename-save").onclick = () => resolveRename(el("rename-input").value.trim());
  el("rename-input").addEventListener("keydown", (event) => {
    if (event.key === "Enter") {
      event.preventDefault();
      resolveRename(el("rename-input").value.trim());
    }
  });
  el("help-close").onclick = () => (el("help-modal").hidden = true);

  el("prompt").addEventListener("input", () => {
    el("prompt").style.height = "auto";
    el("prompt").style.height = `${Math.min(el("prompt").scrollHeight, 220)}px`;
    updateSendState();
  });
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
  el("image-view-close").onclick = () => (el("image-modal").hidden = true);
  el("prompt").addEventListener("keydown", (event) => {
    if (event.key === "Enter" && !event.shiftKey) {
      event.preventDefault();
      send(event.altKey);
    }
  });

  document.addEventListener("keydown", (event) => {
    if (event.key === "Escape") {
      if (!el("confirm-modal").hidden) resolveConfirm(false);
      if (!el("rename-modal").hidden) resolveRename(null);
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
  loadProjects();
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
  for (const session of state.sessions || []) {
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
    container.innerHTML = '<div class="empty" style="margin:12px; font-size:12px; color:var(--faint);">No projects yet. Click "+ New" to add one.</div>';
    return;
  }
  
  // Group sessions by project
  const sessionsByProject = {};
  for (const project of state.projects) {
    sessionsByProject[project.path] = [];
  }
  
  if (state.sessions && state.sessions.length) {
    for (const session of state.sessions) {
      if (sessionsByProject[session.cwd]) {
        sessionsByProject[session.cwd].push(session);
      }
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

      const label = session.name || session.preview || session.id.slice(0, 8);
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
