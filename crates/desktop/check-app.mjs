// Checks the desktop front-end without a window: `ui/app.js` is loaded against
// a stubbed DOM and the Tauri bridge, then driven the way the composer drives it.
//
//   node crates/desktop/check-app.mjs
//
// It covers what `cargo test` cannot reach — the `/mcps` dialog (its listing,
// the toggle, and the no-project and failure paths), the Add-project dialog's
// call into the core, the attachment chips (an image's thumbnail and the
// full-size preview it opens), the question dialog (its title, what a blank
// form sends, and the two ways a request goes away — the run ending, and the CLI
// giving up on it), the reasoning picker (its rows, the level a row applies, and
// the Escape and Close that put it away), and the `/` menu's dispatch of every built-in
// the shared catalog offers — since a Rust test never runs the app's own
// JavaScript. Every command it performs goes through the one Tauri command the
// app registers, so a check in this file that reaches a command by name is
// reaching the same entry point the window does. The
// catalog is read from the built CLI (`target/debug/oxide commands --json`)
// when that binary is present, so a client command the palette offers but the
// app cannot answer is caught here rather than in the window.
import { execFileSync } from "node:child_process";
import { existsSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import vm from "node:vm";

const here = fileURLToPath(new URL("ui/", import.meta.url));
const root = fileURLToPath(new URL("../../", import.meta.url));
const cli = [`${root}target/debug/oxide`, `${root}target/debug/oxide.exe`].find((path) =>
  existsSync(path),
);

const failures = [];
const check = (name, condition, detail = "") => {
  if (condition) {
    console.log(`  ok   ${name}`);
    return;
  }
  console.log(`  FAIL ${name}${detail ? ` — ${detail}` : ""}`);
  failures.push(name);
};

// The catalog the app's `/` menu draws, read from the CLI itself: without it the
// fallback for a command the app cannot perform has nothing to look up. The
// project's own commands, prompt templates and skills load only while it is
// trusted, and a machine that has never answered the trust prompt (a fresh CI
// runner) would list the built-ins alone — the skill checks below would then
// fail for a reason that has nothing to do with the app. The decision is handed
// to the child in a settings file of its own, so the machine's real one stays
// as it was.
let catalog = [];
let catalogSkipped = !cli;
if (cli) {
  const scratch = mkdtempSync(join(tmpdir(), "oxide-check-"));
  const settings = join(scratch, "settings.json");
  writeFileSync(settings, JSON.stringify({ defaultProjectTrust: "always" }));
  try {
    catalog = JSON.parse(
      execFileSync(cli, ["commands", "--json"], {
        cwd: root,
        env: { ...process.env, OXIDE_SETTINGS_FILE: settings },
        encoding: "utf8",
      }),
    );
  } finally {
    rmSync(scratch, { recursive: true, force: true });
  }
}

// ---------- the stubbed page ----------

class StubElement {
  constructor(tag = "div", id = "") {
    this.tagName = tag.toUpperCase();
    this.id = id;
    this.children = [];
    this.style = {};
    this.dataset = {};
    this.hidden = true;
    this.value = "";
    this.textContent = "";
    this.className = "";
    // Classes are kept on `className`, so a state the app toggles (a collapsed
    // listing, the active row of a review) is what the checks read back.
    const classes = () => String(this.className).split(/\s+/).filter(Boolean);
    this.classList = {
      add: (token) => {
        if (!classes().includes(token)) this.className = `${this.className} ${token}`.trim();
      },
      remove: (token) => {
        this.className = classes().filter((name) => name !== token).join(" ");
      },
      toggle: (token, force) => {
        const on = force === undefined ? !classes().includes(token) : Boolean(force);
        if (on) this.classList.add(token);
        else this.classList.remove(token);
        return on;
      },
      contains: (token) => classes().includes(token),
    };
    this.disabled = false;
    this.checked = false;
    this.offsetParent = {};
    this.scrollHeight = 10;
    this.clientHeight = 10;
    this.selectionStart = 0;
    this.selectionEnd = 0;
    this.listeners = {};
    this._innerHTML = "";
  }

  get innerHTML() {
    return this._innerHTML;
  }

  set innerHTML(value) {
    this._innerHTML = String(value);
    this.children = [];
  }

  getBoundingClientRect() {
    return { left: 0, top: 0, right: 300, width: 300, height: 40, bottom: 40 };
  }

  append(...nodes) {
    for (const node of nodes) this.appendChild(node);
  }

  replaceChildren(...nodes) {
    this.children = [];
    this.append(...nodes);
  }

  appendChild(node) {
    this.children.push(node);
    node.parentNode = this;
    // Browsers expose the form control wrapped by a label through `.control`.
    // Keep that relationship so activation checks can follow the label exactly
    // as the desktop webview does.
    if (this.tagName === "LABEL" && ["INPUT", "BUTTON", "SELECT", "TEXTAREA"].includes(node.tagName)) {
      this.control = node;
    }
    return node;
  }

  // A change row puts its diff directly beneath itself, and takes it out again.
  after(node) {
    const parent = this.parentNode;
    if (!parent) return;
    const index = parent.children.indexOf(this);
    parent.children.splice(index + 1, 0, node);
    node.parentNode = parent;
  }

  // Listeners are kept so a check can drive the events the app listens for
  // itself — the document's own click, a row's `mousedown` — which a control's
  // own handler cannot stand in for.
  addEventListener(type, handler) {
    (this.listeners[type] ||= []).push(handler);
  }
  removeEventListener(type, handler) {
    const list = this.listeners[type];
    if (list) this.listeners[type] = list.filter((each) => each !== handler);
  }
  fire(type, event) {
    for (const handler of this.listeners[type] || []) handler(event);
  }
  setPointerCapture() {}
  releasePointerCapture() {}
  setAttribute(name, value) {
    (this.attributes ||= {})[name] = String(value);
  }
  getAttribute(name) {
    return (this.attributes || {})[name] ?? null;
  }
  focus() {
    this.focused = true;
    document.activeElement = this;
  }
  blur() {}
  click() {
    this.onclick?.(press({ target: this, detail: 0 }));
  }
  scrollTo() {}
  // The message box is the one element whose caret the app reads and moves, so
  // the stub keeps one the way a text field does.
  setSelectionRange(start, end) {
    this.selectionStart = start;
    this.selectionEnd = end;
  }

  /// The subset of CSS selectors `app.js` asks for: a class, a
  /// `[data-<key>="value"]` attribute and a `:checked` state, in any order.
  matchesSelector(selector) {
    let rest = String(selector);
    const checked = rest.endsWith(":checked");
    if (checked) {
      if (!this.checked) return false;
      rest = rest.slice(0, -":checked".length);
    }
    const attribute = rest.match(/\[data-([a-z-]+)="([^"]*)"\]/);
    if (attribute) {
      const key = attribute[1].replace(/-([a-z])/g, (_, c) => c.toUpperCase());
      if (String(this.dataset[key]) !== attribute[2]) return false;
      rest = rest.replace(attribute[0], "");
    }
    const classes = String(this.className).split(/\s+/).filter(Boolean);
    for (const token of rest.match(/\.[A-Za-z0-9_-]+/g) || []) {
      if (!classes.includes(token.slice(1))) return false;
    }
    // A `[name]` reads as "this attribute is set", and a leading tag name as the
    // tag itself, which is what `closest("a[href]")` asks for.
    const present = rest.match(/\[([a-zA-Z-]+)\]/);
    if (present) {
      if (this.getAttribute(present[1]) == null) return false;
      rest = rest.replace(present[0], "");
    }
    const tag = rest.match(/^[a-z]+/);
    if (tag && this.tagName !== tag[0].toUpperCase()) return false;
    return true;
  }

  descendants() {
    const found = [];
    for (const child of this.children) found.push(child, ...child.descendants());
    return found;
  }

  querySelector(selector) {
    return this.descendants().find((node) => node.matchesSelector(selector)) || null;
  }
  querySelectorAll(selector) {
    return this.descendants().filter((node) => node.matchesSelector(selector));
  }
  closest(selector) {
    for (let node = this; node; node = node.parentNode) {
      if (node.matchesSelector(selector)) return node;
    }
    return null;
  }

  remove() {
    const parent = this.parentNode;
    if (!parent) return;
    const index = parent.children.indexOf(this);
    if (index >= 0) parent.children.splice(index, 1);
    this.parentNode = null;
  }

  /// The element's own text and markup, then its children, one line each, so a
  /// failure names what was painted.
  outline() {
    const own = [this.className, this.textContent].filter(Boolean).join(": ");
    const lines = [];
    if (this._innerHTML) lines.push(this._innerHTML.replace(/\s+/g, " ").trim());
    if (own) lines.push(`<${this.tagName.toLowerCase()}> ${own}`);
    for (const child of this.children) lines.push(...child.outline().split("\n"));
    return lines.join("\n");
  }
}

// The element kind the page's own markup gives these ids: the page reads a tag
// when it decides what a press is for — a button, a link or a row — so a stub
// standing in for an element `index.html` defines has to be the element the page
// makes it. Everything else the app asks for is a stub of its own.
const pageTags = new Map();
for (const [, tag, id] of readFileSync(`${here}index.html`, "utf8").matchAll(
  /<(input|textarea|select)\b[^>]*\bid="([^"]+)"/g,
)) {
  pageTags.set(id, tag);
}
const elements = new Map();
const elementFor = (id) => {
  if (!elements.has(id)) elements.set(id, new StubElement(pageTags.get(id) || "div", id));
  return elements.get(id);
};
/// A click as the browser delivers it to the control it landed on, which a
/// check hands to that control's own handler.
const press = (over = {}) => ({
  button: 0,
  pointerId: 1,
  clientX: 10,
  clientY: 10,
  // A control inside another one stops the click there, as the browser would
  // have delivered it to the inner one alone.
  stopPropagation() {},
  // A check that cares whether the page refused a default action reads this
  // back: a row of the `@` list refuses the press so the caret stays put.
  preventDefault() {
    this.refused = true;
  },
  ...over,
});
const nextTick = () => new Promise((resolve) => setTimeout(resolve, 0));

// ---------- the fixture the bridge answers with ----------

const servers = [
  {
    name: "filesystem",
    transport: "stdio",
    detail: "npx -y @modelcontextprotocol/server-filesystem /tmp",
    source: "global",
    scope: "global",
    enabled: true,
    state: "connected",
    status: "Connected",
  },
  {
    name: "context7",
    transport: "http",
    detail: "https://mcp.context7.com/mcp/oauth (oauth)",
    source: "global",
    scope: "global",
    enabled: true,
    state: "needs-auth",
    status: "Needs Auth",
  },
  {
    name: "docs",
    transport: "stdio",
    detail: "npx docs-server",
    source: "project",
    scope: "project",
    enabled: false,
    state: "disabled",
    status: "Disabled",
  },
];

const calls = [];
let mcpsError = null;
let createError = null;

// What `check_updates` answers with: the desktop app's own release train, the
// way `oxide_core::updates` resolves it — a `desktop-v*` tag, and the artifact
// this machine would replace this copy with.
let updateAnswer = {
  component: "desktop",
  current: "0.33.0",
  latest: "0.34.0",
  tag: "desktop-v0.34.0",
  pinned: false,
  updateAvailable: true,
  installation: "app bundle",
  installable: true,
  path: "/Applications/Oxide.app",
  advice: null,
  releaseUrl: "https://github.com/jaysonwu991/oxide/releases/tag/desktop-v0.34.0",
  asset: {
    name: "Oxide_0.34.0_aarch64.dmg",
    url: "https://github.com/jaysonwu991/oxide/releases/download/desktop-v0.34.0/Oxide_0.34.0_aarch64.dmg",
    digest: "sha256:86966d1b137203c9ed7366329eb708d02896abce279c3ef33a80d8257a1d68b7",
  },
};
let updateError = null;
// The check's answer as a fresh machine gives it. A test that leaves one behind
// puts this copy back rather than whatever it left.
const OFFERED_UPDATE = { ...updateAnswer };
// An install that could not finish is a rejected `install_update`, which is what
// the app's own download, check or replacement reports.
let installError = null;
// The version a successful install leaves behind, when a test wants one other
// than the release the check resolved: the app resolves the release again as it
// installs, so one published between the check and the click is what lands.
let installedVersion = null;
// What a successful install reports. A Windows installer is started rather than
// run to completion — it asks for elevation and waits for the app to be closed
// — so a test of that machine answers with the release still pending.
let installPending = false;
// What the launch's own install is kept as having said, for a window that
// subscribed to the event channel after it started. The app keeps the newest
// event beside its state and the page asks for it once it is listening; a test
// that wants the page to have missed one sets it here.
let launchHeard = null;
// Installs a test holds in flight, so a check that overlaps one can be measured
// instead of raced.
let installsHeld = 0;
let heldInstalls = [];
const holdNextInstall = () => {
  installsHeld += 1;
};
const releaseHeldInstalls = () => {
  const waiting = heldInstalls;
  heldInstalls = [];
  for (const go of waiting) go();
};
// Checks a test holds in flight, so a reply that arrives after a newer check has
// painted can be measured instead of raced. Each held reply carries the answer
// it would have given at the time it was asked for.
let checksHeld = 0;
let heldChecks = [];
const holdNextCheck = () => {
  checksHeld += 1;
};
const releaseHeldChecks = () => {
  const waiting = heldChecks;
  heldChecks = [];
  for (const go of waiting) go();
};

// What the bridge answers `at_suggestions` with. The token and the rows are the
// core's own rules (`oxide_core::at`, checked by `cargo test`); this only has to
// say what the view does with an answer, so it answers the way the app would
// for the references the checks type — the two rows of a project with one
// folder in it, in the order the core ranks them.
let atError = null;
const atWorkspace = [
  { label: "src/", kind: "folder", insert: "@src/" },
  { label: "src/main.rs", kind: "file", insert: "@src/main.rs " },
];
let answerAt = (text) => defaultAtAnswer(text);
function defaultAtAnswer(text) {
  const start = text.lastIndexOf("@");
  if (start === -1) return { start: 0, end: 0, rows: [] };
  const query = text.slice(start + 1).toLowerCase();
  // A reference that is done (or a word with an `@` in it) has no token open.
  if (!query || /\s/.test(query)) return { start, end: text.length, rows: [] };
  return {
    start,
    end: text.length,
    rows: atWorkspace.filter(
      (row) =>
        row.label.toLowerCase().includes(query) &&
        // The app leaves out a folder the reference already spells, so the row
        // taken next walks into it.
        !(query.endsWith("/") && row.label.toLowerCase() === query),
    ),
  };
}

// The threads the sidebar groups by project and `/sessions` lists for the one
// that is selected, newest first, the way the core orders them.
const existing = [
  {
    id: "fe0031b1",
    name: "Fix the flaky test",
    cwd: "/home/dev/Projects/oxide",
    created_at: 1,
    modified_at: Math.floor(Date.now() / 1000) - 7 * 60,
    message_count: 195,
    preview: "the CI job fails one run in ten",
  },
  {
    id: "7c8031b1",
    name: null,
    cwd: "/home/dev/Projects/oxide",
    created_at: 1,
    modified_at: Math.floor(Date.now() / 1000) - 3 * 86400,
    message_count: 1,
    preview: "say hi",
  },
  {
    id: "a23031b1",
    name: "Other project",
    cwd: "/tmp/elsewhere",
    created_at: 1,
    modified_at: Math.floor(Date.now() / 1000),
    message_count: 4,
    preview: "a thread in another project",
  },
];

// What the bridge answers `all_sessions` with; the empty case is one assignment
// away from the project that has threads, and `threadsError` is the store
// answering nothing at all.
let threads = existing;
let threadsError = null;

// The sidebar's rows as `list_projects` answers them: registered folders first
// (most recently opened first), then projects discovered from sessions. Nothing
// is selected on the way in, so the stub carries two for the picker to offer.
let projectRows = [
  {
    id: "/home/dev/Projects/oxide",
    path: "/home/dev/Projects/oxide",
    name: "oxide",
    registered: true,
    exists: true,
    session_count: 2,
    last_session_at: 1,
    last_opened_at: 9,
  },
  {
    id: "/tmp/elsewhere",
    path: "/tmp/elsewhere",
    name: "elsewhere",
    registered: false,
    exists: true,
    session_count: 1,
    last_session_at: 2,
    last_opened_at: null,
  },
];

// What the bridge answers `change_sides` with: the two sides of one changed
// file as the core aligns them — each line with the number it holds on the side
// it sits on — or a refusal. The checks read what the view paints from an
// answer, so every file the card below lists is given the answer its own check
// needs, and one of them is a file the app can no longer read.
const changeSides = new Map([
  [
    "src/agent.rs",
    {
      lines: [
        { kind: "context", old: 1, new: 1, text: "fn main() {" },
        { kind: "remove", old: 2, new: null, text: "    old();" },
        { kind: "add", old: null, new: 2, text: "    new();" },
      ],
    },
  ],
  [
    "src/tools.rs",
    {
      lines: [
        { kind: "context", old: 1, new: 1, text: "use std::io;" },
        { kind: "context", old: 2, new: 2, text: "use std::path;" },
        { kind: "context", old: 3, new: 3, text: "use std::fs;" },
        // Nine lines neither side touched: longer than the reach the review
        // keeps, so the middle of the stretch folds behind its count.
        ...[4, 5, 6, 7, 8, 9, 10, 11, 12].map((line) => ({
          kind: "context",
          old: line,
          new: line,
          text: `    step_${line}();`,
        })),
        { kind: "remove", old: 13, new: null, text: "    gone();" },
        { kind: "add", old: null, new: 13, text: "    here();" },
        { kind: "context", old: 14, new: 14, text: "}" },
      ],
    },
  ],
  ["assets/logo.png", { binary: true, omitted: false, lines: [] }],
  ["notes.md", { binary: false, omitted: true, lines: [] }],
  ["docs/guide.md", { binary: false, omitted: false, lines: [] }],
]);
const changeSidesError = new Map([
  ["design.md", "the file is no longer in the snapshot"],
]);
let steerAccepted = true;

const invoke = async (command, args = {}) => {
  calls.push([command, args]);
  switch (command) {
    case "list_projects":
      return projectRows.map((row) => ({ ...row }));
    case "mcp_servers":
      if (mcpsError) throw mcpsError;
      if (!String(args.project || "").trim()) throw "select a project first";
      return servers.map((server) => ({ ...server }));
    case "set_mcp_server": {
      if (mcpsError) throw mcpsError;
      const target = servers.find((server) => server.name === args.name);
      if (!target) throw `no MCP server named \`${args.name}\``;
      target.enabled = args.enabled;
      target.state = args.enabled ? "connected" : "disabled";
      target.status = args.enabled ? "Connected" : "Disabled";
      return servers.map((server) => ({ ...server }));
    }
    case "create_project":
      if (createError) throw createError;
      return {
        projects: [{ id: "/tmp/oxide", name: args.name, path: "/tmp/oxide", sessions: 0 }],
        added: "/tmp/oxide",
      };
    case "all_sessions":
      if (threadsError) throw threadsError;
      return threads.map((session) => ({ ...session }));
    case "remove_project":
      projectRows = projectRows.filter((row) => row.id !== args.id);
      return projectRows.map((row) => ({ ...row }));
    case "session_messages": {
      const target = (threads || []).find((session) => session.id === args.id);
      return {
        header: { id: args.id, name: target?.name || null },
        messages: [],
        usage: null,
      };
    }
    case "change_sides": {
      const failure = changeSidesError.get(args.path);
      if (failure) throw failure;
      return changeSides.get(args.path) || { binary: false, omitted: false, lines: [] };
    }
    case "at_suggestions":
      if (atError) throw atError;
      if (!String(args.project || "").trim()) throw "select a project first";
      return answerAt(args.text);
    case "steer_run":
      return steerAccepted;
    case "check_updates": {
      if (updateError) throw updateError;
      const answer = { ...updateAnswer };
      if (checksHeld > 0) {
        checksHeld -= 1;
        await new Promise((resolve) => heldChecks.push(resolve));
      }
      return answer;
    }
    case "install_update": {
      if (installError) throw installError;
      if (installsHeld > 0) {
        installsHeld -= 1;
        await new Promise((resolve) => heldInstalls.push(resolve));
      }
      const version = installedVersion || updateAnswer.latest;
      const tag = `desktop-v${version}`;
      const path = updateAnswer.path;
      const pending = installPending;
      // The check keeps answering with the release it resolves: the install put
      // it on disk, but the build that asks is still the one that started, and
      // only a restart changes that. A window learns the new version by being
      // opened again, never by a check.
      return {
        ok: true,
        pending,
        version,
        tag,
        asset: updateAnswer.asset ? updateAnswer.asset.name : "",
        path,
        text: pending
          ? `The Oxide ${version} installer is running. Finish it, then open Oxide again to run the new version.`
          : `Oxide ${version} is in ${path}. Quit Oxide and open it again to run the new version.`,
      };
    }
    case "launch_update":
      return launchHeard;
    // Everything the rest of `init`/selection asks for; none of it is what this
    // check is about, and all of it stays inside the stub.
    case "list_providers":
    case "list_models":
    case "list_themes":
    case "list_sessions":
    case "list_approvals":
      return [];
    case "list_commands":
      return catalog;
    default:
      return null;
  }
};

