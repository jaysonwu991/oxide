// The chat webview renderer.
//
// It is deliberately dumb: the extension host owns the transcript and sends
// `state` / `push` / `append` / `patch` / `status` / `usage` / `context`
// messages (see src/core/protocol.ts), including the footer's own labels. Like
// the desktop app it decides nothing about the agent; even the `model:` and
// `thinking:` chips arrive as text, so the footer reads the same in both panes.
//
// The Markdown, syntax highlighting and diff renderers are adapted from the
// desktop app (crates/desktop/ui/app.js) so a reply reads the same in both
// front-ends.

(function () {
  const vscode = acquireVsCodeApi();
  const $ = (id) => document.getElementById(id);

  const transcript = $("transcript");
  const empty = $("empty");
  const input = $("input");
  const sendButton = $("send");
  const stopButton = $("stop");
  const statusLabel = $("status");
  const elapsedLabel = $("elapsed");
  const usageRow = $("usage");
  const usageText = $("usage-text");
  const gauge = $("gauge");
  const gaugeFill = $("gauge-fill");
  const metaBox = $("meta");
  const chipBox = $("chips");
  const folderLabel = $("folder");
  const modelLabel = $("model");

  const entries = new Map();
  let chips = [];
  let busy = false;
  let queued = 0;
  let startedAt = 0;
  let elapsedTimer = 0;
  let dirty = new Set();
  let frame = 0;

  // ---------- helpers ----------

  function escapeHtml(text) {
    return String(text)
      .replace(/&/g, "&amp;")
      .replace(/</g, "&lt;")
      .replace(/>/g, "&gt;")
      .replace(/"/g, "&quot;");
  }

  function formatDuration(ms) {
    if (ms < 1000) return `${Math.round(ms)}ms`;
    const seconds = ms / 1000;
    if (seconds < 60) return `${seconds.toFixed(1)}s`;
    return `${Math.floor(seconds / 60)}m ${Math.round(seconds % 60)}s`;
  }

  function atBottom() {
    return transcript.scrollHeight - transcript.scrollTop - transcript.clientHeight < 40;
  }

  function scrollDown(force) {
    if (force || atBottom()) transcript.scrollTop = transcript.scrollHeight;
  }

  // ---------- inline markdown ----------

  function anchor(href, label) {
    const text = label == null ? escapeHtml(href) : label;
    return `<a href="${escapeHtml(href)}" target="_blank" rel="noreferrer">${text}</a>`;
  }

  // A bare URL often ends a sentence, so trailing punctuation stays outside the
  // link and an unbalanced closing bracket is handed back to the text.
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
    // Links and code spans are stashed on the raw text, before escaping, so a
    // URL is never folded into an HTML entity and an entity is never read as
    // part of a URL.
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

  // ---------- syntax highlighting ----------

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
    const groups = [
      [["rs", "rust"], "rust"],
      [["js", "jsx", "ts", "tsx", "javascript", "typescript", "mjs", "cjs"], "js"],
      [["py", "python"], "python"],
      [["go", "golang"], "go"],
      [["sh", "bash", "shell", "zsh", "console"], "bash"],
      [["json", "jsonc"], "json"],
    ];
    for (const [names, key] of groups) {
      if (names.includes(value)) return key;
    }
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

  // ---------- block markdown ----------

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

  /// Renders Markdown for the transcript. Unclosed delimiters stay literal, so
  /// a reply renders cleanly while it is still streaming.
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

  // ---------- tool cards ----------

  // Leading shell noise (`export PATH=…;`, `cd …;`) and label/utility statements
  // that would fill the card header without saying what the call does.
  const CMD_NOISE = /^(?:export\b|cd\b|date\b|pwd\b|true\b|:|echo\b|printf\b|set\b)/;

  function toolSummary(name, rawArgs) {
    let args = {};
    try {
      args = JSON.parse(rawArgs || "{}");
    } catch (error) {
      args = {};
    }
    const subject = String(
      args.command ||
        args.path ||
        args.pattern ||
        args.query ||
        args.url ||
        args.prompt ||
        args.agent ||
        args.name ||
        "",
    )
      .replace(/\s+/g, " ")
      .trim();
    let text = subject;
    if (name === "bash" && subject) {
      const parts = subject
        .split(/\s*(?:&&|\|\||;)\s*/)
        .map((part) => part.trim())
        .filter(Boolean);
      text = parts.find((part) => !CMD_NOISE.test(part)) || parts[parts.length - 1] || subject;
    }
    return {
      text: text.length > 96 ? `${text.slice(0, 96)}…` : text,
      title: subject || text,
    };
  }

  function toolPath(args) {
    const subject = String(args.path || "").trim();
    return subject && !subject.includes("\0") ? subject : "";
  }

  // Output budgets per tool, mirroring the TUI's previews: shell keeps its tail
  // (the exit code is at the end), the readers keep their head.
  const PREVIEW_LINES = { bash: 5, read: 10, read_file: 10, grep: 15, find: 20, glob: 20, ls: 20, list_dir: 20 };

  function previewLines(name) {
    return PREVIEW_LINES[name] || 5;
  }

  function previewText(text, lines) {
    const all = String(text || "").replace(/\s+$/, "");
    if (!all) return { text: "", more: 0 };
    const split = all.split("\n");
    if (split.length <= lines) return { text: all, more: 0 };
    // bash reports its exit code last, so its preview keeps the tail.
    return { text: split.slice(0, lines).join("\n"), more: split.length - lines };
  }

  function previewTail(text, lines) {
    const all = String(text || "").replace(/\s+$/, "");
    if (!all) return { text: "", more: 0 };
    const split = all.split("\n");
    if (split.length <= lines) return { text: all, more: 0 };
    return { text: split.slice(split.length - lines).join("\n"), more: split.length - lines };
  }

  function renderDiff(target, diff) {
    const body = String(diff || "")
      .split("\n")
      .map((line) => {
        let cls = "";
        if (line.startsWith("@@")) cls = "hunk";
        else if (line.startsWith("+")) cls = "add";
        else if (line.startsWith("-")) cls = "del";
        return `<span class="dline ${cls}">${escapeHtml(line)}</span>`;
      })
      .join("");
    return `<div class="diff"><div class="diff-path">${escapeHtml(target || "diff")}</div><pre>${body}</pre></div>`;
  }

  function toolCard(item) {
    const wrap = document.createElement("div");
    wrap.className = "tool";
    const head = document.createElement("div");
    head.className = "thead";
    // A real button control: focusable and keyboard-activatable, so the
    // collapsible tool output is reachable without a mouse.
    head.setAttribute("role", "button");
    head.tabIndex = 0;
    head.setAttribute("aria-expanded", "false");
    const name = document.createElement("span");
    name.className = "tname";
    name.textContent = String(item.name || "tool");
    const arg = document.createElement("span");
    arg.className = "targ";
    const args = parseArgs(item.args);
    const summary = toolSummary(item.name, item.args);
    arg.textContent = summary.text;
    if (summary.title) head.title = summary.title;
    // The header doubles as a link to the file the call touched, but only when
    // the subject really is that path (`read`, `write`, `edit`, `ls`).
    const target = toolPath(args);
    if (target && target === summary.text) {
      arg.classList.add("pathlike");
      arg.dataset.path = target;
      arg.dataset.line = String(args.offset || 0);
      arg.title = `Open ${target}`;
    }
    const state = document.createElement("span");
    state.className = "tstate";
    head.append(name, arg, state);
    const pre = document.createElement("pre");
    pre.className = "tbody";
    const hint = document.createElement("div");
    hint.className = "thint";
    hint.hidden = true;
    wrap.append(head, pre, hint);
    return { el: wrap, pre, hint, state, head, summary };
  }

  function parseArgs(raw) {
    try {
      const value = JSON.parse(raw || "{}");
      return value && typeof value === "object" ? value : {};
    } catch (error) {
      return {};
    }
  }

  function diffTarget(entry) {
    return String(parseArgs(entry.item.args).path || "");
  }

  /// Paints a tool card from its item. `running` cards show the live output.
  function paintTool(entry) {
    const item = entry.item;
    entry.el.classList.toggle("running", item.running);
    entry.el.classList.toggle("done", !item.running && !item.isError);
    entry.el.classList.toggle("error", !item.running && item.isError);
    if (item.running) {
      const elapsed = entry.started ? Date.now() - entry.started : 0;
      entry.state.innerHTML = `<span class="spinner"></span>${elapsed > 1000 ? formatDuration(elapsed) : ""}`;
      entry.pre.textContent = item.output;
      entry.hint.hidden = true;
      return;
    }
    entry.state.textContent = item.isError ? "✖" : "✔";
    if (item.diff && !entry.diffEl) {
      entry.diffEl = document.createElement("div");
      entry.diffEl.innerHTML = renderDiff(diffTarget(entry), item.diff);
      entry.el.appendChild(entry.diffEl);
      entry.expanded = true;
    }
    if (entry.expanded) {
      entry.pre.textContent = item.output;
      entry.hint.hidden = true;
      return;
    }
    const budget = previewLines(item.name);
    const useTail = item.name === "bash";
    const preview = useTail
      ? previewTail(item.output, budget)
      : previewText(item.output, budget);
    entry.pre.textContent =
      preview.more > 0 && useTail ? `… ${preview.more} earlier lines\n${preview.text}` : preview.text;
    entry.hint.hidden = preview.more === 0;
    entry.hint.textContent = `⋯ ${preview.more} ${useTail ? "earlier" : "more"} line${
      preview.more === 1 ? "" : "s"
    } · click to expand`;
  }

  function toggleTool(entry) {
    if (!entry.item || entry.item.running) return;
    entry.expanded = !entry.expanded;
    if (entry.head) entry.head.setAttribute("aria-expanded", String(entry.expanded));
    paintTool(entry);
  }

  function entryOf(node) {
    const row = node.closest("[data-id]");
    return row ? entries.get(Number(row.dataset.id)) : undefined;
  }

  // ---------- transcript items ----------

  function itemNode(item) {
    if (item.kind === "user") {
      const wrap = document.createElement("div");
      wrap.className = "msg user";
      const bubble = document.createElement("div");
      bubble.className = "bubble";
      bubble.textContent = item.text;
      wrap.appendChild(bubble);
      if (item.context && item.context.length) {
        const list = document.createElement("div");
        list.className = "ctx";
        for (const label of item.context) {
          const chip = document.createElement("span");
          chip.className = "chip";
          chip.textContent = label;
          list.appendChild(chip);
        }
        wrap.appendChild(list);
      }
      return { el: wrap };
    }
    if (item.kind === "assistant") {
      const wrap = document.createElement("div");
      wrap.className = "msg assistant";
      const body = document.createElement("div");
      body.className = "body";
      wrap.appendChild(body);
      return { el: wrap, body };
    }
    if (item.kind === "thinking") {
      const el = document.createElement("div");
      el.className = "thinking";
      return { el };
    }
    if (item.kind === "notice") {
      const el = document.createElement("div");
      el.className = `notice ${item.tone || "info"}`;
      el.textContent = item.text;
      return { el };
    }
    const card = toolCard(item);
    return { ...card, started: item.running ? Date.now() : 0 };
  }

  function renderAssistant(entry) {
    entry.body.innerHTML = renderMarkdown(entry.item.text);
  }

  function scheduleRender(entry) {
    dirty.add(entry);
    if (frame) return;
    frame = requestAnimationFrame(() => {
      frame = 0;
      for (const target of dirty) renderAssistant(target);
      dirty = new Set();
    });
  }

  function appendItem(item, keepScroll) {
    const entry = itemNode(item);
    entry.item = item;
    entry.el.dataset.id = String(item.id);
    entries.set(item.id, entry);
    transcript.appendChild(entry.el);
    empty.hidden = true;
    if (item.kind === "assistant") renderAssistant(entry);
    else if (item.kind === "thinking") entry.el.textContent = item.text;
    else if (item.kind === "tool") paintTool(entry);
    scrollDown(keepScroll);
    return entry;
  }

  function apply(message) {
    switch (message.k) {
      case "state": {
        entries.clear();
        transcript.innerHTML = "";
        transcript.appendChild(empty);
        empty.hidden = message.items.length > 0;
        transcript.classList.toggle("hide-thinking", message.showThinking === false);
        folderLabel.textContent = message.folder || "Oxide";
        modelLabel.textContent = message.model ? `${message.model}` : "";
        for (const item of message.items) appendItem(item, false);
        setStatus(message.status, message.busy, message.queued);
        setFooter(message.footer);
        setChips(message.context);
        scrollDown(true);
        return;
      }
      case "push":
        appendItem(message.item, false);
        return;
      case "remove": {
        const entry = entries.get(message.id);
        if (entry) {
          entry.el.remove();
          entries.delete(message.id);
        }
        return;
      }
      case "append": {
        const entry = entries.get(message.id);
        if (!entry) return;
        if (message.field === "text") entry.item.text += message.delta;
        else entry.item.output += message.delta;
        if (entry.item.kind === "assistant") scheduleRender(entry);
        else if (entry.item.kind === "thinking") entry.el.textContent = entry.item.text;
        else paintTool(entry);
        scrollDown(false);
        return;
      }
      case "patch": {
        const entry = entries.get(message.id);
        if (!entry) return;
        Object.assign(entry.item, message.patch);
        paintTool(entry);
        scrollDown(false);
        return;
      }
      case "status":
        setStatus(message.status, message.busy, message.queued);
        setFooter(message.footer);
        return;
      case "usage":
        if (message.footer) setFooter(message.footer);
        return;
      case "context":
        setChips(message.context);
        return;
      default:
        return;
    }
  }

  // ---------- footer ----------

  function setStatus(text, isBusy, queuedCount) {
    busy = Boolean(isBusy);
    queued = queuedCount || 0;
    statusLabel.textContent = queued > 0 ? `${text} · ${queued} queued` : text;
    statusLabel.classList.toggle("busy", busy);
    stopButton.hidden = !busy;
    sendButton.textContent = busy ? "Queue" : "Send";
    updateElapsed();
    updateSendState();
  }

  /// A running turn reports how long it has been running, like the terminal's
  /// status row (and the tool panels' live `Elapsed`) do.
  function updateElapsed() {
    if (!busy) {
      clearInterval(elapsedTimer);
      elapsedTimer = 0;
      startedAt = 0;
      elapsedLabel.hidden = true;
      return;
    }
    if (!startedAt) startedAt = Date.now();
    elapsedLabel.hidden = false;
    elapsedLabel.textContent = `${((Date.now() - startedAt) / 1000).toFixed(1)}s`;
    if (!elapsedTimer) elapsedTimer = setInterval(updateElapsed, 500);
  }

  /// The footer the extension host composes: the chips under the transcript
  /// (`model: … · thinking: … · access: … · session: …`), the branch, the usage
  /// line and the context gauge. Everything is a string by the time it gets
  /// here, so this function only paints.
  function setFooter(footer) {
    if (!footer) return;
    metaBox.innerHTML = "";
    for (const chip of footer.chips || []) {
      const el = document.createElement("button");
      el.type = "button";
      el.className = "meta-chip";
      el.dataset.control = chip.id;
      el.textContent = chip.label;
      if (chip.title) el.title = chip.title;
      metaBox.appendChild(el);
    }
    if (footer.info) {
      const branch = document.createElement("span");
      branch.className = "meta-branch";
      branch.textContent = footer.info;
      branch.title = `Current branch: ${footer.info}`;
      metaBox.appendChild(branch);
    }
    usageText.textContent = footer.usage || "";
    usageText.title = footer.usage || "";
    usageRow.hidden = !footer.usage;
    const percent = typeof footer.percent === "number" ? footer.percent : null;
    gauge.hidden = percent === null;
    gauge.dataset.level = footer.level || "ok";
    gauge.title = percent === null ? "" : `${percent}% of the context window used`;
    gaugeFill.style.width = percent === null ? "0%" : `${Math.min(percent, 100)}%`;
  }

  function setChips(next) {
    chips = next || [];
    chipBox.innerHTML = "";
    chipBox.hidden = chips.length === 0;
    for (const chip of chips) {
      const el = document.createElement("span");
      el.className = "chip";
      const label = document.createElement("span");
      label.textContent = chip.label;
      const remove = document.createElement("button");
      remove.textContent = "✕";
      remove.title = "Remove from the next message";
      remove.addEventListener("click", () => {
        vscode.postMessage({ k: "removeContext", id: chip.id });
      });
      el.append(label, remove);
      chipBox.appendChild(el);
    }
    updateSendState();
  }

  function updateSendState() {
    sendButton.disabled = busy ? false : !input.value.trim() && chips.length === 0;
  }

  // ---------- composer ----------

  function resizeInput() {
    input.style.height = "auto";
    input.style.height = `${Math.min(input.scrollHeight, 200)}px`;
  }

  function submit() {
    const text = input.value;
    if (!text.trim() && chips.length === 0) return;
    input.value = "";
    resizeInput();
    updateSendState();
    vscode.postMessage({ k: "send", text });
  }

  input.addEventListener("input", () => {
    resizeInput();
    updateSendState();
  });

  input.addEventListener("keydown", (event) => {
    if (event.key === "Enter" && !event.shiftKey) {
      event.preventDefault();
      submit();
      return;
    }
    if (event.key === "Escape" && busy) {
      event.preventDefault();
      vscode.postMessage({ k: "stop" });
    }
  });

  sendButton.addEventListener("click", () => submit());
  stopButton.addEventListener("click", () => vscode.postMessage({ k: "stop" }));
  $("new-session").addEventListener("click", () => vscode.postMessage({ k: "newSession" }));
  $("resume-session").addEventListener("click", () => vscode.postMessage({ k: "resumeSession" }));

  // The footer's chips are the extension's own commands: each one opens a picker
  // or cycles a setting, and the host re-sends the footer afterwards.
  metaBox.addEventListener("click", (event) => {
    const target = event.target;
    if (!(target instanceof Element)) return;
    const chip = target.closest("[data-control]");
    if (!chip) return;
    vscode.postMessage({ k: "control", control: chip.dataset.control });
  });

  // Links open in the browser and paths open in the editor, because a webview
  // cannot navigate or read the workspace itself.
  transcript.addEventListener("click", (event) => {
    const target = event.target;
    if (!(target instanceof Element)) return;
    const link = target.closest("a[href]");
    if (link) {
      event.preventDefault();
      vscode.postMessage({ k: "openUrl", url: link.getAttribute("href") || "" });
      return;
    }
    const pathEl = target.closest("[data-path]");
    if (pathEl) {
      vscode.postMessage({
        k: "reveal",
        path: pathEl.dataset.path,
        line: Number(pathEl.dataset.line) || 0,
      });
      return;
    }
    const entry = entryOf(target);
    if (entry && target.closest(".thead")) toggleTool(entry);
  });

  transcript.addEventListener("keydown", (event) => {
    if (event.key !== "Enter" && event.key !== " ") return;
    const target = event.target;
    if (!(target instanceof Element) || !target.closest(".thead")) return;
    const entry = entryOf(target);
    if (!entry) return;
    event.preventDefault();
    toggleTool(entry);
  });

  window.addEventListener("message", (event) => apply(event.data));
  resizeInput();
  updateSendState();
  vscode.postMessage({ k: "ready" });
})();
