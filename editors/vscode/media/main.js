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
  const atBox = $("at");
  const branchLabel = $("branch");
  const attachButton = $("attach");
  const composer = $("composer");
  const dropHint = $("dropzone");
  const titleLabel = $("title");
  const dialogBox = $("dialog");
  const dialogTitle = $("dialog-title");
  const dialogSub = $("dialog-sub");
  const dialogNote = $("dialog-note");
  const dialogList = $("dialog-list");
  const dialogRefresh = $("dialog-refresh");
  const dialogClose = $("dialog-close");
  const imageView = $("image-view");
  const imageViewImage = $("image-view-img");
  const imageViewClose = $("image-view-close");

  const entries = new Map();
  let chips = [];
  let attachments = [];
  /// Chip thumbnails by attachment id: the downscaled copy the view paints,
  /// and the `<img>` a pending downscale writes it into.
  const thumbnails = new Map();
  const thumbnailNodes = new Map();
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

  // ---------- questions ----------

  /// A question the model asked through the `ask` tool: the agent is holding the
  /// turn until it is answered, which is what a skill does when it needs a
  /// choice only the user can make. Each question offers its own options —
  /// radios for one answer, boxes for several — and a field for an answer in the
  /// user's own words, so a free-text question and a choice are the same card.
  /// The answers travel to the host, which forwards them over the CLI's request
  /// channel; the card is repainted from the `k: "question"` update it sends
  /// back.
  function questionNode(item) {
    const wrap = document.createElement("div");
    wrap.className = "question";
    const head = document.createElement("div");
    head.className = "qhead";
    const mark = document.createElement("span");
    mark.className = "qmark";
    mark.textContent = "?";
    const title = document.createElement("span");
    title.className = "qtitle";
    title.textContent = String(item.title || "Question");
    head.append(mark, title);
    const body = document.createElement("div");
    body.className = "qbody";
    const actions = document.createElement("div");
    actions.className = "qactions";
    wrap.append(head, body, actions);
    return { el: wrap, body, actions };
  }

  /// One question's own block: its text, its options, and the field for an
  /// answer of the user's own wording. A single-select question preselects its
  /// first option, so an answer can never be blank by accident.
  function questionBlock(question, index) {
    const block = document.createElement("div");
    block.className = "qblock";
    if (question.header) {
      const header = document.createElement("div");
      header.className = "qheader";
      header.textContent = question.header;
      block.appendChild(header);
    }
    const text = document.createElement("p");
    text.className = "qtext";
    text.textContent = String(question.question || "");
    block.appendChild(text);
    const options = Array.isArray(question.options) ? question.options : [];
    options.forEach((option, position) => {
      const row = document.createElement("label");
      row.className = "qoption";
      const input = document.createElement("input");
      input.type = question.multiSelect ? "checkbox" : "radio";
      input.name = `question-${index}`;
      input.value = String(option.label || "");
      input.dataset.question = String(index);
      // One class for both widgets: the input's own type already says whether
      // the answer is the checked radio or the checked boxes.
      input.className = "qchoice";
      const label = document.createElement("span");
      label.className = "qlabel";
      label.textContent = String(option.label || "");
      row.append(input, label);
      if (option.description) {
        const description = document.createElement("span");
        description.className = "qdetail";
        description.textContent = option.description;
        row.appendChild(description);
      }
      if (!question.multiSelect && position === 0) input.checked = true;
      block.appendChild(row);
    });
    const free = document.createElement("input");
    free.className = "qfree";
    free.type = "text";
    free.dataset.question = String(index);
    free.placeholder = options.length ? "Or type an answer…" : "Type an answer…";
    block.appendChild(free);
    return block;
  }

  /// What was ticked and typed, one entry per question, each echoing the
  /// question it belongs to so the model reads the answers in the terms it asked
  /// them in.
  function collectAnswers(entry) {
    return (entry.item.questions || []).map((question, index) => {
      const values = [];
      entry.body
        .querySelectorAll(`.qchoice[data-question="${index}"]:checked`)
        .forEach((input) => values.push(input.value));
      const typed = entry.body.querySelector(`.qfree[data-question="${index}"]`);
      const text = typed ? String(typed.value || "").trim() : "";
      if (text) values.push(text);
      return { question: question.question, values };
    });
  }

  /// Paints a card from its item: a waiting one offers the fields, and an
  /// answered one keeps only what it was answered with.
  function paintQuestion(entry) {
    const item = entry.item;
    const waiting = item.state === "pending";
    entry.el.classList.toggle("waiting", waiting);
    entry.el.classList.toggle("answered", item.state === "answered");
    entry.body.textContent = "";
    entry.actions.textContent = "";
    if (!waiting) {
      const done = document.createElement("span");
      done.className = "qdone";
      done.textContent = item.label || "Answered";
      entry.actions.appendChild(done);
      return;
    }
    for (const [index, question] of (item.questions || []).entries()) {
      entry.body.appendChild(questionBlock(question, index));
    }
    const answer = document.createElement("button");
    answer.type = "button";
    answer.className = "primary";
    answer.textContent = "Answer";
    answer.title = "Send these answers to the agent";
    answer.addEventListener("click", () =>
      vscode.postMessage({
        k: "question",
        requestId: item.requestId,
        answers: collectAnswers(entry),
      }),
    );
    // Skip is a dismissal, not a set of blank answers: the CLI tells the model
    // nobody answered, so it continues with a default instead of waiting.
    const skip = document.createElement("button");
    skip.type = "button";
    skip.className = "ghost";
    skip.textContent = "Skip";
    skip.title = "Answer nothing and let the agent continue with its own default";
    skip.addEventListener("click", () =>
      vscode.postMessage({ k: "question", requestId: item.requestId, answers: [] }),
    );
    entry.actions.append(answer, skip);
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
    if (item.kind === "question") return questionNode(item);
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
    if (isConversation(item)) empty.hidden = true;
    if (item.kind === "assistant") renderAssistant(entry);
    else if (item.kind === "thinking") entry.el.textContent = item.text;
    else if (item.kind === "tool") paintTool(entry);
    else if (item.kind === "approval") paintApproval(entry);
    else if (item.kind === "question") paintQuestion(entry);
    scrollDown(keepScroll);
    return entry;
  }

  /// Whether an item is a message rather than a line about the page. The
  /// welcome block is the new-chat page, and a notice (a closed thread, a
  /// folder that was not opened, a listing that failed) is painted on it: it is
  /// still the page until something is actually said in it.
  function isConversation(item) {
    return item.kind !== "notice";
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
        empty.hidden = message.items.some(isConversation);
        transcript.classList.toggle("hide-thinking", message.showThinking === false);
        titleLabel.textContent = message.title || "New chat";
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
      case "question": {
        const entry = entries.get(message.id);
        if (!entry) return;
        entry.item.state = message.state;
        entry.item.label = message.label;
        paintQuestion(entry);
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
      case "dialog":
        setDialog(message.dialog);
        return;
      case "atSuggestions":
        applyCompletion(message);
        return;
      case "paletteRows":
        applyCompletion(message);
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
    // The phase is only worth a line while there is one: the TUI reserves its
    // status row for a running turn too, and an idle panel says nothing rather
    // than sitting on a dot that reads as a button.
    statusLabel.hidden = !busy && queued === 0;
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
    const live = new Set(attachments.map((attachment) => String(attachment.id)));
    for (const key of [...thumbnails.keys()]) {
      if (!live.has(key)) thumbnails.delete(key);
    }
    for (const key of [...thumbnailNodes.keys()]) {
      if (!live.has(key)) thumbnailNodes.delete(key);
    }
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

  /// Thumbnails already being drawn, by attachment id, so a burst of repaints
  /// is still one decode rather than one per paint.
  const pending = new Set();

  /// The thumbnail a chip paints, by attachment id. The box is rebuilt with
  /// every state message, and a photo-sized preview decoded over and over is
  /// what an out-of-memory crash looks like: the first paint shows what it was
  /// handed while a chip-sized copy is drawn in the background, and every paint
  /// after that uses the small copy.
  function thumbnailFor(attachment) {
    const key = String(attachment.id);
    const cached = thumbnails.get(key);
    if (cached) return cached;
    if (attachment.preview && !pending.has(key)) {
      pending.add(key);
      void downscaleThumbnail(key, attachment.preview);
    }
    return attachment.preview || "";
  }

  async function downscaleThumbnail(key, preview) {
    try {
      const image = await loadImage(preview);
      const longest = Math.max(image.width, image.height);
      if (!longest) return;
      const scale = longest > THUMBNAIL_EDGE ? THUMBNAIL_EDGE / longest : 1;
      const canvas = document.createElement("canvas");
      canvas.width = Math.max(1, Math.round(image.width * scale));
      canvas.height = Math.max(1, Math.round(image.height * scale));
      canvas.getContext("2d").drawImage(image, 0, 0, canvas.width, canvas.height);
      const small = canvas.toDataURL("image/png");
      if (!small || !small.startsWith("data:image/")) return;
      thumbnails.set(key, small);
      const node = thumbnailNodes.get(key);
      if (node) node.src = small;
    } catch (error) {
      // A preview the canvas cannot draw stays as it came.
    } finally {
      pending.delete(key);
    }
  }

  /// An image or PDF the next message will carry: a thumbnail where the host
  /// could send one, a glyph and the size where it could not. An image's
  /// thumbnail opens the full-size one — the panel is narrow and the chip is
  /// small, so the copy on it is not much of a look at what is being sent.
  function attachmentNode(attachment) {
    const el = document.createElement("div");
    el.className = "chip attachment";
    if (attachment.kind === "image" && attachment.preview) {
      const open = document.createElement("button");
      open.type = "button";
      open.className = "chip-open";
      open.title = "Open the full-size image";
      const image = document.createElement("img");
      image.src = thumbnailFor(attachment);
      image.alt = attachment.label;
      thumbnailNodes.set(String(attachment.id), image);
      open.appendChild(image);
      open.addEventListener("click", () => openImage(attachment.preview, attachment.label));
      el.appendChild(open);
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

  /// A chip in the strip above the composer. The file the editor has open is
  /// dashed rather than solid and carries its own glyph: it is tracked, not
  /// attached, and it comes back when another file is opened.
  function contextNode(chip) {
    const el = document.createElement("div");
    el.className = chip.auto ? "chip context auto" : "chip context";
    const glyph = document.createElement("span");
    glyph.className = "chip-glyph";
    glyph.textContent = chip.auto ? "✎" : "❮❯";
    const name = document.createElement("span");
    name.className = "chip-name";
    name.textContent = chip.label;
    el.append(glyph, name, removeNode(chip));
    el.title = chip.auto
      ? `${chip.label} — the file you are editing, sent with the next message`
      : `${chip.label} — inlined into the next message`;
    return el;
  }

  function removeNode(chip) {
    const remove = document.createElement("button");
    remove.type = "button";
    remove.className = "chip-remove";
    remove.textContent = "✕";
    remove.title = chip.auto
      ? "Take this file out of the next message"
      : "Remove from the next message";
    remove.addEventListener("click", () => vscode.postMessage({ k: "removeChip", id: chip.id }));
    return remove;
  }

  /// What the composer is holding of its own. The chip for the file the editor
  /// has open rides along with whatever is sent, but it is not something to
  /// send on its own, so an empty box that only carries it stays empty.
  function pendingCount() {
    return attachments.length + chips.filter((chip) => !chip.auto).length;
  }

  function updateSendState() {
    sendButton.disabled = busy ? false : !input.value.trim() && pendingCount() === 0;
  }

  // ---------- dialogs ----------

  /// The icons a dialog's own rows need: the power switch beside a server and
  /// the trash at the end of a thread, neither of which a character renders the
  /// same way everywhere. The header's Recheck and Close come from the shell,
  /// and every other row is words.
  const POWER_ICON =
    '<svg viewBox="0 0 24 24" width="14" height="14" aria-hidden="true"><path d="M18.36 6.64a9 9 0 1 1-12.73 0" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round"/><path d="M12 2v10" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round"/></svg>';
  const TRASH_ICON =
    '<svg viewBox="0 0 24 24" width="14" height="14" aria-hidden="true"><path d="M4 7h16M10 11v6M14 11v6M6 7l1 13h10l1-13M9 7V4h6v3" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"/></svg>';
  const DIALOG_ICONS = { power: POWER_ICON, trash: TRASH_ICON };

  /// The dialog the host composes (`src/core/dialogs.ts`): the MCP server list
  /// and the session history, painted here rather than in a QuickPick — which
  /// takes over the window, hides the transcript the listing is about, and
  /// cannot be answered while a turn streams. It is one element either way
  /// (`#dialog` sits under the header, ahead of the transcript, in the panel's
  /// own flow) and the host says which end it belongs to: `.pin-footer` moves
  /// it above the footer with CSS `order`, so the MCP list grows up from the
  /// composer block the `/mcps` was typed in while the session list keeps
  /// dropping from the header it is about. A row arrives with the action it
  /// posts, so the view decides nothing about what a click means.
  function setDialog(dialog) {
    dialogList.innerHTML = "";
    if (!dialog) {
      // The class is dropped with it: a hidden element's siblings would still be
      // ordered around it, and the next listing says which end it wants.
      dialogBox.classList.remove("pin-footer");
      dialogBox.hidden = true;
      return;
    }
    dialogBox.classList.toggle("pin-footer", dialog.pin === "footer");
    dialogTitle.textContent = dialog.title || "";
    dialogSub.textContent = dialog.subtitle || "";
    dialogSub.hidden = !dialog.subtitle;
    dialogNote.textContent = dialog.note || "";
    dialogNote.hidden = !dialog.note;
    for (const entry of dialog.rows || []) dialogList.appendChild(dialogRow(entry));
    // The button is an icon with the host's words for a tooltip and for a
    // screen reader, so the shell's own markup is left alone.
    dialogRefresh.hidden = !dialog.refreshLabel;
    dialogRefresh.title = dialog.refreshLabel || "Recheck";
    dialogRefresh.setAttribute("aria-label", dialogRefresh.title);
    dialogRefresh.dataset.action = dialog.refreshAction || "";
    dialogBox.hidden = false;
  }

  /// One row: what it is, what it resolves to underneath, a trailing status word
  /// where one applies, and a button for the rows that turn something over
  /// instead of opening it.
  function dialogRow(entry) {
    const el = document.createElement(entry.action ? "button" : "div");
    el.className = "dialog-row";
    if (entry.action) {
      el.type = "button";
      el.dataset.action = entry.action;
      el.dataset.value = entry.value;
    }
    const main = document.createElement("div");
    main.className = "dialog-main";
    const label = document.createElement("div");
    label.className = "dialog-label";
    label.textContent = entry.label;
    main.appendChild(label);
    if (entry.detail) {
      const detail = document.createElement("div");
      detail.className = "dialog-detail";
      detail.textContent = entry.detail;
      main.appendChild(detail);
    }
    el.appendChild(main);
    if (entry.status) {
      const status = document.createElement("span");
      status.className = `dialog-status tone-${entry.tone || "muted"}`;
      status.textContent = entry.status;
      el.appendChild(status);
    }
    if (entry.button) {
      const button = document.createElement("button");
      button.type = "button";
      // A button at the row's end is icon-only: it is sized for the glyph it
      // carries, and the words the host sent become its tooltip.
      button.className = entry.icon ? `dialog-button icon-${entry.icon}` : "dialog-button";
      button.dataset.action = entry.buttonAction;
      button.dataset.value = entry.value;
      if (entry.icon) {
        button.innerHTML = DIALOG_ICONS[entry.icon] || "";
        button.classList.add(`tone-${entry.tone || "muted"}`);
      } else {
        button.textContent = entry.button;
      }
      button.title = entry.button;
      button.setAttribute("aria-label", entry.button);
      el.appendChild(button);
    }
    el.title = entry.detail ? `${entry.label} — ${entry.detail}` : entry.label;
    return el;
  }

  /// Closing is the host's decision, so it hears about it too: the dialog is the
  /// controller's state, and whichever pane is attached next would otherwise
  /// paint it again.
  function closeDialog() {
    setDialog(null);
    vscode.postMessage({ k: "dialogAction", action: "dialogClose", value: "" });
  }

  /// A click inside a dialog: the nearest element carrying an action posts it
  /// back — a row, or the power switch at its end. Nothing else dismisses the
  /// dialog, which is why the panel behind it stays readable: Close and Escape
  /// are how it goes away.
  dialogBox.addEventListener("click", (event) => {
    const target = event.target;
    if (!(target instanceof Element)) return;
    const action = target.closest("[data-action]");
    if (!action) return;
    vscode.postMessage({
      k: "dialogAction",
      action: action.dataset.action,
      value: action.dataset.value || "",
    });
  });

  dialogClose.addEventListener("click", () => closeDialog());

  // ---------- image preview ----------

  /// The full-size image behind a chip's thumbnail: a chip only has room for a
  /// 96px copy, so a click opens what the host actually sent.
  function openImage(preview, label) {
    if (!preview) return;
    imageViewImage.src = preview;
    imageViewImage.alt = label || "Attachment preview";
    imageView.hidden = false;
  }

  imageViewClose.addEventListener("click", () => {
    imageView.hidden = true;
  });

  imageView.addEventListener("click", (event) => {
    if (event.target === imageView) imageView.hidden = true;
  });

  // Escape closes what is on top — the image first, then the dialog. The
  // composer's own Escape (stop the running turn) is handled on the textarea.
  document.addEventListener("keydown", (event) => {
    if (event.key !== "Escape") return;
    if (!imageView.hidden) {
      imageView.hidden = true;
      return;
    }
    if (!dialogBox.hidden) closeDialog();
  });

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
    if (!text.trim() && pendingCount() === 0) return;
    input.value = "";
    closeCompletion();
    resizeInput();
    updateSendState();
    vscode.postMessage({ k: "send", text });
  }

  // ---------- `@path` completion and the `/` palette ----------

  /// The rows the host offered, the range of the value they replace, which row
  /// is highlighted, and the sequence number the answer belongs to. The host
  /// decides what is being completed and composes the rows (`core/at.ts` for a
  /// reference, `core/palette.ts` for a slash command); the view splices in the
  /// row that was taken and never reads a token itself, so a reference and a
  /// command both complete the way the terminal and the desktop app complete
  /// them. `kind` is what the answer was for: `path` rows replace an `@path`
  /// token, `command` rows replace the whole value with `/name`.
  let rows = [];
  let rowKind = "path";
  let rowStart = 0;
  let rowEnd = 0;
  let rowIndex = 0;
  let rowSeq = 0;

  function closeCompletion() {
    atBox.hidden = true;
    atBox.innerHTML = "";
    rows = [];
    rowIndex = 0;
    // Closing settles the question that was open, so an answer still on its way
    // (a walk of the project is not instant) cannot pop the list back up after
    // Escape, or over a box that has been sent and emptied.
    rowSeq += 1;
  }

  /// Asks the host what the composer is completing: a `/name` while a slash
  /// command is being typed, otherwise an `@path` the caret is sitting in.
  /// Nothing is asked of a value with neither, which is most of them.
  function requestCompletion() {
    if (input.value.startsWith("/")) {
      rowSeq += 1;
      vscode.postMessage({ k: "completePalette", text: input.value, seq: rowSeq });
      return;
    }
    if (input.value.indexOf("@") === -1) {
      closeCompletion();
      return;
    }
    rowSeq += 1;
    vscode.postMessage({
      k: "completeAt",
      text: input.value,
      caret: input.selectionStart || 0,
      seq: rowSeq,
    });
  }

  function rowNode(row, index) {
    return rowKind === "command" ? commandRowNode(row, index) : atRowNode(row, index);
  }

  function atRowNode(row, index) {
    const el = document.createElement("button");
    el.type = "button";
    el.className = index === rowIndex ? "at-row selected" : "at-row";
    el.setAttribute("role", "option");
    el.setAttribute("aria-selected", index === rowIndex ? "true" : "false");
    el.title =
      row.kind === "folder"
        ? `${row.label} — a folder in this project`
        : `${row.label} — inlined into the next message`;
    const glyph = document.createElement("span");
    glyph.className = "at-glyph";
    glyph.textContent = row.kind === "folder" ? "\u25b8" : "\u00b7";
    const name = document.createElement("span");
    name.className = "at-path";
    name.textContent = row.label;
    el.append(glyph, name);
    addRowHandlers(el, index);
    return el;
  }

  /// One row of the `/` palette: the name it inserts, the arguments it takes,
  /// what it does and where it came from. A skill is listed under its own name,
  /// with its description, so taking the row is what loads it.
  function commandRowNode(row, index) {
    const el = document.createElement("button");
    el.type = "button";
    el.className = index === rowIndex ? "cmd-row selected" : "cmd-row";
    el.setAttribute("role", "option");
    el.setAttribute("aria-selected", index === rowIndex ? "true" : "false");
    el.title = `${row.insert.trim()} — ${row.description}`;
    const name = document.createElement("span");
    name.className = "cmd-name";
    name.textContent = `/${row.name}`;
    el.appendChild(name);
    if (row.arguments) {
      const args = document.createElement("span");
      args.className = "cmd-args";
      args.textContent = row.arguments;
      el.appendChild(args);
    }
    const description = document.createElement("span");
    description.className = "cmd-desc";
    description.textContent = row.description;
    el.appendChild(description);
    // A skill's own kind is worth saying: it is loaded as instructions rather
    // than run as a command, and the CLI keeps the two apart.
    const source = document.createElement("span");
    source.className = "cmd-source";
    source.textContent = row.kind === "skill" ? "skill" : row.source;
    el.appendChild(source);
    addRowHandlers(el, index);
    return el;
  }

  /// The click would take the focus out of the message box and drop the caret
  /// the row is completing, so the press is swallowed and the click still
  /// arrives.
  function addRowHandlers(el, index) {
    el.addEventListener("mousedown", (event) => event.preventDefault());
    el.addEventListener("click", () => acceptRow(index));
  }

  function paintCompletion() {
    atBox.innerHTML = "";
    atBox.hidden = rows.length === 0;
    atBox.setAttribute(
      "aria-label",
      rowKind === "command" ? "Commands and skills" : "Files and folders",
    );
    rows.forEach((row, index) => atBox.appendChild(rowNode(row, index)));
  }

  /// Takes a row. An `@path` is replaced by what the host said it stands for —
  /// with a space after a file so the next word can be typed and without one
  /// after a folder, so the query goes on narrowing inside it — while a command
  /// replaces the whole value, since that is the message the CLI resolves.
  function acceptRow(index) {
    const row = rows[index];
    if (!row) return;
    const value = input.value;
    input.value = value.slice(0, rowStart) + row.insert + value.slice(rowEnd);
    const caret = rowStart + row.insert.length;
    input.setSelectionRange(caret, caret);
    const command = rowKind === "command";
    closeCompletion();
    resizeInput();
    updateSendState();
    // A folder opens its own list; a file's token is done, and the host answers
    // that with no rows. A command is left in the box to be sent — and to take
    // arguments, which is why its row is completed rather than run.
    if (!command) requestCompletion();
  }

  function moveRow(step) {
    if (!rows.length) return;
    rowIndex = (rowIndex + step + rows.length) % rows.length;
    paintCompletion();
    // Only the row is brought into view, and only if it is not already: the
    // list is its own scroller, so walking it must not drag the transcript.
    const selected = atBox.children[rowIndex];
    if (selected && selected.scrollIntoView) {
      selected.scrollIntoView({ block: "nearest", inline: "nearest" });
    }
  }

  function applyCompletion(message) {
    // A list for a value the reader has already typed past would put its rows
    // under a caret that has moved, so the sequence number settles it.
    if (message.seq !== rowSeq) return;
    if (!message.rows || !message.rows.length) {
      closeCompletion();
      return;
    }
    rowKind = message.kind === "command" ? "command" : "path";
    rowStart = message.start;
    rowEnd = message.end;
    rows = message.rows;
    rowIndex = 0;
    paintCompletion();
  }

  /// The keys that move the caret without changing the value, so a token the
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

  input.addEventListener("input", () => {
    resizeInput();
    updateSendState();
    requestCompletion();
  });

  // The caret moves without the value changing, which a token the caret has
  // been moved into needs. While the list is up the arrows walk it instead, so
  // the click is what re-asks from there.
  input.addEventListener("keyup", (event) => {
    if (atBox.hidden && CARET_KEYS.has(event.key)) requestCompletion();
  });
  input.addEventListener("click", () => requestCompletion());

  input.addEventListener("keydown", (event) => {
    if (!atBox.hidden) {
      if (event.key === "ArrowDown") {
        event.preventDefault();
        moveRow(1);
        return;
      }
      if (event.key === "ArrowUp") {
        event.preventDefault();
        moveRow(-1);
        return;
      }
      if (event.key === "Tab" || (event.key === "Enter" && !event.shiftKey)) {
        event.preventDefault();
        acceptRow(rowIndex);
        return;
      }
      if (event.key === "Escape") {
        event.preventDefault();
        closeCompletion();
        return;
      }
    }
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
  /// The core's own limit (`oxide_core::media::MAX_ATTACHMENT_BYTES`), checked
  /// before the blob is read: a data URL for a 200 MB file is a 270 MB string
  /// in this document, and the CLI would refuse it at the far end anyway.
  const MAX_ATTACHMENT_BYTES = 20 * 1024 * 1024;
  /// How small a chip's thumbnail is drawn, and the size of the copy kept for
  /// the repaints that follow.
  const THUMBNAIL_EDGE = 96;

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
    if (file.size > MAX_ATTACHMENT_BYTES) {
      vscode.postMessage({
        k: "notice",
        text: `${file.name || "That file"} is past the ${MAX_ATTACHMENT_BYTES / (1024 * 1024)} MB attachment limit.`,
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