const document = {
  getElementById: elementFor,
  createElement: (tag) => new StubElement(tag),
  querySelector: () => null,
  querySelectorAll: () => [],
  activeElement: null,
  // The page closes over the document, so the listeners it puts there are kept
  // too: a link answers a press the way a control answers for itself.
  addEventListener(type, handler) {
    (this.listeners[type] ||= []).push(handler);
  },
  fire(type, event) {
    for (const handler of this.listeners[type] || []) handler(event);
  },
  listeners: {},
  body: new StubElement("body"),
  documentElement: new StubElement("html"),
};

globalThis.document = document;
// The app listens for the events the Rust side emits; the handlers are kept so
// the checks can emit one the way a finished turn does.
const listeners = new Map();
const invokes = [];
globalThis.window = {
  __TAURI__: {
    // The page performs every command through the app's one Tauri command, so
    // the stub unwraps what the page sent the way `oxide_invoke` does and logs
    // the command the app was reached through: the checks below reach the
    // fixture by the name in the page's own call.
    core: {
      invoke: (name, payload = {}) => {
        invokes.push([name, payload]);
        return invoke(payload.command, payload.args || {});
      },
    },
    event: {
      listen: async (name, handler) => {
        if (!listeners.has(name)) listeners.set(name, []);
        listeners.get(name).push(handler);
      },
    },
  },
  innerWidth: 1280,
  innerHeight: 900,
  addEventListener: () => {},
  history: { replaceState: () => {} },
  matchMedia: () => ({ matches: false, addEventListener: () => {} }),
  requestAnimationFrame: (callback) => setTimeout(callback, 0),
  localStorage: { getItem: () => null, setItem: () => {}, removeItem: () => {} },
};
globalThis.localStorage = globalThis.window.localStorage;
globalThis.requestAnimationFrame = globalThis.window.requestAnimationFrame;
globalThis.matchMedia = globalThis.window.matchMedia;
globalThis.addEventListener = () => {};

/// `readFileAsDataUrl` resolves `reader.result`, so the stub fills that in from
/// the file's own data URL.
globalThis.FileReader = class {
  readAsDataURL(file) {
    setTimeout(() => {
      if (!file.dataUrl) {
        this.error = new Error("read failed");
        this.onerror?.();
        return;
      }
      this.result = file.dataUrl;
      this.onload?.();
    }, 0);
  }
};

/// The markup ships the composer ready to type in — a project is not chosen for
/// the reader on the way in, so the box is not gated behind one — and what a
/// message with nowhere to run is answered by is the picker the chip opens.

const source = readFileSync(`${here}app.js`, "utf8");
vm.runInThisContext(
  source +
    "\nglobalThis.__app = { send, runSlashCommand, state, createProjectState," +
    " newChatFromSidebar, openProjects, renderProjects, pickProject, updateProjectChip, renderWelcome, recentThreads, loadProjects," +
    " openCreateProject, addCreateProjectTypedPath, saveCreateProject, openModels," +
    " refreshPaletteEntries, paletteMatches, renderPalette, runPaletteEntry," +
    " addAttachment, addAttachmentFiles, el, showQuestion, showQuestionStep, questionNext," +
    " answerQuestion, collectAnswers, requestAt, acceptAt, moveAt, closeAt, atKey," +
    " showApproval," +
    " resetTranscript, renderChanges, closeReview, undoChanges," +
    " loadSessions, renderProjectsTree, renderSessions, renderMcps, openSession, renderProjectMeta, updateSendState, toggleBusyMessageMode, setBusy, setIdle," +
    " loadMcps, openSessions," +
    " listedSessions, selectSessionFromTree, removeSession," +
    " updateChips," +
    " startTool, finishTool, toggleTool, openUpdate, installUpdate, installedUpdate, catchUpOnLaunchUpdate, launchDismissed };\n",
);

const app = globalThis.__app;
const status = () => String(elementFor("status-text").textContent);
const projectCalls = (command) => calls.filter(([name]) => name === command);
/// Delivers an event to the app the way Tauri does (`event.payload`).
const emit = async (name, payload) => {
  for (const handler of listeners.get(name) || []) await handler({ payload });
};

// ---------- what the window opens on ----------

console.log("launch");
// `init()` ran as the source loaded; its project list arrives on a promise, so
// let the startup chain settle before reading what it opened — which is a window
// with no project in it. A folder is not chosen for the reader on the way in:
// the home state is shown, the composer is ready, and the chip under it takes a
// folder when they pick one.
await nextTick();
check("opened on no project", app.state.project === null, String(app.state.project));
check(
  "marked no project as the active one",
  elementFor("projects-tree").children.every(
    (group) => !String(group.children[0]?.className).includes("active"),
  ),
  elementFor("projects-tree").outline(),
);
check("left the composer ready to type in", elementFor("prompt").disabled === false);
check(
  "named the composer's chip as the way to pick a folder",
  elementFor("project-name").textContent === "Choose a project" &&
    elementFor("project").classList.contains("empty") &&
    /Pick the folder/.test(String(elementFor("project").title)),
  `${elementFor("project-name").textContent} / ${elementFor("project").title}`,
);

// The home state is where a thread is picked up again — the newest threads across
// every folder the sidebar lists, nearest first — since a window that opens on no
// project should open on something to do rather than on an empty box.
const recentRows = () => elementFor("transcript").querySelectorAll(".recent-thread");
const recentText = (index) => recentRows().map((row) => row.children[index].textContent).join(" / ");
const recentRow = (title) =>
  recentRows().find((row) => row.children[0].textContent === title);
check(
  "offered the newest threads across every project",
  recentText(0) === "Fix the flaky test / say hi / Other project",
  recentText(0),
);
check(
  "named the folder each of them is in",
  recentText(1) === "oxide · 7m ago / oxide · 3d ago / elsewhere · just now",
  recentText(1),
);
// Resuming from the home state points the composer at the thread's own project
// first, which is the folder its rows and its next message belong to.
calls.length = 0;
recentRow("Fix the flaky test").onclick();
await nextTick();
await nextTick();
check(
  "opened the thread a home row named, in its own project",
  app.state.project === "/home/dev/Projects/oxide" &&
    app.state.session === "fe0031b1" &&
    projectCalls("session_messages")[0]?.[1]?.project === "/home/dev/Projects/oxide",
  `${app.state.project} / ${app.state.session} / ${JSON.stringify(calls.map(([name]) => name))}`,
);
// A listing that arrives while a thread is on screen belongs to the sidebar, not
// to the transcript: only a transcript holding nothing but the welcome is
// repainted, so the `all_sessions` every turn ends with cannot clear the thread
// it just finished.
const sent = document.createElement("div");
sent.className = "sent-message";
elementFor("transcript").appendChild(sent);
await app.loadSessions();
check(
  "left a thread on screen alone when the listing arrived",
  elementFor("transcript").querySelectorAll(".sent-message").length === 1,
  elementFor("transcript").outline(),
);
// The picker the chip opens carries the same folders the sidebar draws, and
// picking one opens that project — which is also how a thread starts in it,
// since selecting a project clears the transcript for the next message.
app.state.session = null;
app.state.project = null;
app.resetTranscript();
el("projects-modal").hidden = true;
calls.length = 0;
el("project").onclick();
check(
  "opened the folder picker from the composer's chip",
  elementFor("projects-modal").hidden === false &&
    elementFor("project-list").children.length === projectRows.length &&
    calls.length === 0,
  `${elementFor("projects-modal").hidden} / ${elementFor("project-list").outline()}`,
);
check(
  "listed each folder by its name and its path",
  elementFor("project-list").outline().includes("oxide") &&
    elementFor("project-list").outline().includes("/tmp/elsewhere") &&
    elementFor("project-list").outline().includes("elsewhere"),
  elementFor("project-list").outline(),
);
// Escape puts it away like the app's other panels, which is where the picker's
// own id in the overlay list has to be.
document.fire("keydown", { key: "Escape" });
check("closed the picker on Escape", elementFor("projects-modal").hidden === true);
el("project").onclick();
elementFor("project-list").children[1].onclick();
await nextTick();
check(
  "opened the folder the picker named",
  app.state.project === projectRows[1].path &&
    elementFor("projects-modal").hidden === true &&
    elementFor("project-name").textContent === "elsewhere" &&
    !elementFor("project").classList.contains("empty"),
  `${app.state.project} / ${elementFor("project-name").textContent}`,
);
// Picking the project already open only puts the picker away: selecting it again
// would clear a transcript the reader did not ask to leave.
app.state.session = "6f3031b2beef";
calls.length = 0;
el("project").onclick();
elementFor("project-list").children[1].onclick();
await nextTick();
check(
  "left a folder that was already open alone",
  app.state.project === projectRows[1].path &&
    app.state.session === "6f3031b2beef" &&
    elementFor("projects-modal").hidden === true &&
    calls.length === 0,
  `${app.state.session} / ${JSON.stringify(calls.map(([name]) => name))}`,
);
// The chip can be clicked before `list_projects` has answered, when there is
// still nothing to offer: the picker is repainted with the folders that arrive
// rather than left saying there are none until it is closed and opened again.
app.state.projects = [];
el("projects-modal").hidden = true;
el("project").onclick();
check(
  "offered no folders before the list had arrived",
  elementFor("project-list").outline().includes("No projects yet"),
  elementFor("project-list").outline(),
);
await app.loadProjects();
check(
  "filled the picker that was already open",
  elementFor("projects-modal").hidden === false &&
    elementFor("project-list").children.length === projectRows.length,
  `${elementFor("projects-modal").hidden} / ${elementFor("project-list").outline()}`,
);
el("projects-modal").hidden = true;
// A turn belongs to the folder it started in — its run id is that thread's — so
// a folder picked under it is refused rather than leaving the next Queue or
// Steer pointed at the run of a project the chip no longer names.
app.state.session = "6f3031b2beef";
app.setBusy();
calls.length = 0;
el("project").onclick();
elementFor("project-list").children[0].onclick();
await nextTick();
check(
  "refused to switch folders under a running turn",
  app.state.project === projectRows[1].path &&
    app.state.session === "6f3031b2beef" &&
    app.state.busy === true &&
    /turn is running/.test(status()),
  `${app.state.project} / ${app.state.session} / ${app.state.busy} / ${status()}`,
);
// A thread in another project is the same switch by another door, so it lands on
// the same refusal: the run's own folder is the one it stays in.
calls.length = 0;
await app.selectSessionFromTree({ id: "fe0031b1", cwd: projectRows[0].path });
check(
  "refused to resume another folder's thread under it too",
  app.state.project === projectRows[1].path &&
    app.state.session === "6f3031b2beef" &&
    calls.length === 0 &&
    /turn is running/.test(status()),
  `${app.state.project} / ${app.state.session} / ${JSON.stringify(calls.map(([name]) => name))}`,
);
app.setIdle();

// The sidebar's head is the way into a thread, and it never picks a folder for
// the reader: with nothing open it asks which one, with one open it starts the
// thread there, and a running turn owns the thread it is on.
app.state.project = null;
app.state.session = null;
el("projects-modal").hidden = true;
calls.length = 0;
await app.newChatFromSidebar();
check(
  "asked which folder to start in rather than choosing one",
  app.state.project === null &&
    elementFor("projects-modal").hidden === false &&
    calls.length === 0,
  `${app.state.project} / ${elementFor("projects-modal").hidden} / ${JSON.stringify(calls.map(([name]) => name))}`,
);
check(
  "left `/new` to ask for a folder too",
  (await app.runSlashCommand("/new")) === true &&
    app.state.project === null &&
    elementFor("projects-modal").hidden === false,
  `${app.state.project} / ${elementFor("projects-modal").hidden}`,
);
el("projects-modal").hidden = true;
app.state.session = "6f3031b2beef";
app.state.project = projectRows[1].path;
calls.length = 0;
await app.newChatFromSidebar();
check(
  "started it in the folder on screen without re-selecting one",
  app.state.project === projectRows[1].path &&
    app.state.session === null &&
    !calls.some(([name]) => name === "project_info"),
  `${app.state.project} / ${app.state.session} / ${JSON.stringify(calls.map(([name]) => name))}`,
);
app.setBusy();
app.state.session = "6f3031b2beef";
calls.length = 0;
await app.newChatFromSidebar();
check(
  "refused a new thread while a turn was running",
  app.state.session === "6f3031b2beef" &&
    app.state.project === projectRows[1].path &&
    calls.length === 0 &&
    /turn is running/.test(status()),
  `${app.state.session} / ${JSON.stringify(calls.map(([name]) => name))} / ${status()}`,
);
app.setIdle();
const openedWith = app.state.projects;
app.state.projects = [];
app.state.project = null;
el("create-project-modal").hidden = true;
await app.newChatFromSidebar();
check(
  "offered to add a project when there is none to run in",
  elementFor("create-project-modal").hidden === false,
  String(elementFor("create-project-modal").hidden),
);
// Nothing to pick at all is the one case that cannot be answered by the picker,
// and every control that wants a folder gets the same dialog for it.
el("create-project-modal").hidden = true;
calls.length = 0;
await app.openModels();
check(
  "offered to add one from the model chip too",
  elementFor("create-project-modal").hidden === false &&
    elementFor("projects-modal").hidden === true &&
    calls.length === 0,
  `${elementFor("create-project-modal").hidden} / ${elementFor("projects-modal").hidden} / ${JSON.stringify(calls.map(([name]) => name))}`,
);
el("create-project-modal").hidden = true;
app.state.projects = openedWith;

// A message with nowhere to run is answered by the picker rather than sent
// against the directory the app was launched in, and what was typed stays in the
// box while the folder is chosen. The model catalog is read from a project's own
// config, so it asks the same way.
el("prompt").value = "hello";
calls.length = 0;
await app.send();
check(
  "asked for a folder before sending a message",
  calls.length === 0 &&
    el("prompt").value === "hello" &&
    elementFor("projects-modal").hidden === false &&
    status() === "Select a project first.",
  `${status()} / ${el("prompt").value} / ${JSON.stringify(calls.map(([name]) => name))}`,
);
// The folder is picked as the last step of that message rather than in place of
// it: what was typed and what was attached to it are still there to send, and
// only the transcript is the new project's. The model catalog is read from a
// project's own config, so its chip asks for the folder the same way.
calls.length = 0;
el("projects-modal").hidden = true;
await app.openModels();
check(
  "told the reader a model needs a folder first, and asked for one",
  calls.length === 0 &&
    elementFor("models-modal").hidden === true &&
    elementFor("projects-modal").hidden === false &&
    status() === "Select a project first.",
  `${status()} / ${elementFor("projects-modal").hidden} / ${JSON.stringify(calls.map(([name]) => name))}`,
);
app.addAttachment("shot.png", "data:image/png;base64,iVBORw0KGgo=");
elementFor("project-list").children[0].onclick();
await nextTick();
check(
  "kept the message that was waiting for a folder",
  app.state.project === projectRows[0].path &&
    el("prompt").value === "hello" &&
    app.state.attachments.length === 1,
  `${app.state.project} / ${el("prompt").value} / ${app.state.attachments.length}`,
);
// Switching folders with a draft in the box is a different thing — the chips
// were attached to a message meant for the folder being left — so they go.
el("project").onclick();
elementFor("project-list").children[1].onclick();
await nextTick();
check(
  "dropped a draft's attachments when switching folders",
  app.state.project === projectRows[1].path &&
    el("prompt").value === "hello" &&
    app.state.attachments.length === 0,
  `${app.state.project} / ${app.state.attachments.length}`,
);
el("prompt").value = "";

// Back to what the sections below expect to start from — a project open on
// screen — rather than from a window that has just opened.
await app.pickProject(app.state.projects[0]);
check(
  "opened the folder the picker named after that",
  app.state.project === projectRows[0].path &&
    elementFor("project-name").textContent === "oxide",
  `${app.state.project} / ${elementFor("project-name").textContent}`,
);

// ---------- the composer's popovers ----------

