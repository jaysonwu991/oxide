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
  const branchLabel = $("branch");
  const attachButton = $("attach");
  const composer = $("composer");
  const dropHint = $("dropzone");
  const folderLabel = $("folder");
  const modelLabel = $("model");

  const entries = new Map();
  let chips = [];
  let attachments = [];
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

  /// Whether the view is pinned to the newest line. The reader decides: scrolling
  /// away stops the follow, coming back to the bottom resumes it. The scroll
  /// event that reports it is asynchronous, so a message re-measures before it
  /// grows the content as well (`apply`), and the scroll that follows a delta is
  /// taken in the frame that paints it — a reply that adds more than the
  /// tolerance while its paint is still pending would otherwise read as a reader
  /// who scrolled away, and the view would stop following for the rest of the
  /// turn.
  let following = true;

  function atBottom() {
    return transcript.scrollHeight - transcript.scrollTop - transcript.clientHeight < 40;
  }

  function scrollDown(force) {
    if (force) following = true;
    if (following) transcript.scrollTop = transcript.scrollHeight;
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
  /// Replace a streaming body without losing the reader's place. Writing the
  /// text of a scroll container back resets it to the top, so a running command
  /// would keep showing its first line instead of its latest; a body that was
  /// already at its bottom is put back there. Output that has finished is left at
  /// the top, since expanding a card is a request to read it from the beginning.
  function setOutput(el, text, running) {
    const follow = running && el.scrollHeight - el.scrollTop - el.clientHeight < 4;
    el.textContent = text;
    if (follow) el.scrollTop = el.scrollHeight;
  }

  function paintTool(entry) {
    const item = entry.item;
    entry.el.classList.toggle("running", item.running);
    entry.el.classList.toggle("done", !item.running && !item.isError);
    entry.el.classList.toggle("error", !item.running && item.isError);
    if (item.running) {
      const elapsed = entry.started ? Date.now() - entry.started : 0;
      entry.state.innerHTML = `<span class="spinner"></span>${elapsed > 1000 ? formatDuration(elapsed) : ""}`;
      setOutput(entry.pre, item.output, true);
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
      setOutput(entry.pre, item.output, false);
      entry.hint.hidden = true;
      return;
    }
    const budget = previewLines(item.name);
    const useTail = item.name === "bash";
    const preview = useTail
      ? previewTail(item.output, budget)
      : previewText(item.output, budget);
    setOutput(
      entry.pre,
      preview.more > 0 && useTail ? `… ${preview.more} earlier lines\n${preview.text}` : preview.text,
      false,
    );
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

  // ---------- approvals ----------

  /// The three answers the CLI accepts, in the order the card offers them.
  const APPROVAL_ACTIONS = [
    { decision: "deny", label: "Deny", className: "ghost", title: "Refuse the tool and tell the agent to stop" },
    { decision: "once", label: "Allow once", className: "primary", title: "Run the tool this one time" },
    { decision: "always", label: "Always allow", className: "primary", title: "Run it, and stop asking about this tool in this project" },
  ];

  /// A tool the agent is holding until the user answers. The answer travels to
  /// the host, which forwards it over the CLI's request channel; the card is
  /// repainted from the `k: "approval"` update the host sends back.
  function approvalNode(item) {
    const wrap = document.createElement("div");
    wrap.className = "approval";
    const head = document.createElement("div");
    head.className = "ahead";
    const lock = document.createElement("span");
    lock.className = "alock";
    lock.textContent = "\u{1F512}";
    const title = document.createElement("span");
    title.className = "atitle";
    title.textContent = "Permission required";
    head.append(lock, title);
    const what = document.createElement("div");
    what.className = "awhat";
    const tool = document.createElement("span");
    tool.className = "atool";
    tool.textContent = String(item.tool || "tool");
    const sub = document.createElement("span");
    sub.className = "asub";
    sub.textContent = String(item.title || "");
    what.append(tool, sub);
    const detail = document.createElement("pre");
    detail.className = "adetail";
    detail.textContent = String(item.detail || "");
    detail.hidden = !item.detail;
    const actions = document.createElement("div");
    actions.className = "aactions";
    wrap.append(head, what, detail, actions);
    return { el: wrap, actions, detail };
  }

  /// Paints a card from its item: the waiting state offers the three answers,
  /// and an answered one keeps only what it was answered with.
  function paintApproval(entry) {
    const item = entry.item;
    const waiting = item.state === "pending";
    entry.el.classList.toggle("waiting", waiting);
    entry.el.classList.toggle("allowed", item.state === "once" || item.state === "always");
    entry.el.classList.toggle("denied", item.state === "deny" || item.state === "closed");
    entry.actions.textContent = "";
    if (!waiting) {
      const done = document.createElement("span");
      done.className = "adone";
      done.textContent = item.label || "Answered";
      entry.actions.appendChild(done);
      return;
    }
    for (const action of APPROVAL_ACTIONS) {
      const button = document.createElement("button");
      button.type = "button";
      button.className = action.className;
      button.textContent = action.label;
      button.title = action.title;
      button.addEventListener("click", () =>
        vscode.postMessage({
          k: "approval",
          requestId: item.requestId,
          decision: action.decision,
        }),
      );
      entry.actions.appendChild(button);
    }
    const hint = document.createElement("span");
    hint.className = "awaiting";
    hint.textContent = "Waiting for your answer…";
    entry.actions.appendChild(hint);
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
    if (item.kind === "approval") return approvalNode(item);
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
      // The scroll that came with this delta ran before this paint, so follow the
      // height it just added; without it the last line of a streamed reply stays
      // under the fold.
      if (following) transcript.scrollTop = transcript.scrollHeight;
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
    else if (item.kind === "approval") paintApproval(entry);
    scrollDown(keepScroll);
    return entry;
  }

  function apply(message) {
    // Measured before anything grows, so it reflects the frame the reader is
    // looking at rather than the output that has just arrived, and so a scroll
    // whose event has not been dispatched yet is still seen in time.
    following = atBottom();
    switch (message.k) {
      case "state": {
        entries.clear();
        transcript.innerHTML = "";
        transcript.appendChild(empty);
        empty.hidden = message.items.length > 0;
        transcript.classList.toggle("hide-thinking", message.showThinking === false);
        folderLabel.textContent = message.title || "New chat";
        modelLabel.textContent = message.model ? `${message.model}` : "";
        for (const item of message.items) appendItem(item, false);
        setStatus(message.status, message.busy, message.queued);
        setFooter(message.footer);
        setChips(message.context, message.attachments);
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
      case "approval": {
        const entry = entries.get(message.id);
        if (!entry) return;
        entry.item.state = message.state;
        entry.item.label = message.label;
        paintApproval(entry);
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
        setChips(message.context, message.attachments);
        return;
      default:
        return;
    }
  }

  // ---------- footer ----------

  function setStatus(text, isBusy, queuedCount) {
    busy = Boolean(isBusy);
    queued = queuedCount || 0;
    const label = queued > 0 ? `${text} · ${queued} queued` : text;
    // A running turn keeps its live spinner; an idle one is a quiet dot.
    statusLabel.textContent = label;
    statusLabel.classList.toggle("busy", busy);
    statusLabel.title = busy ? "Oxide is working" : "Ready for the next message";
    stopButton.hidden = !busy;
    sendButton.title = busy ? "Queue for after this turn (Alt+Enter)" : "Send (Enter)";
    sendButton.setAttribute("aria-label", busy ? "Queue" : "Send");
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

  /// The footer the extension host composes: the row of chips above the
  /// composer (`model: … · thinking: … · access: … · session: …`), the usage
  /// line, the branch and the context gauge. Everything is a string by the time
  /// it gets here, so this function only paints.
  function setFooter(footer) {
    if (!footer) return;
    metaBox.innerHTML = "";
    for (const chip of footer.chips || []) {
      metaBox.appendChild(chipNode(chip.label, chip.id, chip.title));
    }
    const info = footer.info || "";
    branchLabel.textContent = info;
    branchLabel.title = info ? `Current branch: ${info}` : "";
    usageText.textContent = footer.usage || "";
    usageText.title = footer.usage || "";
    usageRow.hidden = !footer.usage;
    const percent = typeof footer.percent === "number" ? footer.percent : null;
    gauge.hidden = percent === null;
    gauge.dataset.level = footer.level || "ok";
    gauge.title = percent === null ? "" : `${percent}% of the context window used`;
    gaugeFill.style.width = percent === null ? "0%" : `${Math.min(percent, 100)}%`;
  }

  /// One clickable chip. The host's label reads `model: glm-5 · 128.0k`, so the
  /// key is split off and painted quietly: the same string the terminal shows,
  /// with a value that a glance can find.
  function chipNode(label, control, title) {
    const el = document.createElement("button");
    el.type = "button";
    el.className = "meta-chip";
    el.dataset.control = control;
    const at = String(label).indexOf(": ");
    if (at > 0) {
      const key = document.createElement("span");
      key.className = "chip-key";
      key.textContent = String(label).slice(0, at);
      const value = document.createElement("span");
      value.className = "chip-value";
      value.textContent = String(label).slice(at + 2);
      el.append(key, value);
    } else {
      el.textContent = String(label);
    }
    el.title = title || `${label} — click to change`;
    return el;
  }

  function setChips(next, nextAttachments) {
    chips = next || [];
    attachments = nextAttachments || [];
    chipBox.innerHTML = "";
    chipBox.hidden = chips.length + attachments.length === 0;
    for (const attachment of attachments) chipBox.appendChild(attachmentNode(attachment));
    for (const chip of chips) chipBox.appendChild(contextNode(chip));
    if (chips.length + attachments.length > 1) {
      const clear = document.createElement("button");
      clear.type = "button";
      clear.className = "chip-clear";
      clear.textContent = "Clear";
      clear.title = "Remove everything pending";
      clear.addEventListener("click", () => vscode.postMessage({ k: "clearChips" }));
      chipBox.appendChild(clear);
    }
    updateSendState();
  }

  /// An image or PDF the next message will carry: a thumbnail where the host
  /// could send one, a glyph and the size where it could not.
  function attachmentNode(attachment) {
    const el = document.createElement("div");
    el.className = "chip attachment";
    if (attachment.kind === "image" && attachment.preview) {
      const image = document.createElement("img");
      image.src = attachment.preview;
      image.alt = attachment.label;
      el.appendChild(image);
    } else {
      const glyph = document.createElement("span");
      glyph.className = "chip-glyph";
      glyph.textContent = attachment.kind === "pdf" ? "▤" : "▣";
      el.appendChild(glyph);
    }
    const text = document.createElement("span");
    text.className = "chip-text";
    const name = document.createElement("span");
    name.className = "chip-name";
    name.textContent = attachment.label;
    const detail = document.createElement("span");
    detail.className = "chip-detail";
    detail.textContent = attachment.detail || "";
    text.append(name, detail);
    el.append(text, removeNode(attachment));
    el.title = `${attachment.label}${attachment.detail ? ` · ${attachment.detail}` : ""}`;
    return el;
  }

  function contextNode(chip) {
    const el = document.createElement("div");
    el.className = "chip context";
    const glyph = document.createElement("span");
    glyph.className = "chip-glyph";
    glyph.textContent = "❮❯";
    const name = document.createElement("span");
    name.className = "chip-name";
    name.textContent = chip.label;
    el.append(glyph, name, removeNode(chip));
    el.title = `${chip.label} — inlined into the next message`;
    return el;
  }

  function removeNode(chip) {
    const remove = document.createElement("button");
    remove.type = "button";
    remove.className = "chip-remove";
    remove.textContent = "✕";
    remove.title = "Remove from the next message";
    remove.addEventListener("click", () => vscode.postMessage({ k: "removeChip", id: chip.id }));
    return remove;
  }

  function updateSendState() {
    const pending = chips.length + attachments.length > 0;
    sendButton.disabled = busy ? false : !input.value.trim() && !pending;
  }

  // ---------- composer ----------

  /// The composer starts two rows tall (the host's `<textarea rows="2">`) and
  /// grows with the message, up to a point where scrolling takes over.
  const MAX_INPUT_HEIGHT = 200;

  function resizeInput() {
    input.style.height = "auto";
    input.style.height = `${Math.min(input.scrollHeight, MAX_INPUT_HEIGHT)}px`;
    input.style.overflowY = input.scrollHeight > MAX_INPUT_HEIGHT ? "auto" : "hidden";
  }

  function submit() {
    const text = input.value;
    if (!text.trim() && chips.length + attachments.length === 0) return;
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

  // ---------- attachments ----------

  /// The longest edge an image is downscaled to before it is sent on, matching
  /// `oxide_core::media` (and the desktop composer's canvas resize).
  const MAX_IMAGE_EDGE = 1568;
  /// What the host accepts, so a pasted blob is refused here rather than
  /// written to a file first.
  const ATTACHABLE = ["image/png", "image/jpeg", "image/gif", "image/webp", "image/bmp", "application/pdf"];

  function readAsDataUrl(file) {
    return new Promise((resolve, reject) => {
      const reader = new FileReader();
      reader.onload = () => resolve(String(reader.result || ""));
      reader.onerror = () => reject(reader.error || new Error("read failed"));
      reader.readAsDataURL(file);
    });
  }

  function loadImage(dataUrl) {
    return new Promise((resolve, reject) => {
      const image = new Image();
      image.onload = () => resolve(image);
      image.onerror = () => reject(new Error("could not decode image"));
      image.src = dataUrl;
    });
  }

  /// Downscales a pasted screenshot on a canvas, so a retina screenshot does not
  /// travel to the host at full resolution. The original is kept when it is
  /// already small, is a GIF (which would lose its animation) or cannot be
  /// decoded.
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
      return resized && resized.startsWith("data:image/") ? resized : dataUrl;
    } catch (error) {
      return dataUrl;
    }
  }

  /// A pasted or dropped blob. It is sent to the host as a data URL, which
  /// writes it to a file the CLI can read (`--image <path>`).
  async function attachBlob(file) {
    const mime = file.type;
    if (!ATTACHABLE.includes(mime)) {
      vscode.postMessage({
        k: "notice",
        text: "Only images and PDFs can be attached from a paste or a drop.",
      });
      return;
    }
    try {
      let dataUrl = await readAsDataUrl(file);
      if (mime.startsWith("image/")) dataUrl = await resizeImageDataUrl(dataUrl, mime);
      vscode.postMessage({ k: "attach", name: file.name, data: dataUrl });
    } catch (error) {
      vscode.postMessage({
        k: "notice",
        text: `Could not read ${file.name || "the attachment"}.`,
      });
    }
  }

  /// A drop carries real paths when the webview can see them, which is what the
  /// host prefers: a file on disk needs no copy and its text can be inlined.
  /// A blob is the fallback, so an image dragged in from outside still lands.
  function attachDataTransfer(transfer) {
    const files = Array.from((transfer && transfer.files) || []);
    const paths = [];
    const blobs = [];
    for (const file of files) {
      if (file.path) paths.push(String(file.path));
      else blobs.push(file);
    }
    if (paths.length) vscode.postMessage({ k: "attachFiles", paths });
    for (const file of blobs) void attachBlob(file);
  }

  attachButton.addEventListener("click", () => vscode.postMessage({ k: "pickFiles" }));

  // A pasted screenshot becomes an attachment; a text paste keeps its default
  // behavior, because a message is usually what the clipboard holds.
  input.addEventListener("paste", (event) => {
    const items = Array.from((event.clipboardData && event.clipboardData.items) || []);
    const files = items
      .filter((item) => item.kind === "file")
      .map((item) => item.getAsFile())
      .filter(Boolean);
    if (!files.length) return;
    event.preventDefault();
    for (const file of files) void attachBlob(file);
  });

  function endDrag() {
    composer.classList.remove("dragging");
    dropHint.hidden = true;
  }
  document.addEventListener("dragenter", (event) => {
    if (!event.dataTransfer || !Array.from(event.dataTransfer.types || []).includes("Files")) return;
    composer.classList.add("dragging");
    dropHint.hidden = false;
  });
  document.addEventListener("dragover", (event) => {
    if (composer.classList.contains("dragging")) event.preventDefault();
  });
  document.addEventListener("dragleave", (event) => {
    // A drag crossing a child element fires `dragleave` too, so the overlay is
    // only dropped once the pointer has left the document.
    if (!event.relatedTarget) endDrag();
  });
  document.addEventListener("dragend", endDrag);
  document.addEventListener("drop", (event) => {
    endDrag();
    if (!event.dataTransfer) return;
    event.preventDefault();
    attachDataTransfer(event.dataTransfer);
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

  transcript.addEventListener("scroll", () => {
    following = atBottom();
  });

  // The pane gets shorter when the composer grows — an attachment chip arrives,
  // the message wraps to another line, the usage line below wraps — which pushes
  // the newest line under the fold with nothing left to bring it back. A reader
  // who was at the bottom is put back on it.
  if (typeof ResizeObserver === "function") {
    new ResizeObserver(() => {
      if (following) transcript.scrollTop = transcript.scrollHeight;
    }).observe(transcript);
  }

  window.addEventListener("message", (event) => apply(event.data));
  resizeInput();
  updateSendState();
  vscode.postMessage({ k: "ready" });
})();
