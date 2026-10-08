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

  // Inline, currentColor icons keep the composer independent of VS Code's icon
  // font while still following every light, dark and high-contrast theme.
  const CONTROL_ICONS = {
    model: '<svg viewBox="0 0 16 16" aria-hidden="true"><path d="m8 1.8 5.5 3.1v6.2L8 14.2l-5.5-3.1V4.9L8 1.8Z"/><path d="m2.8 5 5.2 3 5.2-3M8 8v5.8"/></svg>',
    reasoning: '<svg viewBox="0 0 16 16" aria-hidden="true"><path d="M8 1.7 9 5l3.3 1L9 7l-1 3.3L7 7 3.7 6 7 5l1-3.3ZM12.7 9.3l.6 1.8 1.7.6-1.7.6-.6 1.7-.6-1.7-1.8-.6 1.8-.6.6-1.8ZM3.5 10.2l.5 1.3 1.3.5-1.3.5-.5 1.3-.5-1.3-1.3-.5 1.3-.5.5-1.3Z"/></svg>',
    agent: '<svg viewBox="0 0 16 16" aria-hidden="true"><circle cx="8" cy="5.2" r="2.5"/><path d="M3.2 13.8c.4-3 2-4.4 4.8-4.4s4.4 1.4 4.8 4.4"/></svg>',
    access: '<svg viewBox="0 0 16 16" aria-hidden="true"><path d="M8 1.7c1.7 1.2 3.3 1.7 5 1.8v3.8c0 3.2-1.7 5.5-5 7-3.3-1.5-5-3.8-5-7V3.5c1.7-.1 3.3-.6 5-1.8Z"/><path d="m5.6 8 1.5 1.5 3.5-3.5"/></svg>',
  };

  const transcript = $("transcript");
  const empty = $("empty");
  const emptyLead = $("empty-lead");
  const suggestions = $("empty-suggestions");
  const input = $("input");

  /// What the home state offers a reader with nothing to say yet, each filling
  /// the composer rather than sending: the desktop app's own four, in its own
  /// words, so a fresh panel reads the same in both front-ends.
  const SUGGESTIONS = [
    "Explain this codebase and its architecture.",
    "Find and fix the highest-priority bug in this repository.",
    "Add tests for the most important untested code path.",
    "Review the working tree changes and summarize the risks.",
  ];  const sendButton = $("send");
  const stopButton = $("stop");
  const modeBar = $("mode-bar");
  const modeQueue = $("mode-queue");
  const modeSteer = $("mode-steer");
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
  const branchWrap = $("branch-wrap");
  const attachButton = $("attach");
  const composer = $("composer");
  const dropHint = $("dropzone");
  const titleLabel = $("title");
  const dialogBox = $("dialog");
  const dialogTitle = $("dialog-title");
  const dialogSub = $("dialog-sub");
  const dialogNote = $("dialog-note");
  const dialogList = $("dialog-list");
  const dialogCount = $("dialog-count");
  const dialogSearch = $("dialog-search");
  const dialogSearchInput = $("dialog-search-input");
  const dialogSearchClear = $("dialog-search-clear");
  const dialogRefresh = $("dialog-refresh");
  const dialogClose = $("dialog-close");
  const historyButton = $("resume-session");
  const imageView = $("image-view");
  const imageViewImage = $("image-view-img");
  const imageViewName = $("image-view-name");
  const imageViewClose = $("image-view-close");
  const reviewBox = $("review");
  const reviewTitle = $("review-title");
  const reviewTotal = $("review-total");
  const reviewFiles = $("review-files");
  const reviewClose = $("review-close");
  const runStrip = $("run-strip");
  const runStripText = $("run-strip-text");
  const runOpenButton = $("run-open");

  const entries = new Map();
  let chips = [];
  let attachments = [];
  /// Chip thumbnails by attachment id: the downscaled copy the view paints,
  /// and the `<img>` a pending downscale writes it into.
  const thumbnails = new Map();
  const thumbnailNodes = new Map();
  let busy = false;
  let busyMessageMode = "queue";
  let queued = 0;
  /// The turn's own thread when it is not the one on screen (`state.run`):
  /// `{ sessionId, title, running }`, or null while the panel's own transcript
  /// is the run's. It is what the strip paints and what stops the composer
  /// offering a Send whose answer would land in a conversation nobody is
  /// reading.
  let runThread = null;
  let startedAt = 0;
  let elapsedTimer = 0;
  let dirty = new Set();
  let frame = 0;
  /// The change card a review is open on and the file it is showing. The listing
  /// is the card's own, so a review never disagrees with the rows behind it.
  let reviewItem = null;
  let reviewIndex = 0;
  let imageReturnFocus = null;

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

  // A side bar should summarize activity rather than turn every tool into a
  // terminal. Keep a small useful sample here; the fold still exposes the full
  // result. Shell keeps its tail (where the exit code is), readers keep the head.
  const PREVIEW_LINES = { bash: 3, read: 4, read_file: 4, grep: 5, find: 5, glob: 5, ls: 5, list_dir: 5 };

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

  /// What a preview line's own marker means: an addition, a removal, a hunk
  /// header, or context. The CLI renders one file's preview as
  /// `+/-/  <old> <new>  <text>`, so the marker is the first character.
  function diffClass(line) {
    if (line.startsWith("@@")) return "hunk";
    if (line.startsWith("+")) return "add";
    if (line.startsWith("-")) return "del";
    return "";
  }

  /// One file's preview as the lines of a `.diff` block, which is what the tool
  /// cards paint: the CLI's own compact, line-numbered render of the call's
  /// change.
  function diffLines(diff) {
    return String(diff || "")
      .split("\n")
      .map((line) => `<span class="dline ${diffClass(line)}">${escapeHtml(line)}</span>`)
      .join("");
  }

  function renderDiff(target, diff) {
    return `<div class="diff"><div class="diff-path">${escapeHtml(target || "diff")}</div><pre>${diffLines(diff)}</pre></div>`;
  }

  function toolCard(item) {
    const wrap = document.createElement("div");
    wrap.className = "tool";
    const head = document.createElement("div");
    head.className = "thead";
    // Whether the header is a control is decided per paint, once the card knows
    // what it holds: only a card with something behind it is a button.
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
    // The card's second fold handle, under the output it kept: the words that
    // offer the rest of a command are the same gesture as the header's caret,
    // and after a long result they are the closer one to the reader.
    const hint = document.createElement("div");
    hint.className = "thint";
    hint.hidden = true;
    hint.setAttribute("role", "button");
    hint.tabIndex = 0;
    hint.setAttribute("aria-expanded", "false");
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

  /// The line counts a card's one-line header shows, counted from the `+`/`-`
  /// markers the way `oxide_core::changes` counts them, so a call's numbers
  /// agree with the change listing's.
  function diffCounts(diff) {
    let added = 0;
    let removed = 0;
    for (const line of String(diff || "").split("\n")) {
      if (line.startsWith("+")) added += 1;
      else if (line.startsWith("-")) removed += 1;
    }
    if (!added && !removed) return "";
    return `+${added} −${removed}`;
  }

  /// Replace a streaming body without losing the reader's place: writing the
  /// text of a scroll container back resets it to the top, so a running command
  /// would keep showing its first line instead of its latest, and a body that
  /// was already at its bottom is put back there. Output that has finished is
  /// left at the top, since expanding a card is a request to read it from the
  /// beginning.
  function setOutput(el, text, running) {
    const follow = running && el.scrollHeight - el.scrollTop - el.clientHeight < 4;
    el.textContent = text;
    if (follow) el.scrollTop = el.scrollHeight;
  }

  /// Gives the header the fold's own semantics, or takes them away. A header is
  /// a button — a tab stop, a fold handle, something a screen reader announces —
  /// only while the card has a rest behind it: a card already showing everything
  /// it has is plain text, so a press on it would reveal nothing.
  function setFold(entry, foldable) {
    entry.el.classList.toggle("foldable", foldable);
    entry.head.tabIndex = foldable ? 0 : -1;
    if (foldable) {
      entry.head.setAttribute("role", "button");
      entry.head.setAttribute("aria-expanded", String(Boolean(entry.expanded)));
    } else {
      entry.head.removeAttribute("role");
      entry.head.removeAttribute("aria-expanded");
    }
  }

  /// Paints a tool card from its item. A call that changed a file reads as one
  /// line — the path, and how many lines moved — with its own diff kept for the
  /// reader who clicks the card, since the turn's changes are listed together by
  /// the change card at the end of the run. `running` cards show the live output.
  function paintTool(entry) {
    const item = entry.item;
    entry.el.classList.toggle("running", Boolean(item.running));
    // Each state is named as a boolean: `classList.toggle` with an absent force
    // is a plain toggle, so an item that omits one would otherwise flip a class
    // on (`error`, for a replayed card, whose `isError` is not set).
    entry.el.classList.toggle("done", Boolean(!item.running && !item.isError && !item.unknown));
    entry.el.classList.toggle("error", Boolean(!item.running && item.isError));
    entry.el.classList.toggle("unknown", Boolean(!item.running && item.unknown));
    entry.el.classList.toggle("expanded", Boolean(entry.expanded));
    if (item.running) {
      setFold(entry, false);
      // The call's own start, carried in the item: a card rebuilt from a `state`
      // message counts from the call rather than from the moment it was painted.
      const elapsed = item.startedAt ? Date.now() - item.startedAt : 0;
      entry.state.innerHTML = `<span class="spinner"></span>${elapsed > 1000 ? formatDuration(elapsed) : ""}`;
      setOutput(entry.pre, item.output, true);
      entry.hint.hidden = true;
      return;
    }
    const counts = item.diff ? diffCounts(item.diff) : "";
    // A call replayed out of a stored thread has no state to paint: the store
    // did not record whether it landed, so the card marks the call as unrecorded
    // rather than claiming the success (`✔`) or the failure (`✖`) it cannot know.
    const mark = item.unknown ? "•" : item.isError ? "✖" : "✔";
    // What the call took, kept beside its counts — measured by the host where
    // the call began, so a repaint and the other pane agree. A stored call took
    // no time here and carries none.
    const spent = item.elapsed ? ` ${formatDuration(item.elapsed)}` : "";
    entry.state.textContent = `${mark}${counts ? ` ${counts}` : ""}${spent}`;
    entry.state.title = item.unknown ? "Resumed thread: how this call ended was not recorded" : "";
    entry.inline = Boolean(item.diff);
    if (item.diff && !entry.diffEl) {
      entry.diffEl = document.createElement("div");
      entry.diffEl.innerHTML = renderDiff(diffTarget(entry), item.diff);
      entry.el.appendChild(entry.diffEl);
    }
    if (entry.diffEl) entry.diffEl.hidden = !entry.expanded;
    if (entry.expanded) {
      entry.pre.hidden = false;
      setOutput(entry.pre, item.output, false);
      // A card with something folded offers the fold back where the output
      // ends, so a long result can be closed from under it as well.
      entry.hint.hidden = !entry.folded;
      entry.hint.textContent = "Show less";
    } else if (entry.inline) {
      entry.folded = false;
      entry.pre.hidden = true;
      entry.hint.hidden = true;
    } else {
      const budget = previewLines(item.name);
      const useTail = item.name === "bash";
      const preview = useTail
        ? previewTail(item.output, budget)
        : previewText(item.output, budget);
      setOutput(entry.pre, preview.text, false);
      entry.folded = preview.more > 0;
      entry.hint.hidden = !entry.folded;
      // The line the card kept is counted where the reader would look for it,
      // rather than twice: as a marker in the body and again as this handle.
      entry.hint.textContent = `Show ${preview.more} ${useTail ? "earlier" : "more"} line${
        preview.more === 1 ? "" : "s"
      }`;
    }
    // The header is the card's fold handle only while something is behind it:
    // anything else is plain text, with no caret, no tab stop and no button.
    setFold(entry, Boolean(entry.inline || entry.folded));
  }

  function toggleTool(entry) {
    if (!entry.item || entry.item.running) return;
    // A card showing everything it has has no second state to move to, so a
    // press on it does nothing rather than opening a fold with nothing in it.
    if (!entry.inline && !entry.folded) return;
    entry.expanded = !entry.expanded;
    const expanded = String(entry.expanded);
    if (entry.head) entry.head.setAttribute("aria-expanded", expanded);
    if (entry.hint) entry.hint.setAttribute("aria-expanded", expanded);
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
  /// choice only the user can make. The card asks one question at a time — its
  /// own wording, its options as rows (radios for one answer, boxes for several)
  /// with a row for an answer in the user's own words, and `Next` walking to the
  /// question after it — and says where in them the reader is with `N of M
  /// questions` and a dash per question, so a call that asks several things is
  /// never one card taller than the panel. Every question's fields stay built,
  /// so stepping back finds what was already answered still there. The answers
  /// travel to the host, which forwards them over the CLI's request channel; the
  /// card is repainted from the `k: "question"` update it sends back.
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
    // The block of fields each question owns (kept, so an answer survives a walk
    // back to it) and which of them is on screen.
    return { el: wrap, body, actions, blocks: [], stepIndex: 0 };
  }

  /// What a question's hint reads: whether one label answers it or several. A
  /// question answered in the reader's own words has nothing to hint at.
  function questionHint(question) {
    const options = Array.isArray(question.options) ? question.options : [];
    if (!options.length) return "";
    return question.multiSelect ? "Select all that apply" : "Select one answer";
  }

  /// One question's own fields: its options, and the row that takes an answer in
  /// the user's own words. A single choice offers that row the way the dialog it
  /// follows does — as one of the choices, with the field under it — so the
  /// typed text is the answer instead of a second one beside the picked label.
  /// A single-select question preselects its first option, so an answer can
  /// never be blank by accident.
  function questionBlock(question, index) {
    const block = document.createElement("div");
    block.className = "qblock";
    block.hidden = true;
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
    if (options.length && !question.multiSelect) {
      const row = document.createElement("label");
      row.className = "qoption qown";
      const own = document.createElement("input");
      own.type = "radio";
      own.name = `question-${index}`;
      // The row's value is empty, so the collector never sends its label as an
      // answer: choosing it is what lets the typed text answer instead.
      own.value = "";
      own.dataset.question = String(index);
      own.className = "qchoice qownchoice";
      const label = document.createElement("span");
      label.className = "qlabel";
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

  /// What was ticked and typed, one entry per question, each echoing the
  /// question it belongs to so the model reads the answers in the terms it asked
  /// them in.
  function collectAnswers(entry) {
    return (entry.item.questions || []).map((question, index) => {
      const values = [];
      entry.body
        .querySelectorAll(`.qchoice[data-question="${index}"]:checked`)
        .forEach((input) => {
          const value = String(input.value || "").trim();
          if (value) values.push(value);
        });
      const field = entry.body.querySelector(`.qfree[data-question="${index}"]`);
      const typed = field ? String(field.value || "").trim() : "";
      // Only a single choice has a row asking for the user's own words, and its
      // text answers the question when that row is the chosen one.
      const own = entry.body.querySelector(`.qownchoice[data-question="${index}"]`);
      if (typed && (!own || own.checked)) values.push(typed);
      return { question: question.question, values };
    });
  }

  /// The buttons a step offers: the dismissal, `Back` while there is one to go
  /// back to, and `Next` — `Submit` on the last question, which is where the
  /// whole set is sent.
  function paintQuestionActions(entry) {
    const count = (entry.item.questions || []).length;
    const last = entry.stepIndex + 1 >= count;
    entry.actions.textContent = "";
    const dismiss = document.createElement("button");
    dismiss.type = "button";
    dismiss.className = "ghost";
    dismiss.textContent = "Dismiss";
    dismiss.title = "Answer nothing and let the agent continue with its own default";
    dismiss.addEventListener("click", () =>
      vscode.postMessage({ k: "question", requestId: entry.item.requestId, answers: [] }),
    );
    entry.actions.appendChild(dismiss);
    if (entry.stepIndex > 0) {
      const back = document.createElement("button");
      back.type = "button";
      // Whichever button leads the group on the right carries the margin that
      // pushes it there, since Back is only there once there is one.
      back.className = "ghost qback";
      back.textContent = "Back";
      back.title = "Go back to the previous question";
      back.addEventListener("click", () => {
        entry.stepIndex -= 1;
        paintQuestionStep(entry);
      });
      entry.actions.appendChild(back);
    }
    const next = document.createElement("button");
    next.type = "button";
    next.className = entry.stepIndex > 0 ? "primary" : "primary qlead";
    next.textContent = last ? "Submit" : "Next";
    next.title = last ? "Send these answers to the agent" : "Go to the question after this one";
    next.addEventListener("click", () => {
      if (!last) {
        entry.stepIndex += 1;
        paintQuestionStep(entry);
        return;
      }
      vscode.postMessage({
        k: "question",
        requestId: entry.item.requestId,
        answers: collectAnswers(entry),
      });
    });
    entry.actions.appendChild(next);
    const hint = document.createElement("span");
    hint.className = "awaiting";
    hint.textContent = "Waiting for your answer…";
    entry.actions.appendChild(hint);
  }

  /// Paints the step on screen: how many questions there are and which one this
  /// is, the question itself with its own header above it, and the buttons this
  /// position allows. Only the question being asked has its fields shown; the
  /// other blocks stay where they are, holding what was answered in them.
  function paintQuestionStep(entry) {
    const questions = entry.item.questions || [];
    const count = questions.length;
    const index = Math.min(Math.max(entry.stepIndex || 0, 0), count - 1);
    entry.stepIndex = index;
    const question = questions[index] || {};
    // One question needs no counter: there is nothing to step through.
    entry.step.hidden = count < 2;
    entry.steplabel.textContent = count < 2 ? "" : `${index + 1} of ${count} questions`;
    entry.dashes.textContent = "";
    for (let position = 0; count > 1 && position < count; position += 1) {
      const dash = document.createElement("span");
      dash.className = `qdash ${
        position < index ? "done" : position === index ? "current" : "todo"
      }`;
      entry.dashes.appendChild(dash);
    }
    // The card's own title already names a question that was asked on its own,
    // so a single question is not said twice.
    const title = String(entry.item.title || "");
    entry.kicker.textContent = question.header && question.header !== title ? question.header : "";
    entry.kicker.hidden = !entry.kicker.textContent;
    entry.text.textContent = String(question.question || "");
    entry.text.hidden = !entry.text.textContent || entry.text.textContent === title;
    entry.hint.textContent = questionHint(question);
    entry.hint.hidden = !entry.hint.textContent;
    for (const [position, block] of entry.blocks.entries()) {
      block.hidden = position !== index;
    }
    // A question with nothing to pick is answered in the reader's own words, so
    // the caret goes where the answer goes. One that offers options is left
    // alone, so a keystroke is not taken by a field nobody was asked to fill in,
    // and a repaint that finds the caret already in the field never interrupts
    // the answer being typed into it.
    const shown = entry.blocks[index];
    const free = shown ? shown.querySelector(".qfree") : null;
    const options = Array.isArray(question.options) ? question.options : [];
    if (free && !options.length && document.activeElement !== free) free.focus();
    paintQuestionActions(entry);
  }

  /// Paints a card from its item: a waiting one is built question by question
  /// and painted on the first of them, and an answered one keeps only what it
  /// was answered with.
  function paintQuestion(entry) {
    const item = entry.item;
    const waiting = item.state === "pending";
    entry.el.classList.toggle("waiting", waiting);
    entry.el.classList.toggle("answered", item.state === "answered");
    entry.body.textContent = "";
    entry.actions.textContent = "";
    entry.blocks = [];
    if (!waiting) {
      const done = document.createElement("span");
      done.className = "qdone";
      done.textContent = item.label || "Answered";
      entry.actions.appendChild(done);
      return;
    }
    const step = document.createElement("div");
    step.className = "qstep";
    const steplabel = document.createElement("span");
    steplabel.className = "qsteplabel";
    const dashes = document.createElement("span");
    dashes.className = "qdashes";
    step.append(steplabel, dashes);
    const heading = document.createElement("div");
    heading.className = "qheading";
    const kicker = document.createElement("div");
    kicker.className = "qheader";
    const text = document.createElement("p");
    text.className = "qtext";
    const hint = document.createElement("div");
    hint.className = "qhint";
    heading.append(kicker, text, hint);
    entry.body.append(step, heading);
    for (const [index, question] of (item.questions || []).entries()) {
      const block = questionBlock(question, index);
      entry.blocks.push(block);
      entry.body.appendChild(block);
    }
    entry.step = step;
    entry.steplabel = steplabel;
    entry.dashes = dashes;
    entry.kicker = kicker;
    entry.text = text;
    entry.hint = hint;
    paintQuestionStep(entry);
  }

  // ---------- transcript items ----------

  /// The files a finished turn changed, which the CLI reports from the run's own
  /// shadow snapshot — so a file a shell command or a formatter wrote is listed
  /// the same as an edited one. The card is a listing rather than a diff: a row
  /// opens VS Code's own diff editor for that file (the host serves the baseline
  /// side out of the snapshot) and the header opens the whole turn in the
  /// multi-file diff, so the panel carries no diff format of its own. What each
  /// row says, and how it is badged, is composed by the host.
  ///
  /// **Review** opens the turn's files over the panel, one row at a time, so the
  /// reader can walk a turn without leaving the chat: each row it lands on opens
  /// that file in VS Code's diff editor, which is where the two sides are drawn.
  function changesCard(item) {
    const wrap = document.createElement("div");
    wrap.className = "changes";
    const head = document.createElement("div");
    head.className = "changes-head";
    const open = document.createElement("button");
    open.type = "button";
    open.className = "changes-open";
    open.dataset.action = "openAllChanges";
    open.innerHTML = CHANGES_ICON;
    open.title = "Open every file in VS Code's diff editor";
    open.setAttribute("aria-label", open.title);
    const label = document.createElement("span");
    label.className = "changes-title";
    label.textContent = item.title || `Edited ${(item.rows || []).length} files`;
    head.append(open, label);
    if (item.totals) {
      const total = document.createElement("span");
      total.className = "changes-total";
      total.textContent = item.totals;
      head.appendChild(total);
    }
    // What a card says instead of its action once the turn has been put back,
    // and the action itself: both are painted from the item's own state, so a
    // card a newer turn took the Undo away from says nothing and offers nothing.
    const note = document.createElement("span");
    note.className = "changes-note";
    note.textContent = "Undone";
    note.hidden = true;
    head.appendChild(note);
    const rows = item.rows || [];
    if (rows.length) {
      const review = document.createElement("button");
      review.type = "button";
      review.className = "changes-review";
      review.textContent = "Review";
      review.title = "Walk this turn's files, each in VS Code's diff editor";
      review.setAttribute("aria-label", review.title);
      review.addEventListener("click", () => openReview(item));
      const undo = document.createElement("button");
      undo.type = "button";
      undo.className = "changes-undo";
      undo.textContent = "Undo";
      undo.title = "Put this turn's files back to how the run found them";
      undo.setAttribute("aria-label", undo.title);
      undo.addEventListener("click", () => {
        vscode.postMessage({ k: "undoChanges", id: item.id });
      });
      head.append(review, undo);
    }
    wrap.appendChild(head);

    const list = document.createElement("div");
    list.className = "changes-list";
    for (const row of rows) {
      const el = document.createElement("button");
      el.type = "button";
      el.className = "change-row";
      el.dataset.index = String(row.index);
      el.dataset.action = "openChangeDiff";
      const badge = document.createElement("span");
      badge.className = `change-badge change-${row.status || "modified"}`;
      badge.textContent = row.letter || "M";
      const name = document.createElement("span");
      name.className = "change-path";
      name.textContent = row.path;
      const detail = document.createElement("span");
      detail.className = "change-detail";
      detail.textContent = row.detail;
      el.append(badge, name, detail);
      el.title = row.title || row.path;
      list.appendChild(el);
    }
    wrap.appendChild(list);
    // A long listing folds away behind one row, as the desktop app's card does:
    // the rows past it are the view's to hide, and the words the row reads with
    // — including the way back — are the host's.
    if (item.more) {
      const more = item.more;
      let all = false;
      const toggle = document.createElement("button");
      toggle.type = "button";
      toggle.className = "changes-more";
      toggle.dataset.action = "changesMore";
      toggle.textContent = more.closed;
      toggle.addEventListener("click", () => {
        all = !all;
        for (const [index, row] of [...list.children].entries()) {
          row.hidden = !all && index >= more.visible;
        }
        toggle.textContent = all ? more.open : more.closed;
      });
      for (const [index, row] of [...list.children].entries()) {
        if (index >= more.visible) row.hidden = true;
      }
      wrap.appendChild(toggle);
    }
    return { el: wrap };
  }

  /// A change card's own state, from the item it was built with: its Undo is
  /// offered only while its turn is the newest one — an older restore would take
  /// the newer turn's work with it — and a card that has been put back says so
  /// instead of offering the same thing again.
  function paintChanges(entry) {
    const undo = entry.el.querySelector(".changes-undo");
    if (undo) undo.hidden = entry.item.undone || !entry.item.undoable;
    const note = entry.el.querySelector(".changes-note");
    if (note) note.hidden = !entry.item.undone;
  }

  /// A chip under a sent bubble: the file or attachment's name, and — for an
  /// image the composer had a thumbnail of — the picture itself, which opens the
  /// full-size one the way the composer's own chip does. What was sent is what
  /// the reader scrolls back to, rather than the name it was filed under.
  function sentChip(chip) {
    if (!chip.preview) {
      const named = document.createElement("span");
      named.className = "chip";
      named.textContent = chip.label;
      return named;
    }
    const el = document.createElement("button");
    el.type = "button";
    el.className = "chip pic";
    el.title = `${chip.label} · open the full-size image`;
    el.setAttribute("aria-label", `Open ${chip.label || "the image"}`);
    const image = document.createElement("img");
    image.src = chip.preview;
    image.alt = chip.label || "";
    // WebKit drags a picture out of the page by default, and a drag that starts
    // on it is the one gesture whose click the webview withholds.
    image.draggable = false;
    el.appendChild(image);
    el.addEventListener("click", () => openImage(chip.preview, chip.label));
    return el;
  }

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
        for (const chip of item.context) {
          list.appendChild(sentChip(chip));
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
    if (item.kind === "changes") return changesCard(item);
    return toolCard(item);
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
    else if (item.kind === "changes") paintChanges(entry);
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

  /// The home state, which the folder open decides: with one it is the
  /// invitation and the suggestions, each filling the composer; without one
  /// there is nowhere to run, so the page says so instead of inviting a message
  /// the send would refuse.
  function paintHome(folder) {
    const open = Boolean(folder);
    emptyLead.textContent = open
      ? "Ask Oxide to make a change, explain code, or run something."
      : "Open a folder to run Oxide: sessions and context are per project.";
    if (open && !suggestions.children.length) {
      for (const text of SUGGESTIONS) {
        const button = document.createElement("button");
        button.type = "button";
        button.dataset.prompt = text;
        button.textContent = text;
        suggestions.appendChild(button);
      }
    }
    suggestions.hidden = !open;
  }

  /// Fills the composer with a suggestion, which is where the reader then edits
  /// or sends it: the same four the desktop app offers, in the same words.
  function fillComposer(text) {
    input.value = text;
    closeCompletion();
    resizeInput();
    updateSendState();
    input.focus();
  }

  function apply(message) {
    // Measured before anything grows, so it reflects the frame the reader is
    // looking at rather than the output that has just arrived, and so a scroll
    // whose event has not been dispatched yet is still seen in time.
    following = atBottom();
    switch (message.k) {
      case "state": {
        // The transcript is rebuilt from scratch, so the card a review was
        // opened from may be gone with it.
        closeReview();
        entries.clear();
        transcript.innerHTML = "";
        transcript.appendChild(empty);
        empty.hidden = message.items.some(isConversation);
        transcript.classList.toggle("hide-thinking", message.showThinking === false);
        titleLabel.textContent = message.title || "New chat";
        paintHome(message.folder);
        for (const item of message.items) appendItem(item, false);
        setStatus(message.status, message.busy, message.queued);
        setRun(message.run);
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
      case "changes": {
        const entry = entries.get(message.id);
        if (!entry || entry.item.kind !== "changes") return;
        entry.item.undoable = message.undoable;
        entry.item.undone = message.undone;
        paintChanges(entry);
        scrollDown(false);
        return;
      }
      case "status":
        setStatus(message.status, message.busy, message.queued);
        setRun(message.run);
        setFooter(message.footer);
        // The title changes when the first message is sent, before the next
        // `state` message repaints the view, so the header follows the send.
        titleLabel.textContent = message.title || "New chat";
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
      case "insert":
        insertReference(message.text || "");
        return;
      case "focusComposer":
        focusComposer();
        return;
      default:
        return;
    }
  }

  // ---------- footer ----------

  /// The turn's own thread, when it is not the one on screen. The label is the
  /// same one the desktop app's strip carries, because the two front-ends say
  /// the same thing about the same situation: the running turn owns the
  /// conversation it started in, and this is where that conversation is named
  /// and how the reader gets back to it.
  function setRun(run) {
    runThread = run || null;
    const away = Boolean(runThread);
    runStrip.hidden = !away;
    if (away) {
      const label = runThread.title || "another thread";
      const text = runThread.running
        ? `A turn is running in “${label}”`
        : `A turn finished in “${label}”`;
      runStripText.textContent = text;
      runStrip.title = `Open “${label}”`;
      runOpenButton.title = `Open “${label}”`;
      runOpenButton.setAttribute("aria-label", `Open “${label}”`);
      runStrip.setAttribute("aria-label", `${text}. Open it.`);
    } else {
      runStrip.removeAttribute("aria-label");
      runStrip.title = "";
    }
    // The corner follows: a message typed here while the run is in another
    // thread has nowhere on screen to be answered, so the panel offers Stop
    // alone — the host refuses the send in the same words the desktop app does.
    updateSendState();
  }

  /// Whether the run is writing into a thread other than the one on screen.
  function runAway() {
    return Boolean(runThread);
  }

  function setStatus(text, isBusy, queuedCount) {
    busy = Boolean(isBusy);
    if (!busy) busyMessageMode = "queue";
    queued = queuedCount || 0;
    const label = queued > 0 ? `${text} · ${queued} queued` : text;
    // The phase is only worth a line while there is one: the TUI reserves its
    // status row for a running turn too, and an idle panel says nothing rather
    // than sitting on a dot that reads as a button.
    statusLabel.hidden = !busy && queued === 0;
    statusLabel.textContent = label;
    statusLabel.classList.toggle("busy", busy);
    statusLabel.title = busy ? "Oxide is working" : "Ready for the next message";
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

  /// The footer the extension host composes: compact controls in the composer's
  /// own toolbar, plus the usage line, branch and context gauge below it.
  /// Everything is a string by the time it gets here, so this function only
  /// paints.
  function setFooter(footer) {
    if (!footer) return;
    metaBox.innerHTML = "";
    for (const chip of footer.chips || []) {
      metaBox.appendChild(chipNode(chip.label, chip.id, chip.title));
    }
    const info = footer.info || "";
    branchLabel.textContent = info;
    branchLabel.title = info ? `Current branch: ${info}` : "";
    branchWrap.hidden = !info;
    const percent = typeof footer.percent === "number" ? footer.percent : null;
    usageText.textContent = percent === null ? "—" : `${percent}%`;
    usageText.title = footer.usage || "";
    usageText.setAttribute("tabindex", "0");
    usageText.setAttribute("aria-label", footer.usage ? `Usage: ${footer.usage}` : "No usage reported");
    usageRow.title = footer.usage || "";
    usageRow.hidden = !footer.usage;
    gauge.hidden = percent === null;
    gauge.dataset.level = footer.level || "ok";
    gauge.title = percent === null ? "" : `${percent}% of the context window used`;
    gaugeFill.style.width = percent === null ? "0%" : `${Math.min(percent, 100)}%`;
  }

  /// One icon-only control. Its full current value stays in the accessible name
  /// and tooltip, so compacting the toolbar does not make it mysterious.
  function chipNode(label, control, title) {
    const el = document.createElement("button");
    el.type = "button";
    el.className = "meta-chip";
    el.dataset.control = control;
    el.setAttribute("aria-label", String(label));
    const icon = document.createElement("span");
    icon.className = "control-icon";
    icon.innerHTML = CONTROL_ICONS[control] || CONTROL_ICONS.model;
    el.appendChild(icon);
    el.title = title ? `${label}\n${title}` : `${label} — click to change`;
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

  /// An image, a PDF or a text file the next message will carry: a thumbnail
  /// where the host could send one, a glyph and the size where it could not. An
  /// image's thumbnail opens the full-size one — the panel is narrow and the
  /// chip is small, so the copy on it is not much of a look at what is being
  /// sent.
  function attachmentNode(attachment) {
    const el = document.createElement("div");
    el.className = "chip attachment";
    if (attachment.kind === "image" && attachment.preview) {
      const open = document.createElement("button");
      open.type = "button";
      open.className = "chip-open";
      open.title = "Open the full-size image";
      open.setAttribute("aria-label", `Open ${attachment.label || "the image"}`);
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
      glyph.textContent = attachment.kind === "image" ? "▣" : "▤";
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
    // The label and the tooltip say the same thing: a chip that says which lines
    // it holds is only readable if the words around it say so too.
    const what = chip.detail
      ? chip.detail
      : chip.auto
        ? "the file you are editing, sent with the next message"
        : "inlined into the next message";
    el.title = `${chip.label} — ${what}`;
    el.setAttribute("aria-label", el.title);
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

  /// The corner of the composer holds one action rather than two buttons side by
  /// side: while a turn runs with nothing to say it is Stop, and the moment the
  /// box holds something it is Send — which the host queues for after the turn,
  /// since the CLI is asked for a follow-up turn rather than steering the one in
  /// flight. The desktop app swaps the same two the same way, so the button the
  /// reader is aiming at does not move as the box is typed into.
  /// The Queue/Steer bar on the composer's own top row: the way a message typed
  /// while a turn runs is delivered. It is up exactly while there is a turn to
  /// deliver into in the thread on screen — a run in another thread is reached by
  /// opening it, and the bar would be a choice about a message this composer
  /// refuses to send. The marked way is the one Enter sends with, so the key
  /// hints follow the mark: the marked option carries `Enter`, the other its own
  /// key, and either can be picked with the pointer.
  function paintModeBar(busyHere) {
    modeBar.hidden = !busyHere;
    const queue = busyMessageMode === "queue";
    modeQueue.classList.toggle("active", queue);
    modeSteer.classList.toggle("active", !queue);
    for (const [option, label, marked] of [
      [modeQueue, "Queue as the next turn after the current response", queue],
      [modeSteer, "Steer the active response", !queue],
    ]) {
      const title = `${label} (${marked ? "Enter" : "Alt+Enter"})`;
      option.title = title;
      option.setAttribute("aria-label", title);
      // Which way is marked is a state rather than a word: the accent says it to
      // a reader looking at the bar, and this says the same thing to a screen
      // reader rather than leaving the mark to the stylesheet alone.
      option.setAttribute("aria-pressed", marked ? "true" : "false");
    }
  }

  function setBusyMessageMode(mode) {
    busyMessageMode = mode === "steer" ? "steer" : "queue";
    updateSendState();
  }

  function updateSendState() {
    const hasText = Boolean(input.value.trim()) || pendingCount() > 0;
    // The corner's own actions belong to the thread this composer is showing. A
    // turn running in another thread is not this transcript's, so a message
    // typed here would be steered into that run and its reply would land in a
    // conversation that is not on screen: the corner offers Stop alone until the
    // reader is back in it (the strip above is the way there), and the typed text
    // is kept rather than sent. The host refuses the send in the same words.
    const here = !runAway();
    sendButton.disabled = !hasText;
    sendButton.hidden = busy && (!hasText || !here);
    stopButton.hidden = !busy || (hasText && here);
    paintModeBar(busy && here);
    const action = busyMessageMode === "steer" ? "Steer the active response" : "Queue as the next turn";
    sendButton.title = busy ? `${action} (Enter)` : "Send (Enter)";
    sendButton.setAttribute("aria-label", busy ? busyMessageMode === "steer" ? "Steer" : "Queue" : "Send");
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

  /// The change card's own glyph: a split view with a file on each side, which
  /// is what a click there opens. It is drawn in the card's header beside the
  /// listing's name, so the whole-turn action reads as the header itself.
  const CHANGES_ICON =
    '<svg viewBox="0 0 24 24" width="14" height="14" aria-hidden="true"><path d="M12 3v18" fill="none" stroke="currentColor" stroke-width="1.6"/><path d="M4 7h5M4 11h6M20 13h-5M20 17h-6" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round"/></svg>';

  /// The box's own ✕ appears once there is something to clear: it is the field's
  /// affordance rather than the host's, so it is the field it reads.
  function syncSearchClear() {
    dialogSearchClear.hidden = !dialogSearchInput.value;
  }

  /// The dialog the host composes (`src/core/dialogs.ts`): the MCP server list
  /// and the session history, painted here rather than in a QuickPick — which
  /// takes over the window, hides the transcript the listing is about, and
  /// cannot be answered while a turn streams. It is one element either way
  /// (`#dialog` sits under the header, ahead of the transcript, in the panel's
  /// own flow) and the host says which end it belongs near: `.pin-footer` moves
  /// the same complete card above the footer with CSS `order`, so the MCP list
  /// grows near the composer block where `/mcp` was typed while session history
  /// remains near the header it describes. A row arrives with the action it
  /// posts, so the view decides nothing about what a click means.
  ///
  /// Two fields are the head's: the count beside the title — how many rows the
  /// listing holds, which is a number a narrow pane cannot read off a scrollbar
  /// and what a search narrows — and whether the listing carries a search box.
  /// Only the host knows whether filtering is worth offering, and only it has
  /// the rows the filter applies to, so the box asks it and the answer comes
  /// back as a whole new dialog.
  function setDialog(dialog) {
    dialogList.innerHTML = "";
    // The header's own button is the same state: it says whether the history is
    // on screen, so the two can never disagree about which click closes it.
    historyButton.setAttribute("aria-expanded", dialog && dialog.kind === "sessions" ? "true" : "false");
    if (!dialog) {
      // The class is dropped with it: a hidden element's siblings would still be
      // ordered around it, and the next listing says which end it wants.
      dialogBox.classList.remove("pin-footer");
      dialogBox.hidden = true;
      dialogSearch.hidden = true;
      dialogSearchInput.value = "";
      syncSearchClear();
      return;
    }
    dialogBox.classList.toggle("pin-footer", dialog.pin === "footer");
    dialogTitle.textContent = dialog.title || "";
    const count = dialog.count || 0;
    dialogCount.textContent = count ? String(count) : "";
    dialogCount.hidden = !count;
    // The box is the host's, not a filter the view keeps: it is shown when the
    // host says the listing can be filtered, and put back to the filter that
    // listing was composed with — unless the caret is in it, since a repaint
    // under a reader who is typing would take the text away from them.
    dialogSearch.hidden = dialog.search !== true;
    dialogSearchInput.placeholder = dialog.searchPlaceholder || "Search sessions…";
    if (dialog.search === true && document.activeElement !== dialogSearchInput) {
      dialogSearchInput.value = dialog.query || "";
    }
    syncSearchClear();
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
  /// instead of opening it. The kind the host sent is the row's class, which is
  /// the only thing that tells a thread apart from a way out of the listing:
  /// both are buttons, and both say what they do in words.
  function dialogRow(entry) {
    const el = document.createElement(entry.action ? "button" : "div");
    el.className = entry.kind ? `dialog-row ${entry.kind}` : "dialog-row";
    // The thread the panel has open, marked by the host: the row keeps its place
    // in the list, and the mark is what says which one it is — so the listing
    // never loses the thread the header names.
    if (entry.current) el.classList.add("current");
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

  /// The listing's search box. The rows are the host's, so every keystroke asks
  /// it for the listing that filter leaves rather than hiding rows here: the
  /// count beside the title and the note a search with no matches carries are
  /// the host's too, and a view that filtered its own rows would say something
  /// different from the one in the pane beside it.
  dialogSearchInput.addEventListener("input", () => {
    syncSearchClear();
    vscode.postMessage({ k: "dialogSearch", text: dialogSearchInput.value });
  });

  dialogSearchClear.addEventListener("click", () => {
    dialogSearchInput.value = "";
    syncSearchClear();
    dialogSearchInput.focus();
    vscode.postMessage({ k: "dialogSearch", text: "" });
  });

  // ---------- image preview ----------

  /// The full-size image behind a chip's thumbnail: a chip only has room for a
  /// 96px copy, so a click opens what the host actually sent.
  function openImage(preview, label) {
    if (!preview) return;
    imageReturnFocus = document.activeElement || null;
    imageViewImage.src = preview;
    imageViewImage.alt = label || "Attachment preview";
    imageViewImage.title = label || "Attachment preview";
    imageViewName.textContent = label || "Attachment preview";
    imageView.hidden = false;
    imageViewClose.focus();
  }

  function closeImage() {
    if (imageView.hidden) return;
    imageView.hidden = true;
    const target = imageReturnFocus;
    imageReturnFocus = null;
    if (target && typeof target.focus === "function") target.focus();
  }

  imageViewClose.addEventListener("click", closeImage);

  imageView.addEventListener("click", (event) => {
    if (event.target === imageView) closeImage();
  });

  imageView.addEventListener("keydown", (event) => {
    if (event.key !== "Tab") return;
    event.preventDefault();
    imageViewClose.focus();
  });

  // ---------- review ----------

  /// A turn's changes over the panel: the card's own rows, walked with the arrow
  /// keys. The diff itself is VS Code's — taking a row, which is what walking to
  /// one does, opens that file in the editor's own diff view, and does it as a
  /// preview without taking the keyboard, so walking on replaces the same tab
  /// rather than leaving a turn's worth of them behind. The rows are the card's,
  /// so the review and the card behind it cannot disagree about the turn.
  function openReview(item) {
    if (!item || !item.rows || !item.rows.length) return;
    reviewItem = item;
    reviewIndex = 0;
    // Shown before it is painted: the row it opens on takes the focus, and one
    // inside a hidden sheet cannot.
    reviewBox.hidden = false;
    paintReview();
    openReviewFile(reviewIndex);
  }

  function closeReview() {
    reviewItem = null;
    reviewBox.hidden = true;
  }

  /// Takes the row at `index`, wrapping at either end: walking and clicking are
  /// the same act, which is what shows the file the reader is on.
  function selectReview(index) {
    if (!reviewItem) return;
    const count = reviewItem.rows.length;
    reviewIndex = ((index % count) + count) % count;
    paintReview();
    openReviewFile(reviewIndex);
  }

  function openReviewFile(index) {
    if (!reviewItem) return;
    vscode.postMessage({ k: "openChangeDiff", id: reviewItem.id, index, review: true });
  }

  function paintReview() {
    const item = reviewItem;
    if (!item) return;
    reviewTitle.textContent = item.title || "Changes";
    reviewTotal.textContent = item.totals || "";
    reviewTotal.hidden = !item.totals;
    reviewFiles.innerHTML = "";
    let on = null;
    item.rows.forEach((row, index) => {
      const active = index === reviewIndex;
      const node = document.createElement("button");
      node.type = "button";
      node.className = active ? "change-row active" : "change-row";
      node.setAttribute("role", "option");
      node.setAttribute("aria-selected", active ? "true" : "false");
      node.title = row.title || row.path;
      const badge = document.createElement("span");
      badge.className = `change-badge change-${row.status}`;
      badge.textContent = row.letter || "M";
      const name = document.createElement("span");
      name.className = "change-path";
      name.textContent = row.path;
      const detail = document.createElement("span");
      detail.className = "change-detail";
      detail.textContent = row.detail || "";
      node.append(badge, name, detail);
      node.addEventListener("click", () => selectReview(index));
      reviewFiles.appendChild(node);
      if (active) on = node;
    });
    // The row the reader is on holds the focus, so Enter opens it again — and a
    // long listing is scrolled to it rather than left where it was.
    if (on) on.focus();
  }

  reviewClose.addEventListener("click", () => closeReview());

  // The files are walked with the arrows, the way a changes view is. The diff
  // opens beside the panel rather than in front of it, so the caret stays in the
  // composer and the next arrow walks on.
  document.addEventListener("keydown", (event) => {
    if (reviewBox.hidden) return;
    const step = event.key === "ArrowDown" ? 1 : event.key === "ArrowUp" ? -1 : 0;
    if (!step) return;
    event.preventDefault();
    selectReview(reviewIndex + step);
  });

  // Escape closes what is on top — the review, then the image, then the dialog.
  // The composer's own Escape (stop the running turn) is handled on the textarea.
  document.addEventListener("keydown", (event) => {
    if (event.key !== "Escape") return;
    if (!reviewBox.hidden) {
      closeReview();
      return;
    }
    if (!imageView.hidden) {
      closeImage();
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

  function submit(opposite = false) {
    const text = input.value;
    if (!text.trim() && pendingCount() === 0) return;
    // The corner offers Stop alone while the run is in another thread, and Enter
    // is the same corner: the message stays in the box rather than being steered
    // into a run whose reply would land in a conversation that is not on screen.
    // It is not lost — the strip above is the way into that thread, and what is
    // typed here is still here when it is opened. The host refuses the same send
    // in the same words, for anything that reaches it another way.
    if (busy && runAway()) return;
    // The bar's own mark is what Enter and the corner follow; the key beside the
    // mark sends the other way, so a message can be queued or steered without
    // moving the mark first.
    const markedQueue = busyMessageMode === "queue";
    const queue = opposite ? !markedQueue : markedQueue;
    const mode = busy && !queue ? "steer" : "queue";
    input.value = "";
    busyMessageMode = "queue";
    closeCompletion();
    resizeInput();
    updateSendState();
    vscode.postMessage(busy ? { k: "send", text, mode } : { k: "send", text });
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

  /// Writes a reference the host built — the editor's insert shortcut read the
  /// file and the selection — into the box at the caret, keeping it off the
  /// words around it the way a typed reference sits, and leaves the caret after
  /// it so the question goes on being typed. What is replaced is whatever was
  /// selected, as a paste would replace it.
  function insertReference(text) {
    const value = input.value;
    const start = input.selectionStart ?? value.length;
    const end = input.selectionEnd ?? start;
    const before = value.slice(0, start);
    const after = value.slice(end);
    const lead = before && !/\s$/.test(before) ? " " : "";
    const tail = after && !/^\s/.test(after) ? " " : "";
    const inserted = `${lead}${text}${tail}`;
    const caret = before.length + inserted.length;
    input.value = before + inserted + after;
    input.setSelectionRange(caret, caret);
    // The reference is complete as written, so a list left open belongs to a
    // value the caret has moved out of.
    closeCompletion();
    resizeInput();
    updateSendState();
    input.focus();
  }

  /// Where bringing the chat forward leaves the caret. Focusing the pane does
  /// not focus its message box, so the host asks for it.
  function focusComposer() {
    const end = input.value.length;
    input.focus();
    input.setSelectionRange(end, end);
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
      if (event.key === "Escape" && !event.metaKey && !event.ctrlKey && !event.altKey) {
        event.preventDefault();
        closeCompletion();
        return;
      }
    }
    if (event.key === "Enter" && !event.shiftKey) {
      event.preventDefault();
      submit(event.altKey);
      return;
    }
    // A modified Escape is VS Code's (`Cmd+Esc` toggles the caret between the
    // editor and the panel), so only a plain one stops the turn.
    if (event.key === "Escape" && busy && !event.metaKey && !event.ctrlKey && !event.altKey) {
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
  /// writes it to a file the CLI can read (`--image <path>`). A paste that
  /// cannot be read here — macOS refuses a webview's read of a copied file in
  /// the Desktop, Documents or Downloads folder — asks the host to read the
  /// clipboard through the CLI instead, which attaches what the pasteboard
  /// itself carries.
  async function attachBlob(file, { fallbackToClipboard = false } = {}) {
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
      if (fallbackToClipboard) {
        vscode.postMessage({ k: "attachClipboard" });
        return;
      }
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
    for (const file of files) void attachBlob(file, { fallbackToClipboard: true });
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
  // A suggestion is a message to start from rather than a message to send: it
  // goes into the box, where the reader names the file or narrows the question.
  suggestions.addEventListener("click", (event) => {
    const target = event.target;
    if (!(target instanceof Element)) return;
    const button = target.closest("[data-prompt]");
    if (button) fillComposer(button.dataset.prompt || "");
  });
  // The Queue/Steer bar: the mark is what Enter sends with, the other option is
  // what its own key sends, and either can be picked with the pointer.
  modeQueue.addEventListener("click", () => setBusyMessageMode("queue"));
  modeSteer.addEventListener("click", () => setBusyMessageMode("steer"));
  stopButton.addEventListener("click", () => vscode.postMessage({ k: "stop" }));
  $("new-session").addEventListener("click", () => vscode.postMessage({ k: "newSession" }));
  historyButton.addEventListener("click", () => vscode.postMessage({ k: "resumeSession" }));
  // The strip's own control: the turn's thread is on another one, so this is the
  // way back to it — the same door the desktop app's strip offers, and the same
  // control id its view posts.
  runOpenButton.addEventListener("click", () => vscode.postMessage({ k: "control", control: "openRun" }));

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
      // VS Code's own webview host opens every http(s) link it sees clicked —
      // its `handleInnerClick` posts `did-click-link` to the workbench, which
      // hands the URL to the opener service — and it does not ask whether the
      // page has already dealt with that click. A click let through to it as
      // well would open a second browser tab beside this one, so it stops at
      // this listener, short of the host's own (a bubble listener on the inner
      // frame's window, which stopping propagation never reaches).
      event.stopPropagation();
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
    // A change card's row (or its header) asks the host for VS Code's own diff
    // editor; the view knows only which row was clicked, and the host looks the
    // turn's baseline up in the transcript it owns.
    const change = target.closest(".change-row, .changes-open");
    if (change) {
      const entry = entryOf(change);
      if (entry && entry.item.kind === "changes") {
        if (change.classList.contains("change-row")) {
          vscode.postMessage({
            k: "openChangeDiff",
            id: entry.item.id,
            index: Number(change.dataset.index) || 0,
          });
        } else {
          vscode.postMessage({ k: "openAllChanges", id: entry.item.id });
        }
        return;
      }
    }
    const entry = entryOf(target);
    if (entry && target.closest(".thead, .thint")) toggleTool(entry);
  });

  transcript.addEventListener("keydown", (event) => {
    if (event.key !== "Enter" && event.key !== " ") return;
    const target = event.target;
    if (!(target instanceof Element) || !target.closest(".thead, .thint")) return;
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
  // The host toggles the caret between the editor and the chat, so it has to
  // know which side it is on: the pane's own window focus follows the caret in
  // and out of it.
  window.addEventListener("focus", () => vscode.postMessage({ k: "focus" }));
  window.addEventListener("blur", () => vscode.postMessage({ k: "blur" }));
  resizeInput();
  updateSendState();
  vscode.postMessage({ k: "ready" });
})();