// The listings are part of the composer rather than windows over the app: each
// is a sibling above `.composer` inside `.composer-wrap`, so it grows out of the
// composer's top edge and stays stuck to it.
console.log("popovers");
const shell = readFileSync(`${here}index.html`, "utf8");
// Comments come off the stylesheet before anything reads it: the checks below
// parse rules out of it, and a `{` or a `}` written in a comment shifts what the
// parse pairs up.
const sheet = readFileSync(`${here}style.css`, "utf8").replace(/\/\*[\s\S]*?\*\//g, "");
const shellAt = (needle) => shell.indexOf(needle);
const buttonFor = (id) => {
  const at = shell.indexOf(`id="${id}"`);
  if (at < 0) return "";
  return shell.slice(shell.lastIndexOf("<button", at), shell.indexOf("</button>", at));
};
check(
  "attached the listings to the composer",
  shellAt('class="composer-wrap"') < shellAt('id="projects-modal"') &&
    shellAt('id="projects-modal"') < shellAt('id="mcps-modal"') &&
    shellAt('id="mcps-modal"') < shellAt('id="sessions-modal"') &&
    shellAt('id="sessions-modal"') < shellAt('class="composer"'),
  `${shellAt('id="projects-modal"')} / ${shellAt('id="mcps-modal"')} / ${shellAt('id="sessions-modal"')} / ${shellAt('class="composer"')}`,
);
// The project chip sits in the composer's own row, before the controls that
// describe the message: it is the folder the message goes to. It carries words
// rather than a glyph, since with nothing open it is how a first thread starts.
const composerBar = shell.slice(
  shell.indexOf('class="composer-bar"'),
  shell.indexOf('id="send"'),
);
check(
  "put the project chip in the composer's row, with its own words",
  composerBar.includes('id="project"') &&
    /<span id="project-name"[^>]*>[^<]+<\/span>/.test(composerBar) &&
    shell.indexOf('id="project"') < shell.indexOf('id="attach"'),
  composerBar.slice(0, 420),
);
// A project is not selected for the reader on the way in, so the box a message
// is typed into is not gated behind one in the markup either.
check(
  "shipped the composer ready to type in",
  /<textarea id="prompt"/.test(shell) && !/<textarea id="prompt"[^>]*disabled/.test(shell),
  shell.slice(shell.indexOf('<textarea id="prompt"'), shell.indexOf('<textarea id="prompt"') + 120),
);
check(
  "left the popovers out of the overlays",
  !/<div id="(projects|mcps|sessions)-modal" class="overlay"/.test(shell),
);
for (const [id, label] of [
  ["projects-add", "New project"],
  ["projects-close", "Close"],
  ["mcps-refresh", "Recheck the servers"],
  ["mcps-close", "Close"],
  ["sessions-new", "New thread"],
  ["sessions-close", "Close"],
]) {
  const button = buttonFor(id);
  check(
    `made ${id} an icon-only button`,
    // The words are in the tooltip rather than the button: the label exactly,
    // then either the line break a second hint follows or the closing quote.
    new RegExp(`title="${label}(&#10;|")`).test(button) &&
      button.includes(`aria-label="${label}"`) &&
      button.includes("<svg"),
    button,
  );
}
// The review's head carries glyphs like the rest of the chrome: the two that
// walk the turn's files, and the one that closes it — the extension's own close
// in its own class, drawn from the same path.
check(
  "drew the review's head as glyphs",
  ["review-prev", "review-next", "review-close"].every(
    (id) => buttonFor(id).includes("<svg") && /<\/svg>$/.test(buttonFor(id)),
  ),
  `${buttonFor("review-prev")} / ${buttonFor("review-close")}`,
);

// The sidebar's foot keeps the two that belong to the window rather than to a
// message — the release check and the theme — and everything else it used to
// hold stands in the composer beside the message's chips, where it is reachable
// before a project is even open.
const sidebarFoot = shell.slice(shellAt('class="sidebar-foot"'), shellAt("</aside>"));
const composerLeft = shell.slice(shellAt('class="composer-left"'), shellAt('class="composer-right"'));
const idsIn = (markup) => (markup.match(/id="[a-z-]+"/g) || []).join(" ");
check(
  "kept Check for Updates and Theme in the sidebar, and moved the rest to the composer",
  /id="theme"[^>]*title="Theme"[^>]*aria-label="Theme"/.test(sidebarFoot) &&
    /id="update"[^>]*title="Check for updates"/.test(sidebarFoot) &&
    !/id="(connect|trust|permissions|help)"/.test(sidebarFoot) &&
    ["connect", "trust", "permissions", "help"].every((id) =>
      composerLeft.includes(`id="${id}"`),
    ) &&
    shellAt('id="reasoning"') < shellAt('id="connect"'),
  `foot: ${idsIn(sidebarFoot)} · composer: ${idsIn(composerLeft)}`,
);
// The chrome is icon-first, the way the extension's is: each control is a glyph
// whose words live in its tooltip, so the row reads as buttons rather than as a
// sentence. The model and the thinking level are the same chrome with their
// value in the tooltip instead of on the chip, the way the panel's chips carry
// it.
for (const [id, title, label] of [
  ["attach", "Attach images or PDFs (paste with the platform paste shortcut)", "Attach images or PDFs"],
  ["model", "Model", "Model"],
  ["reasoning", "Thinking level", "Thinking level"],
  ["connect", "Connect a provider", "Connect a provider"],
  ["trust", "Project trust", "Project trust"],
  ["permissions", "Saved tool approvals", "Saved tool approvals"],
  ["help", "Keyboard shortcuts", "Keyboard shortcuts"],
]) {
  const button = buttonFor(id);
  check(
    `made ${id} an icon-only button`,
    button.includes('class="chip chip-icon"') &&
      button.includes(`title="${title}"`) &&
      button.includes(`aria-label="${label}"`) &&
      button.includes("<svg") &&
      /<\/svg>$/.test(button),
    button,
  );
}
// The two front-ends lay their composer out the same way round: what the
// extension's own row carries leads — the attach button, the model, the thinking
// level, the project's access — and the controls only this window has follow.
// The panel's own order is read out of the source that composes it rather than
// restated here, so a reorder there fails this check instead of drifting: the
// ids this window has no control for (the panel's agent chip, which this app
// reaches through its own palette) are skipped.
const footerSource = readFileSync(
  new URL("../../editors/vscode/src/core/footer.ts", import.meta.url),
  "utf8",
);
const chipsBlock = (footerSource.match(/chips: \[([\s\S]*?)\n\s*\],/) || [])[1] || "";
const extensionChips = [...chipsBlock.matchAll(/id: "([a-z]+)"/g)].map((match) => match[1]);
const windowIdFor = { model: "model", reasoning: "reasoning", access: "trust" };
const sharedOrder = extensionChips.map((id) => windowIdFor[id]).filter(Boolean);
const composerOrder = ["attach", "model", "reasoning", "trust", "connect", "permissions", "help"];
const rowOrder = idsIn(composerLeft);
const standsInOrder = (list) =>
  list.every((id, index) => rowOrder.includes(id) && (index === 0 || rowOrder.indexOf(list[index - 1]) < rowOrder.indexOf(id)));
check(
  "kept the extension's own order in the composer's row",
  extensionChips.length === 4 &&
    standsInOrder(sharedOrder) &&
    standsInOrder(composerOrder) &&
    shellAt('id="trust"') < shellAt('class="composer-sep"'),
  `${extensionChips.join(" ")} → ${rowOrder}`,
);
check(
  "sized an icon chip a circle of the row's own height, its glyph centered",
  /\.chip-icon \{[^}]*display: inline-flex;[^}]*align-items: center;[^}]*justify-content: center;[^}]*\}/.test(
    sheet,
  ),
  String(sheet.indexOf(".chip-icon")),
);
// The trust control is a glyph now, so the color is the only thing that can say
// which decision is saved for this project.
check(
  "wore the accent on the trusted project's control",
  /\.chip\.active \{[^}]*color: var\(--accent\);[^}]*\}/.test(sheet),
  String(sheet.indexOf(".chip.active")),
);
// A glyph a control wears in both front-ends is the same path in both, held
// verbatim here beside the extension's own sources: a control drawn one way in
// the panel and another in this window fails the check instead of drifting
// quietly, and an icon nothing matches here is what a reviewer would have to
// diff by eye.
const extensionIcons = [
  readFileSync(new URL("../../editors/vscode/src/chatView.ts", import.meta.url), "utf8"),
  readFileSync(new URL("../../editors/vscode/media/main.js", import.meta.url), "utf8"),
].join("\n");
const sharedGlyphs = {
  attach:
    "M21.44 11.05l-9.19 9.19a6 6 0 0 1-8.49-8.49l9.19-9.19a4 4 0 0 1 5.66 5.66l-9.2 9.19a2 2 0 0 1-2.83-2.83l8.49-8.48",
  new: "M8 3.6v8.8M3.6 8h8.8",
  send: "M8 13.4V3.4M4 7.4 8 3.4l4 4",
  stop: '<rect x="4" y="4" width="8" height="8" rx="1.7"',
  refresh: "M20.49 15a9 9 0 1 1-2.12-9.36L23 10",
  close: "M18 6 6 18M6 6l12 12",
  power: "M18.36 6.64a9 9 0 1 1-12.73 0",
  model: "m8 1.8 5.5 3.1v6.2L8 14.2l-5.5-3.1V4.9L8 1.8Z",
  reasoning:
    "M8 1.7 9 5l3.3 1L9 7l-1 3.3L7 7 3.7 6 7 5l1-3.3ZM12.7 9.3l.6 1.8 1.7.6-1.7.6-.6 1.7-.6-1.7-1.8-.6 1.8-.6.6-1.8ZM3.5 10.2l.5 1.3 1.3.5-1.3.5-.5 1.3-.5-1.3-1.3-.5 1.3-.5.5-1.3Z",
  access:
    "M8 1.7c1.7 1.2 3.3 1.7 5 1.8v3.8c0 3.2-1.7 5.5-5 7-3.3-1.5-5-3.8-5-7V3.5c1.7-.1 3.3-.6 5-1.8Z",
};
const windowSources = `${shell}\n${source}`;
for (const [name, path] of Object.entries(sharedGlyphs)) {
  const inExtension = extensionIcons.includes(path);
  const inWindow = windowSources.includes(path);
  check(
    `drew ${name} the way the VS Code extension does`,
    inExtension && inWindow,
    `extension: ${inExtension} · window: ${inWindow}`,
  );
}
// The window around the sidebar already names the app, so the sidebar's own head
// is not a second brand: it is the way into a conversation — the extension's own
// plus with its words beside it, in the one row above the lists that has room for
// them. The Projects header keeps the glyph alone, so the window's two pluses are
// the same drawing and only one of them is worded.
const newChatButton = buttonFor("new-chat");
check(
  "made the sidebar's head a New Chat control instead of a second brand",
  shellAt('id="new-chat"') > 0 &&
    shellAt('id="new-chat"') < shellAt('class="projects-section"') &&
    !/class="brand"/.test(shell) &&
    newChatButton.includes(`title="New Chat&#10;`) &&
    newChatButton.includes('aria-label="New Chat"') &&
    newChatButton.includes(`<path d="${sharedGlyphs.new}"`) &&
    /<\/svg>\s*<span>New Chat<\/span>/.test(newChatButton),
  newChatButton,
);
const newProjectButton = buttonFor("create-project-btn-tree");
check(
  "left the Projects header one glyph and its tooltip",
  newProjectButton.includes('class="icon"') &&
    newProjectButton.includes(`title="New project&#10;`) &&
    newProjectButton.includes('aria-label="New project"') &&
    newProjectButton.includes(`<path d="${sharedGlyphs.new}"`) &&
    /<\/svg>$/.test(newProjectButton),
  newProjectButton,
);

// A scrollbar is painted over the last pixels of the box it scrolls, so every
// list that scrolls keeps that lane clear with its own right padding: a row ends
// beside the bar rather than under it, an overlay bar (which takes no lane of its
// own) lands in the padding instead of on the row, and the buttons a row carries
// stay clickable. The width is read out of the sheet's own scrollbar rule, so a
// list that loses its padding — or a lane made wider than the padding that was
// left for it — fails here instead of only in a screenshot.
const scrolledRows = [
  ".conversation",
  ".question-body",
  ".projects-tree",
  ".mcp-list",
  ".session-list",
  ".project-list",
  ".palette-list",
  ".model-list",
  ".review-files",
];
const laneWidth = Number(
  (sheet.match(/::-webkit-scrollbar\s*\{\s*width:\s*([\d.]+)px/) || [])[1],
);
const rightPaddingOf = (selector) => {
  let value = null;
  for (const [, selectors, body] of sheet.matchAll(/([^{}]+)\{([^}]*)\}/g)) {
    const applies = selectors
      .split(",")
      .some((one) => one.trim() === selector || one.trim().endsWith(` ${selector}`));
    if (!applies) continue;
    const padding = /(?:^|;)\s*padding:\s*([^;]+)/.exec(body);
    if (padding) {
      const parts = padding[1].trim().split(/\s+/);
      value = parseFloat(parts.length > 1 ? parts[1] : parts[0]);
    }
    const right = /(?:^|;)\s*padding-right:\s*([^;]+)/.exec(body);
    if (right) value = parseFloat(right[1]);
  }
  return value;
};
for (const selector of scrolledRows) {
  const padding = rightPaddingOf(selector);
  check(
    `kept ${selector}'s rows clear of the scrollbar's lane`,
    Number.isFinite(padding) && Number.isFinite(laneWidth) && padding >= laneWidth + 2,
    `lane ${laneWidth} · padding-right ${padding}`,
  );
}

// The sidebar's own button, because the menu item that opens the same dialog
// is macOS's: Windows and Linux build no menu bar, so the window is the way in
// there. The dialog reports the CLI's release, so the note under it says which
// installation an install would touch.
const updateButton = buttonFor("update");
check(
  "offered Check for Updates in the sidebar's foot",
  updateButton.includes('class="icon"') &&
    updateButton.includes('title="Check for updates"') &&
    updateButton.includes('aria-label="Check for updates"') &&
    sidebarFoot.includes('id="update"'),
  updateButton,
);
// The two that stayed in the foot are glyphs too, so the sidebar's own row has
// no words in it either.
check(
  "drew the sidebar's foot as glyphs",
  ["theme", "update"].every((id) => buttonFor(id).includes("<svg") && /<\/svg>$/.test(buttonFor(id))),
  `${buttonFor("theme")} / ${updateButton}`,
);
check(
  "put the dialog among the overlays Escape closes",
  /<div id="update-modal" class="overlay" role="dialog" aria-modal="true" aria-labelledby="update-title" hidden>/.test(
    shell,
  ) && shellAt('id="update"') < shellAt('id="update-modal"'),
  String(shellAt('id="update-modal"')),
);
check(
  "gave the dialog a title, a note, a body and its own actions",
  /<p class="dialog-sub" id="update-note">/.test(shell) &&
    shellAt('id="update-note"') < shellAt('id="update-body"') &&
    shellAt('id="update-body"') < shellAt('id="update-close"') &&
    shellAt('id="update-notes"') < shellAt('id="update-close"') &&
    shellAt('id="update-close"') < shellAt('id="update-install"'),
  `${shellAt('id="update-body"')} / ${shellAt('id="update-close"')}`,
);
check(
  "offered nothing before the check has answered",
  /id="update-notes" class="ghost" hidden/.test(shell) &&
    /id="update-install" class="primary" hidden/.test(shell),
  shell.slice(shellAt('id="update-notes"') - 20, shellAt('id="update-install"') + 40),
);
// A release the launch installed on its own is not the app that is running, so
// what is left to do is a restart: the dialog offers it beside the install it
// replaced, and only one of the two is ever on screen.
check(
  "offered the restart that runs an install, beside the install it replaced",
  /id="update-restart" class="primary" hidden/.test(shell) &&
    shellAt('id="update-install"') < shellAt('id="update-restart"'),
  shell.slice(shellAt('id="update-install"'), shellAt('id="update-restart"') + 40),
);
// An install nobody in the window asked for has to be visible without opening a
// dialog, so its row is the sidebar's own — between the projects it lists and
// the foot — with the restart that runs it beside a glyph that puts the row
// away without stopping the install.
check(
  "put the launch's own install in the sidebar, between the projects and its foot",
  /<div id="update-banner" class="update-banner" role="status" hidden>/.test(shell) &&
    shellAt('class="sidebar"') < shellAt('id="update-banner"') &&
    shellAt('id="update-banner"') < shellAt('class="sidebar-foot"') &&
    /id="update-banner-restart" class="primary small" hidden/.test(shell) &&
    /id="update-banner-dismiss"[^>]*title="Dismiss/.test(shell) &&
    /<\/svg>$/.test(buttonFor("update-banner-dismiss")),
  String(shellAt('id="update-banner"')),
);

// ---------- `@path` completion ----------

console.log("@path completion");
const atBox = elementFor("at-list");
const atRows = elementFor("at-rows");
const composer = elementFor("prompt");
const tick = () => new Promise((resolve) => setTimeout(resolve, 0));
/// Types a value into the composer and asks the app about it the way the input
/// handler does, with the caret where the text field would have it.
const typeAt = async (value, caret = null) => {
  composer.value = value;
  composer.selectionStart = caret ?? value.length;
  await app.requestAt();
};

check(
  "hung the list off the composer as the palette's own box",
  /<div id="at-list" class="palette" hidden>/.test(shell) &&
    /id="at-rows" class="palette-list" role="listbox"/.test(shell),
  "",
);

app.state.project = "/home/dev/Projects/oxide";
calls.length = 0;
await typeAt("review @sr");
check(
  "offered the project's own files for the reference",
  atBox.hidden === false && atRows.children.length === 2,
  atRows.outline(),
);
check(
  "marked the first row as the one Enter takes",
  String(atRows.children[0].className).includes("active"),
  String(atRows.children[0].className),
);
check(
  "drew a folder as a folder and a file as a file",
  String(atRows.children[0].innerHTML).includes("\u25b8") &&
    String(atRows.children[1].innerHTML).includes("\u00b7") &&
    String(atRows.children[1].innerHTML).includes("src/main.rs"),
  atRows.outline(),
);
check(
  "told the app where the caret is",
  projectCalls("at_suggestions").at(-1)?.[1]?.caret === 10 &&
    projectCalls("at_suggestions").at(-1)?.[1]?.text === "review @sr",
  JSON.stringify(projectCalls("at_suggestions").at(-1)),
);

app.moveAt(1);
check(
  "walked the list with the arrows",
  String(atRows.children[1].className).includes("active"),
  String(atRows.children[1].className),
);
app.moveAt(-1);
app.atKey({ key: "Enter" });
check(
  "spliced the folder in and left its reference open",
  composer.value === "review @src/" && composer.selectionStart === 12,
  `${composer.value} @ ${composer.selectionStart}`,
);
await tick();
check(
  "offered what is inside the folder next",
  atBox.hidden === false &&
    atRows.children.length === 1 &&
    String(atRows.children[0].innerHTML).includes("src/main.rs"),
  atRows.outline(),
);
app.atKey({ key: "Tab" });
check(
  "spliced the file in with a space after it",
  composer.value === "review @src/main.rs " && composer.selectionStart === 20,
  `${composer.value} @ ${composer.selectionStart}`,
);
await tick();
check("closed the list once the reference was done", atBox.hidden === true);

// A row of the list is clicked like anything else, while the press is refused
// so the caret stays in the message box it is completing into.
await typeAt("review @sr");
const atPress = press();
atRows.children[0].fire("mousedown", atPress);
check(
  "kept the caret in the message box when a row is clicked",
  atPress.refused === true,
  String(atPress.refused),
);
atRows.children[0].onclick(atPress);
check(
  "took the row a click landed on",
  composer.value === "review @src/" && composer.selectionStart === 12,
  `${composer.value} @ ${composer.selectionStart}`,
);

// The webview owns control activation. The page never synthesizes a click or
// changes focus on a control's behalf.
const composerButton = new StubElement("button", "composer-button");
let composerButtonClicks = 0;
composerButton.onclick = () => composerButtonClicks++;
document.activeElement = elementFor("prompt");
const buttonPress = press({ target: composerButton });
document.fire("mousedown", buttonPress);
check(
  "left a Chromium button press native while the message box had the caret",
  buttonPress.refused !== true,
  String(buttonPress.refused),
);
document.fire("mouseup", buttonPress);
composerButton.onclick(buttonPress);
await nextTick();
check("handled Chromium's one click exactly once", composerButtonClicks === 1, composerButtonClicks);

// A failed read is painted into the list instead of becoming an empty result.
for (const [name, listId, breakRead, read] of [
  [
    "MCP",
    "mcp-list",
    () => {
      mcpsError = "connection refused";
    },
    app.loadMcps,
  ],
  [
    "sessions",
    "sessions-list",
    () => {
      threadsError = "permission denied";
    },
    app.openSessions,
  ],
]) {
  const page = elementFor(listId);
  const list = new StubElement("div", listId);
  elements.set(listId, list);
  list.innerHTML = "";
  breakRead();
  await read();
  check(
    `painted the ${name} failure`,
    list.outline().includes("Could not"),
    list.outline() || "painted no failure",
  );
  elements.set(listId, page);
}
mcpsError = null;
threadsError = null;
elementFor("mcps-modal").hidden = true;
elementFor("sessions-modal").hidden = true;
await app.loadMcps();
await app.loadSessions();

document.activeElement = null;
elementFor("create-project-modal").hidden = true;

await typeAt("review @sr");
app.atKey({ key: "Escape" });
check("closed the list on Escape", atBox.hidden === true);
check("left a key the list does not own alone", app.atKey({ key: "a" }) === false);

// The caret moves without the value changing, so the ask carries wherever it
// landed rather than the end of the message.
await typeAt("review @src/main.rs more", 18);
check(
  "asked about the caret the reader moved into a reference",
  projectCalls("at_suggestions").at(-1)?.[1]?.caret === 18,
  JSON.stringify(projectCalls("at_suggestions").at(-1)),
);

// An answer that arrives after the value has moved on must not paint its rows
// under a caret they no longer belong to.
const held = [];
answerAt = (text) =>
  new Promise((resolve) =>
    held.push(() =>
      resolve({
        start: 0,
        end: 0,
        rows: [{ label: `from:${text}`, kind: "file", insert: `@${text} ` }],
      }),
    ),
  );
const firstAsk = app.requestAt();
composer.value = "review @sr/docs";
composer.selectionStart = 16;
const secondAsk = app.requestAt();
held[1]();
await secondAsk;
held[0]();
await firstAsk;
check(
  "painted the answer for the value that is there",
  String(atRows.children[0]?.innerHTML).includes("from:review @sr/docs"),
  atRows.outline(),
);
answerAt = (text) => defaultAtAnswer(text);

// An app that cannot answer leaves the list closed rather than throwing into
// the window, and a project that is not selected is not asked about at all.
atError = "could not read the project";
await typeAt("review @sr");
check("closed the list when the app refused", atBox.hidden === true);
atError = null;
calls.length = 0;
const project = app.state.project;
app.state.project = null;
await typeAt("review @sr");
check(
  "asked for nothing without a project",
  projectCalls("at_suggestions").length === 0 && atBox.hidden === true,
);
app.state.project = project;

// The message is on its way, so the list has nothing left to complete.
await typeAt("review @sr");
await app.send(false);
check(
  "closed the list when the message went out",
  atBox.hidden === true && composer.value === "",
  `${atBox.hidden} / ${composer.value}`,
);
app.state.busy = false;
app.state.runId = null;

// ---------- /mcps ----------

const driveMcps = async () => {
  elementFor("mcp-list").children = [];
  elementFor("prompt").value = "/mcps";
  await app.send(false);
  return elementFor("mcp-list").outline();
};

console.log("/mcps");
app.state.project = null;
calls.length = 0;
let opened = await driveMcps();
check("consumed the composer line instead of prompting", projectCalls("send_prompt").length === 0);
check("did not ask the core for a project it has", projectCalls("mcp_servers").length === 0);
check("said a project is needed", status() === "Select a project first.", status());
check("left the dialog closed", elementFor("mcps-modal").hidden === true);
check("painted nothing", opened.trim() === "", opened);

app.state.project = "/home/dev/Projects/oxide";
calls.length = 0;
opened = await driveMcps();
check("asked for this project's servers", projectCalls("mcp_servers")[0]?.[1]?.project === app.state.project);
check("opened the dialog", elementFor("mcps-modal").hidden === false);
check("cleared the composer", elementFor("prompt").value === "");
for (const [name, state, text] of [
  ["filesystem", "connected", "Connected"],
  ["context7", "needs-auth", "Needs Auth"],
  ["docs", "disabled", "Disabled"],
]) {
  check(`listed ${name} as ${state}`, opened.includes(`mcp-name: ${name}`) && opened.includes(`mcp-status state-${state}: ${text}`), opened);
}
check(
  "showed the transport, detail and source of a server",
  opened.includes("stdio · npx -y @modelcontextprotocol/server-filesystem /tmp") &&
    opened.includes("source: global") &&
    opened.includes("http · https://mcp.context7.com/mcp/oauth (oauth)"),
  opened,
);
// The toggle writes through the core and redraws from its answer.
const rowFor = (name) =>
  elementFor("mcp-list").children.find((row) =>
    row.children[0].children[0].textContent === name,
  );
check("a row exposes its name, status and toggle", Boolean(rowFor("filesystem")));
check(
  "offered a power switch for a running server and for a stopped one",
  String(rowFor("filesystem").children[0].children[2].title).startsWith("Disable filesystem") &&
    String(rowFor("docs").children[0].children[2].title).startsWith("Enable docs"),
  elementFor("mcp-list").outline(),
);
check(
  "painted each switch as an icon, sized to the row",
  rowFor("filesystem").children[0].children[2].className === "icon mcp-toggle on" &&
    rowFor("docs").children[0].children[2].className === "icon mcp-toggle" &&
    rowFor("filesystem").children[0].children[2].innerHTML.includes("<svg"),
  elementFor("mcp-list").outline(),
);
const toggle = rowFor("filesystem").children[0].children[2];
calls.length = 0;
await toggle.onclick();
check(
  "asked the core to turn the server off in this project",
  JSON.stringify(projectCalls("set_mcp_server")[0]?.[1]) ===
    JSON.stringify({ project: app.state.project, name: "filesystem", enabled: false }),
  JSON.stringify(projectCalls("set_mcp_server")),
);
const after = elementFor("mcp-list").outline();
check(
  "redrew from the core's answer",
  rowFor("filesystem").children[0].children[1].textContent === "Disabled" &&
    String(rowFor("filesystem").children[0].children[2].title).startsWith("Enable filesystem") &&
    rowFor("filesystem").children[0].children[2].className === "icon mcp-toggle",
  after,
);
check("reported the toggle", status() === "Ready", status());

// A failing core call is shown, not swallowed into an empty dialog.
mcpsError = "connection refused";
await driveMcps();
const failed = elementFor("mcp-list").innerHTML;
check("showed why the listing failed", String(failed).includes("connection refused"), String(failed));
mcpsError = null;

// Only the bare command is the app's own; an argument is the agent's.
calls.length = 0;
elementFor("mcp-list").children = [];
elementFor("mcps-modal").hidden = true;
elementFor("prompt").value = "/mcp list";
await app.send(false);
check(
  "sent `/mcp list` on as a prompt",
  projectCalls("send_prompt")[0]?.[1]?.prompt === "/mcp list",
  JSON.stringify(projectCalls("send_prompt")),
);
check(
  "left the dialog closed and asked for nothing",
  elementFor("mcps-modal").hidden === true && projectCalls("mcp_servers").length === 0,
);
app.state.busy = false;
app.state.runId = null;

// ---------- /sessions ----------

console.log("/sessions");
// A command the app performs itself never reaches the model, and the list is
// drawn in the app rather than handed to the window as a native picker.
app.state.project = "/home/dev/Projects/oxide";
app.state.projects = [
  { id: "/home/dev/Projects/oxide", name: "oxide", path: "/home/dev/Projects/oxide" },
];
elementFor("sessions-modal").hidden = true;
calls.length = 0;
const handled = await app.runSlashCommand("/sessions");
check("consumed the command instead of prompting", handled === true && projectCalls("send_prompt").length === 0);
check("asked the core for the project's threads", projectCalls("all_sessions").length === 1, JSON.stringify(projectCalls("all_sessions")));
check("opened the dialog", elementFor("sessions-modal").hidden === false);

const listed = elementFor("sessions-list").children;
check("listed this project's threads only", listed.length === 2, String(listed.length));
check(
  "named a thread by its name, and an unnamed one by what was sent",
  listed[0]?.children[0]?.children[0]?.textContent === "Fix the flaky test" &&
    listed[1]?.children[0]?.children[0]?.textContent === "say hi",
  elementFor("sessions-list").outline(),
);
check(
  "said how long ago it was used and how much is in it",
  listed[0]?.children[0]?.children[1]?.textContent === "7m ago · 195 messages" &&
    listed[1]?.children[0]?.children[1]?.textContent === "3d ago · 1 message",
  elementFor("sessions-list").outline(),
);
check(
  "titled a row with the words it shows rather than its own elements",
  listed[0]?.title === "Fix the flaky test — 7m ago · 195 messages" &&
    listed[1]?.title === "say hi — 3d ago · 1 message",
  String(listed[0]?.title),
);

// Picking a row opens that thread and closes the dialog; the row is a button so
// the keyboard reaches it too.
calls.length = 0;
check("made each row a button", listed[0]?.tagName.toLowerCase() === "button");
await listed[0].onclick();
check("closed the dialog on the pick", elementFor("sessions-modal").hidden === true);
check(
  "loaded the thread that was picked",
  projectCalls("session_messages")[0]?.[1]?.id === "fe0031b1",
  JSON.stringify(projectCalls("session_messages")),
);
check("left `/sessions <id>` to the agent", (await app.runSlashCommand("/session fe0031b1")) === false);

threads = [];
// With no thread open either: the thread the window is in is listed even when
// the store has none, so an empty listing is the case with nothing open at all.
const openThread = app.state.session;
app.state.session = null;
await app.runSlashCommand("/sessions");
check(
  "said so when the project has no threads",
  elementFor("sessions-list").innerHTML.includes("No threads for this project yet"),
  elementFor("sessions-list").innerHTML,
);
app.state.session = openThread;

// A store that cannot be read is not the same as a project with no threads, and
// saying so is the whole point of a listing opened where it was asked.
threadsError = "permission denied";
await app.runSlashCommand("/sessions");
check(
  "showed why the thread listing failed instead of an empty one",
  elementFor("sessions-list").innerHTML.includes("permission denied") &&
    !elementFor("sessions-list").innerHTML.includes("No threads for this project yet"),
  elementFor("sessions-list").innerHTML,
);
threadsError = null;

threads = existing;
app.state.sessions = existing;
elementFor("sessions-modal").hidden = true;

// ---------- the Add-project dialog ----------

console.log("create project");
app.state.project = null;
app.openCreateProject();
check("opened the dialog with empty fields", elementFor("create-project-name").value === "" && elementFor("create-project-path").value === "");
check("kept the dialog open until it is saved", elementFor("create-project-modal").hidden === false);

elementFor("create-project-path").value = "~/Projects/oxide";
app.addCreateProjectTypedPath();
check("took the typed path as a folder", app.createProjectState.folders.length === 1, JSON.stringify(app.createProjectState.folders));
check("named the project after the folder", elementFor("create-project-name").value === "oxide", elementFor("create-project-name").value);
check("listed the folder as removable", elementFor("create-project-folders").outline().includes("~/Projects/oxide"), elementFor("create-project-folders").outline());

calls.length = 0;
await app.saveCreateProject();
const created = projectCalls("create_project")[0]?.[1];
check(
  "asked the core to create it from the folder",
  JSON.stringify(created) === JSON.stringify({ name: "oxide", folders: ["~/Projects/oxide"] }),
  JSON.stringify(created),
);
check("closed the dialog", elementFor("create-project-modal").hidden === true);
check("selected the new project", app.state.project === "/tmp/oxide", String(app.state.project));

createError = "Failed to add folder /tmp/oxide: No such file or directory";
app.state.project = null;
app.openCreateProject();
elementFor("create-project-path").value = "/tmp/oxide";
app.addCreateProjectTypedPath();
await app.saveCreateProject();
const message = elementFor("create-project-error").textContent;
check("reported why the project could not be added", String(message).includes("No such file or directory"), String(message));
check("left the dialog open to correct it", elementFor("create-project-modal").hidden === false);
createError = null;

// ---------- attachment previews ----------

console.log("attachments");
const shot = "data:image/png;base64,iVBORw0KGgo=";
check("kept an image attachment", app.addAttachment("shot.png", shot) === true);
const chips = () => elementFor("attachments").children;
const chip = chips()[0];
// The chip is [thumbnail, name, remove]; the thumbnail is the button that opens
// the full-size preview, which is what makes the image inspectable before it is
// sent to a model that may be looking at a downscaled copy.
const opener = chip && chip.children[0];
check(
  "showed a thumbnail that opens the full image",
  opener?.tagName.toLowerCase() === "button" && opener.children[0]?.src === shot,
  elementFor("attachments").outline(),
);
check("named the attachment", chip?.children[1]?.textContent === "shot.png", chip?.children[1]?.textContent);
// WebKit drags a picture out of the page by default, and a drag started on the
// thumbnail is the one gesture whose click is withheld, so the picture is not
// allowed to become one in the first place.
check(
  "let a thumbnail not be dragged out of the composer",
  opener?.children[0]?.draggable === false &&
    /\.att-open img \{ -webkit-user-drag: none;[^}]*\}/.test(sheet),
  String(opener?.children[0]?.draggable),
);
document.activeElement = opener;
opener.focused = false;
elementFor("image-view-close").focused = false;
opener.onclick();
check(
  "opened the full-size preview",
  elementFor("image-view-img").src === shot && elementFor("image-modal").hidden === false,
  String(elementFor("image-modal").hidden),
);
check(
  "moved focus into the full-size preview",
  document.activeElement === elementFor("image-view-close"),
  document.activeElement?.id,
);
let trappedPreviewTab = false;
elementFor("image-modal").fire("keydown", {
  key: "Tab",
  preventDefault() { trappedPreviewTab = true; },
});
check(
  "kept keyboard focus inside the full-size preview",
  trappedPreviewTab && document.activeElement === elementFor("image-view-close"),
  `${trappedPreviewTab} / ${document.activeElement?.id}`,
);
elementFor("image-view-close").onclick();
check(
  "closed it again and returned focus to its thumbnail",
  elementFor("image-modal").hidden === true && document.activeElement === opener,
  `${elementFor("image-modal").hidden} / ${document.activeElement?.id}`,
);
check(
  "gave the preview an icon to close it with",
  /id="image-view-close"[^>]*aria-label="Close"[^>]*>✕$/.test(buttonFor("image-view-close")),
  buttonFor("image-view-close"),
);
check(
  "put that icon outside the picture, in the preview's own head row",
  /class="image-head"[\s\S]*id="image-view-close"[\s\S]*id="image-view-img"/.test(shell) &&
    /\.image-view \{[^}]*flex-direction: column;[^}]*\}/.test(sheet) &&
    !/class="image-frame"/.test(shell) &&
    !/#image-view-close \{[^}]*position: absolute;[^}]*\}/.test(sheet) &&
    /#image-view-close \{ color: var\(--error\); \}/.test(sheet),
  `${shellAt("class=\"image-head\"")} / ${sheet.indexOf("#image-view-close {")}`,
);
// Every button the shell declares is wired to an action of its own.
const shellButtons = [...shell.matchAll(/<button[^>]*\bid="([^"]+)"/g)].map((m) => m[1]);
const unwiredButtons = shellButtons.filter((id) => typeof elementFor(id).onclick !== "function");
check(
  "wired every button the shell declares to an action",
  shellButtons.length > 0 && unwiredButtons.length === 0,
  unwiredButtons.join(", "),
);
// The thumbnail is an ordinary button, so a keyboard activation fires the same
// click a mouse does: the picture never becomes a drag instead.
elementFor("image-modal").hidden = true;
opener.onclick({ detail: 0 });
check(
  "opened the preview from the thumbnail's click",
  elementFor("image-view-img").src === shot && elementFor("image-modal").hidden === false,
  String(elementFor("image-modal").hidden),
);
elementFor("image-view-close").onclick();

check("kept a PDF attachment", app.addAttachment("report.pdf", "data:application/pdf;base64,AA") === true);
const pdf = chips()[1];
check(
  "gave a PDF a glyph instead of a thumbnail",
  pdf?.children[0]?.className === "att-file" && pdf.children[0].textContent === "📄",
  elementFor("attachments").outline(),
);
check("refused any other file", app.addAttachment("notes.txt", "data:text/plain;base64,AA") === false);
check(
  "refused a format the webview cannot paint",
  app.addAttachment("scan.tif", "data:image/tiff;base64,AA") === false &&
    status() === "Only PNG, JPEG, GIF, WebP, BMP and PDF can be attached",
  status(),
);
app.state.attachments = [];

// A paste or a pick reads the file into a data URL before anything else, so
// the size is checked first: an over-large file never becomes a string in the
// webview, and the core would refuse it at the far end anyway.
await app.addAttachmentFiles([
  { name: "huge.pdf", size: 26 * 1024 * 1024, type: "application/pdf", dataUrl: "data:application/pdf;base64,AA" },
]);
check(
  "refused a file past the attachment limit without reading it",
  app.state.attachments.length === 0 && status().includes("the attachment limit is 20.0 MB"),
  status(),
);
await app.addAttachmentFiles([
  { name: "small.pdf", size: 1024, type: "application/pdf", dataUrl: "data:application/pdf;base64,AA" },
]);
check(
  "attached the file that was within the limit",
  app.state.attachments.length === 1 && app.state.attachments[0].name === "small.pdf",
  JSON.stringify(app.state.attachments),
);
app.state.attachments = [];

// The browser has already typed a picked file, so a format nothing here can
// paint is refused before it is read: a TIFF with nothing readable behind it
// would fail the read (and base64 itself) if the check came after it.
await app.addAttachmentFiles([{ name: "scan.tif", size: 1024, type: "image/tiff" }]);
check(
  "refused a format the webview cannot paint without reading it",
  app.state.attachments.length === 0 && status() === "Cannot attach scan.tif: image/tiff is not one of PNG, JPEG, GIF, WebP, BMP and PDF",
  status(),
);
// A blob the browser did not type is still decided by the data URL it becomes.
await app.addAttachmentFiles([
  { name: "shot.png", size: 1024, type: "", dataUrl: "data:image/png;base64,AA" },
]);
check(
  "attached an untyped blob that reads as an image",
  app.state.attachments.length === 1 && app.state.attachments[0].name === "shot.png",
  JSON.stringify(app.state.attachments),
);
app.state.attachments = [];

// A chip is [thumbnail, name, remove]: each button is clicked on its own, and
// the browser delivers the click to the one under the pointer rather than to
// the chip around it.
app.addAttachment("shot.png", shot);
const chipRemove = chips()[0].querySelector(".att-remove");
chipRemove.onclick(press({ target: chipRemove }));
check(
  "removed the chip from its own ✕, without opening the picture beside it",
  app.state.attachments.length === 0 && elementFor("image-modal").hidden === true,
  `${app.state.attachments.length} / ${elementFor("image-modal").hidden}`,
);
elementFor("help-modal").hidden = false;
elementFor("help-close").onclick(press());
check(
  "closed a dialog from its own button",
  elementFor("help-modal").hidden === true,
  String(elementFor("help-modal").hidden),
);

// A link opens once per click, and through the app, because the webview cannot
// navigate to a remote page itself.
const link = new StubElement("a");
link.setAttribute("href", "https://example.com/docs");
elementFor("transcript").appendChild(link);
const openedLinks = () => calls.filter(([name]) => name === "open_url");
calls.length = 0;
document.fire("click", press({ target: link }));
check(
  "opened a link from a click, through the app",
  openedLinks().length === 1 && openedLinks()[0][1]?.url === "https://example.com/docs",
  JSON.stringify(calls),
);
calls.length = 0;
document.fire("click", press({ target: elementFor("transcript") }));
check(
  "left a click that landed on no link alone",
  openedLinks().length === 0,
  JSON.stringify(calls),
);
link.remove();
calls.length = 0;

console.log("questions");
app.state.project = "/home/dev/Projects/oxide";
app.state.pendingQuestion = null;
calls.length = 0;
app.showQuestion({
  id: 42,
  questions: [
    {
      header: "Database",
      question: "Which database should the migration target?",
      options: [
        { label: "Postgres", description: "The production store" },
        { label: "SQLite", description: "Local development" },
      ],
    },
    {
      question: "Which extras?",
      multiSelect: true,
      options: [{ label: "Indexes" }, { label: "Fixtures" }],
    },
  ],
});
const dialog = elementFor("question-body");
const blocks = dialog.children;
/// The dash per question, which says where in the list the reader is.
const dashes = () => elementFor("question-progress").children;
check("opened the question dialog", elementFor("question").hidden === false);
check(
  "asked the question it opens on, with that question's own header above it",
  elementFor("question-title").textContent === "Which database should the migration target?" &&
    elementFor("question-header").textContent === "Database" &&
    elementFor("question-header").hidden === false,
  `${elementFor("question-header").textContent} / ${elementFor("question-title").textContent}`,
);
check(
  "counted the questions and marked the one being asked",
  elementFor("question-step").textContent === "1 of 2 questions" &&
    elementFor("question-step-row").hidden === false &&
    dashes().length === 2 &&
    dashes()[0].className.includes("current") &&
    dashes()[1].className.includes("todo"),
  `${elementFor("question-step").textContent} ${dashes().map((dash) => dash.className).join(",")}`,
);
check(
  "asked one question at a time",
  blocks.length === 2 && blocks[0].hidden === false && blocks[1].hidden === true,
  blocks.map((block) => block.hidden).join(","),
);
check(
  "named the single question's hint",
  elementFor("question-hint").textContent === "Select one answer" &&
    elementFor("question-hint").hidden === false,
  elementFor("question-hint").textContent,
);
const firstBlock = blocks[0];
const optionRows = firstBlock.children.filter((row) => row.tagName === "LABEL");
check(
  "offered a single answer's options as radios",
  optionRows.length === 3 && optionRows.every((row) => row.children[0].type === "radio"),
  dialog.outline(),
);
check(
  "preselected a single-answer question's first option",
  optionRows[0].children[0].checked === true && optionRows[1].children[0].checked === false,
  dialog.outline(),
);
check(
  "painted each option's description under its label",
  firstBlock.outline().includes("The production store"),
  firstBlock.outline(),
);
// The user's own wording is one of the choices, the way the dialog it follows
// offers it, with the field to type it in under the row.
check(
  "offered the user's own answer as a choice with a field under it",
  optionRows[2].children[1].textContent === "Type your own answer" &&
    optionRows[2].children[0].value === "" &&
    firstBlock.querySelector(".question-free").placeholder === "Type your answer…",
  firstBlock.outline(),
);

// Chromium owns native radio activation while a text field is focused.
const secondRadio = optionRows[1].control;
let radioActivations = 0;
secondRadio.click = () => {
  radioActivations++;
  secondRadio.checked = true;
};
document.activeElement = firstBlock.querySelector(".question-free");
const radioLabelPress = press({ target: optionRows[1] });
document.fire("mousedown", radioLabelPress);
secondRadio.click();
document.fire("mouseup", radioLabelPress);
await nextTick();
check(
  "left radio activation to Chromium",
  radioActivations === 1 && secondRadio.checked === true,
  `${radioActivations} / ${secondRadio.checked}`,
);
check(
  "left the counter and Back out of the first step",
  elementFor("question-back").hidden === true &&
    elementFor("question-submit").textContent === "Next",
  elementFor("question-submit").textContent,
);

// `Next` walks on, keeping every question's fields built, so stepping back
// finds what was answered still there.
optionRows[0].children[0].checked = false;
optionRows[1].children[0].checked = true;
// Text left under a picked option is not an answer: only the row asking for the
// user's own words lets it speak.
firstBlock.querySelector(".question-free").value = "temporary";
app.questionNext();
check(
  "stepped to the question after it",
  elementFor("question-step").textContent === "2 of 2 questions" &&
    elementFor("question-title").textContent === "Which extras?" &&
    blocks[0].hidden === true &&
    blocks[1].hidden === false,
  `${elementFor("question-step").textContent} / ${elementFor("question-title").textContent}`,
);
check(
  "left the header out for a question that has none",
  elementFor("question-header").hidden === true &&
    elementFor("question-header").textContent === "",
  elementFor("question-header").textContent,
);
check(
  "marked the first question as answered and offered to go back",
  dashes()[0].className.includes("done") &&
    dashes()[1].className.includes("current") &&
    elementFor("question-back").hidden === false,
  dashes().map((dash) => dash.className).join(","),
);
check(
  "named the last step Submit",
  elementFor("question-submit").textContent === "Submit",
  elementFor("question-submit").textContent,
);
const secondBlock = blocks[1];
check(
  "offered a multi-select question's options as checkboxes",
  secondBlock.querySelectorAll(".question-choice").every((input) => input.type === "checkbox"),
  secondBlock.outline(),
);
const firstCheckbox = secondBlock.querySelectorAll(".question-choice")[0];
let checkboxActivations = 0;
firstCheckbox.click = () => {
  checkboxActivations++;
  firstCheckbox.checked = !firstCheckbox.checked;
};
document.activeElement = secondBlock.querySelector(".question-free");
const checkboxPress = press({ target: firstCheckbox });
document.fire("mousedown", checkboxPress);
firstCheckbox.click();
document.fire("mouseup", checkboxPress);
await nextTick();
check(
  "left checkbox activation to Chromium",
  checkboxActivations === 1 && firstCheckbox.checked === true,
  `${checkboxActivations} / ${firstCheckbox.checked}`,
);
firstCheckbox.checked = false;
check(
  "named a multi-select question's hint",
  elementFor("question-hint").textContent === "Select all that apply",
  elementFor("question-hint").textContent,
);
check(
  "kept the free-text field part of every question",
  dialog.querySelectorAll(".question-free").length === 2,
  dialog.outline(),
);

// Walking back finds the first question's own answer still painted.
app.showQuestionStep(0);
check(
  "brought back what the first question was answered with",
  optionRows[1].children[0].checked === true &&
    optionRows[0].children[0].checked === false &&
    firstBlock.querySelector(".question-free").value === "temporary",
  firstBlock.outline(),
);

// The last step's Submit sends the whole set at once, with the answer the first
// question kept while its step was away.
app.questionNext();
secondBlock.querySelectorAll(".question-choice")[1].checked = true;
secondBlock.querySelector(".question-free").value = "and a third";
app.questionNext();
const answered = calls.find(([name]) => name === "resolve_question");
check("answered the question over the bridge", Boolean(answered), JSON.stringify(calls));
check(
  "sent what was picked and typed against the question it belongs to",
  answered &&
    answered[1].id === 42 &&
    answered[1].answers[0].question === "Which database should the migration target?" &&
    JSON.stringify(answered[1].answers[0].values) === '["SQLite"]' &&
    JSON.stringify(answered[1].answers[1].values) === '["Fixtures","and a third"]',
  JSON.stringify(answered && answered[1]),
);
check("closed the dialog once it was answered", elementFor("question").hidden === true);

// The row asking for the user's own words answers in place of the option that
// was picked, so the label that only means "typing" is never sent as an answer.
calls.length = 0;
app.showQuestion({
  id: 50,
  questions: [
    { question: "Which one?", options: [{ label: "Alpha" }, { label: "Beta" }] },
  ],
});
const ownBlock = elementFor("question-body").children[0];
const ownRows = ownBlock.children.filter((row) => row.tagName === "LABEL");
ownRows[0].children[0].checked = false;
ownRows[2].children[0].checked = true;
ownBlock.querySelector(".question-free").value = "Gamma";
app.answerQuestion(false);
const own = calls.find(([name]) => name === "resolve_question");
check(
  "sent the typed answer instead of the picked option",
  own &&
    own[1].id === 50 &&
    JSON.stringify(own[1].answers[0].values) === '["Gamma"]',
  JSON.stringify(own && own[1]),
);

// A single question is a dialog of its own: no counter, no dashes, no Back.
calls.length = 0;
app.showQuestion({
  id: 51,
  questions: [{ question: "Proceed?", options: [{ label: "Yes" }, { label: "No" }] }],
});
check(
  "left out the counter for a single question",
  elementFor("question-step-row").hidden === true && dashes().length === 0,
  `${elementFor("question-step-row").hidden} ${dashes().length}`,
);
check(
  "offered no Back and named the only step Submit",
  elementFor("question-back").hidden === true &&
    elementFor("question-submit").textContent === "Submit",
  elementFor("question-submit").textContent,
);

// Dismiss answers with nothing, which is how the model is told the question was
// dismissed instead of never asked.
calls.length = 0;
app.showQuestion({
  id: 43,
  questions: [{ question: "Proceed?", options: [{ label: "Yes" }, { label: "No" }] }],
});
await app.answerQuestion(true);
const dismissed = calls.find(([name]) => name === "resolve_question");
check(
  "dismissed with no answers at all",
  dismissed && dismissed[1].id === 43 && dismissed[1].answers.length === 0,
  JSON.stringify(dismissed && dismissed[1]),
);

// A request with no questions is nothing to paint, and a dialog the app never
// opened is not answered twice.
calls.length = 0;
app.state.pendingQuestion = null;
app.showQuestion({ id: 44, questions: [] });
await app.answerQuestion(false);
check(
  "ignored a question with nothing in it",
  elementFor("question").hidden === true && calls.length === 0,
  JSON.stringify(calls),
);

// Every step carries its own question's header, which is what the dialog it
// follows reads as the question being asked.
calls.length = 0;
app.showQuestion({
  id: 45,
  questions: [
    { question: "Proceed?", options: [{ label: "Yes" }] },
    { header: "Details", question: "Which ones?" },
  ],
});
check(
  "headed the first step with the first question, not the first header it finds",
  elementFor("question-title").textContent === "Proceed?" &&
    elementFor("question-header").hidden === true,
  `${elementFor("question-title").textContent} / ${elementFor("question-header").textContent}`,
);
app.questionNext();
check(
  "headed the second step with its own question and header",
  elementFor("question-title").textContent === "Which ones?" &&
    elementFor("question-header").textContent === "Details" &&
    elementFor("question-header").hidden === false,
  `${elementFor("question-header").textContent} / ${elementFor("question-title").textContent}`,
);
await app.answerQuestion(true);

// Answering an empty form is the same dismissal as pressing Dismiss, so the agent
// hears one thing rather than a set of blank answers.
calls.length = 0;
app.showQuestion({ id: 46, questions: [{ question: "Anything to add?" }] });
await app.answerQuestion(false);
const blank = calls.find(([name]) => name === "resolve_question");
check(
  "sent an empty form as a dismissal",
  blank && blank[1].id === 46 && blank[1].answers.length === 0,
  JSON.stringify(blank && blank[1]),
);

// A request that times out while the turn keeps running closes its own dialog,
// and leaves a dialog for another request alone.
calls.length = 0;
app.showQuestion({ id: 47, questions: [{ question: "Still there?" }] });
await emit("question-closed", { id: 48 });
check(
  "left a dialog open when another request closed",
  elementFor("question").hidden === false,
  String(elementFor("question").hidden),
);
await emit("question-closed", { id: 47 });
check(
  "hid the dialog when its request timed out",
  elementFor("question").hidden === true && app.state.pendingQuestion === null,
  JSON.stringify(app.state.pendingQuestion),
);
await app.answerQuestion(false);
check(
  "sent nothing for a request that timed out",
  !calls.some(([name]) => name === "resolve_question"),
  JSON.stringify(calls),
);

// A run that ends takes a question it left waiting with it, so the dialog does
// not offer an answer the finished turn can never read.
calls.length = 0;
app.showQuestion({ id: 49, questions: [{ question: "One more thing?" }] });
await emit("agent-end", { runId: 1 });
check(
  "hid the dialog when the run ended",
  elementFor("question").hidden === true && app.state.pendingQuestion === null,
  JSON.stringify(app.state.pendingQuestion),
);
await app.answerQuestion(false);
check(
  "sent nothing for a question a finished run left waiting",
  !calls.some(([name]) => name === "resolve_question"),
  JSON.stringify(calls),
);

// The body scrolls between the question's head and its actions, and WebKit draws
// that scrollbar over the right edge of what it scrolls: the option card beside
// it lost the border that says how far it reaches. Styling the scrollbar gives
// the track a lane of a known width, and the body's own right padding has to
// clear it, or the cards are back under it.
const questionSteps = sheet.slice(sheet.indexOf(".question-body {"), sheet.indexOf(".question {"));
const questionLane = /::-webkit-scrollbar \{ width: (\d+)px; \}/.exec(questionSteps);
const questionGutter = /padding-right: (\d+)px;/.exec(questionSteps);
check(
  "kept the question body's scrollbar off the option cards",
  questionLane !== null &&
    questionGutter !== null &&
    Number(questionGutter[1]) > Number(questionLane[1]) &&
    /\.question-body::-webkit-scrollbar-thumb \{[^}]*background-clip: padding-box;[^}]*\}/.test(
      sheet,
    ),
  `${questionGutter && questionGutter[1]}px inset beside a ${questionLane && questionLane[1]}px track`,
);

// ---------- an edited file's own card ----------

// A call that changed a file reads as one line — the path, and how many lines
// moved — because the turn's changes are listed together by the card above; the
// diff it kept is there for the reader who clicks the card.
console.log("an edit tool's card");
const editCard = app.startTool(
  "edit",
  JSON.stringify({ path: "src/main.rs", edits: [{ oldText: "a", newText: "b" }] }),
);
app.finishTool(editCard, "Successfully replaced 1 block(s) in src/main.rs.", {
  diff: { path: "src/main.rs", text: "-  1      a\n+      1  b" },
  elapsed: 12,
});
check(
  "counted the lines the call moved beside its state",
  editCard.tstate.textContent.includes("+1 −1"),
  editCard.tstate.textContent,
);
check(
  "kept the card to a single line",
  editCard.pre.hidden === true && editCard.hint.hidden === true,
  `body hidden: ${editCard.pre.hidden}, hint hidden: ${editCard.hint.hidden}`,
);
check(
  "held the diff back until it is asked for",
  Boolean(editCard.diffEl) && editCard.diffEl.hidden === true,
  String(editCard.diffEl && editCard.diffEl.hidden),
);
check(
  "painted the diff under the file it changed",
  editCard.diffEl.innerHTML.includes("src/main.rs") &&
    editCard.diffEl.innerHTML.includes("dline add"),
  editCard.diffEl.innerHTML,
);
app.toggleTool(editCard);
check(
  "showed the call's own diff when the card is expanded",
  editCard.diffEl.hidden === false &&
    editCard.pre.hidden === false &&
    editCard.pre.textContent.includes("Successfully replaced 1 block(s)"),
  `diff hidden: ${editCard.diffEl.hidden}, body hidden: ${editCard.pre.hidden}`,
);
app.toggleTool(editCard);
check(
  "folded it away again",
  editCard.diffEl.hidden === true && editCard.pre.hidden === true,
  `diff hidden: ${editCard.diffEl.hidden}, body hidden: ${editCard.pre.hidden}`,
);
// Its diff is what it folds, so it is a fold even though no lines were held
// back: the two handles are buttons a keyboard can reach.
check(
  "made the diff card a fold a keyboard can work",
  editCard.head.tabIndex === 0 && editCard.hint.tabIndex === 0,
  `head tabIndex: ${editCard.head.tabIndex}, hint tabIndex: ${editCard.hint.tabIndex}`,
);
// A card for a call that changed no file is not the one-line form: it keeps the
// preview of what it printed, so the treatment above is only for a diff.
const plainCard = app.startTool("bash", JSON.stringify({ command: "cargo test" }));
app.finishTool(plainCard, "ok\nline two\nline three\nline four\n", { elapsed: 30 });
check(
  "left a call that changed nothing reading as before",
  plainCard.pre.hidden === false &&
    !plainCard.diffEl &&
    plainCard.hint.hidden === false &&
    plainCard.tstate.textContent.startsWith("✔"),
  `body hidden: ${plainCard.pre.hidden}, diff: ${Boolean(plainCard.diffEl)}, hint hidden: ${
    plainCard.hint.hidden
  }, state: ${plainCard.tstate.textContent}`,
);
check(
  "cut the preview to the first lines with the rest behind the hint",
  plainCard.pre.textContent === "ok\nline two\nline three" &&
    plainCard.hint.textContent === "⋯ 1 more line · click to expand",
  JSON.stringify([plainCard.pre.textContent, plainCard.hint.textContent]),
);
// The hint reads "click to expand", so that click is the one a reader makes:
// it has to be the card's other handle on the output, not a label that ignores
// the pointer over it.
plainCard.hint.onclick?.();
check(
  "expanded the output from the hint that offers it",
  plainCard.pre.hidden === false &&
    plainCard.hint.hidden === true &&
    plainCard.pre.textContent.includes("line four") &&
    String(plainCard.block.className).includes("expanded"),
  `body: ${JSON.stringify(plainCard.pre.textContent)}, hint hidden: ${plainCard.hint.hidden}`,
);
plainCard.hint.onclick?.();
check(
  "folded it away again from the same hint",
  plainCard.hint.hidden === false &&
    !plainCard.pre.textContent.includes("line four") &&
    !String(plainCard.block.className).includes("expanded"),
  `hint hidden: ${plainCard.hint.hidden}, body: ${JSON.stringify(plainCard.pre.textContent)}`,
);
// "Click to expand" is one way in, not the only one: the card is a fold, so
// both of its handles take Enter and Space, and a key the card does not own is
// left to whatever else is listening.
plainCard.hint.onkeydown?.({ key: "Enter", preventDefault() {} });
check(
  "expanded the output with Enter on the hint",
  plainCard.pre.textContent.includes("line four") &&
    plainCard.hint.tabIndex === 0 &&
    String(plainCard.block.className).includes("expanded"),
  `body: ${JSON.stringify(plainCard.pre.textContent)}, tabIndex: ${plainCard.hint.tabIndex}`,
);
plainCard.head.onkeydown?.({ key: " ", preventDefault() {} });
check(
  "folded it away again with Space on the head row",
  plainCard.hint.hidden === false &&
    !String(plainCard.block.className).includes("expanded"),
  `hint hidden: ${plainCard.hint.hidden}`,
);
let cardPrevented = false;
plainCard.head.onkeydown?.({ key: "a", preventDefault: () => (cardPrevented = true) });
check(
  "left a key the card does not own alone",
  !cardPrevented && plainCard.hint.hidden === false,
  `prevented: ${cardPrevented}, hint hidden: ${plainCard.hint.hidden}`,
);
const shortCard = app.startTool("bash", JSON.stringify({ command: "true" }));
app.finishTool(shortCard, "one line\n", { elapsed: 4 });
check(
  "left a card with nothing behind it as text, not a button",
  shortCard.hint.tabIndex === -1 && shortCard.head.tabIndex === -1,
  `hint tabIndex: ${shortCard.hint.tabIndex}, head tabIndex: ${shortCard.head.tabIndex}`,
);

// ---------- a finished turn's changes ----------

console.log("turn changes");
app.state.project = "/home/dev/Projects/oxide";
const transcriptCards = () =>
  elementFor("transcript").children.filter((node) => String(node.className).includes("changes"));
const file = (path, status, added, removed, diff = null, binary = false) => ({
  path,
  status,
  added,
  removed,
  binary,
  diff: diff || `${status === "added" ? "+" : "-"}     1  ${path}`,
});

// The card a run leaves when it ends: the files the turn's snapshot lists, with
// the totals its header shows. A file a shell command wrote rides along because
// the listing is the work tree's diff rather than the tools' arguments.
await emit("agent-end", {
  runId: 1,
  project: app.state.project,
  baseline: "9f1c0d2",
  after: "5e0aa91",
  changes: {
    added: 12,
    removed: 3,
    files: [
      file("src/agent.rs", "modified", 8, 2, "-  1  1  old\n+     1  new"),
      file("src/tools.rs", "modified", 3, 1, "-  4  4  gone\n+     4  here"),
      file("assets/logo.png", "modified", 0, 0, null, true),
      file("notes.md", "added", 1, 0),
      file("docs/guide.md", "modified", 0, 0),
      file("design.md", "modified", 2, 2),
    ],
  },
});
const cards = transcriptCards();
check("left a card for the files the turn changed", cards.length === 1, String(cards.length));
const card = cards[0];
const head = card.children[0];
const list = card.children[1];
check(
  "led with a file tile, and offered Undo before Review",
  String(head.children[0].className) === "changes-icon" &&
    head.children[0].innerHTML.includes("<svg") &&
    head.children[5].children[0].textContent === "Undo" &&
    head.children[5].children[1].textContent === "Review",
  `${head.children[0].className} / ${head.children[5].children.map((b) => b.textContent).join(", ")}`,
);
// The transcript is a flex column and the card hides its own overflow, which
// together let a flex item shrink below the rows it holds: without its own
// height the card collapses to nothing in a long conversation, leaving no card
// on screen while the node is still in the transcript.
check(
  "kept the card's own height so a busy transcript cannot squeeze it away",
  /\.changes \{[^}]*flex: none;[^}]*\}/.test(sheet),
  `${sheet.indexOf(".changes {")}`,
);
check(
  "named the card after what it lists",
  head.children[2].textContent === "Edited 6 files",
  head.children[2].textContent,
);
check(
  "counted the turn's additions and deletions",
  head.children[3].innerHTML.includes("+12") && head.children[3].innerHTML.includes("−3"),
  head.children[3].innerHTML,
);
check("listed one row per changed file", list.children.length === 6, String(list.children.length));
check(
  "folded the rows past what the card shows behind their count",
  card.children[2].hidden === false && card.children[2].textContent === "Show 1 more file",
  card.children[2].textContent,
);
const changedRow = list.children[0];
check(
  "badged the row with its status and counted its lines",
  changedRow.innerHTML.includes("change-status modified") &&
    changedRow.innerHTML.includes(">M<") &&
    changedRow.innerHTML.includes("src/agent.rs") &&
    changedRow.innerHTML.includes("+8") &&
    changedRow.innerHTML.includes("−2"),
  changedRow.innerHTML,
);
check(
  "said a binary file has no lines to count",
  list.children[2].innerHTML.includes("binary"),
  list.children[2].innerHTML,
);

// A row opens its own diff where it sits, so reading one file leaves the rest of
// the listing alone, and closes it again on a second click.
changedRow.onclick();
check(
  "opened the row's diff beneath it",
  changedRow.diffPanel &&
    changedRow.diffPanel.innerHTML.includes("dline del") &&
    changedRow.diffPanel.innerHTML.includes("dline add") &&
    list.children[1] === changedRow.diffPanel,
  changedRow.diffPanel && changedRow.diffPanel.innerHTML,
);
changedRow.onclick();
check(
  "closed the diff again",
  changedRow.diffPanel === null && list.children.length === 6,
  String(list.children.length),
);

// The review is the card's own listing beside whichever file is selected, whose
// two sides — what the run found, out of the project's shadow snapshot, and what
// is on disk now — the app hands over one file at a time.
const reviewTick = () => new Promise((resolve) => setTimeout(resolve, 0));
const reviewParts = () => elementFor("review-diff").children;
const reviewRows = () => reviewParts()[1]?.children || [];
const splitCells = (row) => row.children.map((cell) => cell.textContent);
head.children[5].children[1].onclick({ stopPropagation() {} });
check("opened the review", elementFor("review-modal").hidden === false);
check(
  "headed it with the card's own title and totals",
  elementFor("review-title").textContent === "Edited 6 files" &&
    elementFor("review-total").innerHTML.includes("+12"),
  `${elementFor("review-title").textContent} / ${elementFor("review-total").innerHTML}`,
);
check(
  "listed every file the card lists, the rows its own listing folds included",
  elementFor("review-files").children.length === 6,
  String(elementFor("review-files").children.length),
);
check(
  "named the file it was opened on",
  reviewParts()[0].innerHTML.includes("src/agent.rs") &&
    reviewParts()[0].innerHTML.includes("change-status modified"),
  reviewParts()[0].innerHTML,
);
check(
  "asked the app for that file's sides at the turn's own baseline",
  calls.some(
    ([name, args]) =>
      name === "change_sides" &&
      args.path === "src/agent.rs" &&
      args.baseline === "9f1c0d2" &&
      args.project === app.state.project,
  ),
  JSON.stringify(calls.at(-1)),
);
await reviewTick();
check(
  "painted both sides, each line with the number it holds there",
  reviewRows().length === 3 &&
    reviewRows()[0].className.includes("split-row context") &&
    JSON.stringify(splitCells(reviewRows()[0])) ===
      JSON.stringify(["1", "fn main() {", "1", "fn main() {"]),
  JSON.stringify(reviewRows().map(splitCells)),
);
check(
  "left the side a line is missing from empty",
  reviewRows()[1].className.includes("remove") &&
    JSON.stringify(splitCells(reviewRows()[1])) === JSON.stringify(["2", "    old();", "", ""]) &&
    reviewRows()[2].className.includes("add") &&
    JSON.stringify(splitCells(reviewRows()[2])) === JSON.stringify(["", "", "2", "    new();"]),
  JSON.stringify(reviewRows().map(splitCells)),
);
elementFor("review-files").children[1].onclick();
await reviewTick();
check(
  "showed the file that was clicked",
  reviewParts()[0].innerHTML.includes("src/tools.rs"),
  reviewParts()[0].innerHTML,
);
check(
  "read only the file it is showing",
  calls.filter(([name, args]) => name === "change_sides" && args.path === "src/tools.rs").length ===
    1,
  JSON.stringify(calls.map(([name, args]) => [name, args.path])),
);
const gap = reviewRows().find((row) => row.classList.contains("unmodified"));
check(
  "folded the stretch neither side changed behind its count",
  Boolean(gap) && gap.textContent === "6 unmodified lines",
  gap ? gap.textContent : "no fold",
);
const folded = reviewRows().length;
gap.onclick({ stopPropagation() {} });
check(
  "opened the folded stretch where it sits",
  reviewRows().length > folded &&
    !reviewRows().some((row) => row.classList.contains("unmodified")),
  `${folded} -> ${reviewRows().length}`,
);
const reviewKey = (key) => elementFor("review-modal").onkeydown({ key, preventDefault() {} });
reviewKey("ArrowDown");
await reviewTick();
check(
  "walked on with the arrow keys, naming a file with no text to show",
  reviewParts()[0].innerHTML.includes("assets/logo.png") &&
    reviewParts()[1].textContent.includes("Binary file"),
  reviewParts()[1].textContent,
);
reviewKey("ArrowDown");
await reviewTick();
check(
  "said a change too large to align is one rather than painting it",
  reviewParts()[0].innerHTML.includes("notes.md") &&
    reviewParts()[1].textContent.includes("too large"),
  reviewParts()[1].textContent,
);
check(
  "marked the row the review is showing",
  elementFor("review-files").children[3].classList.contains("active") &&
    !elementFor("review-files").children[0].classList.contains("active"),
  String(elementFor("review-files").children[3].className),
);
reviewKey("ArrowDown");
await reviewTick();
check(
  "said a file with nothing textual in it has nothing to paint",
  reviewParts()[1].textContent.includes("No textual changes"),
  reviewParts()[1].textContent,
);
reviewKey("ArrowDown");
await reviewTick();
check(
  "reported a file it could not read instead of showing an empty one",
  reviewParts()[1].textContent.includes("no longer in the snapshot"),
  reviewParts()[1].textContent,
);
elementFor("review-files").children[0].onclick();
await reviewTick();
reviewKey("ArrowUp");
await reviewTick();
check(
  "wrapped around from the first file",
  reviewParts()[0].innerHTML.includes("design.md"),
  reviewParts()[0].innerHTML,
);

// Undo puts the project back to the run's own baseline, which the payload
// carried, rather than to the repository's last commit.
calls.length = 0;
head.children[5].children[0].onclick({ stopPropagation() {} });
elementFor("confirm-ok").onclick();
await new Promise((resolve) => setTimeout(resolve, 0));
const undone = calls.find(([name]) => name === "undo_turn");
check(
  "put the files back through the turn's own baseline and project",
  undone &&
    undone[1].baseline === "9f1c0d2" &&
    undone[1].project === app.state.project &&
    // The state the turn left, which the restore checks the work tree still has.
    undone[1].after === "5e0aa91",
  JSON.stringify(undone),
);
check(
  "said the card was undone",
  head.children[4].textContent === "Undone" &&
    head.children[4].hidden === false &&
    head.children[5].children[0].hidden === true,
  `${head.children[4].textContent} / ${head.children[4].hidden}`,
);

// A listing longer than the card shows folds the rest behind a button rather
// than pushing the transcript away.
await emit("agent-end", {
  runId: 2,
  project: app.state.project,
  baseline: "aa11bb2",
  after: "b3c4d5e",
  changes: {
    added: 7,
    removed: 0,
    files: ["a.js", "b.js", "c.js", "d.js", "e.js", "f.js", "g.js"].map((path) =>
      file(path, "modified", 1, 0),
    ),
  },
});
const longCard = transcriptCards().at(-1);
const longList = longCard.children[1];
const more = longCard.children[2];
check(
  "showed the first rows and folded the rest",
  longList.children.filter((row) => !row.hidden).length === 5 &&
    more.hidden === false &&
    more.textContent === "Show 2 more files",
  `${longList.children.filter((row) => !row.hidden).length} rows / ${more.textContent}`,
);
more.onclick({ stopPropagation() {} });
check(
  "revealed the rest when asked",
  longList.children.every((row) => !row.hidden) && more.textContent === "Show less",
  `${more.textContent}`,
);
longCard.children[0].onclick();
check(
  "collapsed the whole listing from its header",
  String(longCard.className).includes("collapsed"),
  String(longCard.className),
);
check(
  "left out a side with nothing to count",
  longCard.children[0].children[3].innerHTML.includes("+7") &&
    !longCard.children[0].children[3].innerHTML.includes("−0"),
  longCard.children[0].children[3].innerHTML,
);

// Only the newest turn can be put back: an older card's baseline is the state
// before that turn, so its own Undo would take every change made since with it.
await emit("agent-end", {
  runId: 5,
  project: app.state.project,
  baseline: "77aa44c",
  after: "1c9f2ee",
  changes: {
    added: 2,
    removed: 1,
    files: [
      file("src/main.rs", "modified", 2, 1),
      // A rename or a mode change moves no lines, which its row says rather
      // than counting zero to zero.
      file("docs/moved.md", "modified", 0, 0, "-  1  1  old\n+     1  new"),
    ],
  },
});
const newest = transcriptCards().at(-1);
check(
  "offered Undo on the newest turn alone",
  newest.children[0].children[5].children[0].hidden === false &&
    longCard.children[0].children[5].children[0].hidden === true,
  `${newest.children[0].children[5].children[0].hidden} / ${longCard.children[0].children[5].children[0].hidden}`,
);
check(
  "said a row with no lines moved has none to count",
  newest.children[1].children[1].innerHTML.includes("no line changes"),
  newest.children[1].children[1].innerHTML,
);

// A turn that ran in another project — the window switched while it was still
// going — leaves no card here: it would sit under the wrong transcript, and its
// Undo would reach into the wrong work tree.
await emit("agent-end", {
  runId: 6,
  project: "/home/dev/other-project",
  baseline: "0123abc",
  after: "456def0",
  changes: { added: 1, removed: 0, files: [file("other.rs", "added", 1, 0)] },
});
check(
  "dropped a card for a turn in another project",
  transcriptCards().length === 3 &&
    !transcriptCards().some((node) => String(node.innerHTML).includes("other.rs")),
  String(transcriptCards().length),
);
check(
  "kept the newest card's Undo when that turn was dropped",
  newest.children[0].children[5].children[0].hidden === false,
  `${newest.children[0].children[5].children[0].hidden}`,
);

// A turn that moved no lines at all — a binary file rewritten, a mode change —
// leaves its rows to say so and a header with no total, rather than a `+0 −0`
// beside them.
await emit("agent-end", {
  runId: 7,
  project: app.state.project,
  baseline: "beef123",
  after: "cafe456",
  changes: {
    added: 0,
    removed: 0,
    files: [file("assets/logo.png", "modified", 0, 0, null, true)],
  },
});
const quiet = transcriptCards().at(-1);
check(
  "left a turn that moved no lines with no total to show",
  quiet.children[0].children[3].innerHTML === "" &&
    quiet.children[1].children[0].innerHTML.includes("binary"),
  `${quiet.children[0].children[3].innerHTML} / ${quiet.children[1].children[0].innerHTML}`,
);

// A turn that changed nothing — or a backend that could not take a baseline —
// leaves no card rather than an empty one.
await emit("agent-end", { runId: 3, baseline: "cc22dd3", changes: { files: [], added: 0, removed: 0 } });
await emit("agent-end", { runId: 4 });
check(
  "left no card for a turn that changed nothing",
  transcriptCards().length === 4,
  String(transcriptCards().length),
);

// A new chat drops the transcript's cards along with it, and the review it may
// have open with them.
app.resetTranscript();
check(
  "dropped the cards when the thread was reset",
  transcriptCards().length === 0 &&
    app.state.changes.length === 0 &&
    elementFor("review-modal").hidden === true,
  `${transcriptCards().length} cards / ${app.state.changes.length} kept`,
);

// ---------- the / menu ----------

console.log("slash commands");
if (catalogSkipped) {
  // Worth saying out loud rather than passing silently.
  console.log("  skip the catalog check: target/debug/oxide is not built");
} else {
  const client = catalog.filter((entry) => entry.kind === "client");
  check("the catalog offers client commands", client.length > 0);
  app.state.project = "/home/dev/Projects/oxide";
  // The `/` menu loads the catalog as it opens; a command the app cannot
  // perform is named from it rather than sent on to the model.
  await app.refreshPaletteEntries();
  check(
    "the palette loaded the catalog",
    app.state.palette.length === catalog.length,
    `${app.state.palette.length} of ${catalog.length}`,
  );
  // A client command belongs to the app, not to the model: a spelling the app
  // does not name is sent on as a prompt, which is the failure this catches.
  const unperformed = [];
  for (const entry of client) {
    for (const spelling of [entry.name, ...entry.aliases]) {
      elementFor("status-text").textContent = "";
      const handled = await app.runSlashCommand(`/${spelling}`);
      const said = status();
      check(`/${spelling} is answered by the app`, handled === true, `handled=${handled}`);
      if (said.includes("is not available in the desktop app yet")) unperformed.push(`/${spelling}`);
    }
  }
  if (unperformed.length) {
    console.log(`  note the app answers these with a "not available" note: ${unperformed.join(", ")}`);
  }

  // The catalog's other half is the project's own commands, prompt templates and
  // skills. A skill is listed under its own name — the spelling that loads it —
  // so taking the row is what activates it: the composer is completed with
  // `/name`, and sending that is the prompt the CLI expands rather than
  // something the app answers itself.
  const skills = catalog.filter((entry) => entry.kind === "skill");
  check(
    "the catalog offers the project's skills",
    skills.length > 0,
    `${catalog.length} entries, none a skill`,
  );
  if (skills.length) {
    const skill = skills[0];
    elementFor("prompt").value = `/${skill.name.slice(0, 3)}`;
    const rows = app.paletteMatches() || [];
    check(
      `the palette offers /${skill.name} under its own name`,
      rows.some((entry) => entry.name === skill.name && entry.kind === "skill"),
      JSON.stringify(rows.map((entry) => entry.name)),
    );

    // The painted row names the skill, its arguments and what it does, and says
    // it is a skill rather than where the file came from.
    app.renderPalette();
    const painted = elementFor("palette-list").children.find((row) =>
      String(row.innerHTML).includes(`>/${skill.name}<`),
    );
    const markup = String(painted && painted.innerHTML);
    check(
      "the row carries the skill's own name, arguments and description",
      Boolean(painted) &&
        markup.includes(skill.arguments || "[arguments]") &&
        markup.includes(skill.description),
      elementFor("palette-list").outline(),
    );
    check("the row says it is a skill", markup.includes('class="source">skill<'), markup);

    // Taking the row completes it, since a skill takes arguments and is the
    // message the CLI expands rather than a client command.
    elementFor("prompt").value = `/${skill.name}`;
    app.runPaletteEntry(skill);
    check(
      "taking a skill's row completes it rather than running it",
      elementFor("prompt").value === `/${skill.name} `,
      elementFor("prompt").value,
    );

    // Sending it is a prompt: the CLI resolves the name and loads the skill, so
    // the agent reads its instructions for this turn.
    app.state.busy = false;
    app.state.runId = null;
    calls.length = 0;
    await app.send(false);
    const sent = projectCalls("send_prompt");
    check(
      "sending a skill goes to the agent as the prompt the CLI expands",
      sent.length === 1 && sent[0][1].prompt === `/${skill.name}`,
      JSON.stringify(sent),
    );
    app.state.busy = false;
    app.state.runId = null;
  }
}

// ---------- the thread on screen has a name ----------

console.log("thread title");
// The shell ships the header empty: a thread has no title until a turn has one,
// and "New task" was a name the sidebar never used for it.
check(
  "shipped the header with no placeholder title",
  /id="thread-title"[^>]*>\s*<\/div>/.test(shell),
  shell.slice(Math.max(0, shellAt('id="thread-title"') - 60), shellAt('id="thread-title"') + 60),
);

// The provider is the composer's own chip, so the header carries only what the
// user has to act on.
const windowBefore = app.state.contextWindow;
app.state.contextWindow = 128000;
app.renderProjectMeta({
  model: "deepseek-flash",
  provider: "deepseek",
  hasKey: true,
  trust: null,
});
check(
  "left the provider out of the header",
  elementFor("project-meta").textContent === "" && elementFor("project-meta").hidden === true,
  elementFor("project-meta").textContent,
);
// The chip is a glyph, so the value it carries is the tooltip the row shows and
// the name a screen reader reads — the way the extension's own chips carry it.
check(
  "named the composer's chip for the model it would run",
  elementFor("model").getAttribute("aria-label") === "model: deepseek-flash · 128.0k" &&
    elementFor("model").title === "model: deepseek-flash · 128.0k\nSwitch model",
  `${elementFor("model").getAttribute("aria-label")} / ${JSON.stringify(elementFor("model").title)}`,
);
app.state.reasoning = "high";
app.updateChips();
check(
  "named the thinking chip for the level it would think at",
  elementFor("reasoning").getAttribute("aria-label") === "thinking: high" &&
    elementFor("reasoning").title === "thinking: high\nChoose the reasoning level (Shift+Tab cycles)",
  `${elementFor("reasoning").getAttribute("aria-label")} / ${JSON.stringify(elementFor("reasoning").title)}`,
);
app.state.reasoning = "auto";
app.updateChips();

// The level a turn thinks at is a choice among five rather than a step, so the
// chip opens a picker instead of walking the cycle: the levels it lists are the
// same cycle the terminal's Shift+Tab walks, read out of the panel's own source
// rather than restated here, so the two front-ends cannot drift apart.
const extensionLevels = [
  ...((footerSource.match(/REASONING_LEVELS = \[([^\]]*)\]/) || [])[1] || "").matchAll(/"([a-z]+)"/g),
].map((match) => match[1]);
const reasoningRows = () => elementFor("reasoning-list").children;
const reasoningNames = () => reasoningRows().map((row) => row.children[0]?.textContent);
const currentReasoning = () =>
  reasoningRows()
    .filter((row) => row.classList.contains("active"))
    .map((row) => row.children[0]?.textContent)
    .join(" ");
const pressedReasoning = () =>
  reasoningRows()
    .filter((row) => row.getAttribute("aria-pressed") === "true")
    .map((row) => row.children[0]?.textContent)
    .join(" ");
const reasoningRow = (level) => reasoningRows().find((row) => row.children[0].textContent === level);
// The chip has the keyboard when a reader clicks it, which is what the picker
// gives back when it closes.
elementFor("reasoning").focus();
elementFor("reasoning").click();
check(
  "opened a picker for the level instead of stepping through it",
  elementFor("reasoning-modal").hidden === false &&
    reasoningNames().join(" ") === extensionLevels.join(" ") &&
    reasoningRows().every((row) => row.children[1]?.textContent) &&
    currentReasoning() === "auto" &&
    pressedReasoning() === "auto",
  `${elementFor("reasoning-modal").hidden} / ${reasoningNames().join(" ")} / ${extensionLevels.join(" ")} / ${currentReasoning()} / ${pressedReasoning()}`,
);
// A model that always thinks still does at `off`: GLM 5.3 and later are asked for
// their lowest effort instead of for none at all (`llm::client::glm_forces_thinking`),
// so the row asks for no thinking rather than promising an answer without any.
check(
  "did not promise an answer without any thinking",
  /^Turn thinking off where the model allows it$/.test(reasoningRow("off")?.children[1]?.textContent),
  reasoningRow("off")?.children[1]?.textContent,
);
// The dialog is `aria-modal`, so the keyboard belongs to it: opening puts it on
// the level in use, not on the chip behind the overlay, where a reader has to
// tab past the rest of the page to reach a row at all.
check(
  "put the keyboard on the level in use",
  document.activeElement === reasoningRow("auto"),
  document.activeElement === reasoningRow("auto") ? "auto" : String(document.activeElement?.id),
);
// Tab walks the picker's own controls, so nothing behind it can take a
// keystroke while it is up.
const tabbedReasoning = (shiftKey = false) =>
  elementFor("reasoning-modal").fire("keydown", { key: "Tab", shiftKey, preventDefault() {} });
tabbedReasoning();
const tabbedOnward = document.activeElement === reasoningRow("off");
elementFor("reasoning-close").focus();
tabbedReasoning();
const tabbedRound = document.activeElement === reasoningRow("auto");
tabbedReasoning(true);
const shiftTabbedRound = document.activeElement === elementFor("reasoning-close");
check(
  "kept Tab inside the picker, and turned it round at the ends",
  tabbedOnward && tabbedRound && shiftTabbedRound,
  `${tabbedOnward} / ${tabbedRound} / ${shiftTabbedRound} / ${document.activeElement?.id}`,
);
// Shift+Tab is the levels' own shortcut, and while the picker is up it walks the
// rows instead: the page's cycle must not take the same key, or a reader walking
// the list would watch the mark move under them and the dialog shut.
const heldLevel = app.state.reasoning;
document.fire("keydown", { key: "Tab", shiftKey: true, preventDefault() {} });
check(
  "left the level alone while Shift+Tab walked the picker",
  app.state.reasoning === heldLevel && elementFor("reasoning-modal").hidden === false,
  `${app.state.reasoning} / ${heldLevel} / ${elementFor("reasoning-modal").hidden}`,
);
// A shortcut that does change the level with the picker up moves its mark rather
// than leaving the dialog showing the level it just left — and leaves the
// keyboard on the mark, since the list is rebuilt to do it.
document.fire("keydown", { key: "r", ctrlKey: true, preventDefault() {} });
check(
  "moved the picker's mark when a shortcut changed the level under it",
  app.state.reasoning === "off" &&
    elementFor("reasoning-modal").hidden === false &&
    currentReasoning() === "off" &&
    pressedReasoning() === "off" &&
    document.activeElement === reasoningRow("off"),
  `${app.state.reasoning} / ${currentReasoning()} / ${document.activeElement?.id}`,
);
reasoningRow("medium").click();
check(
  "took the level a row named and put the picker away",
  app.state.reasoning === "medium" &&
    elementFor("reasoning-modal").hidden === true &&
    elementFor("reasoning").getAttribute("aria-label") === "thinking: medium",
  `${app.state.reasoning} / ${elementFor("reasoning-modal").hidden} / ${elementFor("reasoning").getAttribute("aria-label")}`,
);
// Every close path hands the keyboard back — the row here, Escape below — so a
// reader who picked a level is not left at the top of the page.
check(
  "handed the keyboard back to the chip a pick was made from",
  document.activeElement === elementFor("reasoning"),
  document.activeElement?.id,
);
elementFor("reasoning").click();
check(
  "marked the level it is on when the picker is opened again",
  currentReasoning() === "medium" &&
    pressedReasoning() === "medium" &&
    document.activeElement === reasoningRow("medium"),
  `${currentReasoning()} / ${pressedReasoning()} / ${document.activeElement?.id}`,
);
// A picker that is not in `OVERLAYS` stays over the app through the key that
// closes every dialog — and Escape puts the keyboard back where it was too.
document.fire("keydown", { key: "Escape", preventDefault() {} });
check(
  "closed the picker on Escape like every other dialog",
  elementFor("reasoning-modal").hidden === true &&
    document.activeElement === elementFor("reasoning"),
  `${elementFor("reasoning-modal").hidden} / ${document.activeElement?.id}`,
);
// There is nothing to pick before a project is open — the level would be
// replaced by the one that project's config resolves to — so the chip gives the
// answer the app's other project-bound commands give.
const holdingProject = app.state.project;
app.state.project = null;
elementFor("reasoning").click();
check(
  "gave the app's own answer when no project is open",
  elementFor("reasoning-modal").hidden === true && status() === "Select a project first.",
  `${elementFor("reasoning-modal").hidden} / ${status()}`,
);
app.state.project = holdingProject;
await app.runSlashCommand("/reasoning");
check(
  "opened the picker from the bare command",
  elementFor("reasoning-modal").hidden === false,
  String(elementFor("reasoning-modal").hidden),
);
await app.runSlashCommand("/reasoning low");
check(
  "took a level typed after the command",
  app.state.reasoning === "low" &&
    elementFor("reasoning-modal").hidden === true &&
    elementFor("reasoning").getAttribute("aria-label") === "thinking: low",
  `${app.state.reasoning} / ${elementFor("reasoning-modal").hidden} / ${elementFor("reasoning").getAttribute("aria-label")}`,
);
await app.runSlashCommand("/reasoning nope");
check(
  "named the levels when the argument is not one",
  status() === "Reasoning must be one of auto, off, low, medium, high.",
  status(),
);
elementFor("reasoning").click();
// The keyboard is moved off the opener first, so the check reads the restore
// rather than an element that was still focused for another reason.
reasoningRow("high").focus();
elementFor("reasoning-close").click();
check(
  "shipped the picker in the markup with a way out of it",
  shellAt('id="reasoning-modal"') > 0 &&
    shellAt('id="reasoning-list"') > shellAt('id="reasoning-modal"') &&
    /id="reasoning-close"[^>]*>Close<\/button>/.test(shell) &&
    elementFor("reasoning-modal").hidden === true &&
    /\.reasoning-option \{[^}]*\}/.test(sheet),
  `${shellAt('id="reasoning-modal"')} / ${shellAt('id="reasoning-list"')} / ${elementFor("reasoning-modal").hidden}`,
);
// The third way out, and the last one that has to give the keyboard back.
check(
  "handed the keyboard back when the Close button was used",
  document.activeElement === elementFor("reasoning"),
  document.activeElement?.id,
);
// ...and the shortcut still walks the levels when no picker is up.
document.fire("keydown", { key: "Tab", shiftKey: true, preventDefault() {} });
check(
  "walked the levels on Shift+Tab when no picker is up",
  app.state.reasoning === "medium" &&
    elementFor("reasoning").getAttribute("aria-label") === "thinking: medium",
  `${app.state.reasoning} / ${elementFor("reasoning").getAttribute("aria-label")}`,
);
// A dialog a screen reader is told about, named by its own title: the overlay
// covers the app, so what is behind it is not what a reader is answering.
check(
  "shipped it as a dialog named by its own title",
  /id="reasoning-modal"[^>]*role="dialog"[^>]*aria-modal="true"[^>]*aria-labelledby="reasoning-title"/.test(
    shell,
  ) && /<h2 id="reasoning-title">Reasoning effort<\/h2>/.test(shell),
  shellAt('aria-labelledby="reasoning-title"'),
);
app.state.reasoning = "auto";
app.updateChips();
app.state.contextWindow = windowBefore;
app.renderProjectMeta({
  model: "deepseek-flash",
  provider: "deepseek",
  hasKey: false,
  trust: { required: true, trusted: false },
});
check(
  "named what the user has to fix instead",
  elementFor("project-meta").textContent === "no API key · project resources off" &&
    elementFor("project-meta").hidden === false,
  elementFor("project-meta").textContent,
);

// A turn titles its thread while it runs: the sidebar lists it then, and the
// header names it by the label that row shows.
threadsError = null;
app.resetTranscript();
check(
  "left a new task without a title",
  elementFor("thread-title").textContent === "",
  elementFor("thread-title").textContent,
);
const running = {
  id: "bb22cc33",
  name: null,
  // The project the app has open, since the sidebar groups a thread under it.
  cwd: (app.state.projects[0] || projectRows[0]).path,
  created_at: 2,
  modified_at: Math.floor(Date.now() / 1000),
  message_count: 1,
  preview: "rename the update command",
};
threads = [running, ...existing];
await emit("agent-start", {
  runId: 41,
  sessionId: running.id,
  title: "rename the update command",
});
check(
  "listed the running thread in the sidebar",
  elementFor("projects-tree").outline().includes("rename the update command"),
  elementFor("projects-tree").outline(),
);
check(
  "named the header after the same label the sidebar row shows",
  elementFor("thread-title").textContent === "rename the update command",
  elementFor("thread-title").textContent,
);
check(
  "took the session the turn is writing to",
  app.state.session === running.id,
  String(app.state.session),
);

// A thread the user named is listed under that name in both places, and a
// resumed thread is named from the row that was opened.
threads = [{ ...running, name: "Rename oxide update" }, ...existing];
await app.loadSessions();
check(
  "named a renamed thread the same way in both places",
  elementFor("thread-title").textContent === "Rename oxide update",
  elementFor("thread-title").textContent,
);
await app.openSession(existing[0]);
await new Promise((resolve) => setTimeout(resolve, 0));
check(
  "named a resumed thread the way its sidebar row does",
  elementFor("thread-title").textContent === "Fix the flaky test",
  elementFor("thread-title").textContent,
);

// A thread with nothing to be named after — no stored name, and a first message
// the title rules make no prose of — is listed under its id, so the header says
// the same thing rather than going blank the moment the listing arrives.
const untitled = { ...running, id: "0f1e2d3c", name: null, preview: "" };
threads = [untitled, ...existing];
await app.loadSessions();
await app.openSession(untitled);
await new Promise((resolve) => setTimeout(resolve, 0));
check(
  "named a thread the sidebar calls by its id the same way",
  elementFor("thread-title").textContent === "0f1e2d3c" &&
    elementFor("projects-tree").outline().includes("0f1e2d3c"),
  `${elementFor("thread-title").textContent} / ${elementFor("projects-tree").outline()}`,
);

// ---------- running-turn context in the composer's corner ----------

console.log("composer action");
// Stop and Send swap as the box gains text, and both are glyphs — the send
// arrow and the stop square the extension's own composer wears.
check(
  "drew the composer's corner as glyphs",
  ["send", "stop"].every((id) => buttonFor(id).includes("<svg") && /<\/svg>$/.test(buttonFor(id))),
  `${buttonFor("send")} / ${buttonFor("stop")}`,
);
// Stop and Send swap as the box gains text. New context waits by default; the
// user can deliberately switch it to steering the active response.
app.state.attachments = [];
app.setIdle();
elementFor("prompt").value = "";
app.updateSendState();
check(
  "offered Send with nothing typed",
  elementFor("send").hidden === false && elementFor("stop").hidden === true,
  `send ${elementFor("send").hidden} / stop ${elementFor("stop").hidden}`,
);
app.setBusy();
check(
  "offered Stop alone while a turn runs with an empty box",
  elementFor("stop").hidden === false &&
    elementFor("send").hidden === true &&
    elementFor("busy-message-mode").hidden === true,
  `send ${elementFor("send").hidden} / stop ${elementFor("stop").hidden} / mode ${elementFor("busy-message-mode").hidden}`,
);
elementFor("prompt").value = "keep going";
app.updateSendState();
check(
  "offered Queue by default once there was new context",
  elementFor("send").hidden === false &&
    elementFor("send").disabled === false &&
    elementFor("stop").hidden === true &&
    elementFor("busy-message-mode").hidden === false &&
    elementFor("busy-message-mode").textContent === "Queue" &&
    elementFor("send").title === "Queue as the next turn (Enter)",
  `${elementFor("busy-message-mode").textContent} / ${elementFor("send").title}`,
);
elementFor("busy-message-mode").onclick();
check(
  "made steering a deliberate visible choice",
  elementFor("busy-message-mode").textContent === "Steer" &&
    elementFor("send").title === "Steer the active response (Enter)",
  `${elementFor("busy-message-mode").textContent} / ${elementFor("send").title}`,
);
elementFor("prompt").value = "";
app.updateSendState();
check(
  "went back to Stop when the box was emptied again",
  elementFor("stop").hidden === false &&
    elementFor("send").hidden === true &&
    elementFor("busy-message-mode").hidden === true,
  `send ${elementFor("send").hidden} / stop ${elementFor("stop").hidden} / mode ${elementFor("busy-message-mode").hidden}`,
);
app.setIdle();
check(
  "gave Send back, alone, when the turn ended",
  elementFor("send").hidden === false &&
    elementFor("stop").hidden === true &&
    elementFor("busy-message-mode").hidden === true &&
    elementFor("send").title === "Send (Enter)",
  `${elementFor("send").title} / stop ${elementFor("stop").hidden}`,
);
// The accessible name follows the action the button performs, so a screen
// reader hears what the click will do rather than the label it was built with.
const cornerLabel = () => elementFor("send").getAttribute("aria-label");
app.setBusy();
elementFor("prompt").value = "keep going";
app.updateSendState();
const queuedLabel = cornerLabel();
elementFor("busy-message-mode").onclick();
const steeredLabel = cornerLabel();
app.setIdle();
elementFor("prompt").value = "";
app.updateSendState();
check(
  "named the corner's action for a screen reader",
  queuedLabel === "Queue" && steeredLabel === "Steer" && cornerLabel() === "Send",
  `${queuedLabel} / ${steeredLabel} / ${cornerLabel()}`,
);

calls.length = 0;
app.setBusy();
app.state.runId = 42;
elementFor("prompt").value = "run this after your current answer";
app.updateSendState();
elementFor("send").onclick({ detail: 1 });
await nextTick();
let runningMessage = projectCalls("steer_run").at(-1);
check(
  "queued new context for the next response by default",
  runningMessage?.[1]?.followUp === true,
  JSON.stringify(runningMessage),
);
check(
  "returned to the safe Queue default after sending",
  app.state.busyMessageMode === "queue",
  app.state.busyMessageMode,
);

calls.length = 0;
elementFor("prompt").value = "change direction now";
app.updateSendState();
elementFor("busy-message-mode").onclick();
elementFor("send").onclick({ detail: 1 });
await nextTick();
runningMessage = projectCalls("steer_run").at(-1);
check(
  "steered the active response when that choice was selected",
  runningMessage?.[1]?.followUp === false,
  JSON.stringify(runningMessage),
);

// A response can finish between drawing Send and the app receiving the
// message. Codex keeps that prompt and starts it as the next turn; it must not
// report a successful queue operation and silently lose it.
calls.length = 0;
steerAccepted = false;
elementFor("prompt").value = "do this in the next turn";
app.updateSendState();
elementFor("send").onclick({ detail: 1 });
await nextTick();
check(
  "kept a message rejected at the completion boundary",
  app.state.pendingSends[0]?.prompt === "do this in the next turn" &&
    projectCalls("send_prompt").length === 0,
  JSON.stringify({ pending: app.state.pendingSends, calls }),
);
elementFor("prompt").value = "and this one after it";
app.updateSendState();
elementFor("send").onclick({ detail: 1 });
await nextTick();
check(
  "kept concurrent rejected messages in submission order",
  app.state.pendingSends.map((pending) => pending.prompt).join(" | ") ===
    "do this in the next turn | and this one after it",
  JSON.stringify(app.state.pendingSends),
);
await emit("agent-end", { runId: 42 });
check(
  "started the first raced message as Codex's next turn",
  projectCalls("send_prompt").at(-1)?.[1]?.prompt === "do this in the next turn" &&
    app.state.pendingSends[0]?.prompt === "and this one after it",
  JSON.stringify(calls),
);
await emit("agent-end", { runId: 43 });
check(
  "started the next raced message only after the preceding turn",
  projectCalls("send_prompt").at(-1)?.[1]?.prompt === "and this one after it" &&
    app.state.pendingSends.length === 0,
  JSON.stringify(calls),
);
steerAccepted = true;
app.setIdle();
app.state.runId = null;

// The corner action is an ordinary button: the click it gets, whether the
// browser calls it a mouse click or a keyboard activation, sends the message.
calls.length = 0;
elementFor("prompt").value = "keep going";
app.setIdle();
app.updateSendState();
elementFor("send").onclick({ detail: 0 });
await nextTick();
const keyboardSend = calls.map(([name]) => name);
check("sent from a keyboard activation", keyboardSend.length > 0, JSON.stringify(calls));

calls.length = 0;
elementFor("prompt").value = "keep going";
app.setIdle();
app.updateSendState();
elementFor("send").onclick({ detail: 1 });
await nextTick();
check(
  "sent on the click a mouse produces as well",
  JSON.stringify(calls.map(([name]) => name)) === JSON.stringify(keyboardSend),
  JSON.stringify(calls),
);

calls.length = 0;
app.state.session = null;
elementFor("prompt").value = "just the once";
app.setIdle();
app.updateSendState();
elementFor("send").onclick({ detail: 1 });
await nextTick();
const sends = () => calls.filter(([name]) => name === "send_prompt");
check("sent once from one click", sends().length === 1, JSON.stringify(calls));
// A window with no thread on screen is starting one: sending `latest` would
// append this message to whichever thread was used last, which is not the
// thread the reader is looking at and not one the sidebar would gain.
check(
  "opened a thread of its own when the window had none",
  sends()[0]?.[1]?.session === "new",
  JSON.stringify(sends()[0]?.[1]),
);
app.state.session = "cafe0000cafe0000";
calls.length = 0;
elementFor("prompt").value = "carry on";
app.setIdle();
app.updateSendState();
elementFor("send").onclick({ detail: 0 });
await nextTick();
check(
  "kept sending into the thread on screen",
  sends()[0]?.[1]?.session === "cafe0000cafe0000",
  JSON.stringify(sends()[0]?.[1]),
);
app.state.session = null;

console.log("a thread that is not stored yet");
// `all_sessions` lists what is on disk, and a thread that was just started has
// not written its first entry: every listing — the sidebar's tree, its count for
// the project and the sessions list — stands it in until the store catches up.
app.setIdle();
app.state.projects = [{ name: "oxide", path: "/tmp/oxide", registered: false }];
app.state.project = "/tmp/oxide";
app.state.projectName = "oxide";
app.state.sessions = [
  {
    id: "1111111111111111",
    name: "An older thread",
    cwd: "/tmp/oxide",
    created_at: 1,
    modified_at: 2,
    message_count: 4,
    preview: "",
    path: "/tmp/sessions/older.jsonl",
  },
];
app.state.session = "6f3031b2beef";
app.state.runTitle = "Fix the sidebar";
await app.renderProjectsTree();
const tree = elementFor("projects-tree").outline();
check(
  "listed the thread the window is in beside the stored ones",
  tree.includes("Fix the sidebar") && tree.includes("An older thread"),
  tree,
);
const group = elementFor("projects-tree").children[0];
check(
  "counted it for its project",
  group?.children[0]?.children[2]?.textContent === 2,
  String(group?.children[0]?.children[2]?.textContent),
);
check(
  "marked it as the thread on screen",
  String(group?.children[1]?.children[0]?.className).includes("active"),
  String(group?.children[1]?.children[0]?.className),
);
app.renderSessions();
const sessionRows = elementFor("sessions-list").outline();
check(
  "listed it in the project's session list too",
  sessionRows.includes("Fix the sidebar") && sessionRows.includes("An older thread"),
  sessionRows,
);
// It is the window's own row while the store has no file behind it, so it is
// stamped in the store's unit — Unix seconds, the number `sessionAge` subtracts
// from `Date.now() / 1000` — and neither listing routes it through the commands
// that read a session off disk.
const synthetic = app.listedSessions().find((session) => session.id === "6f3031b2beef");
check(
  "stamped it in the unit the store reports",
  Number.isInteger(synthetic.modified_at) &&
    synthetic.created_at === synthetic.modified_at &&
    synthetic.modified_at <= Date.now() / 1000 &&
    synthetic.modified_at > Date.now() / 1000 - 5,
  `${synthetic.created_at} / ${synthetic.modified_at}`,
);
const syntheticRow = group?.children[1]?.children[0];
calls.length = 0;
const selectPress = press({ target: syntheticRow });
syntheticRow.onclick(selectPress);
await nextTick();
check(
  "selected the thread already on screen without reading a file",
  calls.every(([name]) => name !== "session_messages") && app.state.runTitle === "Fix the sidebar",
  `${JSON.stringify(calls.map(([name]) => name))} / ${app.state.runTitle}`,
);
const listedRow = elementFor("sessions-list").children[0];
calls.length = 0;
const rowPress = press({ target: listedRow });
listedRow.onclick(rowPress);
await nextTick();
check(
  "opened that thread from the sessions list the same way",
  calls.every(([name]) => name !== "session_messages") && app.state.session === "6f3031b2beef",
  `${JSON.stringify(calls.map(([name]) => name))} / ${app.state.session}`,
);
check(
  "offered no ✕ on a thread with nothing stored",
  !syntheticRow.children.some((node) => String(node.className).includes("row-remove")),
  syntheticRow.children.map((node) => String(node.className)).join(","),
);
calls.length = 0;
await app.removeSession(synthetic);
check(
  "deleted nothing for a thread the store has not written",
  !calls.length && app.state.session === "6f3031b2beef",
  JSON.stringify(calls.map(([name]) => name)),
);
app.state.sessions.unshift({ ...app.state.sessions[0], id: "6f3031b2beef", name: "Fix the sidebar" });
await app.renderProjectsTree();
check(
  "stopped standing in once the store listed it",
  elementFor("projects-tree").outline().split("Fix the sidebar").length - 1 === 1,
  elementFor("projects-tree").outline(),
);

console.log("removing a project");
// A row's ✕ is inside the row it removes, so the press it takes is the ✕'s and
// not the row's, and the dialog that opens answers with the choice it was
// opened with.
const removeTarget = {
  id: "/home/dev/Projects/oxide",
  path: "/home/dev/Projects/oxide",
  name: "oxide",
  registered: true,
  exists: true,
  session_count: 2,
};
app.state.projects = [removeTarget];
app.state.sessions = [
  { id: "a1", cwd: removeTarget.path, name: "One" },
  { id: "a2", cwd: removeTarget.path, name: "Two" },
];
app.state.project = null;
await app.renderProjectsTree();
const removeRow = elementFor("projects-tree").children[0].children[0];
const removeButton = removeRow.children.find((node) => String(node.className).includes("row-remove"));
calls.length = 0;
// The ✕ sits in the row that selects the project, so it stops the click there
// rather than letting both act on one gesture.
let stoppedAtTheRow = false;
const removePress = press({
  target: removeButton,
  stopPropagation() {
    stoppedAtTheRow = true;
  },
});
removeButton.onclick(removePress);
await nextTick();
check(
  "opened the confirm from the row's own ✕, not the row",
  elementFor("confirm-modal").hidden === false && app.state.project === null,
  `${elementFor("confirm-modal").hidden} / ${app.state.project}`,
);
check(
  "kept that click from reaching the row around it",
  stoppedAtTheRow === true,
  String(stoppedAtTheRow),
);
check(
  "offered the second choice because the project has threads",
  elementFor("confirm-alt").hidden === false &&
    /Delete 2 threads/.test(elementFor("confirm-alt").textContent),
  `${elementFor("confirm-alt").hidden} / ${elementFor("confirm-alt").textContent}`,
);
calls.length = 0;
elementFor("confirm-alt").onclick(press({ target: elementFor("confirm-alt") }));
await nextTick();
check(
  "answered with that dialog's own choice",
  calls.filter(([name]) => name === "delete_session").length === 2,
  JSON.stringify(calls.map(([name]) => name)),
);

// ---------- check for updates ----------

// The dialog is the app's own and so is the release it offers: the desktop's
// `desktop-v*` train, resolved by the shared `oxide_core::updates`, with the
// artifact this machine installs — never the CLI's release list.
console.log("check for updates");
app.state.project = "/home/dev/Projects/oxide";
const updateTitle = () => String(elementFor("update-title").textContent);
const updateBody = () => String(elementFor("update-body").innerHTML);
const updateNote = () => String(elementFor("update-note").textContent);
const installButton = elementFor("update-install");

calls.length = 0;
elementFor("update").onclick(press({ target: elementFor("update") }));
check("asked the app's own update check", projectCalls("check_updates").length === 1, JSON.stringify(calls.map(([name]) => name)));
check("opened the dialog from the sidebar's own button", elementFor("update-modal").hidden === false);
await nextTick();
check(
  "named the release the app's own train resolved",
  updateTitle() === "Oxide 0.34.0 is available",
  updateTitle(),
);
check(
  "said what installing does and what picks the new version up",
  /this app's place/.test(updateNote()) && /quit and opened again/.test(updateNote()),
  updateNote(),
);
check(
  "showed what this machine has and what the release is",
  updateBody().includes("0.33.0") && updateBody().includes("desktop-v0.34.0"),
  updateBody(),
);
check(
  "showed the installation the app found itself and its path",
  updateBody().includes("app bundle") && updateBody().includes("/Applications/Oxide.app"),
  updateBody(),
);
check(
  "named the artifact this machine would download",
  updateBody().includes("Oxide_0.34.0_aarch64.dmg"),
  updateBody(),
);
check(
  "offered the install this installation can do",
  installButton.hidden === false && installButton.textContent === "Install 0.34.0",
  `${installButton.hidden} / ${installButton.textContent}`,
);
check(
  "offered the desktop release's own page",
  elementFor("update-notes").hidden === false,
);

calls.length = 0;
await elementFor("update-notes").onclick();
check(
  "opened that release in the browser through the app",
  projectCalls("open_url")[0]?.[1]?.url === updateAnswer.releaseUrl &&
    updateAnswer.releaseUrl.includes("desktop-v0.34.0"),
  JSON.stringify(projectCalls("open_url")),
);

// Installing is the app's own: it downloads the artifact its release train
// publishes for this machine, verifies it and replaces this copy in place.
calls.length = 0;
await installButton.onclick();
check("ran the app's own install", projectCalls("install_update").length === 1, JSON.stringify(calls.map(([name]) => name)));
check(
  "did not ask the CLI-shaped check what landed",
  projectCalls("check_updates").length === 0,
  JSON.stringify(calls.map(([name]) => name)),
);
check(
  "reported the release the install put in place",
  updateTitle() === "Oxide 0.34.0 is installed",
  updateTitle(),
);
check(
  "told the reader what to do to run it",
  /Restart Oxide to run the new version/.test(updateNote()),
  updateNote(),
);
// The restart is the dialog's own action then: the install put the release in
// place, and only a new process runs it.
const restartButton = elementFor("update-restart");
check(
  "offered the restart that runs it",
  restartButton.hidden === false && installButton.hidden === true,
  `${restartButton.hidden} / ${installButton.hidden}`,
);
calls.length = 0;
await restartButton.onclick();
check(
  "restarted the app through the app's own command",
  projectCalls("restart_app").length === 1,
  JSON.stringify(calls.map(([name]) => name)),
);
check(
  "reported what the install did, by tag and by file",
  updateBody().includes("desktop-v0.34.0") &&
    updateBody().includes("Oxide_0.34.0_aarch64.dmg") &&
    updateBody().includes("/Applications/Oxide.app"),
  updateBody(),
);
check(
  "stopped offering an install that has happened",
  installButton.hidden === true && updateTitle() !== "Oxide 0.34.0 is up to date",
  `${installButton.hidden} / ${updateTitle()}`,
);
// A window that installed a release keeps that install for the rest of the
// session: the build running here is still the older one, so a later check
// resolves the release again. The checks below start from a window that has not
// installed it.
app.installedUpdate.answer = null;

// A check that could not reach GitHub is a failure the app names, not an
// up-to-date machine.
updateAnswer = OFFERED_UPDATE;
updateError = "Error: requesting the release\n\nCaused by: connection reset by peer";
calls.length = 0;
await app.openUpdate();
check(
  "reported a check that failed as one",
  updateTitle() === "Could not check for updates",
  updateTitle(),
);
check(
  "showed the app's own words for it",
  updateBody().includes("connection reset by peer") && updateBody().includes("update-error"),
  updateBody(),
);
check(
  "offered no install and no stale release page for it",
  installButton.hidden === true && elementFor("update-notes").hidden === true,
  `${installButton.hidden} / ${elementFor("update-notes").hidden}`,
);
updateError = "no <release> & no network";
await app.openUpdate();
check(
  "escaped what the release lookup said rather than trusting it as markup",
  updateBody().includes("&lt;release&gt; &amp;") && !updateBody().includes("<release>"),
  updateBody(),
);
updateError = null;

// An up-to-date app still reports the version it is on.
updateAnswer = { ...OFFERED_UPDATE, current: OFFERED_UPDATE.latest, updateAvailable: false };
await app.openUpdate();
check(
  "reported an up-to-date app by version",
  updateTitle() === "Oxide 0.34.0 is up to date",
  updateTitle(),
);
check(
  "kept the versions and the installation on screen",
  updateBody().includes("0.34.0") && updateBody().includes("/Applications/Oxide.app"),
  updateBody(),
);
check("offered no install for it", installButton.hidden === true);
check("offered no advice it does not need", !updateBody().includes("update-advice"), updateBody());

// A copy the app may not write over — a checkout's build, a system package, a
// bundle an administrator installed — gets the advice the app composed instead
// of an install button.
updateAnswer = {
  ...OFFERED_UPDATE,
  installation: "source build",
  path: "",
  installable: false,
  advice:
    "Download Oxide_0.34.0_aarch64.dmg from the release page and install it the way this copy was installed.",
};
await app.openUpdate();
check(
  "let the app decide an installation it cannot replace",
  installButton.hidden === true && updateBody().includes("update-advice"),
  `${installButton.hidden} / ${updateBody()}`,
);
check(
  "named the download to use instead",
  updateBody().includes("Oxide_0.34.0_aarch64.dmg") && updateBody().includes("source build"),
  updateBody(),
);

// An install that fails reports the app's own error, and the dialog goes back
// to being a thing to retry rather than one that looks installed.
updateAnswer = OFFERED_UPDATE;
installError = "mounting the downloaded disk image: hdiutil failed with exit status: 1";
await app.openUpdate();
await installButton.onclick();
check(
  "reported an install that failed as one",
  updateTitle() === "Could not install the update",
  updateTitle(),
);
check("showed why the install failed", updateBody().includes("hdiutil failed"), updateBody());
check("left the release page reachable", elementFor("update-notes").hidden === false);
installError = null;

// The macOS menu item has no page of its own, so it asks the window for the
// dialog its button opens.
elementFor("update-modal").hidden = true;
calls.length = 0;
await emit("check-updates", {});
await nextTick();
check(
  "opened the same dialog from the menu item's own event",
  elementFor("update-modal").hidden === false && projectCalls("check_updates").length === 1,
  JSON.stringify(calls.map(([name]) => name)),
);
// Escape closes every overlay, which is where the dialog's own id has to be.
document.fire("keydown", { key: "Escape" });
check("closed it on Escape", elementFor("update-modal").hidden === true);
// A second press must not start a second install over the same installation.
updateAnswer = OFFERED_UPDATE;
await app.openUpdate();
await nextTick();
calls.length = 0;
const [first, second] = [app.installUpdate(), app.installUpdate()];
await Promise.all([first, second]);
check(
  "ran one install at a time",
  projectCalls("install_update").length === 1,
  JSON.stringify(calls.map(([name]) => name)),
);
check("reported the install that ran", updateTitle() === "Oxide 0.34.0 is installed", updateTitle());
app.installedUpdate.answer = null;
elementFor("update-close").onclick();
check("closed it from its own button", elementFor("update-modal").hidden === true);

// The check is asked for from two places — the sidebar button and the macOS menu
// item — and either can be pressed again before the first answer arrives. The
// answer for the question nobody is waiting on is dropped rather than painted,
// which is what keeps a stale release (or a stale failure) from replacing what
// the newer check found.
updateAnswer = OFFERED_UPDATE;
holdNextCheck();
const stale = app.openUpdate();
updateAnswer = { ...OFFERED_UPDATE, current: OFFERED_UPDATE.latest, updateAvailable: false };
const fresh = app.openUpdate();
await nextTick();
check(
  "painted the newer check's answer",
  updateTitle() === "Oxide 0.34.0 is up to date",
  updateTitle(),
);
releaseHeldChecks();
await Promise.all([stale, fresh]);
check(
  "dropped the answer for the check it had already moved past",
  updateTitle() === "Oxide 0.34.0 is up to date" && installButton.hidden === true,
  `${updateTitle()} / ${installButton.hidden}`,
);

// The app resolves the release again as it installs, so a release published
// between the check and the click is the one that lands. What the dialog reports
// is the version the install itself resolved, not the one it offered.
installedVersion = "0.35.0";
updateAnswer = OFFERED_UPDATE;
await app.openUpdate();
check("offered the release the check resolved", updateTitle() === "Oxide 0.34.0 is available");
await installButton.onclick();
check(
  "reported the version that landed rather than the one it offered",
  updateTitle() === "Oxide 0.35.0 is installed",
  updateTitle(),
);
check(
  "named that release on the row it reported",
  updateBody().includes("desktop-v0.35.0"),
  updateBody(),
);
installedVersion = null;
updateAnswer = OFFERED_UPDATE;
app.installedUpdate.answer = null;

// A check started while an install worked asked about the build that was
// running when it started, so it resolves the release the install was putting
// in place and would offer it a second time. The install is the newer word on
// this installation — it is what the window will be running after the restart —
// and its report stays, whether it arrives before or after that check.
console.log("an install and the check that overlaps it");
updateAnswer = OFFERED_UPDATE;
await app.openUpdate();
check(
  "offered the release before installing it",
  updateTitle() === "Oxide 0.34.0 is available",
  updateTitle(),
);
holdNextInstall();
const installing = installButton.onclick();
await nextTick();
holdNextCheck();
const overlapping = app.openUpdate();
await nextTick();
releaseHeldInstalls();
await installing;
check(
  "reported the install that finished while a check was in flight",
  updateTitle() === "Oxide 0.34.0 is installed",
  updateTitle(),
);
releaseHeldChecks();
await overlapping;
check(
  "kept the install's report when the check that overlapped it answered",
  updateTitle() === "Oxide 0.34.0 is installed" && installButton.hidden === true,
  `${updateTitle()} / ${installButton.hidden}`,
);
check(
  "stopped offering the release it had just installed",
  installButton.hidden === true && !/is available/.test(updateTitle()),
  `${installButton.hidden} / ${updateTitle()}`,
);
// A check asked for afterwards is about the same installation, and the release
// it resolves is the one on disk: the dialog repeats what the install did
// rather than offering it again.
calls.length = 0;
await app.openUpdate();
check(
  "answered a later check with the install rather than the release",
  updateTitle() === "Oxide 0.34.0 is installed" && projectCalls("check_updates").length === 1,
  `${updateTitle()} / ${JSON.stringify(calls.map(([name]) => name))}`,
);
app.installedUpdate.answer = null;

// The Windows installer is a program of its own, which this app starts and
// cannot wait on: it asks for elevation and for Oxide to be closed, and it can
// be cancelled. The dialog reports the launch it is, never a version in place.
installedVersion = "0.34.0";
installPending = true;
updateAnswer = OFFERED_UPDATE;
await app.openUpdate();
await installButton.onclick();
check(
  "reported a started installer as running rather than installed",
  updateTitle() === "The Oxide 0.34.0 installer is running",
  updateTitle(),
);
check(
  "said what finishes that install",
  /Finish the installer/.test(updateNote()) && !/is installed/.test(updateBody()),
  `${updateNote()} / ${updateBody()}`,
);
check(
  "offered no second install while one is running",
  installButton.hidden === true,
  String(installButton.hidden),
);
// The installer owns this machine's copy of the app: it asks for elevation and
// for Oxide to be closed, so there is no release in place for a restart to run.
check(
  "offered no restart for an install the installer owns",
  restartButton.hidden === true && installButton.hidden === true,
  String(restartButton.hidden),
);
installedVersion = null;
installPending = false;
app.installedUpdate.answer = null;
updateAnswer = OFFERED_UPDATE;

// ---------- the update a launch installs on its own ----------

// The app keeps itself current the way its other front-ends check for one, so
// the window is told about an install rather than asking: each step arrives as
// an event nobody in the window clicked for, and what is left when a release
// lands is the restart that runs it. The row is the sidebar's own, since the
// window may be showing a conversation when it appears.
console.log("the update a launch installs on its own");
const banner = elementFor("update-banner");
const bannerText = () => String(elementFor("update-banner-text").textContent);
const bannerRestart = elementFor("update-banner-restart");

// A window that has just opened has nothing to say about an install yet.
check(
  "said nothing about an install before the shell reported one",
  banner.hidden === true,
  String(banner.hidden),
);

await emit("update-progress", { stage: "downloading", version: "0.34.0" });
await nextTick();
check(
  "told the reader about the install the launch started",
  banner.hidden === false && /Downloading Oxide 0.34.0/.test(bannerText()),
  `${banner.hidden} / ${bannerText()}`,
);
check(
  "offered no restart for a release that is not in place yet",
  bannerRestart.hidden === true,
  String(bannerRestart.hidden),
);

// The release is on disk and the process is not: what the row offers is the
// restart that runs it.
const launchAnswer = {
  ok: true,
  version: "0.34.0",
  tag: "desktop-v0.34.0",
  asset: "Oxide_0.34.0_aarch64.dmg",
  path: "/Applications/Oxide.app",
  text: "Restart Oxide to run the new version.",
  pending: false,
};
await emit("update-ready", launchAnswer);
await nextTick();
check(
  "said the release was installed, and left the restart to do it",
  banner.hidden === false &&
    /Oxide 0.34.0 is installed/.test(bannerText()) &&
    bannerRestart.hidden === false,
  `${banner.hidden} / ${bannerText()} / ${bannerRestart.hidden}`,
);
calls.length = 0;
await bannerRestart.onclick();
check(
  "restarted the app from the row the launch painted",
  projectCalls("restart_app").length === 1,
  JSON.stringify(calls.map(([name]) => name)),
);

// A turn owns the process — its tools write files and its stream is read here —
// so a restart is refused while one runs rather than killing a live run.
app.state.busy = true;
elementFor("status-text").textContent = "";
calls.length = 0;
await bannerRestart.onclick();
check(
  "refused a restart while a turn was running",
  projectCalls("restart_app").length === 0 && /A turn is running/.test(status()),
  `${JSON.stringify(calls.map(([name]) => name))} / ${status()}`,
);
app.state.busy = false;

// The install is remembered the way a click's own install is, so a check that
// resolves the release now on disk reports it instead of offering it again.
calls.length = 0;
updateAnswer = OFFERED_UPDATE;
await app.openUpdate();
check(
  "answered the dialog's own check with the install the launch performed",
  updateTitle() === "Oxide 0.34.0 is installed" &&
    installButton.hidden === true &&
    restartButton.hidden === false,
  `${updateTitle()} / ${installButton.hidden} / ${restartButton.hidden}`,
);
elementFor("update-close").onclick();
app.installedUpdate.answer = null;

// The row belongs to the window rather than to the install: putting it away
// stops nothing, and an install that could not finish is a line rather than a
// dialog nobody asked for.
calls.length = 0;
await elementFor("update-banner-dismiss").onclick();
check(
  "put the row away without stopping the install",
  banner.hidden === true && calls.length === 0,
  `${banner.hidden} / ${JSON.stringify(calls.map(([name]) => name))}`,
);
await emit("update-failed", { message: "the download did not verify" });
await nextTick();
check(
  "reported an install that failed as a line, not a dialog",
  banner.hidden === true &&
    elementFor("update-modal").hidden === true &&
    /The download did not verify|did not verify/.test(status()),
  `${banner.hidden} / ${elementFor("update-modal").hidden} / ${status()}`,
);
elementFor("status-text").textContent = "";

// A window that opened after the install started heard none of it: the events
// were emitted into a page that was not listening, and the one that must not be
// missed is the report the restart hangs on. The shell keeps the newest of them
// beside its state, and the page asks for what it has already said as it starts,
// from the same channel every other command travels on.
app.launchDismissed.yes = false;
check(
  "asked what the launch's install had already said as it started",
  invokes.some(([, payload]) => payload.command === "launch_update"),
  JSON.stringify(invokes.map(([, payload]) => payload.command).slice(0, 4)),
);
launchHeard = { event: "update-ready", payload: launchAnswer };
await app.catchUpOnLaunchUpdate();
check(
  "painted the install a page that opened late would have missed",
  banner.hidden === false &&
    /Oxide 0.34.0 is installed/.test(bannerText()) &&
    bannerRestart.hidden === false,
  `${banner.hidden} / ${bannerText()} / ${bannerRestart.hidden}`,
);

// What the row says goes on arriving — the install is still working — so a
// dismissal is kept apart from it: a row that came back with the next stage is
// one the reader cannot put away.
calls.length = 0;
await elementFor("update-banner-dismiss").onclick();
await emit("update-progress", { stage: "installing", version: "0.34.0" });
await nextTick();
check(
  "left the row the reader put away away, whatever the install reports next",
  banner.hidden === true,
  String(banner.hidden),
);
launchHeard = null;
app.launchDismissed.yes = false;

// A dialog opened while the launch's install was running was asked about the
// build that started, so it offers the release the install has just put in
// place: the install is the newest word on the dialog too, and its Install row is
// one that would install what is already there.
app.installedUpdate.answer = null;
updateAnswer = { ...OFFERED_UPDATE };
calls.length = 0;
await app.openUpdate();
check(
  "offered the release the dialog's own check resolved",
  installButton.hidden === false && restartButton.hidden === true,
  `${installButton.hidden} / ${restartButton.hidden}`,
);
await emit("update-ready", launchAnswer);
await nextTick();
check(
  "repaired the open dialog with the install that landed under it",
  updateTitle() === "Oxide 0.34.0 is installed" &&
    installButton.hidden === true &&
    restartButton.hidden === false,
  `${updateTitle()} / ${installButton.hidden} / ${restartButton.hidden}`,
);
elementFor("update-close").onclick();
app.installedUpdate.answer = null;

// ---------- the bridge the window is reached through ----------

// Every command the page performs goes through the one Tauri command the app
// registers, with the command's own name and arguments inside it: the page
// reaches no plugin's IPC directly, which is what keeps `pick_folder` and
// `open_url` answering with the state the other commands hold.
console.log("the bridge");
check(
  "reached the app through its own command, never a plugin's",
  invokes.length > 0 &&
    invokes.every(
      ([name, payload]) =>
        name === "oxide_invoke" &&
        typeof payload.command === "string" &&
        payload.command.length > 0 &&
        payload.args !== null &&
        typeof payload.args === "object",
    ),
  JSON.stringify(invokes.slice(0, 2)),
);

console.log(failures.length ? `\n${failures.length} failed` : "\nall checks passed");
process.exit(failures.length ? 1 : 0);
