// Checks the desktop front-end without a window: `ui/app.js` is loaded against
// a stubbed DOM and the bridge the window is reached through, then driven the
// way the composer drives it.
//
//   node crates/desktop/check-app.mjs
//
// It covers what `cargo test` cannot reach — the `/mcp` dialog (its listing,
// the toggle, and the no-project and failure paths), the Add-project dialog's
// call into the core, the attachment chips (an image's thumbnail and the
// full-size preview it opens), the question dialog (its title, what a blank
// form sends, and the two ways a request goes away — the run ending, and the CLI
// giving up on it), the reasoning picker (its rows, the level a row applies, and
// the Escape and Close that put it away), and the `/` menu's dispatch of every built-in
// the shared catalog offers — since a Rust test never runs the app's own
// JavaScript. Every command it performs goes through the one command the app
// answers on its bridge, so a check in this file that reaches a command by name is
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
    if (name === "type") this.type = String(value);
  }
  getAttribute(name) {
    return (this.attributes || {})[name] ?? null;
  }
  focus() {
    this.focused = true;
    document.activeElement = this;
  }
  // A box that is blurred is the box the caret was in giving it up, which the
  // page does itself when a press lands on a control somewhere else.
  blur() {
    if (document.activeElement !== this) return;
    this.focused = false;
    document.activeElement = null;
  }
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

  matches(selector) {
    return String(selector)
      .split(",")
      .some((part) => this.matchesSelector(part.trim()));
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
      if (node.matches(selector)) return node;
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
    name: "macos-arm64-Oxide.dmg",
    url: "https://github.com/jaysonwu991/oxide/releases/download/desktop-v0.34.0/macos-arm64-Oxide.dmg",
    digest: "sha256:86966d1b137203c9ed7366329eb708d02896abce279c3ef33a80d8257a1d68b7",
  },
  // The release's own notes, as the API holds them: the changelog the dialog
  // paints instead of sending the reader to the release page for it.
  notes:
    "## What's changed\n\n- **Fixed** the sidebar squashing a long thread title\n- Added `oxide context`\n\nSee [#174](https://github.com/jaysonwu991/oxide/pull/174).",
  releasedAt: "2026-10-06",
};
let updateError = null;
// The check's answer as a fresh machine gives it. A test that leaves one behind
// puts this copy back rather than whatever it left.
const OFFERED_UPDATE = { ...updateAnswer };
// An install that could not finish is a rejected `install_update`, which is what
// the app's own download, check or replacement reports.
let installError = null;
// A cancel the shell could not take, which is the one failure the dialog has no
// step for: it is the reader's own request going nowhere.
let cancelError = null;
// Whether an install in flight has been asked to stop, which is what the shell
// answers it with when it lands: a cancelled install put nothing in place.
let installCancelled = false;
// The names the window gave the installs it asked for, and the names a cancel
// addressed. A cancel only reaches an install that answers to its own name, so
// a test can tell whether the two halves agree on which install is which.
const installTokens = [];
const cancelledTokens = [];
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

// The threads the sidebar groups by project and `/session` lists for the one
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

const providerRows = [
  {
    name: "openai",
    label: "OpenAI",
    description: "GPT models",
    keyUrl: "https://platform.openai.com/api-keys",
    local: false,
    stored: true,
    active: true,
    credential: "key",
  },
  {
    name: "anthropic",
    label: "Anthropic",
    description: "Claude models",
    keyUrl: "https://console.anthropic.com/settings/keys",
    local: false,
    stored: false,
    active: false,
    credential: "key",
  },
  {
    name: "bedrock",
    label: "Amazon Bedrock",
    description: "Claude and Nova models on AWS",
    keyUrl: "https://docs.aws.amazon.com/bedrock/",
    local: false,
    stored: false,
    active: false,
    credential: "external",
  },
  {
    name: "ollama",
    label: "Ollama",
    description: "Local models served by Ollama",
    keyUrl: "",
    local: true,
    stored: false,
    active: false,
    credential: "none",
  },
];
// The providers the core's own listing hands the dialog (`oxide providers
// --json`): a name to search on, a label, a description, the state the row
// shows beside it, and where its credential comes from — a key kept here, one
// the machine already has, or none at all. The table holds every provider a
// client can connect — one of them a server on this machine with no key at all
// — which is why the dialog searches it rather than listing it. Nothing is
// connected on the way in: a window that never opened the dialog has no
// providers, which is what the sections above expect of `/logout`.
let providersAnswer = [];
// What the harness' `read_clipboard` answers, for the paste a webview could not
// read itself.
let clipboardAnswer = null;
// What the harness' `reasoning_levels` answers, which is the active model's own
// levels read from the model cache. The full set stands in for a model whose
// listing advertised nothing; a check below narrows it.
let reasoningAnswer = {
  reasoning: "auto",
  reasoningLevels: ["off", "minimal", "low", "medium", "high", "xhigh", "max"],
  supportsReasoning: true,
};

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
// What `project_info` answers while a check asks for the folder's own facts,
// and nothing everywhere else.
let projectInfo = null;
// What `git_info` answers for the folder's repository: the bar's own read, so a
// check can drive a branch, a worktree or a folder that is no repository.
let gitAnswer = {
  repo: true,
  root: "/home/dev/Projects/oxide",
  branch: "main",
  detached: "",
};
let gitError = null;
// What `session_messages` answers for a stored thread's own totals, and nothing
// everywhere else.
let sessionUsage = null;
// What a real `project_info` answers for a folder, for the sections that need the
// window a turn would resolve rather than the stub's silence.
const projectFacts = (contextWindow) => ({
  provider: "openai",
  model: "gpt-4.1",
  reasoning: "auto",
  reasoningLevels: null,
  supportsReasoning: false,
  contextWindow,
  hasKey: true,
  trust: { required: false, trusted: false, awaiting: false },
});

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
    case "send_prompt":
      // The backend answers with the run's id, which the window holds while the
      // turn is starting — before `agent-start` says which thread it is in.
      return 62;
    case "remove_project":
      projectRows = projectRows.filter((row) => row.id !== args.id);
      return projectRows.map((row) => ({ ...row }));
    case "session_messages": {
      const target = (threads || []).find((session) => session.id === args.id);
      return {
        header: { id: args.id, name: target?.name || null },
        messages: [],
        usage: sessionUsage,
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
      installTokens.push(String((args && args.token) || ""));
      if (installsHeld > 0) {
        installsHeld -= 1;
        await new Promise((resolve) => heldInstalls.push(resolve));
      }
      // A release that cannot be fetched, verified or put in place is reported
      // once the install is under way, which is where a real one fails.
      if (installError) throw installError;
      // A cancel the window asked for while this was in flight: the app answers
      // with the cancel rather than a release, since nothing was put in place.
      if (installCancelled) {
        installCancelled = false;
        return { ok: false, cancelled: true };
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
    case "cancel_update": {
      if (cancelError) throw cancelError;
      const token = String((args && args.token) || "");
      cancelledTokens.push(token);
      // A cancel addresses one install: the one that answers to that name, and
      // no other.
      if (token !== "" && installTokens.includes(token)) installCancelled = true;
      return {};
    }
    case "launch_update":
      return launchHeard;
    // Everything the rest of `init`/selection asks for; none of it is what this
    // check is about, and all of it stays inside the stub.
    case "list_providers":
      return providersAnswer;
    case "list_models":
    case "list_themes":
    case "list_sessions":
    case "list_approvals":
      return [];
    case "list_commands":
      // The core answers a projectless ask with the built-ins alone (see
      // `palette_entries`): a command, a prompt template and a skill are read
      // from a folder, and the home state has none to read.
      return String(args.project || "").trim()
        ? catalog
        : catalog.filter((entry) => entry.source === "builtin");
    case "read_clipboard":
      return clipboardAnswer;
    case "reasoning_levels":
      return { ...reasoningAnswer, reasoningLevels: [...reasoningAnswer.reasoningLevels] };
    case "project_info":
      // A folder's own facts are answered only where a check needs what a real
      // answer sets — the sections above set that state themselves, since the
      // stub is what the window must not depend on.
      return projectInfo;
    case "git_info":
      if (gitError) throw gitError;
      if (!String(args.project || "").trim()) throw "select a project first";
      return { ...gitAnswer };
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
const invokes = [];
globalThis.window = {
  // The page speaks the preload bridge's own envelope rather than a generated
  // one, so the stub is that bridge: a request the page hands to the host is
  // recorded — the checks below reach the fixture by the name in the page's own
  // call — answered the way `oxide_invoke` answers it, and the answer handed
  // back the way the host hands one over. The user bridge is the one the page
  // posts on; the event bridge is left out, as a webview that has one has the
  // other, and the fallback is what the page reaches for without it.
  __electrobun: {},
  __electrobunHostBridge: {
    postMessage(packet) {
      packet = JSON.parse(packet);
      invokes.push([packet.method, packet.params]);
      const answer = invoke(packet.params.command, packet.params.args || {});
      Promise.resolve(answer).then(
        (payload) => receive({ type: "response", id: packet.id, success: true, payload }),
        (error) =>
          receive({
            type: "response",
            id: packet.id,
            success: false,
            error: String(error?.message || error),
          }),
      );
    },
  },
  innerWidth: 1280,
  innerHeight: 900,
  // The window keeps the listeners the page puts on it, so a check can fire the
  // one event only the reader can cause — coming back to the window.
  listeners: {},
  addEventListener(type, handler) {
    (this.listeners[type] ||= []).push(handler);
  },
  fire(type, event) {
    for (const handler of this.listeners[type] || []) handler(event);
  },
  history: { replaceState: () => {} },
  matchMedia: () => ({ matches: false, addEventListener: () => {} }),
  requestAnimationFrame: (callback) => setTimeout(callback, 0),
  localStorage: { getItem: () => null, setItem: () => {}, removeItem: () => {} },
};
/// Hands a packet to the page the way the host's own `evaluate_javascript` does.
const receive = (packet) => globalThis.window.__electrobun.receiveMessageFromHost(packet);
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
    "\nglobalThis.__app = { send, runSlashCommand, state, createProjectState, ICONS," +
    " newChatFromSidebar, openProjects, renderProjects, pickProject, updateProjectChip, renderWelcome, recentThreads, loadProjects," +
    " openCreateProject, addCreateProjectTypedPath, saveCreateProject, openModels," +
    " openConnect, renderProviders, saveConnect, renderLoginFields," +
    " refreshPaletteEntries, paletteMatches, renderPalette, runPaletteEntry," +
    " addAttachment, addAttachmentFiles, el, showQuestion, showQuestionStep, questionNext," +
    " answerQuestion, collectAnswers, requestAt, acceptAt, moveAt, closeAt, atKey," +
    " showApproval," +
    " resetTranscript, renderChanges, closeReview, undoChanges," +
    " updateRunBanner, viewingRun, openRun, handleEvent, removeProject, clearSelectedProject, selectProject," +
    " loadSessions, renderProjectsTree, renderSessions, renderMcps, openSession, renderProjectMeta, updateSendState, toggleBusyMessageMode, setBusy, setIdle," +
    " loadGit," +
    " loadMcps, openSessions," +
    " listedSessions, selectSessionFromTree, removeSession," +
    " updateChips, openReasoning," +
    " startTool, finishTool, toggleTool, openUpdate, installUpdate, cancelUpdate," +
    " installedUpdate, catchUpOnLaunchUpdate, handleUpdateProgress, handleUpdateReady, handleUpdateFailed," +
    " renderMarkdown, SCROLLBAR_LINGER };\n",
);

const app = globalThis.__app;
const status = () => String(elementFor("status-text").textContent);
const projectCalls = (command) => calls.filter(([name]) => name === command);
/// Delivers an event to the app the way the host does (`event.payload`).
const emit = async (name, payload) => {
  await receive({ type: "message", id: name, payload });
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
// What the sidebar draws for a folder is the app's own drawing, on the same grid
// and at the same weight as the composer's controls under it — not an emoji,
// whose shape and size belong to whatever font the machine has.
const firstProjectRow = elementFor("projects-tree").children[0].children[0];
check(
  "drew a project's folder as the app's own glyph",
  String(firstProjectRow.children[0].innerHTML) === app.ICONS.folder,
  firstProjectRow.outline(),
);
// Both of the row's controls are drawings too, and both are the VS Code panel's
// own: the plus is the panel's plus, and the ✕ is the one its listing uses.
check(
  "drew a row's controls from the app's own set",
  String(firstProjectRow.children[3].innerHTML) === app.ICONS.plus &&
    String(firstProjectRow.children[4].innerHTML) === app.ICONS.close,
  firstProjectRow.outline(),
);
// The home state's mark is the app's own drawing as well, and the transcript
// signs each answer with the same one.
check(
  "drew the home state's mark as the app's own glyph",
  elementFor("transcript").querySelector(".welcome-mark")?.innerHTML === app.ICONS.mark,
  elementFor("transcript").outline(),
);
check(
  "named the composer's chip as the way to pick a folder",
  elementFor("project-name").textContent === "Choose a project" &&
    elementFor("project").classList.contains("unset") &&
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
// The home state is painted before either listing answers, so a store that cannot
// be read still lets the folders that did arrive repaint it — otherwise the
// welcome keeps saying to add a folder that the sidebar is already showing.
app.state.project = null;
app.state.projects = [];
app.resetTranscript();
const blankHome = elementFor("transcript").outline();
threadsError = "permission denied";
app.state.projects = projectRows.map((row) => ({ ...row }));
app.state.sessions = existing;
await app.loadSessions();
check(
  "repainted the home state when the threads could not be read",
  blankHome.includes("Add a folder") &&
    !elementFor("transcript").outline().includes("Add a folder") &&
    elementFor("transcript").querySelectorAll(".recent-thread").length > 0 &&
    /Failed to load threads/.test(status()),
  `${blankHome} / ${elementFor("transcript").outline()} / ${status()}`,
);
threadsError = null;
await app.loadSessions();
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
    !elementFor("project").classList.contains("unset"),
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
// A turn belongs to the folder it started in — its run id is that thread's — and
// the window may move to another folder while it works: the run's thread is not
// the one on screen any more, so its transcript is parked and painted on under
// the strip's own name for it, while the reader gets the folder they picked.
app.state.session = "6f3031b2beef";
app.state.runSession = "6f3031b2beef";
app.state.runProject = projectRows[1].path;
app.state.runTitle = "Fix the sidebar";
app.state.parked = null;
app.setBusy();
calls.length = 0;
el("project").onclick();
elementFor("project-list").children[0].onclick();
await nextTick();
check(
  "opened another folder under a running turn without losing its thread",
  app.state.project === projectRows[0].path &&
    app.state.session === null &&
    app.state.busy === true &&
    app.state.runSession === "6f3031b2beef" &&
    app.state.parked?.session === "6f3031b2beef" &&
    elementFor("run-banner").hidden === false &&
    elementFor("run-banner-text").textContent === "A turn is running in “Fix the sidebar”",
  `${app.state.project} / ${app.state.session} / ${app.state.parked?.session} / ${elementFor("run-banner-text").textContent}`,
);
// A thread in another folder is the same switch by another door: opening it
// leaves the run's own thread and folder alone, with the strip still saying
// which thread it is in.
calls.length = 0;
await app.selectSessionFromTree({ id: "fe0031b1", cwd: projectRows[0].path });
check(
  "resumed another folder's thread under it too",
  app.state.project === projectRows[0].path &&
    app.state.session === "fe0031b1" &&
    app.state.runSession === "6f3031b2beef" &&
    calls.some(([name]) => name === "session_messages") &&
    /A turn is running in “Fix the sidebar”/.test(elementFor("run-banner-text").textContent),
  `${app.state.project} / ${app.state.session} / ${JSON.stringify(calls.map(([name]) => name))}`,
);
// The strip is the way back to it, folder and all: the run's thread lives in the
// folder the window has left, so opening it goes there first and hands the
// transcript the run has been painting on the whole time back to the reader.
calls.length = 0;
el("run-banner").click();
await nextTick();
check(
  "went back to the run's own folder and thread from the strip",
  app.state.project === projectRows[1].path &&
    app.state.session === "6f3031b2beef" &&
    app.state.parked === null &&
    elementFor("run-banner").hidden === true &&
    !calls.some(([name]) => name === "session_messages"),
  `${app.state.project} / ${app.state.session} / ${app.state.parked} / ${JSON.stringify(calls.map(([name]) => name))}`,
);
// A turn that ends while the reader is in another folder leaves its thread parked
// with the folder that thread's work is in: the strip still names it — as one
// whose turn has finished — and opening it takes the window back there rather
// than leaving a transcript on screen that belongs to a folder it is not in.
app.state.runSession = "6f3031b2beef";
app.state.runProject = projectRows[1].path;
app.state.runTitle = "Fix the sidebar";
app.setBusy();
await app.selectProject({ name: "oxide", path: projectRows[0].path });
app.setIdle();
check(
  "offered a finished run's thread while the reader was in another folder",
  app.state.parked?.session === "6f3031b2beef" &&
    app.state.runProject === null &&
    elementFor("run-banner").hidden === false &&
    elementFor("run-banner-text").textContent === "A turn finished in “Fix the sidebar”",
  `${app.state.parked?.session} / ${app.state.runProject} / ${elementFor("run-banner-text").textContent}`,
);
// The thread is still one the window knows about, and one it knows the folder of:
// a row standing in for it carries that folder rather than the one on screen, so
// the store is never asked for a thread it has not got.
check(
  "listed a finished run's thread under the folder it belongs to",
  app
    .listedSessions()
    .some((session) => session.id === "6f3031b2beef" && session.cwd === projectRows[1].path),
  JSON.stringify(app.listedSessions().map((session) => `${session.id}@${session.cwd}`)),
);
calls.length = 0;
el("run-banner").click();
await nextTick();
check(
  "went back to a finished run's own folder from the strip",
  app.state.project === projectRows[1].path &&
    app.state.session === "6f3031b2beef" &&
    app.state.parked === null,
  `${app.state.project} / ${app.state.session} / ${app.state.parked}`,
);
// A run still in flight is left alone by a thread opened in another folder: the
// store is read for the thread the reader asked for, and the run's own waits.
app.state.session = null;
app.state.runSession = "6f3031b2beef";
app.state.runProject = projectRows[1].path;
app.state.runTitle = "Fix the sidebar";
app.state.parked = null;
app.setBusy();
await app.selectProject({ name: "oxide", path: projectRows[0].path });
calls.length = 0;
await app.selectSessionFromTree({ id: "fe0031b1", cwd: projectRows[0].path });
check(
  "resumed another folder's thread under it too",
  app.state.project === projectRows[0].path &&
    app.state.session === "fe0031b1" &&
    app.state.runSession === "6f3031b2beef" &&
    calls.some(([name]) => name === "session_messages") &&
    /A turn is running in “Fix the sidebar”/.test(elementFor("run-banner-text").textContent),
  `${app.state.project} / ${app.state.session} / ${JSON.stringify(calls.map(([name]) => name))}`,
);
app.setIdle();
app.state.runTitle = "";
app.state.runProject = null;
app.state.parked = null;

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

// The launch looks a provider's catalog up beside the window, so the answer
// lands after a folder has been opened and the window the model chip carries
// has already been read once. Only that window is read again: a full load of
// the folder's facts would hand back the level the folder remembers, which is
// the reader's own choice from the moment they pick one.
console.log("the window the launch's catalog answered with");
app.setIdle();
// What the section above left the composer on, put back at the end of this one:
// the level here is the check's own, not the composer's.
const catalogLevel = {
  reasoning: app.state.reasoning,
  picked: app.state.reasoningPicked,
};
app.state.reasoningPicked = false;
app.state.reasoning = "low";
projectInfo = {
  model: "deepseek-flash",
  contextWindow: 2000000,
  reasoning: "low",
  reasoningLevels: [],
  hasKey: true,
  trust: null,
};
app.state.contextWindow = 128000;
calls.length = 0;
await emit("model-catalog", {});
await nextTick();
check(
  "repainted the model chip with the window the catalog answered with",
  app.state.contextWindow === 2000000 &&
    elementFor("model").title === "model: deepseek-flash · 2.00M\nSwitch model",
  `${app.state.contextWindow} / ${elementFor("model").title}`,
);
// The level picked while that lookup was in flight is the one the next turn is
// sent with, so a background answer cannot take it back.
app.state.reasoningPicked = true;
app.state.reasoning = "high";
await emit("model-catalog", {});
await nextTick();
check(
  "left the level the reader picked while the lookup was in flight",
  app.state.reasoning === "high" && app.state.reasoningPicked === true,
  `${app.state.reasoning} / ${app.state.reasoningPicked}`,
);
projectInfo = null;
el("prompt").value = "carry on";
calls.length = 0;
await app.send(false);
const carried = projectCalls("send_prompt");
check(
  "sent the next turn with that level rather than the folder's own",
  carried.length === 1 && carried[0][1].reasoning === "high",
  JSON.stringify(carried.map(([, args]) => args.reasoning)),
);
app.setIdle();
el("prompt").value = "";
app.state.reasoning = catalogLevel.reasoning;
app.state.reasoningPicked = catalogLevel.picked;

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
// The composer's own top row names where the turn runs — the folder the message
// goes to and the branch that folder is on — beside the window's status, what the
// thread has spent and the context reading that spend is measured against. It is
// the composer's first row, read before anything is typed below it, and the
// folder carries words rather than a glyph, since with nothing open it is how a
// first thread starts. The app claims no machine: a folder on this computer is the
// one environment it has, so a readout saying so is a constant rather than a fact.
const factsRow = shell.slice(
  shell.indexOf('<div class="composer-facts">'),
  shell.indexOf('<div id="attachments"'),
);
check(
  "put the folder and the branch on the composer's own top row",
  !shell.includes('class="composer-head"') &&
    !shell.includes('id="machine"') &&
    !shell.includes("This computer") &&
    factsRow.includes('id="project"') &&
    /<span id="project-name"[^>]*>[^<]+<\/span>/.test(factsRow) &&
    factsRow.includes('id="git-branch"') &&
    factsRow.includes('id="context-ring"') &&
    shell.indexOf('<div class="composer-facts">') <
      shell.indexOf('<textarea id="prompt"') &&
    shell.indexOf('id="composer"') < shell.indexOf('<div class="composer-facts">') &&
    shell.indexOf('id="project"') < shell.indexOf('id="attach"'),
  factsRow.slice(0, 420),
);
// The row's own facts are readouts rather than controls — the branch is a fact
// about the folder and the context reading is a reading — each with its words in a
// tooltip rather than a handler; the folder chip beside them is the one control
// there.
check(
  "left the row's own readouts without handlers",
  factsRow.includes('<span id="git"') &&
    !/id="(git|context-ring)"[^>]*onclick/.test(factsRow) &&
    /id="git"[^>]*hidden/.test(factsRow) &&
    /id="context-ring"[^>]*hidden/.test(factsRow),
  factsRow.slice(0, 420),
);
// The context reading ends the row — after the totals, which are what take the
// row's own auto margin — and the arc escalates through the same two thresholds
// the terminal's footer colors its own percentage by: dim on its own, the theme's
// amber past 70% and its error color past 90%. The percent beside the arc is part
// of that one reading rather than a second one, so the totals do not spell it out
// as well.
check(
  "kept the context reading at the row's end, in the color of the level",
  /\.context-ring \{[^}]*display: inline-flex;/.test(sheet) &&
    !/\.context-ring \{[^}]*margin-left: auto;/.test(sheet) &&
    /\.composer-facts \.spend \{ margin-left: auto;/.test(sheet) &&
    shellAt('id="usage"') < shellAt('id="context-ring"') &&
    factsRow.includes('id="context-percent"') &&
    !/bits\.push\(`ctx \$\{/.test(source) &&
    /\.context-ring\[data-level="warn"\] \{ color: var\(--tool\); \}/.test(sheet) &&
    /\.context-ring\[data-level="high"\] \{ color: var\(--error\); \}/.test(sheet),
  sheet.slice(sheet.indexOf(".context-ring"), sheet.indexOf(".context-ring") + 200),
);
// The row is one line of facts and readings, however long the folder's own names
// are: each fact keeps the room it needs and none of them wraps, while the branch
// — the name that can be arbitrarily long — is the one that gives way, cut with an
// ellipsis and clipped with its glyph when there is nothing left for it, so a
// squeezed row leaves neither a second line nor a stray mark.
check(
  "held the row to one line, with the branch the fact that gives way",
  /\.fact \{[^}]*flex: none;[^}]*white-space: nowrap;/.test(sheet) &&
    /\.composer-facts \.spend \{[^}]*white-space: nowrap;/.test(sheet) &&
    /#git \{ flex: 0 3 auto; overflow: hidden; \}/.test(sheet) &&
    /#git-branch \{[^}]*min-width: 0;[^}]*text-overflow: ellipsis;/.test(sheet),
  sheet.slice(sheet.indexOf("#git {"), sheet.indexOf("#git {") + 200),
);
// The row is chrome about the turn rather than a word from the conversation, so it
// wears the window's dim tone and its small size — the tone the status line it
// replaced carried, which the row would otherwise inherit from the box as a
// message's own text, leaving `Ready` and the totals reading as something the
// reply said.
check(
  "kept the row in the window's own dim text",
  /\.composer-facts \{[^}]*color: var\(--faint\);[^}]*font-size: 11.5px;/.test(sheet) &&
    /\.fact \{[^}]*color: var\(--faint\);[^}]*font-size: 11.5px;/.test(sheet),
  sheet.slice(sheet.indexOf(".composer-facts {"), sheet.indexOf(".composer-facts {") + 200),
);
// The chip that asks for a folder wears a marker of its own rather than the
// sheet's placeholder class: `.empty` centers whatever wears it, and on the
// composer's top row — whose other auto margin belongs to the totals — that put
// the chip in the middle of the row the moment the folder went away.
check(
  "gave the folder chip a marker of its own rather than the app's empty state",
  source.includes('el("project").classList.toggle("unset", !open)') &&
    !/el\("project"\)\.classList\.toggle\("empty"/.test(source) &&
    /\.project-chip\.unset \{/.test(sheet) &&
    /\.empty \{[^}]*margin: auto;/.test(sheet),
  `${source.indexOf('classList.toggle("unset", !open)')} / ${sheet.indexOf(".project-chip.unset")}`,
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
  branch: "M4 4.5v7M5.5 11c4 0 6.5-1.5 6.5-4.5",
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
// The rest of the window's own art — the drawings that belong to this app rather
// than to both front-ends: a project's folder, the document a file attachment
// shows, the caret a card folds by, the check a picker puts on the row in use,
// and the mark the app signs an answer with. Each is held in `ICONS` and sized
// by the sheet, and each of the two `sharedGlyphs` it also uses is the panel's
// own path rather than a look-alike.
const windowGlyphs = {
  folder: "M20.2 19.4a1.8 1.8 0 0 0 1.8-1.8v-8",
  file: "M14 3H7a2 2 0 0 0-2 2v14",
  caret: "M5.5 8.6 12 14.8l6.5-6.2",
  check: "m5.6 8 1.5 1.5 3.5-3.5",
  mark: "M12 2.4 21.6 12 12 21.6 2.4 12Z",
  plus: sharedGlyphs.new,
  close: sharedGlyphs.close,
  power: sharedGlyphs.power,
};
for (const [name, path] of Object.entries(windowGlyphs)) {
  check(
    `drew ${name} from the app's own set`,
    String(app.ICONS[name]).includes(path),
    String(app.ICONS[name]),
  );
}
// A control wears a drawing, never a character. These are the characters the
// window's controls used to be typed as — a `✕` for a row's delete, a `📁` for a
// project, a `✓` for a check — and a character is whatever shape and size the
// machine's own font gives it, which is how an emoji folder ended up beside the
// composer's stroked paperclip. Each is a path in `ICONS` instead, and one
// coming back is what this fails on.
const typedGlyphs = ["📁", "📄", "＋", "✕", "×", "◆", "✓", "▾", "▸"];
// Two marks in this window are still typed, and are meant to be: both are text
// in the panel's own webview, so drawing them here would make the two front-ends
// read the same state two ways — the divergence this check exists to catch
// rather than an instance of it. `✔`/`✖` is what a tool card's state reads as,
// and `media/main.js` picks between those same two characters. The rest are text
// for the same reason: `☐`/`☑` are an assistant's task markers in this window's
// Markdown and in the panel's, `✦` marks a thinking block the way the terminal's
// renderer does, and `·` is the separator a line of derived facts is joined
// with. A mark in a line of text is not a control's drawing.
const textCharacters = ["·", "☐", "☑", "✔", "✖", "✦"];
const sharedCharacters = ["✔", "✖", "☐", "☑"];
// So the characters the window's own strings wear are pinned rather than
// sampled: a control wearing a character is caught whichever one it is, not only
// the nine this pass replaced, and the two the panel shares have to be there — a
// pair that drifted apart in either direction fails here rather than in a
// reader's eye. What counts is a character written as a string of its own (a
// `"✖"` or a `"⌘"`), since that is a mark standing in for a drawing; the same
// character inside a longer string is prose, an em dash or an ellipsis in a
// sentence.
const standaloneCharacters = [
  ...new Set(
    [...source.matchAll(/"([^"\n]{1,3})"/g)]
      .flatMap((match) => [...match[1]])
      .filter((character) => character.codePointAt(0) > 126),
  ),
].sort();
check(
  "left no control wearing a character where a glyph belongs",
  typedGlyphs.every((character) => !source.includes(`"${character}"`)),
  typedGlyphs.filter((character) => source.includes(`"${character}"`)).join(" "),
);
check(
  "typed the characters the panel types, and only those",
  standaloneCharacters.join("") === [...textCharacters].sort().join("") &&
    sharedCharacters.every((character) => extensionIcons.includes(`"${character}"`)),
  `${standaloneCharacters.join("")} / panel ${
    sharedCharacters.filter((character) => extensionIcons.includes(`"${character}"`)).length
  }/${sharedCharacters.length}`,
);
// A control is drawn before the pointer reaches it: a ✕ that exists only while
// the row is pointed at is a control the reader cannot click without hovering it
// first, and the row changes what it says as the cursor crosses it. Every rule
// this sheet writes for a hovered or keyboard-held element is read here with the
// same selector at rest, and one whose resting rule hides the element —
// `opacity: 0`, `display: none`, `visibility: hidden` — fails. Highlighting the
// control that is under the pointer, or dimming one, is drawn either way.
const sheetRules = [...sheet.matchAll(/([^{}]+)\{([^}]*)\}/g)].map(([, selectors, body]) => [
  selectors.trim(),
  body,
]);
const hiddenAtRest = (body) =>
  /(^|[;\s])opacity:\s*0(?:[;\s]|$)/.test(body) ||
  /display:\s*none/.test(body) ||
  /visibility:\s*hidden/.test(body);
const restingBody = (selectors) =>
  sheetRules
    .filter(([each]) => each === selectors.replace(/:hover|:focus(-within|-visible)?/g, "").trim())
    .map(([, body]) => body)
    .join(";");
const hiddenUntilPointedAt = sheetRules
  .filter(([selectors]) => /:hover|:focus/.test(selectors))
  .filter(([selectors]) => hiddenAtRest(restingBody(selectors)))
  .map(([selectors]) => selectors);
check(
  "left no control waiting for the pointer before it exists",
  hiddenUntilPointedAt.length === 0,
  hiddenUntilPointedAt.join(" | "),
);
// A hover is a highlight, never the reason for a press: a rule that changes
// where a thing is while the pointer is on it moves that thing out from under
// the click the reader is already making. So what a hovered or keyboard-held
// rule may change is how the element looks — its paint, never its box.
const cosmeticOnHover = [
  "background",
  "background-color",
  "background-image",
  "border-color",
  "box-shadow",
  "color",
  "cursor",
  "fill",
  "filter",
  "opacity",
  "outline",
  "outline-color",
  "outline-offset",
  "stroke",
  "text-decoration",
  "text-decoration-color",
];
const movedOnHover = sheetRules
  .filter(([selectors]) => /:hover|:focus/.test(selectors))
  .flatMap(([selectors, body]) =>
    body
      .split(";")
      .map((declaration) => declaration.split(":")[0].trim())
      .filter((property) => property && !property.startsWith("/*"))
      .filter((property) => !cosmeticOnHover.includes(property))
      .map((property) => `${selectors} { ${property} }`),
  );
check(
  "made a hover a highlight rather than a press's reason",
  movedOnHover.length === 0,
  movedOnHover.join(" | "),
);
// Nothing invisible stands where a press lands. An element at `opacity: 0` is
// drawn as nothing and still answers the click — a control hidden that way is
// the reader's first press spent on a thing they cannot see; `pointer-events`
// is how a press is let through what is drawn; and the `[hidden]` an overlay
// carries is only what hides it while no later rule can outrank it, since a
// dialog the app has put away must not be painted over the window again.
const invisibleRules = [
  ...sheetRules
    .filter(([, body]) => /(^|[;\s])opacity:\s*0(?:[;\s]|$)/.test(body))
    .map(([selectors]) => `${selectors} { opacity: 0 }`),
  ...sheetRules
    .filter(([, body]) => /pointer-events/.test(body))
    .map(([selectors]) => `${selectors} { pointer-events }`),
];
check(
  "left nothing invisible where a press lands",
  invisibleRules.length === 0 && /\[hidden\][^{]*\{[^}]*display:\s*none\s*!important/.test(sheet),
  invisibleRules.join(" | ") ||
    (/\[hidden\]/.test(sheet) ? "the [hidden] guard lost its !important" : "the [hidden] guard is gone"),
);
// What a row carries is on the row: the ✕ that removes a thread or a folder and
// the `+` that starts a task in one. Each takes the room the row already
// reserves for it, so a long title ellipsizes against them rather than pushing
// them out.
const rowRule = (selector) => {
  const start = sheet.indexOf(`${selector} {`);
  return start < 0 ? "" : sheet.slice(start, sheet.indexOf("}", start));
};
const drawnOnTheRow = [".row-add", ".row-remove"];
check(
  "drew each row's controls without pointing at it",
  drawnOnTheRow.every((selector) => {
    const rule = rowRule(selector);
    return rule.includes("color:") && !hiddenAtRest(rule);
  }),
  drawnOnTheRow.map((selector) => rowRule(selector).replace(/\s+/g, " ")).join(" | "),
);
// A thread's row carries its title and its ✕, and nothing else: the `⌘1`…`⌘9`
// badge it used to wear named a key the row does not have to list — the
// shortcuts dialog is where that list is written down — and a badge beside the
// ✕ was a second thing the title had to make room for. The title is the
// flexible cell either way, so the ✕ keeps the row's own right-hand room.
check(
  "kept a thread's row down to its title and its ✕",
  !/"shortcut"/.test(source) &&
    !/\.session-item \.shortcut/.test(sheet) &&
    /\.session-item \.name \{ flex: 1/.test(sheet) &&
    /position: absolute/.test(rowRule(".row-remove")),
  `badge ${/"shortcut"/.test(source)} / rule ${
    /\.session-item \.shortcut/.test(sheet)
  } / ${rowRule(".session-item .name").replace(/\s+/g, " ")}`,
);
// The controls a row draws stand in the room the row reserves for them rather
// than over what the row says: `padding-right` is that room, so a press on a
// thread's name is a press on the name and never on the ✕ beside it.
const rowBodies = (selector) =>
  sheetRules
    .filter(([selectors]) =>
      selectors.split(",").some((each) => each.trim() === selector),
    )
    .map(([, body]) => body)
    .join(";");
const lastPixels = (text, property) => {
  const match = [
    ...text.matchAll(new RegExp(`(?:^|[;\\s])${property}:\\s*([\\d.]+)px`, "g")),
  ].pop();
  return match ? Number(match[1]) : NaN;
};
const rightPadding = (row) => {
  const text = rowBodies(row);
  const explicit = lastPixels(text, "padding-right");
  if (!Number.isNaN(explicit)) return explicit;
  const shorthand = [...text.matchAll(/(?:^|[;\s])padding:\s*([^;]+)/g)].pop();
  const values = shorthand
    ? [...shorthand[1].matchAll(/([\d.]+)px/g)].map(([, value]) => Number(value))
    : [];
  if (!values.length) return NaN;
  return values.length === 1 ? values[0] : values[1];
};
const roomForControl = (row, control) =>
  rightPadding(row) -
  (lastPixels(rowBodies(control), "right") + lastPixels(rowBodies(control), "width"));
const rowControls = [
  [".session-item", ".row-remove"],
  [".project-item", ".row-remove"],
  [".project-item", ".row-add"],
];
check(
  "kept each row's controls in the room the row reserves for them",
  rowControls.every(([row, control]) => roomForControl(row, control) >= 0),
  rowControls
    .map(([row, control]) => `${row} ${control} ${roomForControl(row, control)}px`)
    .join(" | "),
);
// The shortcut is a reference to a key, so the list itself is written down in
// the shortcuts dialog, and the handler takes either platform's modifier.
check(
  "named the shortcut's list in the help dialog and its keys in the handler",
  /<tr><td>⌘1…⌘9 \/ Ctrl\+1…9<\/td><td>Open the thread at that place in the sidebar<\/td><\/tr>/.test(
    shell,
  ) && /!event\.ctrlKey && !event\.metaKey/.test(source),
  shell.slice(shell.indexOf("⌘1…⌘9"), shell.indexOf("⌘1…⌘9") + 120),
);
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
  ".update-changelog",
  ".projects-tree",
  ".mcp-list",
  ".session-list",
  ".project-list",
  ".palette-list",
  ".model-list",
  ".provider-list",
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

// The bar is an overlay bar: drawn while its box is scrolled and taken away
// again once it stops, so it is never a seam down the side of what it scrolls.
// The lane above stays reserved either way, which is what keeps the thumb
// unpainted at rest rather than the scrollbar absent. Only the mark `app.js`
// puts on the element that scrolled paints it.
const ruleFor = (selector) => {
  for (const [, selectors, body] of sheet.matchAll(/([^{}]+)\{([^}]*)\}/g)) {
    if (selectors.trim() === selector) return body.trim();
  }
  return "";
};
const restingThumb = ruleFor("::-webkit-scrollbar-thumb");
const shownThumb = ruleFor(".scrolling::-webkit-scrollbar-thumb");
const paints = (body) => /(^|;)\s*background(?:-color)?:\s*(?!transparent)\S/.test(body);
check(
  "drew a scrollbar only while its box was being scrolled",
  /(^|;)\s*background(?:-color)?:\s*transparent\s*(;|$)/.test(restingThumb) && paints(shownThumb),
  `${restingThumb} | ${shownThumb}`,
);
// And the bar it paints is the thumb the lane was drawn for: a `background`
// shorthand in the rule that paints it would reset the `background-clip` the
// resting thumb is inset by, and the bar would come back the full width of its
// lane — a slab down the side of the list again, at the moment it is shown.
check(
  "kept the bar the mark paints inset inside its lane",
  !/(^|;)\s*background\s*:/.test(shownThumb) || /background-clip\s*:/.test(shownThumb),
  shownThumb,
);
// And the mark itself: put on the element that scrolled — a scrollbar belongs to
// the element it scrolls, and a scroll does not bubble, so the listener is the
// capture-phase one rather than a listener per list — and taken off again once
// that box has stopped. The stub fires the listener the page registered.
const scrolledBox = elementFor("projects-tree");
document.fire("scroll", { target: scrolledBox });
check(
  "marked the box that scrolled while it was scrolled",
  scrolledBox.classList.contains("scrolling"),
  scrolledBox.className,
);
await new Promise((resolve) => setTimeout(resolve, app.SCROLLBAR_LINGER + 80));
check(
  "took the scrollbar away again once the box stopped",
  !scrolledBox.classList.contains("scrolling"),
  scrolledBox.className,
);

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
  "gave the dialog a title, a note, a body, the release's notes and its own actions",
  /<p class="dialog-sub" id="update-note">/.test(shell) &&
    shellAt('id="update-note"') < shellAt('id="update-body"') &&
    shellAt('id="update-body"') < shellAt('id="update-changelog"') &&
    shellAt('id="update-changelog"') < shellAt('id="update-progress"') &&
    shellAt('id="update-progress"') < shellAt('id="update-page"') &&
    shellAt('id="update-page"') < shellAt('id="update-close"') &&
    shellAt('id="update-close"') < shellAt('id="update-install"'),
  `${shellAt('id="update-body"')} / ${shellAt('id="update-close"')}`,
);
check(
  "offered nothing before the check has answered",
  /id="update-page" class="ghost" hidden/.test(shell) &&
    /id="update-install" class="primary" hidden/.test(shell) &&
    /id="update-changelog" class="update-changelog" hidden/.test(shell) &&
    /id="update-progress" class="update-progress" hidden/.test(shell),
  shell.slice(shellAt('id="update-changelog"') - 20, shellAt('id="update-install"') + 40),
);
// One dialog holds the three steps of one install, so each step's own actions
// are in it: the release that is out (Dismiss and Update), the download as it
// happens (Cancel and Download in background), and the restart that runs what
// landed (Later and Restart). Only one step is ever on screen, which is what
// `app.js` shows and hides by id.
check(
  "carried every step's own way out of the one dialog",
  ["update-later", "update-cancel", "update-background"].every((id) =>
    new RegExp(`id="${id}" class="ghost" hidden`).test(shell),
  ) &&
    /id="update-restart" class="primary" hidden/.test(shell) &&
    shellAt('id="update-later"') < shellAt('id="update-install"') &&
    shellAt('id="update-install"') < shellAt('id="update-restart"'),
  shell.slice(shellAt('id="update-later"'), shellAt('id="update-restart"') + 40),
);
// The bar's own fill is what moves, so the box that holds it is the piece of
// chrome the app repaints rather than the whole dialog: a stage reported while
// the reader is elsewhere must not rebuild the buttons under their pointer.
check(
  "drew the update's progress bar inside the dialog",
  /<div id="update-progress" class="update-progress" hidden>\s*<div id="update-progress-fill" class="update-progress-fill">/.test(
    shell,
  ),
  String(shellAt('id="update-progress-fill"')),
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
// so the caret stays in the message box it is completing into: that list is the
// box's own, and taking a row finishes the text in it rather than leaving it.
await typeAt("review @sr");
const atPress = press();
atRows.children[0].fire("mousedown", atPress);
check(
  "kept the caret in the message box when a row is clicked",
  atPress.refused === true,
  String(atPress.refused),
);
document.activeElement = elementFor("prompt");
const atDocumentPress = press({ target: atRows.children[0] });
document.fire("mousedown", atDocumentPress);
check(
  "left the caret in the box a completion row belongs to",
  atDocumentPress.refused === true && document.activeElement === elementFor("prompt"),
  `${atDocumentPress.refused} / ${document.activeElement?.id ?? "(none)"}`,
);
atRows.children[0].onclick(atPress);
check(
  "took the row a click landed on",
  composer.value === "review @src/" && composer.selectionStart === 12,
  `${composer.value} @ ${composer.selectionStart}`,
);

// WebKit may spend the first press after editing only moving focus to a button,
// withholding the click until the next press. The page keeps that focus default
// off the press and leaves the one native click in charge — and the box the
// reader has moved on from gives up the caret with it, so the next thing typed
// goes where they clicked. If a WebKit version withholds the click anyway, the
// page supplies the single one that was withheld.
const composerButton = new StubElement("button", "composer-button");
let composerButtonClicks = 0;
composerButton.onclick = () => composerButtonClicks++;
const composerRow = new StubElement("div", "composer-row");
const composerLink = new StubElement("a", "composer-link");
composerLink.setAttribute("href", "https://example.com/docs");
document.activeElement = elementFor("prompt");
const buttonPress = press({ target: composerButton });
document.fire("mousedown", buttonPress);
check(
  "kept the press off the focus default that spends its click",
  buttonPress.refused === true,
  String(buttonPress.refused),
);
check(
  "took the caret out of the box a button press landed away from",
  document.activeElement === null,
  document.activeElement?.id ?? "(none)",
);
// The browser delivers the press's own click after mouseup: the page must not
// add a second.
document.fire("mouseup", press({ target: composerButton }));
document.fire("click", press({ target: composerButton }));
composerButton.onclick(buttonPress);
await nextTick();
check(
  "handled one native click exactly once",
  composerButtonClicks === 1,
  String(composerButtonClicks),
);

// A press the browser released somewhere other than the control it began on is
// cancelled: the fallback must not turn a drag-off into a click.
document.activeElement = elementFor("prompt");
const draggedPress = press({ target: composerButton });
document.fire("mousedown", draggedPress);
document.fire("mouseup", press({ target: composerRow }));
await nextTick();
check(
  "supplied nothing for a press released away from its control",
  composerButtonClicks === 1,
  String(composerButtonClicks),
);

// A disabled control never answers a click, and the fallback must not force one.
const disabledButton = new StubElement("button", "disabled-button");
disabledButton.disabled = true;
let disabledClicks = 0;
disabledButton.onclick = () => disabledClicks++;
document.activeElement = elementFor("prompt");
document.fire("mousedown", press({ target: disabledButton }));
document.fire("mouseup", press({ target: disabledButton }));
await nextTick();
check("supplied nothing for a disabled control", disabledClicks === 0, String(disabledClicks));

// A release from another button while the primary press is still held must not
// run the fallback early; the primary release is the one that finishes it.
document.activeElement = elementFor("prompt");
const chordedPress = press({ target: composerButton });
document.fire("mousedown", chordedPress);
document.fire("mouseup", press({ target: composerButton, button: 2 }));
await nextTick();
check(
  "left a primary press open through a secondary release",
  composerButtonClicks === 1,
  String(composerButtonClicks),
);
document.fire("mouseup", press({ target: composerButton }));
await nextTick();
check(
  "supplied the primary press's click once on its own release",
  composerButtonClicks === 2,
  String(composerButtonClicks),
);

// Older WebKit ends the editor before it dispatches mousedown, so
// `activeElement` is already empty when the page first sees the press. The
// preceding `focusout` keeps that press associated with the editor, and the
// click the webview omitted is supplied once the press is over.
document.activeElement = elementFor("prompt");
document.fire("focusout", press({ target: elementFor("prompt") }));
document.activeElement = null;
const preBlurredButtonPress = press({ target: composerButton });
document.fire("mousedown", preBlurredButtonPress);
check(
  "kept a press WebKit dispatched after it had already blurred the editor",
  preBlurredButtonPress.refused === true,
  String(preBlurredButtonPress.refused),
);
document.fire("mouseup", press({ target: composerButton }));
await nextTick();
check(
  "supplied the click the pre-blurred press never produced",
  composerButtonClicks === 3,
  String(composerButtonClicks),
);

// The memory of an ended editing session lasts only for its own press.
document.activeElement = elementFor("prompt");
document.fire("focusout", press({ target: elementFor("prompt") }));
document.activeElement = null;
await nextTick();
const laterButtonPress = press({ target: composerButton });
document.fire("mousedown", laterButtonPress);
check(
  "forgot an ended editing session before a later unrelated press",
  laterButtonPress.refused !== true,
  String(laterButtonPress.refused),
);
document.fire("mouseup", laterButtonPress);

// A dialog's box is the same case as the message box: a press on a control
// elsewhere in the window leaves it, and it still answers one click.
const dialogBox = elementFor("model-filter");
document.activeElement = dialogBox;
const filterButtonPress = press({ target: composerButton });
document.fire("mousedown", filterButtonPress);
check(
  "took the caret out of a dialog's box when a button was pressed",
  filterButtonPress.refused === true && document.activeElement !== dialogBox,
  `${filterButtonPress.refused} / ${document.activeElement?.id ?? "(none)"}`,
);
document.fire("mouseup", press({ target: composerButton }));
document.fire("click", press({ target: composerButton }));
await nextTick();

// With no editor active the press keeps its ordinary browser behavior, and no
// click is ever supplied on its behalf.
document.activeElement = null;
const ordinaryButtonPress = press({ target: composerButton });
document.fire("mousedown", ordinaryButtonPress);
check(
  "left a button's ordinary focus behavior alone when no editor was active",
  ordinaryButtonPress.refused !== true,
  String(ordinaryButtonPress.refused),
);
document.fire("mouseup", ordinaryButtonPress);
await nextTick();
check(
  "supplied nothing for a press with no editing session",
  composerButtonClicks === 3,
  String(composerButtonClicks),
);

// A press into another text field is the browser's to place the caret with.
const otherField = new StubElement("input", "other-field");
document.activeElement = elementFor("prompt");
const fieldPress = press({ target: otherField });
document.fire("mousedown", fieldPress);
check(
  "left another text field's focus behavior to the browser",
  fieldPress.refused !== true,
  String(fieldPress.refused),
);
document.fire("mouseup", fieldPress);

// A non-editor input holding focus is not an editing session.
const checkbox = new StubElement("input", "checkbox");
checkbox.setAttribute("type", "checkbox");
document.activeElement = checkbox;
const checkboxButtonPress = press({ target: composerButton });
document.fire("mousedown", checkboxButtonPress);
check(
  "left a button press alone while a non-editor input held focus",
  checkboxButtonPress.refused !== true,
  String(checkboxButtonPress.refused),
);
document.fire("mouseup", checkboxButtonPress);

// A row the page wires a click to is a control, so a press on it leaves the box
// the reader was typing in too; a row with no click of its own is left native.
const composerRowControl = new StubElement("div", "composer-row-control");
composerRowControl.onclick = () => {};
document.activeElement = elementFor("prompt");
const rowControlPress = press({ target: composerRowControl });
document.fire("mousedown", rowControlPress);
check(
  "took the caret out of the box a clickable row's press left",
  rowControlPress.refused === true && document.activeElement === null,
  `${rowControlPress.refused} / ${document.activeElement?.id ?? "(none)"}`,
);
document.fire("click", press({ target: composerRowControl }));
document.fire("mouseup", rowControlPress);

const composerRowPress = press({ target: composerRow });
document.fire("mousedown", composerRowPress);
check(
  "left a row with no click of its own native while the editor had the caret",
  composerRowPress.refused !== true,
  String(composerRowPress.refused),
);
document.fire("mouseup", composerRowPress);

document.activeElement = elementFor("prompt");
const composerLinkPress = press({ target: composerLink });
document.fire("mousedown", composerLinkPress);
check(
  "took the caret out of the box a link's press left",
  composerLinkPress.refused === true && document.activeElement === null,
  `${composerLinkPress.refused} / ${document.activeElement?.id ?? "(none)"}`,
);
document.fire("click", press({ target: composerLink }));
document.fire("mouseup", composerLinkPress);
document.activeElement = null;

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

// ---------- /mcp ----------

const driveMcps = async () => {
  elementFor("mcp-list").children = [];
  elementFor("prompt").value = "/mcp";
  await app.send(false);
  return elementFor("mcp-list").outline();
};

console.log("/mcp");
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

// ---------- /session ----------

console.log("/session");
// A command the app performs itself never reaches the model, and the list is
// drawn in the app rather than handed to the window as a native picker.
app.state.project = "/home/dev/Projects/oxide";
app.state.projects = [
  { id: "/home/dev/Projects/oxide", name: "oxide", path: "/home/dev/Projects/oxide" },
];
elementFor("sessions-modal").hidden = true;
calls.length = 0;
const handled = await app.runSlashCommand("/session");
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
check("left `/session <id>` to the agent", (await app.runSlashCommand("/session fe0031b1")) === false);

threads = [];
// With no thread open either: the thread the window is in is listed even when
// the store has none, so an empty listing is the case with nothing open at all.
const openThread = app.state.session;
app.state.session = null;
await app.runSlashCommand("/session");
check(
  "said so when the project has no threads",
  elementFor("sessions-list").innerHTML.includes("No threads for this project yet"),
  elementFor("sessions-list").innerHTML,
);
app.state.session = openThread;

// A store that cannot be read is not the same as a project with no threads, and
// saying so is the whole point of a listing opened where it was asked.
threadsError = "permission denied";
await app.runSlashCommand("/session");
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
  "gave a PDF the app's own file glyph instead of a thumbnail",
  pdf?.children[0]?.className === "att-file" && pdf.children[0].innerHTML === app.ICONS.file,
  elementFor("attachments").outline(),
);
// Its ✕ is a drawing too, the same one a row's delete wears.
check(
  "drew the chip's remove as the app's own glyph",
  pdf?.children[2]?.className === "att-remove" && pdf.children[2].innerHTML === app.ICONS.close,
  elementFor("attachments").outline(),
);
// A text file rides along as its own text and a TIFF is an image the core
// converts, so neither is turned away for its type; both wear the file glyph,
// since there is no picture to draw for either.
check(
  "kept a text attachment",
  app.addAttachment("notes.txt", "data:text/plain;base64,YQ==") === true,
  status(),
);
const notes = chips()[2];
check(
  "gave a text file the app's own file glyph instead of a thumbnail",
  notes?.children[0]?.className === "att-file",
  elementFor("attachments").outline(),
);
check(
  "kept an image the core converts",
  app.addAttachment("scan.tif", "data:image/tiff;base64,AA") === true,
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

// The type a browser declared is no gate: the core reads the bytes a data URL
// carries, and a picked source file can arrive typed
// `application/octet-stream`, so refusing on the declared type is how a valid
// attachment never reaches it. The size is the one thing decided before the
// read, which the file past the limit above shows.
await app.addAttachmentFiles([
  {
    name: "notes.md",
    size: 1024,
    type: "application/octet-stream",
    dataUrl: "data:application/octet-stream;base64,IyBub3Rlcwo=",
  },
]);
check(
  "attached a text file the browser typed as a generic blob",
  app.state.attachments.length === 1 && app.state.attachments[0].name === "notes.md",
  JSON.stringify(app.state.attachments),
);
app.state.attachments = [];
// A blob the browser did not type is decided by the data URL it becomes.
await app.addAttachmentFiles([
  { name: "shot.png", size: 1024, type: "", dataUrl: "data:image/png;base64,AA" },
]);
check(
  "attached an untyped blob that reads as an image",
  app.state.attachments.length === 1 && app.state.attachments[0].name === "shot.png",
  JSON.stringify(app.state.attachments),
);
app.state.attachments = [];

// macOS refuses a webview's read of a copied file in the Desktop, Documents or
// Downloads folder. A paste asks the harness, which reads the pasteboard the
// way the terminal's Ctrl+V does; the picker never falls back, since a file the
// reader chose is not the clipboard.
clipboardAnswer = { name: "clipboard-icon.png", dataUrl: "data:image/png;base64,AA" };
await app.addAttachmentFiles([{ name: "Earlier Lines.png", size: 1024, type: "image/png" }], {
  fallbackToClipboard: true,
});
check(
  "attached the pasteboard picture when a pasted file could not be read",
  app.state.attachments.length === 1 && app.state.attachments[0].name === "clipboard-icon.png",
  JSON.stringify(app.state.attachments),
);
app.state.attachments = [];
await app.addAttachmentFiles([{ name: "Earlier Lines.png", size: 1024, type: "image/png" }]);
check(
  "did not reach for the clipboard for a file the reader picked",
  app.state.attachments.length === 0,
  JSON.stringify(app.state.attachments),
);
clipboardAnswer = null;

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

// A choice row keeps the editor focused through mousedown for the same WebKit
// reason as a button, then its one native click activates the input it labels.
const secondRadio = optionRows[1].control;
let radioActivations = 0;
secondRadio.click = () => {
  radioActivations++;
  secondRadio.checked = true;
};
document.activeElement = firstBlock.querySelector(".question-free");
const radioLabelPress = press({ target: optionRows[1] });
document.fire("mousedown", radioLabelPress);
check(
  "kept the question editor focused while a radio row was pressed",
  radioLabelPress.refused === true,
  String(radioLabelPress.refused),
);
// The row's one browser click activates the input it labels after mouseup; the
// page must not supply a second.
document.fire("mouseup", radioLabelPress);
document.fire("click", press({ target: optionRows[1] }));
secondRadio.click();
await nextTick();
check(
  "handled the radio row's native activation once",
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
check(
  "kept the question editor focused while a checkbox was pressed",
  checkboxPress.refused === true,
  String(checkboxPress.refused),
);
document.fire("mouseup", checkboxPress);
document.fire("click", press({ target: firstCheckbox }));
firstCheckbox.click();
await nextTick();
check(
  "handled the checkbox's native activation once",
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
// What folds wears the caret and the pointer; a card showing everything it has
// is text, so its header does not offer a fold that does nothing. The sheet
// draws the caret for `.foldable` alone — the rule the panel's own card
// follows — and reserves its width either way, so a card's name does not jump
// when it finishes and the caret appears.
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
check(
  "offered the caret and the pointer only on a card that folds",
  String(editCard.block.className).includes("foldable") &&
    String(plainCard.block.className).includes("foldable") &&
    !String(shortCard.block.className).includes("foldable") &&
    editCard.head.children[0].innerHTML === app.ICONS.caret &&
    /\.tool \.tool-caret \{[^}]*visibility: hidden;/.test(sheet) &&
    /\.tool\.foldable \.tool-caret \{ visibility: visible; \}/.test(sheet) &&
    /\.tool\.foldable \.thead \{ cursor: pointer; \}/.test(sheet) &&
    /\.tool\.expanded \.tool-caret \{ transform: rotate\(0deg\); \}/.test(sheet) &&
    !/\.tool \.thead \{[^}]*cursor: pointer/.test(sheet) &&
    !/\.tool .thead::before/.test(sheet),
  `${editCard.block.className} / ${plainCard.block.className} / ${shortCard.block.className} / ${editCard.head.children[0].innerHTML}`,
);
// A finished call is a line of the transcript rather than a box around it, so a
// long run reads as a log of what happened: the surface is kept for the card
// still being written and the one that failed, each with its state on the left
// edge, and a card's output sits below the reply's own text rather than
// competing with it.
const errorCard = app.startTool("bash", JSON.stringify({ command: "false" }));
app.finishTool(errorCard, "boom\n", { isError: true });
const runningCard = app.startTool("bash", JSON.stringify({ command: "sleep 1" }));
check(
  "kept a surface for the card being written and the one that failed",
  /\.tool \{[^}]*background: none;[^}]*border: none;/.test(sheet) &&
    /\.tool\.running, \.tool\.error \{ padding: 9px 12px; background: var\(--tool-bg\); \}/.test(sheet) &&
    /\.tool\.running \{ box-shadow: inset 2px 0 0 var\(--accent\); \}/.test(sheet) &&
    /\.tool\.error \{ box-shadow: inset 2px 0 0 var\(--error\); \}/.test(sheet) &&
    /\.tool pre \{\s*color: var\(--dim\);/.test(sheet) &&
    String(errorCard.block.className).includes("error") &&
    String(runningCard.block.className).includes("running") &&
    !/running|error/.test(String(editCard.block.className)),
  `${errorCard.block.className} / ${runningCard.block.className} / ${editCard.block.className}`,
);
// Everything in the conversation sits in one measured column — the reply, a card
// a call or a turn left, the thinking block, the composer's box and its status
// line — so the window's edges line up rather than each row picking its own.
const columnCarriers = [".msg {", ".tool {", ".changes {", ".thinking {", ".composer {", ".popover {"];
const columnWidth = (selector) => {
  const at = sheet.indexOf(selector);
  if (at < 0) return "";
  return (sheet.slice(at, sheet.indexOf("}", at)).match(/max-width: (\d+)px/) || [])[1] || "";
};
const columnWidths = columnCarriers.map(columnWidth);
check(
  "kept the conversation and the composer in one column",
  columnWidths.every((width) => width === "780"),
  columnCarriers.map((selector, index) => `${selector} ${columnWidths[index]}`).join(" · "),
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
    head.children[0].innerHTML === app.ICONS.file &&
    head.children[5].children[0].textContent === "Undo" &&
    head.children[5].children[1].textContent === "Review",
  `${head.children[0].className} / ${head.children[5].children.map((b) => b.textContent).join(", ")}`,
);
// The card's fold and each row's are the same caret, drawn pointing down: a
// closed row turns it to the right, an open one leaves it as it is.
const changedRowCaret = transcriptCards()[0].children[1].children[0];
check(
  "drew the card's fold and its rows' carets from the app's own set",
  head.children[1].innerHTML === app.ICONS.caret &&
    String(changedRowCaret.innerHTML).includes(app.ICONS.caret),
  `${head.children[1].innerHTML} / ${changedRowCaret.innerHTML}`,
);
// The caret is drawn pointing down, so a row that is closed turns it to the
// right and one that is open leaves it alone — while the card's own fold is the
// other way round, since the card starts open.
check(
  "turned a closed row's caret to its row, and the card's to its own state",
  /\.change-chev \{[^}]*transform: rotate\(-90deg\)/.test(sheet) &&
    /\.change-row\.open \.change-chev \{ transform: rotate\(0deg\)/.test(sheet) &&
    /\.changes\.collapsed \.changes-caret \{ transform: rotate\(-90deg\)/.test(sheet),
  `${sheet.slice(sheet.indexOf(".change-chev {"), sheet.indexOf(".change-diff {"))}`,
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
  // A client command belongs to the app, not to the model: a name the app does
  // not answer is sent on as a prompt, which is the failure this catches.
  const unperformed = [];
  for (const entry of client) {
    elementFor("status-text").textContent = "";
    const handled = await app.runSlashCommand(`/${entry.name}`);
    const said = status();
    check(`/${entry.name} is answered by the app`, handled === true, `handled=${handled}`);
    if (said.includes("is not available in the desktop app yet")) unperformed.push(`/${entry.name}`);
  }
  if (unperformed.length) {
    console.log(`  note the app answers these with a "not available" note: ${unperformed.join(", ")}`);
  }
  // One spelling per command: the catalog carries a name and no second spelling
  // for it — not even an empty list of them — and the page resolves nothing
  // beyond the name it was given.
  const aliased = client.filter((entry) => "aliases" in entry);
  check(
    "the catalog declares no alias for a client command",
    aliased.length === 0,
    aliased.map((entry) => entry.name).join(", "),
  );
  // A retired spelling is not the app's: it has no arm to reach, so it stays a
  // prompt for the CLI rather than being answered here.
  elementFor("status-text").textContent = "";
  for (const retired of ["/mcps", "/approvals", "/access", "/thinking", "/sessions", "/login"]) {
    check(`${retired} is not the app's own command`, (await app.runSlashCommand(retired)) === false);
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

    // The palette is the box's own list too, so a press on a row leaves the
    // caret where the row is about to complete into rather than taking it.
    document.activeElement = elementFor("prompt");
    const paletteRowPress = press({ target: painted });
    document.fire("mousedown", paletteRowPress);
    check(
      "left the caret in the box a command row belongs to",
      paletteRowPress.refused === true && document.activeElement === elementFor("prompt"),
      `${paletteRowPress.refused} / ${document.activeElement?.id ?? "(none)"}`,
    );

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

  // The window opens on no project and the composer is ready to type in, so `/`
  // is asked for there too: a command, a prompt template and a skill are read
  // from a folder and the built-ins are what the app performs itself, so the
  // core answers a projectless ask with those rather than leaving the menu empty.
  // The built-ins the core names this app for: a command another front-end
  // performs (`/agent` is the VS Code panel's) is not a row here, since taking
  // it would reach the app's "not available" note rather than doing anything.
  const builtinRows = catalog.filter(
    (entry) =>
      entry.source === "builtin" &&
      (!(entry.front_ends || []).length || entry.front_ends.includes("desktop")),
  );
  const elsewhereRows = client.filter(
    (entry) => (entry.front_ends || []).length && !entry.front_ends.includes("desktop"),
  );
  check(
    "the catalog names commands for other front-ends",
    elsewhereRows.length > 0,
    "no client command is another front-end's, so the rows could not be held",
  );
  app.state.project = null;
  elementFor("prompt").value = "/";
  calls.length = 0;
  await app.refreshPaletteEntries();
  app.renderPalette();
  const homeRows = app.paletteMatches() || [];
  check(
    "asked the core for the palette of no project",
    projectCalls("list_commands").at(-1)?.[1]?.project === "",
    JSON.stringify(projectCalls("list_commands").at(-1)),
  );
  check(
    "listed the built-ins on the home state's `/` menu",
    builtinRows.length > 0 &&
      homeRows.map((entry) => entry.name).join(" ") === builtinRows.map((entry) => entry.name).join(" ") &&
      elementFor("palette-list").children.length === builtinRows.length,
    `${JSON.stringify(homeRows.map((entry) => entry.name))} / ${elementFor("palette-list").outline()}`,
  );
  check(
    "left out the built-ins the catalog gives to another front-end",
    elsewhereRows.every(
      (entry) => !homeRows.some((row) => row.name === entry.name),
    ),
    `${JSON.stringify(elsewhereRows.map((entry) => entry.name))} / ${JSON.stringify(homeRows.map((entry) => entry.name))}`,
  );
  // Typed rather than taken from the menu, one of them names the front-end that
  // performs it instead of promising it here later.
  const elsewhere = elsewhereRows[0];
  elementFor("status-text").textContent = "";
  await app.runSlashCommand(`/${elsewhere.name}`);
  const note = status();
  check(
    `named the front-end that performs /${elsewhere.name}`,
    new RegExp(`/${elsewhere.name} is the (terminal|desktop app|VS Code panel)`).test(note),
    note,
  );
  // A client command belongs to the app wherever the window is, so taking `/new`
  // at home is performed rather than sent to the model: with no folder open it
  // asks for one, which is the picker.
  elementFor("projects-modal").hidden = true;
  elementFor("prompt").value = "/new";
  calls.length = 0;
  await app.runPaletteEntry(homeRows.find((entry) => entry.name === "new"));
  check(
    "performed /new at home rather than sending it to the model",
    elementFor("projects-modal").hidden === false &&
      !calls.some(([name]) => name === "send_prompt"),
    `${elementFor("projects-modal").hidden} / ${JSON.stringify(calls.map(([name]) => name))}`,
  );
  elementFor("projects-modal").hidden = true;
  elementFor("prompt").value = "";
  app.state.project = "/home/dev/Projects/oxide";
  await app.refreshPaletteEntries();
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
// The row in use wears a drawn check rather than a `✓` character, and only that
// row wears one.
check(
  "drew the picker's check on the row in use and nowhere else",
  reasoningRow("auto").children[2].innerHTML === app.ICONS.check &&
    reasoningRow("high").children[2].innerHTML === "",
  `${reasoningRow("auto").children[2].innerHTML} / ${reasoningRow("high").children[2].innerHTML}`,
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
// Until the reader picks a level, the window leaves the choice to the core so a
// resumed session can restore the level it recorded.
check(
  "left the reasoning level to the thread until a row was picked",
  app.state.reasoningPicked === false,
  String(app.state.reasoningPicked),
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
    app.state.reasoningPicked === true &&
    elementFor("reasoning-modal").hidden === true &&
    elementFor("reasoning").getAttribute("aria-label") === "thinking: medium",
  `${app.state.reasoning} / ${app.state.reasoningPicked} / ${elementFor("reasoning-modal").hidden} / ${elementFor("reasoning").getAttribute("aria-label")}`,
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
// A model whose listing advertised its own levels narrows the picker to those,
// so it never offers a level the run would clamp away. The window reads the
// levels the host warmed from the listing before painting the rows.
reasoningAnswer.reasoningLevels = ["off", "low", "high", "max"];
app.state.reasoningLevels = null;
await app.openReasoning();
check(
  "narrowed the picker to the levels the model advertised",
  reasoningNames().join(" ") === "auto off low high max",
  `${reasoningNames().join(" ")} / reasoningAnswer=${JSON.stringify(reasoningAnswer.reasoningLevels)}`,
);
document.fire("keydown", { key: "Escape", preventDefault() {} });
reasoningAnswer.reasoningLevels = ["off", "minimal", "low", "medium", "high", "xhigh", "max"];
app.state.reasoningLevels = null;
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
  status() === "Reasoning must be one of auto, off, minimal, low, medium, high, xhigh, max.",
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

// ---------- markdown ----------

console.log("markdown");
// A Markdown table is rendered from `splitRow`, a string splitter. The review's
// diff row builder once shared the name and shadowed it through hoisting, so a
// transcript holding a table threw `header.map is not a function` — the table's
// "header" was the DOM row the diff builder returns. A table must render both on
// its own and beside a line whose inline code holds a pipe.
const soloTable = app.renderMarkdown("| a | b |\n|---|---|\n| 1 | 2 |");
check(
  "rendered a Markdown table's header and rows",
  soloTable.includes("<table>") &&
    soloTable.includes("<th>a</th>") &&
    soloTable.includes("<td>1</td>"),
  soloTable,
);
const tableByPipe = app.renderMarkdown(
  "A line with `a|b` code.\n\n| x | y |\n|---|---|\n| 1 | 2 |",
);
check(
  "rendered a table beside an inline pipe in code",
  tableByPipe.includes("<table>") && tableByPipe.includes("<code>a|b</code>"),
  tableByPipe,
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
// A box does not paint on focus: no input changes its border when the caret
// moves into it, so every box looks the same whether the caret is in it or not.
// `outline: none` stays to suppress WebKit's own focus ring. The rule is read
// out of the sheet rather than a selector hardcoded here: every `:focus` rule
// that names a box must declare nothing but `outline: none`.
const boxNames = new Set(["composer", "question-free", "question-choice"]);
for (const [, attrs] of shell.matchAll(/<(?:input|textarea)\b([^>]*)>/g)) {
  for (const [, id] of attrs.matchAll(/\bid="([^"]+)"/g)) boxNames.add(id);
  for (const [, cls] of attrs.matchAll(/\bclass="([^"]+)"/g)) {
    cls.split(/\s+/).forEach((name) => name && boxNames.add(name));
  }
}
const focusPaints = [];
for (const [, selector, body] of sheet.matchAll(/([^{}]+)\{([^{}]*)\}/g)) {
  const within = selector.trim();
  if (!/:focus(-visible|-within)?\b/.test(within)) continue;
  const namesBox =
    /\binput\b|\btextarea\b/.test(within) ||
    [...boxNames].some((name) => new RegExp(`[.#]${name}(?![\\w-])`).test(within));
  if (!namesBox) continue;
  // A `:focus-visible` rule is the keyboard's own indicator and may draw an
  // outline — that is what keeps a radio/checkbox a Tab reaches visible. Every
  // other focus rule must be inert but for suppressing WebKit's own ring.
  const keyboardOnly = within.includes(":focus-visible");
  const paints = body
    .split(";")
    .map((decl) => decl.trim())
    .filter(Boolean)
    .filter((decl) =>
      keyboardOnly ? !/^outline(-offset)?:/.test(decl) : !/^outline:\s*none\b/.test(decl),
    );
  if (paints.length) focusPaints.push(`${within} => ${paints.join("; ")}`);
}
check(
  "left every input box looking the same whether the caret is in it or not",
  focusPaints.length === 0,
  focusPaints.join(" | "),
);
check(
  "kept a keyboard-only outline on the question choice",
  /\.question-choice:focus-visible \{[^}]*outline: 2px solid var\(--accent\)/.test(sheet),
  sheet.match(/\.question-choice:focus-visible \{[^}]*\}/)?.[0] || "",
);
const restingClass = elementFor("composer").className;
elementFor("prompt").fire("focus");
check(
  "painted nothing on the composer for the caret in it",
  elementFor("composer").className === restingClass,
  `${restingClass} -> ${elementFor("composer").className}`,
);
elementFor("prompt").fire("blur");
elementFor("prompt").value = "a message";
app.updateSendState();
check(
  "lifted it once the box had something in it",
  elementFor("composer").classList.contains("filled") === true,
  elementFor("composer").className,
);
elementFor("prompt").value = "";
app.updateSendState();
check(
  "set it back down when the box was emptied",
  elementFor("composer").classList.contains("filled") === false,
  elementFor("composer").className,
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

// ---------- the bar above the composer ----------

console.log("the composer's bar");
// What the bar says about the folder a turn runs in: the branch it is on, where
// that was read from, and the window the thread's own tokens are counted against.
// None of it is a control — the branch is a fact about the folder — so what a
// check can drive is the read behind it and the ring's own painting.
const barProject = "/home/dev/Projects/oxide";
app.state.projects = [{ name: "oxide", path: barProject, registered: true }];
app.state.project = null;
app.state.session = null;
app.state.runSession = null;
app.state.sendView = null;
app.state.parked = null;
app.resetTranscript();
calls.length = 0;
await app.selectProject({ name: "oxide", path: barProject });
check(
  "named the branch of the folder that was opened",
  app.state.git?.branch === "main" &&
    el("git").hidden === false &&
    el("git-branch").textContent === "main" &&
    el("git").title === `On branch main\n${gitAnswer.root}` &&
    projectCalls("git_info").length > 0,
  `${JSON.stringify(app.state.git)} / ${el("git-branch").textContent} / ${el("git").title}`,
);
// A folder that is in no repository has no branch to name: the fact goes away
// rather than saying nothing, and the rest of the row — the folder and the ring —
// stays.
gitAnswer = { repo: false, root: "", branch: "", detached: "" };
await app.loadGit();
check(
  "hid the branch in a folder that is no repository",
  el("git").hidden === true &&
    el("git-branch").textContent === "" &&
    el("project-name").textContent === "oxide",
  `${el("git").hidden} / ${el("git-branch").textContent} / ${el("project-name").textContent}`,
);
// A read that failed leaves no chip behind either, rather than naming the branch
// the folder had a moment ago: the row says what it knows, and about this folder's
// repository it knows nothing.
gitError = "permission denied";
await app.loadGit();
check(
  "left no branch behind when the read failed",
  app.state.git === null && el("git").hidden === true && el("git-branch").textContent === "",
  `${JSON.stringify(app.state.git)} / ${el("git").hidden} / ${el("git-branch").textContent}`,
);
gitError = null;
// A detached HEAD has no branch at all: it is named by the commit it is on, and
// the tooltip says which of the two the chip is showing.
gitAnswer = { repo: true, root: barProject, branch: "", detached: "9cdea1c" };
await app.loadGit();
check(
  "named a detached HEAD by its commit",
  el("git").hidden === false &&
    el("git-branch").textContent === "9cdea1c" &&
    el("git").title === `HEAD is detached at 9cdea1c\n${barProject}`,
  `${el("git-branch").textContent} / ${el("git").title}`,
);
// A branch switched in a terminal reaches this window through nothing but the
// reader coming back to it, which is where the banner's own facts are read again.
gitAnswer = { repo: true, root: barProject, branch: "feat/desktop-bar", detached: "" };
calls.length = 0;
window.fire("focus");
await nextTick();
check(
  "read the branch again when the reader came back to the window",
  el("git-branch").textContent === "feat/desktop-bar" &&
    projectCalls("git_info").length === 1 &&
    projectCalls("git_info")[0][1].project === barProject,
  `${el("git-branch").textContent} / ${JSON.stringify(calls.map(([name]) => name))}`,
);
// A turn is where the branch can change — the model runs git like any other
// command — so the bar is read again as the turn ends rather than left saying
// where the folder stood before it.
gitAnswer = { repo: true, root: barProject, branch: "fix/a-turn-switched-it", detached: "" };
calls.length = 0;
await emit("agent-end", { runId: 72, project: barProject });
await nextTick();
check(
  "read the branch again when a turn ended",
  el("git-branch").textContent === "fix/a-turn-switched-it" &&
    projectCalls("git_info").length === 1,
  `${el("git-branch").textContent} / ${JSON.stringify(calls.map(([name]) => name))}`,
);
// The context reading: the arc is filled to the percent the totals line draws
// the spend from, so the two never disagree about how much of the window is gone,
// the percent is spelled out beside the arc, and the tokens behind both are in
// the reading's tooltip.
app.state.contextWindow = 128000;
const ringFill = () => {
  const [filled, whole] = String(el("context-ring-fill").getAttribute("stroke-dasharray"))
    .split(" ")
    .map(Number);
  return (filled / whole) * 100;
};
app.handleEvent({ type: "usage", usage: { input: 30000, output: 40, cost: 0.01 } });
check(
  "filled the ring to the percent it spells out beside the arc",
  el("usage").textContent === "↑ 30000 ↓ 40 · $0.0100" &&
    el("context-ring").hidden === false &&
    Math.round(ringFill()) === 23 &&
    el("context-percent").textContent === "23%" &&
    el("context-ring").title === "23% · 30.0k/128.0k" &&
    el("context-ring").dataset.level === "ok" &&
    el("context-ring").getAttribute("aria-label") === "context window: 23% · 30.0k/128.0k",
  `${el("usage").textContent} / ${el("context-percent").textContent} / ${el("context-ring-fill").getAttribute("stroke-dasharray")} / ${el("context-ring").title}`,
);
// The arc escalates through the same two thresholds the terminal's footer and
// the panel's own gauge color by, so the color reads the same in all three.
app.handleEvent({ type: "usage", usage: { input: 100000, output: 40 } });
const warned = el("context-ring").dataset.level;
app.handleEvent({ type: "usage", usage: { input: 120000, output: 40 } });
const high = el("context-ring").dataset.level;
check(
  "escalated the ring at the same two thresholds as the other front-ends",
  warned === "warn" && high === "high" && Math.round(ringFill()) === 94,
  `${warned} / ${high} / ${ringFill().toFixed(1)}`,
);
// A thread that has spent nothing yet has no percentage: the ring is drawn empty,
// nothing is spelled out beside it, and the tooltip names the window it would be
// measured against.
app.handleEvent({ type: "usage", usage: {} });
check(
  "drew an empty ring for a window nothing has been counted against",
  el("context-ring").hidden === false &&
    Math.round(ringFill()) === 0 &&
    el("context-percent").textContent === "" &&
    el("context-ring").dataset.level === "ok" &&
    el("context-ring").title === "No request yet · 128.0k window",
  `${ringFill()} / ${el("context-percent").textContent} / ${el("context-ring").title}`,
);
// A thread opened again draws its ring from the request it was last run with,
// which the core hands over beside the thread's totals: an empty ring would say
// the conversation had spent nothing when it had spent most of its window.
sessionUsage = {
  input: 120,
  output: 30,
  cacheRead: 0,
  cacheWrite: 0,
  cost: 0.01,
  messageCount: 2,
  contextTokens: 64000,
};
app.state.session = null;
await app.openSession({ id: "a1b2c3d4", cwd: barProject, name: "the bar" });
check(
  "drew the context reading from what the thread was last run with",
  el("context-ring").hidden === false &&
    Math.round(ringFill()) === 50 &&
    el("usage").textContent === "↑ 120 ↓ 30 · $0.0100" &&
    el("context-percent").textContent === "50%" &&
    el("context-ring").title === "50% · 64.0k/128.0k",
  `${el("usage").textContent} / ${el("context-percent").textContent} / ${el("context-ring-fill").getAttribute("stroke-dasharray")} / ${el("context-ring").title}`,
);
sessionUsage = null;
// The window a ring measures against belongs to the folder on screen: switching
// folders drops it, so the thread the new folder opens is not labeled with the
// window of the folder just left — and shows no window at all until that folder's
// own facts arrive.
app.state.contextWindow = 128000;
app.state.session = null;
projectInfo = null;
await app.selectProject({ name: "elsewhere", path: "/tmp/elsewhere" });
check(
  "dropped the window of the folder being left",
  app.state.contextWindow === 0 &&
    el("context-ring").title === "No request yet" &&
    el("context-percent").textContent === "" &&
    Math.round(ringFill()) === 0,
  `${app.state.contextWindow} / ${el("context-percent").textContent} / ${el("context-ring").title}`,
);
// Nothing is open, so there is no branch to name, no turn to place and no window
// to measure a request against: the chip that takes a folder is the whole row, and
// it is the reader's to act on rather than a value among others.
app.clearSelectedProject();
check(
  "left the folder chip alone on the row with nothing open",
  el("context-ring").hidden === true &&
    el("context-percent").textContent === "" &&
    el("git").hidden === true &&
    el("project-name").textContent === "Choose a project" &&
    el("project").classList.contains("unset") &&
    !el("project").classList.contains("empty"),
  `${el("context-ring").hidden} / ${el("git").hidden} / ${el("project-name").textContent} / ${el("project").className}`,
);

// ---------- a turn running in another thread ----------

console.log("reading another thread while a turn runs");
// A turn belongs to the thread it started in, and the reader may open another
// conversation while it works: the run keeps its own thread and folder, and the
// header says where it is rather than the transcript being swapped under it. The
// strip is the way back, the sidebar's row for that thread says it is the one
// working, and nothing of its reply lands in the transcript being read.
check(
  "shipped the header's strip inside the topbar, over the thread's own title",
  shellAt('id="thread-title"') < shellAt('id="run-banner"') &&
    shellAt('id="run-banner"') < shellAt('id="transcript"') &&
    /id="run-banner"[^>]*hidden/.test(shell) &&
    /id="run-banner-text"/.test(shell),
  shell.slice(shellAt('class="topbar"'), shellAt('id="transcript"')),
);
const runningThread = {
  id: "bb22cc33",
  name: "Rename oxide update",
  cwd: "/home/dev/Projects/oxide",
  created_at: 2,
  modified_at: Math.floor(Date.now() / 1000),
  message_count: 1,
  preview: "rename the update command",
};
// The thread the reader moves to: a stored one in the same folder, so opening it
// is the read-only review a run in another thread has to allow.
const readThread = existing[0];
app.state.projects = [{ name: "oxide", path: "/home/dev/Projects/oxide", registered: true }];
app.state.project = "/home/dev/Projects/oxide";
app.state.projectName = "oxide";
threads = [runningThread, readThread];
await app.loadSessions();
app.setIdle();
app.state.changes = [];
app.state.parked = null;
el("transcript").innerHTML = "";
// The turn starts in a thread of its own, from the message the reader typed
// there, and the reader opens another conversation while it works.
app.state.session = null;
app.state.sendView = { session: null };
app.setBusy();
await emit("agent-start", { runId: 62, sessionId: runningThread.id, title: "Rename oxide update" });
await app.openSession(readThread);
check(
  "said where the running turn is while another thread was on screen",
  app.state.session === readThread.id &&
    app.state.runSession === runningThread.id &&
    elementFor("run-banner").hidden === false &&
    elementFor("run-banner-text").textContent === "A turn is running in “Rename oxide update”" &&
    elementFor("run-banner").title === "Open “Rename oxide update”",
  `${app.state.session} / ${app.state.runSession} / ${elementFor("run-banner").hidden} / ${elementFor("run-banner-text").textContent}`,
);
check(
  "left the transcript on the thread the reader had opened",
  elementFor("thread-title").textContent === "Fix the flaky test",
  elementFor("thread-title").textContent,
);
// The sidebar says which row is the one still being written, wherever the reader
// is: the transcript may be another conversation's, so this is what names the run.
await app.renderProjectsTree();
const runningRow = () =>
  elementFor("projects-tree")
    .querySelectorAll(".session-item")
    .find((row) => String(row.className).includes("running"));
check(
  "marked the running thread's row while another thread was on screen",
  Boolean(runningRow()) &&
    Boolean(runningRow().querySelector(".spinner")) &&
    !String(runningRow().className).includes("active"),
  elementFor("projects-tree").outline(),
);
app.renderSessions();
check(
  "marked it in the project's own thread list too",
  elementFor("sessions-list")
    .querySelectorAll(".session-row")
    .some((row) => String(row.className).includes("running")),
  elementFor("sessions-list").outline(),
);
// A mark a stylesheet never styles is a row that says nothing, and the two
// listings put it in different places: in the tree's indent gutter, and on the
// line of the title it belongs to rather than at the row's far edge.
check(
  "styled the running-thread mark in both listings",
  /\.session-item \.session-run \{[^}]*position:\s*absolute/.test(sheet) &&
    /\.session-row \.session-run \{[^}]*align-self:\s*flex-start/.test(sheet) &&
    (sheet.match(/\.session-run \.spinner \{\s*border-color:\s*var\(--accent\)/g) || [])
      .length === 2,
  (sheet.match(/\.session-(?:item|row) \.session-run \{[^}]*\}/g) || []).join(" "),
);
// One conversation's reply is not painted into another's transcript, which is
// what being able to read one while the other runs depends on.
calls.length = 0;
app.handleEvent({
  type: "message_update",
  runId: 62,
  assistantMessageEvent: { type: "text_delta", delta: "half a reply" },
});
check(
  "painted nothing of the run's reply into the thread being read",
  el("transcript").outline() === "",
  el("transcript").outline(),
);
// The window's own readout of the run is not the thread's: how the turn is doing
// is said wherever the reader happens to be.
app.handleEvent({ type: "compaction", runId: 62, summarized: 12 });
check(
  "still said how the run was doing on the thread being read",
  status() === "Compacted 12 earlier messages",
  status(),
);
// The composer belongs to the thread on screen, so its corner offers Stop alone
// while the run is elsewhere, and a message typed here is refused instead of
// being steered into that run.
elementFor("prompt").value = "carry on";
app.updateSendState();
check(
  "offered Stop alone in a thread the turn is not running in",
  elementFor("send").hidden === true &&
    elementFor("stop").hidden === false &&
    elementFor("busy-message-mode").hidden === true,
  `send ${elementFor("send").hidden} / stop ${elementFor("stop").hidden} / mode ${elementFor("busy-message-mode").hidden}`,
);
calls.length = 0;
elementFor("send").onclick({ detail: 1 });
await nextTick();
check(
  "refused to steer a message typed into another thread's composer",
  !calls.some(([name]) => name === "steer_run" || name === "send_prompt") &&
    /^A turn is running in “Rename oxide update”/.test(status()) &&
    elementFor("prompt").value === "carry on",
  `${JSON.stringify(calls.map(([name]) => name))} / ${status()}`,
);
// The header's strip is the way back: it opens the thread the run is in and hands
// back the transcript it has been painting into all along, rather than reading a
// store that is a step behind a reply still streaming into it.
calls.length = 0;
el("run-banner").click();
await nextTick();
check(
  "opened the running thread from the header's own strip",
  app.state.session === runningThread.id &&
    !calls.some(([name]) => name === "session_messages") &&
    /half a reply/.test(el("transcript").outline()) &&
    elementFor("run-banner").hidden === true,
  `${app.state.session} / ${JSON.stringify(calls.map(([name]) => name))} / ${el("transcript").outline()}`,
);
check(
  "handed the composer back to the thread the turn is in",
  elementFor("send").hidden === false && elementFor("stop").hidden === true,
  `send ${elementFor("send").hidden} / stop ${elementFor("stop").hidden}`,
);
calls.length = 0;
app.handleEvent({
  type: "message_update",
  runId: 62,
  assistantMessageEvent: { type: "text_delta", delta: "the rest of the reply" },
});
check(
  "painted the run's own output once the reader was back in its thread",
  /half a reply/.test(el("transcript").outline()) &&
    /the rest of the reply/.test(el("transcript").outline()),
  el("transcript").outline(),
);
// The run goes on counting its turn while the reader is elsewhere, and what it
// counts belongs to the thread it is in: the footer under another conversation
// keeps that conversation's own totals, and the run's wait with its thread.
await app.openSession(readThread);
app.handleEvent({
  type: "usage",
  runId: 62,
  usage: { input: 4000, output: 120, cacheRead: 0, cacheWrite: 0, cost: 0.02 },
});
check(
  "left the footer on the thread being read while the run counted its own",
  app.state.usage?.input !== 4000 &&
    app.state.parked?.session === runningThread.id &&
    app.state.parked.usage?.input === 4000,
  `${app.state.usage?.input} / ${app.state.parked?.usage?.input}`,
);
// A visit to a third thread mid-run drops the pointers into the transcript on
// screen, which is not the run's own: the bubble its next delta extends is the
// one in its own transcript, so the reply stays a single bubble there.
await app.openSession(existing[1]);
el("run-banner").click();
await nextTick();
app.handleEvent({
  type: "message_update",
  runId: 62,
  assistantMessageEvent: { type: "text_delta", delta: " and no more" },
});
check(
  "kept the run's reply in the one bubble it was streaming into",
  el("transcript").querySelectorAll(".assistant").length === 1 &&
    (el("transcript").outline().match(/half a reply/g) || []).length === 1 &&
    /the rest of the reply and no more/.test(el("transcript").outline()),
  `${el("transcript").querySelectorAll(".assistant").length} / ${el("transcript").outline()}`,
);
check(
  "took the run's own totals back with its thread",
  app.state.usage?.input === 4000,
  String(app.state.usage?.input),
);
// The bubble names its speaker with the app's own mark beside the name, the same
// drawing the home state opens with, rather than a `◆` typed into a label.
const speaker = el("transcript").querySelectorAll(".assistant")[0].children[0];
check(
  "signed the reply with the app's own mark rather than a character",
  speaker?.children[0]?.innerHTML === app.ICONS.mark && speaker.children[1]?.textContent === "Oxide",
  `${speaker?.children[0]?.innerHTML} / ${speaker?.children[1]?.textContent}`,
);
// A turn that ends while the reader is elsewhere really did change those files,
// so its card lands in the transcript the run has been painting into — kept for
// the thread that changed them rather than dropped or shown under another
// conversation's reply — and the strip goes on naming that thread, now as one
// whose turn has finished.
await app.openSession(readThread);
el("transcript").innerHTML = "";
await emit("agent-end", {
  runId: 62,
  sessionId: runningThread.id,
  project: "/home/dev/Projects/oxide",
  after: "turn-62",
  changes: {
    files: [{ path: "src/turn.rs", status: "modified", added: 4, removed: 1 }],
    added: 4,
    removed: 1,
  },
});
check(
  "kept a finished turn's card for the thread it was not shown on",
  app.state.parked?.session === runningThread.id &&
    el("transcript").outline() === "" &&
    /src\/turn\.rs/.test(app.state.parked.node.outline()),
  `${app.state.parked?.session} / ${el("transcript").outline()} / ${app.state.parked?.node.outline()}`,
);
check(
  "said a finished turn was waiting in the thread it belonged to",
  elementFor("run-banner").hidden === false &&
    elementFor("run-banner-text").textContent === "A turn finished in “Rename oxide update”",
  `${elementFor("run-banner").hidden} / ${elementFor("run-banner-text").textContent}`,
);
await app.openSession(runningThread);
await nextTick();
check(
  "painted that card when the thread it belongs to was opened",
  app.state.parked === null &&
    /src\/turn\.rs/.test(el("transcript").outline()) &&
    /half a reply/.test(el("transcript").outline()),
  `${app.state.parked} / ${el("transcript").outline()}`,
);
// The thread a turn is writing to is not one the ✕ may take away, even while the
// reader is looking at another conversation: the run appends to that file as it
// works, which is why the delete is refused wherever the reader is.
app.state.changes = [];
app.state.session = readThread.id;
app.state.runSession = runningThread.id;
app.setBusy();
app.state.runId = 63;
calls.length = 0;
const removal = app.removeSession(runningThread);
await nextTick();
check(
  "refused to delete the thread the turn is running in from another thread",
  !calls.some(([name]) => name === "delete_session") &&
    /stop it before deleting/.test(status()) &&
    app.state.session === readThread.id &&
    elementFor("confirm-modal").hidden === true,
  `${JSON.stringify(calls.map(([name]) => name))} / ${status()} / ${elementFor("confirm-modal").hidden}`,
);
// A run that did not refuse would have opened the dialog this answers, so the
// thread is put back and the run goes on.
if (elementFor("confirm-modal").hidden === false) elementFor("confirm-cancel").onclick();
await removal;
app.setIdle();
// A run that starts while the reader is moving: the thread it reports is the one
// the message that started it was composed in, so a reader who has opened another
// conversation in the meantime keeps the one they opened — the run is in a thread
// of its own, and the strip says so.
app.state.changes = [];
app.state.session = null;
app.state.sendView = { session: null };
app.setBusy();
await app.openSession(readThread);
await emit("agent-start", { runId: 63, sessionId: "cc33dd44", title: "A thread of its own" });
check(
  "left the reader in the thread they had opened when a run started",
  app.state.session === readThread.id &&
    app.state.runSession === "cc33dd44" &&
    elementFor("run-banner-text").textContent === "A turn is running in “A thread of its own”",
  `${app.state.session} / ${app.state.runSession} / ${elementFor("run-banner-text").textContent}`,
);
// The row standing in for that thread is a way back to it, the same as the
// header's strip: the store has not written it, so the window switches to it
// itself instead of reading a file that is not there — and what the run painted
// while the reader was here is already in the transcript it is handed back.
app.handleEvent({
  type: "message_update",
  runId: 63,
  assistantMessageEvent: { type: "text_delta", delta: "a first step" },
});
check(
  "painted none of a run started elsewhere into the thread being read",
  el("transcript").outline() === "",
  el("transcript").outline(),
);
calls.length = 0;
await app.renderProjectsTree();
const heldRow = elementFor("projects-tree")
  .querySelectorAll(".session-item")
  .find((row) => String(row.title).includes("a turn is running"));
heldRow.click();
await nextTick();
check(
  "opened the running thread from its own row in the tree",
  app.state.session === "cc33dd44" &&
    calls.every(([name]) => name !== "session_messages") &&
    /a first step/.test(el("transcript").outline()) &&
    elementFor("thread-title").textContent === "A thread of its own" &&
    elementFor("run-banner").hidden === true,
  `${app.state.session} / ${JSON.stringify(calls.map(([name]) => name))} / ${el("transcript").outline()} / ${elementFor("thread-title").textContent}`,
);
// The turn is over, so nothing is running anywhere: the strip goes away with it.
app.setIdle();
check(
  "put the strip away when the running turn ended",
  elementFor("run-banner").hidden === true && app.state.runSession === null,
  `${elementFor("run-banner").hidden} / ${app.state.runSession}`,
);
// A parked thread the ✕ takes away takes its transcript with it: the store has no
// file behind it any more, so the strip that was the way back has nothing to
// open and goes with it.
app.state.session = runningThread.id;
app.state.sendView = { session: readThread.id };
app.state.runProject = "/home/dev/Projects/oxide";
await emit("agent-start", { runId: 72, sessionId: runningThread.id, title: "Rename oxide update" });
app.state.sendView = null;
await app.openSession(readThread);
// The turn ends while the reader is elsewhere, which is what leaves the thread
// parked with a finished turn rather than a running one.
app.setIdle();
// A finished turn's thread is still one this window holds, so its rows go on
// naming it by the title the turn gave it — the same name the strip gives it —
// rather than falling back to its id. The store's own answer is narrowed to the
// thread on screen, which is the moment that title is the only one such a thread
// has.
threads = [readThread];
await app.loadSessions();
await app.renderSessions();
const parkedTree = elementFor("projects-tree").outline();
check(
  "named a finished turn's parked thread by the turn's own title",
  parkedTree.includes("Rename oxide update") &&
    !parkedTree.includes("bb22cc33") &&
    elementFor("sessions-list").outline().includes("Rename oxide update"),
  `${parkedTree} / ${elementFor("sessions-list").outline()}`.split("\n")[0],
);
calls.length = 0;
const parkedRemoval = app.removeSession(runningThread);
await nextTick();
elementFor("confirm-ok").onclick();
await parkedRemoval;
check(
  "dropped a parked transcript when its thread was deleted",
  calls.some(([name]) => name === "delete_session") &&
    app.state.parked === null &&
    elementFor("run-banner").hidden === true,
  `${JSON.stringify(calls.map(([name]) => name))} / ${app.state.parked} / ${elementFor("run-banner").hidden}`,
);
app.state.parked = null;
app.state.changes = [];
threads = existing;
el("transcript").innerHTML = "";
app.state.session = null;

console.log("a turn that has not reported itself yet");
// The message a turn is started from is on screen before the process tells the
// window which thread it created, and the reader may open another conversation
// inside that window. The thread they left is the run's own owner of what it has
// painted whether or not it has a name yet, so it is parked with the run rather
// than dropped when the transcript is rebuilt under it — and the composer is no
// longer that thread's, since a message typed there would be steered into a run
// whose reply belongs to a conversation that is not on screen.
app.state.projects = [
  { name: "oxide", path: "/home/dev/Projects/oxide", registered: true },
  { name: "elsewhere", path: "/tmp/elsewhere", registered: true },
];
app.state.project = "/home/dev/Projects/oxide";
app.state.projectName = "oxide";
app.state.contextWindow = 128000;
threads = existing;
app.setIdle();
app.state.parked = null;
app.state.changes = [];
app.state.session = readThread.id;
el("transcript").innerHTML = "";
elementFor("prompt").value = "keep going";
calls.length = 0;
await app.send(false);
check(
  "held the thread and folder a message was sent from while the turn starts",
  app.state.sendView?.session === readThread.id &&
    app.state.sendView?.project === "/home/dev/Projects/oxide" &&
    app.state.runSession === null,
  `${JSON.stringify(app.state.sendView)} / ${app.state.runSession}`,
);
// A stored thread in the same folder: leaving the starting turn's thread for it
// is the read-only review, and the bubble the reader just typed is the run's.
await app.openSession(existing[1]);
check(
  "parked the thread a message had just been sent from",
  app.state.parked?.session === readThread.id &&
    app.state.parked?.pending === true &&
    /keep going/.test(app.state.parked.node.outline()) &&
    el("transcript").outline() === "",
  `${app.state.parked?.session} / ${app.state.parked?.pending} / ${app.state.parked?.node.outline()}`,
);
elementFor("prompt").value = "carry on";
app.updateSendState();
check(
  "offered Stop alone while the turn had not named the thread it is in",
  elementFor("send").hidden === true &&
    elementFor("stop").hidden === false &&
    elementFor("busy-message-mode").hidden === true,
  `send ${elementFor("send").hidden} / stop ${elementFor("stop").hidden} / mode ${elementFor("busy-message-mode").hidden}`,
);
calls.length = 0;
elementFor("send").onclick({ detail: 1 });
await nextTick();
check(
  "refused a message typed before the turn had named its thread",
  !calls.some(([name]) => name === "steer_run" || name === "send_prompt") &&
    status() === "A turn is running in “Fix the flaky test”; open it to queue or steer, or stop it." &&
    elementFor("prompt").value === "carry on",
  `${JSON.stringify(calls.map(([name]) => name))} / ${status()}`,
);
await emit("agent-start", { runId: 62, sessionId: readThread.id, title: "Fix the flaky test" });
check(
  "keyed the park to the thread the turn reported",
  app.state.parked?.session === readThread.id &&
    app.state.parked?.pending === false &&
    app.state.session === existing[1].id &&
    app.state.runSession === readThread.id,
  `${app.state.parked?.session} / ${app.state.parked?.pending} / ${app.state.session} / ${app.state.runSession}`,
);
app.handleEvent({
  type: "message_update",
  runId: 62,
  assistantMessageEvent: { type: "text_delta", delta: "still going" },
});
check(
  "painted the reply into the thread the message was sent from",
  /keep going/.test(app.state.parked.node.outline()) &&
    /still going/.test(app.state.parked.node.outline()) &&
    el("transcript").outline() === "",
  `${app.state.parked.node.outline()} / ${el("transcript").outline()}`,
);
elementFor("prompt").value = "";
calls.length = 0;
el("run-banner").click();
await nextTick();
check(
  "came back to the bubble it kept rather than the store's copy of the thread",
  app.state.session === readThread.id &&
    !calls.some(([name]) => name === "session_messages") &&
    /keep going/.test(el("transcript").outline()) &&
    /still going/.test(el("transcript").outline()),
  `${app.state.session} / ${JSON.stringify(calls.map(([name]) => name))} / ${el("transcript").outline()}`,
);
// A message that starts a thread of its own has no id for the park to be keyed
// by, so the park waits for the one the run reports and is keyed by it then:
// what the reader left is the way back to a thread the store has not written.
app.setIdle();
app.state.parked = null;
app.state.session = null;
el("transcript").innerHTML = "";
elementFor("prompt").value = "start one";
await app.send(false);
await app.openSession(readThread);
await emit("agent-start", { runId: 63, sessionId: "cc33dd44", title: "A thread of its own" });
check(
  "keyed a park made before the run had an id to the id it created",
  app.state.parked?.session === "cc33dd44" &&
    app.state.parked?.pending === false &&
    app.state.parked?.project === "/home/dev/Projects/oxide" &&
    /start one/.test(app.state.parked.node.outline()) &&
    app.state.session === readThread.id,
  `${app.state.parked?.session} / ${app.state.parked?.pending} / ${app.state.parked?.project} / ${app.state.parked?.node.outline()} / ${app.state.session}`,
);
// A new thread of one folder and a new thread of another both have none, so the
// folder is what tells them apart: a reader who moved to the other project before
// the turn reported itself is not looking at the thread it created, and the strip
// takes them to the one it is in rather than the one being read.
app.setIdle();
app.state.parked = null;
app.state.session = null;
app.state.project = "/home/dev/Projects/oxide";
el("transcript").innerHTML = "";
elementFor("prompt").value = "start one here";
await app.send(false);
await app.selectProject(app.state.projects[1]);
check(
  "moved to the other folder without taking the starting turn's thread",
  app.state.project === "/tmp/elsewhere" && app.state.session === null,
  `${app.state.project} / ${app.state.session}`,
);
await emit("agent-start", { runId: 64, sessionId: "dd44ee55", title: "A thread of its own" });
check(
  "did not adopt the thread a run created in the folder the reader left",
  app.state.session === null &&
    app.state.runSession === "dd44ee55" &&
    elementFor("run-banner").hidden === false &&
    elementFor("run-banner-text").textContent === "A turn is running in “A thread of its own”",
  `${app.state.session} / ${app.state.runSession} / ${elementFor("run-banner").hidden} / ${elementFor("run-banner-text").textContent}`,
);
// A run counts its own tokens while the reader is in another folder, and what the
// footer draws is a fraction of a window. The fraction is taken where the totals
// are drawn — the folder they belong to — so the park carries the raw prompt
// rather than a percentage of whichever window was open when the event arrived.
app.state.contextWindow = 32000;
app.handleEvent({
  type: "usage",
  runId: 64,
  usage: { input: 30000, output: 40, cacheRead: 0, cacheWrite: 0, cost: 0.01 },
});
check(
  "left the run's tokens waiting for its own thread and its own window",
  app.state.parked?.usage?.prompt === 30000 &&
    app.state.parked?.usage?.contextPct == null &&
    app.state.usage?.input !== 30000,
  `${JSON.stringify(app.state.parked?.usage)} / ${JSON.stringify(app.state.usage)}`,
);
// The strip takes the reader back to the run's own folder, whose window arrives
// from that folder's own `project_info` — which the stub answers here the way a
// real one does — so the parked totals are drawn against the window they belong
// to rather than against the one the folder being read had.
projectInfo = projectFacts(128000);
calls.length = 0;
el("run-banner").click();
await nextTick();
await nextTick();
check(
  "opened the run's own folder and drew its tokens against that window",
  app.state.project === "/home/dev/Projects/oxide" &&
    app.state.session === "dd44ee55" &&
    /start one here/.test(el("transcript").outline()) &&
    el("usage").textContent === "↑ 30000 ↓ 40 · $0.0100" &&
    el("context-percent").textContent === "23%",
  `${app.state.project} / ${app.state.session} / ${el("transcript").outline()} / ${el("usage").textContent} / ${el("context-percent").textContent}`,
);
app.setIdle();
app.state.parked = null;
app.state.session = null;
projectInfo = null;
threads = existing;
el("transcript").innerHTML = "";

console.log("opening the thread a finished turn is parked in");
// The header names that thread by the turn's own title too, which is what the
// strip and the sidebar row call it: the store has not written it, so the title
// the window held for it is the only name it has.
app.state.projects = [{ name: "oxide", path: "/home/dev/Projects/oxide", registered: true }];
app.state.project = "/home/dev/Projects/oxide";
app.state.projectName = "oxide";
app.state.session = readThread.id;
app.state.sendView = { session: readThread.id };
app.state.runProject = "/home/dev/Projects/oxide";
await emit("agent-start", { runId: 71, sessionId: runningThread.id, title: "Rename oxide update" });
await app.openSession(existing[1]);
await app.loadSessions();
app.setIdle();
app.state.sendView = null;
calls.length = 0;
el("run-banner").click();
await nextTick();
check(
  "named the thread the strip opened by the turn's own title",
  app.state.session === runningThread.id &&
    elementFor("thread-title").textContent === "Rename oxide update" &&
    app.state.parked === null &&
    !calls.some(([name]) => name === "session_messages"),
  `${app.state.session} / ${elementFor("thread-title").textContent} / ${JSON.stringify(calls.map(([name]) => name))}`,
);
app.state.changes = [];
el("transcript").innerHTML = "";
app.state.session = null;

console.log("a thread that is not stored yet");
// `all_sessions` lists what is on disk, and a thread that was just started has
// not written its first entry: every listing — the sidebar's tree, its count for
// the project and the sessions list — stands it in until the store catches up.
app.setIdle();
app.state.projects = [{ name: "oxide", path: "/tmp/oxide", registered: false }];
app.state.project = "/tmp/oxide";
app.state.projectName = "oxide";
threads = [
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
// The run's own thread and folder, held apart from the ones on screen — the two
// things `startPrompt` records before the turn reports itself.
app.state.sendView = { session: "6f3031b2beef" };
app.state.runProject = "/tmp/oxide";
// The turn that started this thread named it, which is where the window gets a
// title for a thread the store has not written yet.
await emit("agent-start", { runId: 70, sessionId: "6f3031b2beef", title: "Fix the sidebar" });
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
app.setIdle();
app.state.sendView = null;
app.state.session = null;
app.state.parked = null;
threads = existing;

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
// artifact this machine installs — never the CLI's release list. One dialog
// holds the three steps of one install, and what a reader decides on is what the
// release changed, so the release's own notes are in it rather than on a page in
// a browser.
console.log("check for updates");
app.state.project = "/home/dev/Projects/oxide";
const updateTitle = () => String(elementFor("update-title").textContent);
const updateBody = () => String(elementFor("update-body").innerHTML);
const updateNote = () => String(elementFor("update-note").textContent);
const changelog = () => elementFor("update-changelog");
const changelogHtml = () => String(elementFor("update-changelog").innerHTML);
const installButton = elementFor("update-install");
const restartButton = elementFor("update-restart");
const closeButton = elementFor("update-close");
const laterButton = elementFor("update-later");
const cancelButton = elementFor("update-cancel");
const backgroundButton = elementFor("update-background");
const pageButton = elementFor("update-page");
const progressBox = elementFor("update-progress");
const progressFill = elementFor("update-progress-fill");
const updateOpen = () => elementFor("update-modal").hidden === false;
const offered = (button) => button.hidden === false;

calls.length = 0;
elementFor("update").onclick(press({ target: elementFor("update") }));
check(
  "asked the app's own update check",
  projectCalls("check_updates").length === 1,
  JSON.stringify(calls.map(([name]) => name)),
);
check("opened the dialog from the sidebar's own button", updateOpen(), String(elementFor("update-modal").hidden));
await nextTick();
check(
  "named the release the app's own train resolved",
  updateTitle() === "Update Available (v0.34.0)",
  updateTitle(),
);
check(
  "said what installing does and what picks the new version up",
  /this app's place/.test(updateNote()) && /restarted/.test(updateNote()),
  updateNote(),
);
check(
  "showed what this machine has and what the release is",
  updateBody().includes("Current <code>0.33.0</code>") &&
    updateBody().includes("Latest <code>0.34.0</code>") &&
    !updateBody().includes("desktop-v0.34.0"),
  updateBody(),
);
check(
  "showed the installation the app found itself and its path",
  updateBody().includes("app bundle") && updateBody().includes("/Applications/Oxide.app"),
  updateBody(),
);
check(
  "named the artifact this machine would download",
  updateBody().includes("macos-arm64-Oxide.dmg"),
  updateBody(),
);
check(
  "offered the release its own train resolved",
  offered(installButton) && installButton.textContent === "Update",
  `${installButton.hidden} / ${installButton.textContent}`,
);
// What the release changed is the thing a reader decides on, and the release job
// already wrote it as Markdown: the dialog draws it under the version and the day
// it went out rather than sending them to the release page for it.
check(
  "drew the release's own notes under the version and its date",
  offered(changelog()) &&
    changelogHtml().includes("Oxide 0.34.0") &&
    changelogHtml().includes("2026-10-06") &&
    changelogHtml().includes("<h2>What&#39;s changed</h2>") &&
    changelogHtml().includes("<li>") &&
    changelogHtml().includes("<code>oxide context</code>"),
  changelogHtml().slice(0, 200),
);
check(
  "made a link in the notes one the window's own click handling opens",
  /<a href="https:\/\/github.com\/jaysonwu991\/oxide\/pull\/174"[^>]*>#174<\/a>/.test(changelogHtml()),
  changelogHtml(),
);
check(
  "offered no release page for a release it can install and has read",
  offered(pageButton) === false,
  `${pageButton.hidden} / ${changelogHtml().slice(0, 80)}`,
);
check(
  "offered the release that is out and one way to leave it",
  closeButton.textContent === "Dismiss" && !offered(restartButton) && !offered(cancelButton),
  `${closeButton.textContent} / ${restartButton.hidden} / ${cancelButton.hidden}`,
);

// Installing is the app's own: it downloads the artifact its release train
// publishes for this machine, verifies it against the release's digest, and
// replaces this copy in place. The dialog turns into the download it started,
// with the progress bar a release of this size needs.
console.log("the download the dialog started");
calls.length = 0;
holdNextInstall();
const installing = installButton.onclick();
check(
  "turned the dialog into the download it started",
  updateTitle() === "Downloading update" && offered(cancelButton) && offered(backgroundButton),
  `${updateTitle()} / ${cancelButton.hidden} / ${backgroundButton.hidden}`,
);
check(
  "stopped offering the install it is already running",
  !offered(installButton) && !offered(closeButton),
  `${installButton.hidden} / ${closeButton.hidden}`,
);
// The step is one step to the reader, but the engine's own stage is what the
// line under it says: resolving the release and checking its checksum are not a
// download, and saying they are is a sentence the reader can watch be wrong.
check(
  "said what the engine is doing rather than calling every stage a download",
  /Oxide 0.34.0 is being looked up/.test(updateNote()) && /restarted/.test(updateNote()),
  updateNote(),
);
await emit("update-progress", {
  stage: "downloading",
  version: "0.34.0",
  received: 12 * 1024 * 1024,
  total: 48 * 1024 * 1024,
});
check(
  "followed the stage the download reached",
  /Oxide 0.34.0 is being downloaded and put in place/.test(updateNote()),
  updateNote(),
);
check(
  "filled the bar from the stage the app reported",
  offered(progressBox) &&
    progressBox.classList.contains("indeterminate") === false &&
    progressFill.style.width === "25%",
  `${progressBox.hidden} / ${progressFill.style.width}`,
);
check(
  "said how far the download has got",
  updateBody().includes("macos-arm64-Oxide.dmg") &&
    updateBody().includes("12.0 MB of 48.0 MB") &&
    updateBody().includes("25%"),
  updateBody(),
);
check(
  "ran the app's own install",
  projectCalls("install_update").length === 1,
  JSON.stringify(calls.map(([name]) => name)),
);
// A release whose size nobody announced has no fraction to draw, so the bar
// works and the line carries the bytes instead of an invented percentage.
await emit("update-progress", {
  stage: "downloading",
  version: "0.34.0",
  received: 3 * 1024 * 1024,
  total: null,
});
check(
  "showed a download of an unknown size working rather than a fraction of it",
  progressBox.classList.contains("indeterminate") === true &&
    updateBody().includes("3.0 MB downloaded") &&
    !updateBody().includes("%"),
  `${progressBox.classList.contains("indeterminate")} / ${updateBody()}`,
);
// The bar and the bytes are the transfer's. The stages on either side of it
// carry none, and painting their zeroes would replace a bar that had filled with
// one that has just started, over the words "0.0 MB downloaded" — a number going
// backwards in front of the reader.
await emit("update-progress", { stage: "verifying", version: "0.34.0" });
check(
  "took the finished bar away rather than starting it over",
  progressBox.hidden === true &&
    !updateBody().includes("0.0 MB") &&
    updateBody().includes("is being verified"),
  `${progressBox.hidden} / ${updateBody()}`,
);
check(
  "kept the cancel while the release can still be let go of",
  offered(cancelButton) && offered(backgroundButton),
  `${cancelButton.hidden} / ${backgroundButton.hidden}`,
);
// Once the release is being put in place the swap is under way, and an install
// stopped in the middle of it is an installation lost: the dialog stops offering
// to stop it rather than closing on an install that goes on without the reader.
await emit("update-progress", { stage: "installing", version: "0.34.0" });
check(
  "stopped offering to stop an install that can no longer be stopped",
  !offered(cancelButton) && offered(backgroundButton) && progressBox.hidden === true,
  `${cancelButton.hidden} / ${backgroundButton.hidden} / ${progressBox.hidden}`,
);
check(
  "named the step it had reached rather than the one it had finished",
  updateTitle() === "Installing update" && updateBody().includes("is being put in place"),
  `${updateTitle()} / ${updateBody()}`,
);

// Sending the download to the background is putting the dialog away rather than
// stopping the install: the reader is told where it got to when it lands, and
// nothing is asked of the app.
calls.length = 0;
backgroundButton.onclick();
check(
  "put the dialog away without stopping the install",
  elementFor("update-modal").hidden === true &&
    projectCalls("cancel_update").length === 0 &&
    projectCalls("install_update").length === 0,
  JSON.stringify(calls.map(([name]) => name)),
);
await emit("update-progress", {
  stage: "downloading",
  version: "0.34.0",
  received: 20 * 1024 * 1024,
  total: 48 * 1024 * 1024,
});
check(
  "left the dialog the reader put away away, whatever the install reports next",
  elementFor("update-modal").hidden === true,
  String(elementFor("update-modal").hidden),
);

// The release is on disk and the process is not, so what the dialog asks for is
// the restart that runs it — which is the step a download that finished in the
// background comes back as, since the window keeps no row of its own for it.
releaseHeldInstalls();
await installing;
check(
  "came back as the restart the install left to do",
  updateOpen() && updateTitle() === "Restart and install update",
  `${elementFor("update-modal").hidden} / ${updateTitle()}`,
);
check(
  "said what is left to do about it",
  updateNote() === "Update downloaded. You need to restart Oxide to install the update.",
  updateNote(),
);
check(
  "offered the restart beside the way to put it off",
  offered(restartButton) && offered(laterButton) && !offered(installButton) && !offered(cancelButton),
  `${restartButton.hidden} / ${laterButton.hidden} / ${installButton.hidden}`,
);
check(
  "reported the release the install put in place, by version and by file",
  updateBody().includes("Release <code>0.34.0</code>") &&
    updateBody().includes("macos-arm64-Oxide.dmg") &&
    updateBody().includes("/Applications/Oxide.app"),
  updateBody(),
);
check(
  "took the notes away with the decision they were for",
  offered(changelog()) === false && offered(progressBox) === false,
  `${changelog().hidden} / ${progressBox.hidden}`,
);
calls.length = 0;
await restartButton.onclick();
check(
  "restarted the app through the app's own command",
  projectCalls("restart_app").length === 1,
  JSON.stringify(calls.map(([name]) => name)),
);
// A turn owns the process — its tools write files and its stream is read here —
// so a restart is refused while one runs rather than killing a live run.
app.state.busy = true;
elementFor("status-text").textContent = "";
calls.length = 0;
await restartButton.onclick();
check(
  "refused a restart while a turn was running",
  projectCalls("restart_app").length === 0 && /A turn is running/.test(status()),
  `${JSON.stringify(calls.map(([name]) => name))} / ${status()}`,
);
app.state.busy = false;
// Putting it off is putting the dialog away, not forgetting the install: the
// next check reports what is left to do rather than offering it again, since the
// build running here is still the one that started.
laterButton.onclick();
check("put the restart away without forgetting it", elementFor("update-modal").hidden === true);
calls.length = 0;
await app.openUpdate();
check(
  "answered a later check with the install rather than the release",
  updateTitle() === "Restart and install update" &&
    projectCalls("check_updates").length === 1 &&
    !offered(installButton),
  `${updateTitle()} / ${JSON.stringify(calls.map(([name]) => name))}`,
);
app.installedUpdate.answer = null;
closeButton.onclick();
check("closed it from its own button", elementFor("update-modal").hidden === true);

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
  !offered(installButton) && !offered(pageButton),
  `${installButton.hidden} / ${pageButton.hidden}`,
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
check("offered no update for an app that has none", !offered(installButton));
check("offered no advice it does not need", !updateBody().includes("update-advice"), updateBody());

// A copy the app may not write over — a checkout's build, a copy an
// administrator installed — gets the advice the app composed instead of an
// Update button, and the release page it is installed from.
updateAnswer = {
  ...OFFERED_UPDATE,
  installation: "source build",
  path: "",
  installable: false,
  advice:
    "Download macos-arm64-Oxide.dmg from the release page and install it the way this copy was installed.",
};
await app.openUpdate();
check(
  "let the app decide an installation it cannot replace",
  !offered(installButton) && updateBody().includes("update-advice"),
  `${installButton.hidden} / ${updateBody()}`,
);
check(
  "named the download to use instead",
  updateBody().includes("macos-arm64-Oxide.dmg") && updateBody().includes("source build"),
  updateBody(),
);
check("left the release page reachable for a copy that needs it", offered(pageButton));
calls.length = 0;
await pageButton.onclick();
check(
  "opened that release in the browser through the app",
  projectCalls("open_url")[0]?.[1]?.url === updateAnswer.releaseUrl &&
    updateAnswer.releaseUrl.includes("desktop-v0.34.0"),
  JSON.stringify(projectCalls("open_url")),
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
check("left the release page reachable", offered(pageButton));
check(
  "offered the install again rather than a half-installed state",
  !offered(restartButton) && !offered(laterButton) && offered(closeButton),
  `${restartButton.hidden} / ${closeButton.hidden}`,
);
installError = null;

// The macOS menu item has no page of its own, so it asks the window for the
// dialog its button opens.
elementFor("update-modal").hidden = true;
calls.length = 0;
await emit("check-updates", {});
await nextTick();
check(
  "opened the same dialog from the menu item's own event",
  updateOpen() && projectCalls("check_updates").length === 1,
  JSON.stringify(calls.map(([name]) => name)),
);
// Escape closes every overlay, which is where the dialog's own id has to be.
document.fire("keydown", { key: "Escape" });
check("closed it on Escape", elementFor("update-modal").hidden === true);

// A second press must not start a second install over the same installation: the
// one already running is the one the click adopts, which is the download in
// front of the reader rather than a check about a build being replaced.
console.log("an install and the check that overlaps it");
updateAnswer = OFFERED_UPDATE;
app.installedUpdate.answer = null;
await app.openUpdate();
await nextTick();
check("offered the release before installing it", updateTitle() === "Update Available (v0.34.0)", updateTitle());
holdNextInstall();
const firstInstall = app.installUpdate();
await nextTick();
const afterFirst = projectCalls("install_update").length;
const secondInstall = app.installUpdate();
check(
  "adopted the install already running rather than starting a second",
  afterFirst === 1 &&
    projectCalls("install_update").length === 1 &&
    updateTitle() === "Downloading update" &&
    offered(backgroundButton),
  `${afterFirst} / ${projectCalls("install_update").length} / ${updateTitle()}`,
);
// A dialog asked for while an install works is about the build that started —
// the release it would resolve is the one being put in place — so what it paints
// is the install rather than a check nobody is waiting on.
calls.length = 0;
await app.openUpdate();
check(
  "answered the sidebar's own button with the install it is running",
  projectCalls("check_updates").length === 0 && updateTitle() === "Downloading update",
  `${JSON.stringify(calls.map(([name]) => name))} / ${updateTitle()}`,
);
releaseHeldInstalls();
await Promise.all([firstInstall, secondInstall]);
check(
  "reported the one install that ran",
  updateTitle() === "Restart and install update" && projectCalls("install_update").length === 0,
  `${updateTitle()} / ${JSON.stringify(calls.map(([name]) => name))}`,
);
elementFor("update-modal").hidden = true;
app.installedUpdate.answer = null;

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
  updateTitle() === "Oxide 0.34.0 is up to date" && !offered(installButton),
  `${updateTitle()} / ${installButton.hidden}`,
);

// The app resolves the release again as it installs, so a release published
// between the check and the click is the one that lands. What the dialog reports
// is the version the install itself resolved, not the one it offered.
installedVersion = "0.35.0";
updateAnswer = OFFERED_UPDATE;
await app.openUpdate();
await nextTick();
check("offered the release the check resolved", updateTitle() === "Update Available (v0.34.0)", updateTitle());
await installButton.onclick();
check(
  "reported the version that landed rather than the one it offered",
  updateBody().includes("Release <code>0.35.0</code>"),
  updateBody(),
);
installedVersion = null;
app.installedUpdate.answer = null;

// Cancel is the reader stopping the download: the app is told to stop, the
// dialog goes away with it, and nothing is reported as having gone wrong —
// since nothing did.
console.log("cancelling a download");
updateAnswer = OFFERED_UPDATE;
await app.openUpdate();
await nextTick();
holdNextInstall();
const cancelled = app.installUpdate();
await nextTick();
check(
  "turned the dialog into the download it started",
  updateTitle() === "Downloading update",
  updateTitle(),
);
calls.length = 0;
elementFor("status-text").textContent = "";
cancelButton.onclick();
check(
  "asked the app to stop the download",
  projectCalls("cancel_update").length === 1,
  JSON.stringify(calls.map(([name]) => name)),
);
// A cancel names the install it is about rather than "whatever is running": the
// window offers Cancel the moment it has asked for an install, so that request
// can reach the engine before the install it belongs to has been polled — and a
// name the engine can match is what keeps it from being lost to that ordering.
check(
  "named the install it is cancelling",
  installTokens[installTokens.length - 1] === cancelledTokens[cancelledTokens.length - 1] &&
    String(cancelledTokens[cancelledTokens.length - 1]).startsWith("ui-"),
  `${JSON.stringify(installTokens)} / ${JSON.stringify(cancelledTokens)}`,
);
check(
  "closed the dialog the reader stopped it in",
  elementFor("update-modal").hidden === true,
  String(elementFor("update-modal").hidden),
);
releaseHeldInstalls();
await cancelled;
await emit("update-failed", { cancelled: true });
await nextTick();
check(
  "reported no failure for a download the reader stopped",
  elementFor("update-modal").hidden === true && status() === "",
  `${elementFor("update-modal").hidden} / ${status()}`,
);
// A release that is still out is offered again by the next check, since nothing
// was put in place.
calls.length = 0;
await app.openUpdate();
check(
  "offered the release again after a cancel",
  updateTitle() === "Update Available (v0.34.0)" && offered(installButton),
  `${updateTitle()} / ${installButton.hidden}`,
);
closeButton.onclick();

// An install can go wrong once it is under way, and the engine says so through
// the dialog the reader is watching it in: the step it was on is not left
// standing over an install that has ended, and the failure is not dropped for
// having arrived after the install reported a stage of its own.
console.log("an install that goes wrong under the dialog");
updateAnswer = OFFERED_UPDATE;
await app.openUpdate();
await nextTick();
installError = "mounting the downloaded disk image: hdiutil failed with exit status: 1";
holdNextInstall();
const failing = installButton.onclick();
await nextTick();
await emit("update-progress", {
  stage: "downloading",
  version: "0.34.0",
  received: 4 * 1024 * 1024,
  total: 8 * 1024 * 1024,
});
check(
  "showed the download before it went wrong",
  updateTitle() === "Downloading update" && progressFill.style.width === "50%",
  `${updateTitle()} / ${progressFill.style.width}`,
);
releaseHeldInstalls();
await failing;
check(
  "reported the failure over the step it left behind",
  updateTitle() === "Could not install the update" &&
    updateBody().includes("hdiutil failed") &&
    !offered(cancelButton) &&
    !offered(backgroundButton),
  `${updateTitle()} / ${updateBody()} / ${cancelButton.hidden}`,
);
check(
  "offered no restart for an install that never landed",
  !offered(restartButton) && offered(closeButton),
  `${restartButton.hidden} / ${closeButton.hidden}`,
);
installError = null;
closeButton.onclick();
// And one that went wrong while the dialog was away is not silence: the reader
// is told the same way an install nobody is watching tells them, in a line under
// the composer.
console.log("an install that goes wrong while nobody is looking");
app.installedUpdate.answer = null;
elementFor("status-text").textContent = "";
updateAnswer = OFFERED_UPDATE;
await app.openUpdate();
await nextTick();
holdNextInstall();
const behind = app.installUpdate();
await nextTick();
backgroundButton.onclick();
installError = "the download did not verify against the release's checksum";
releaseHeldInstalls();
await behind;
check(
  "reported an install that failed behind a closed dialog as a line",
  elementFor("update-modal").hidden === true &&
    /Could not install the update: .*the download did not verify/.test(status()),
  `${elementFor("update-modal").hidden} / ${status()}`,
);
installError = null;
elementFor("status-text").textContent = "";

// ---------- the update a launch installs on its own ----------

// The app keeps itself current the way its other front-ends check for one, so
// the window is told about an install rather than asking: the release it found
// opens the dialog, each step arrives as a stage, and what is left when a release
// lands is the restart that runs it.
console.log("the update a launch installs on its own");
app.installedUpdate.answer = null;
elementFor("update-modal").hidden = true;
elementFor("status-text").textContent = "";

// The launch's own event names the release rather than carrying its notes, since
// the notes are the release's and the window asks for what it draws.
calls.length = 0;
await emit("update-available", {
  version: "0.34.0",
  tag: "desktop-v0.34.0",
  url: "https://github.com/jaysonwu991/oxide/releases/tag/desktop-v0.34.0",
});
await nextTick();
check(
  "opened the dialog on the release the launch found",
  updateOpen() && projectCalls("check_updates").length === 1,
  `${elementFor("update-modal").hidden} / ${JSON.stringify(calls.map(([name]) => name))}`,
);
check(
  "painted the release with what it changed",
  updateTitle() === "Update Available (v0.34.0)" && offered(changelog()) && offered(installButton),
  `${updateTitle()} / ${changelog().hidden} / ${installButton.hidden}`,
);

await emit("update-progress", {
  stage: "downloading",
  version: "0.34.0",
  received: 1024 * 1024,
  total: 4 * 1024 * 1024,
});
await nextTick();
check(
  "showed the download the launch started",
  updateTitle() === "Downloading update" && progressFill.style.width === "25%",
  `${updateTitle()} / ${progressFill.style.width}`,
);
check(
  "offered no restart for a release that is not in place yet",
  !offered(restartButton) && offered(cancelButton) && offered(backgroundButton),
  `${restartButton.hidden} / ${cancelButton.hidden}`,
);

// The release is on disk and the process is not: what the dialog offers is the
// restart that runs it.
const launchAnswer = {
  ok: true,
  version: "0.34.0",
  tag: "desktop-v0.34.0",
  asset: "macos-arm64-Oxide.dmg",
  path: "/Applications/Oxide.app",
  text: "Oxide 0.34.0 is in /Applications/Oxide.app. Quit Oxide and open it again to run the new version.",
  pending: false,
};
await emit("update-ready", launchAnswer);
await nextTick();
check(
  "opened the restart for the release the launch put in place",
  updateTitle() === "Restart and install update" && offered(restartButton) && offered(laterButton),
  `${updateTitle()} / ${restartButton.hidden} / ${laterButton.hidden}`,
);
calls.length = 0;
await restartButton.onclick();
check(
  "restarted the app from the dialog the launch opened",
  projectCalls("restart_app").length === 1,
  JSON.stringify(calls.map(([name]) => name)),
);

// A window that opened after the install started heard none of it: the events
// were emitted into a page that was not listening, and the ones that must not be
// missed are the stage it is on and the report the restart hangs on. The shell
// keeps the newest of them beside its state, and the page asks for what it has
// already said as it starts, from the same channel every other command travels
// on.
app.installedUpdate.answer = null;
elementFor("update-modal").hidden = true;
check(
  "asked what the launch's install had already said as it started",
  invokes.some(([, payload]) => payload.command === "launch_update"),
  JSON.stringify(invokes.map(([, payload]) => payload.command).slice(0, 4)),
);
launchHeard = {
  event: "update-progress",
  payload: { stage: "downloading", version: "0.34.0", received: 0, total: 0 },
};
await app.catchUpOnLaunchUpdate();
check(
  "opened the dialog on the install a page that started late missed",
  updateOpen() && updateTitle() === "Downloading update",
  `${elementFor("update-modal").hidden} / ${updateTitle()}`,
);
elementFor("update-modal").hidden = true;
launchHeard = { event: "update-ready", payload: launchAnswer };
await app.catchUpOnLaunchUpdate();
check(
  "painted the install a page that started late would have missed",
  updateOpen() && updateTitle() === "Restart and install update" && offered(restartButton),
  `${elementFor("update-modal").hidden} / ${updateTitle()} / ${restartButton.hidden}`,
);
elementFor("update-modal").hidden = true;
// A launch's install that could not finish nobody asked for, so it is a line
// rather than a dialog nobody opened — and asking again is what the window's own
// Check for Updates… is for.
launchHeard = { event: "update-failed", payload: { message: "the download did not verify" } };
elementFor("status-text").textContent = "";
await app.catchUpOnLaunchUpdate();
check(
  "reported an install that failed as a line, not a dialog",
  elementFor("update-modal").hidden === true && /did not verify/.test(status()),
  `${elementFor("update-modal").hidden} / ${status()}`,
);
launchHeard = null;
elementFor("status-text").textContent = "";
app.installedUpdate.answer = null;

// An install nobody in the window asked for is cancelled by the name the engine
// gave it, which the window learns from the stage that install reports: the two
// halves agree on which install a cancel is about without the window having had
// to ask for it.
console.log("the install the window was only told about");
elementFor("update-modal").hidden = true;
await emit("update-progress", {
  stage: "downloading",
  version: "0.34.0",
  received: 1024,
  total: 2048,
  token: "launch-1-1",
});
await nextTick();
calls.length = 0;
await app.openUpdate();
check(
  "answered the sidebar's own button with the install it was told about",
  projectCalls("check_updates").length === 0 &&
    updateTitle() === "Downloading update" &&
    offered(cancelButton),
  `${JSON.stringify(calls.map(([name]) => name))} / ${updateTitle()} / ${cancelButton.hidden}`,
);
calls.length = 0;
cancelButton.onclick();
check(
  "cancelled it by the name the engine gave it",
  cancelledTokens[cancelledTokens.length - 1] === "launch-1-1" &&
    projectCalls("cancel_update").length === 1,
  `${JSON.stringify(cancelledTokens)} / ${JSON.stringify(calls.map(([name]) => name))}`,
);

// ---------- the connect dialog ----------

// The provider table holds every provider a client can connect, so the dialog
// searches it instead of making the reader walk the whole list: the box filters
// the rows by name, label and description, the row it leaves selected is the one
// Save connects, and the fields under it say what that provider needs — a server
// on this machine is not asked for a key it has none of.
console.log("the connect dialog");
providersAnswer = providerRows;
const providerList = elementFor("provider-list");
const listedProviders = () => providerList.children;
const providedRow = (index) => String(listedProviders()[index].innerHTML);
const searchProviders = (query) => {
  el("provider-filter").value = query;
  el("provider-filter").fire("input");
};
calls.length = 0;
await app.openConnect();
check(
  "opened the dialog on the provider already in use",
  calls.some(([name]) => name === "list_providers") &&
    app.state.providerName === "openai" &&
    listedProviders().length === providerRows.length &&
    listedProviders()[0].className.includes("active") &&
    elementFor("connect-modal").hidden === false,
  `${app.state.providerName} / ${listedProviders().length} / ${providedRow(0)}`,
);
check(
  "named each provider by the name a login takes, and what state it is in",
  providedRow(0).includes("openai") &&
    providedRow(0).includes("in use") &&
    providedRow(0).includes("stored") &&
    providedRow(1).includes("anthropic") &&
    !providedRow(1).includes("badge"),
  `${providedRow(0)} | ${providedRow(1)}`,
);
check(
  "answered the key field for the provider in use",
  elementFor("login-key-field").hidden === false &&
    elementFor("login-key-note").hidden === true &&
    el("login-key").placeholder === "Leave empty to reuse the stored key",
  `${el("login-key").placeholder} / ${elementFor("login-key-note").hidden}`,
);
// A search that hides the selected provider cannot leave it selected: the row
// Save would connect is a row the reader can see.
searchProviders("aws");
check(
  "searched the provider table by name and description",
  listedProviders().length === 1 &&
    providedRow(0).includes("Amazon Bedrock") &&
    app.state.providerName === "bedrock",
  `${listedProviders().length} / ${app.state.providerName} / ${providedRow(0)}`,
);
searchProviders("claude models");
check(
  "matched a label and a description as well as a name",
  listedProviders().length === 1 &&
    providedRow(0).includes("Anthropic") &&
    app.state.providerName === "anthropic",
  `${listedProviders().length} / ${app.state.providerName} / ${providedRow(0)}`,
);
searchProviders("nothing here");
check(
  "said so when no provider matches, and selected none",
  listedProviders().length === 0 &&
    app.state.providerName === "" &&
    String(providerList.innerHTML).includes("No provider matches"),
  `${listedProviders().length} / ${app.state.providerName} / ${providerList.innerHTML}`,
);
// A provider the machine already has a credential for is asked for none either:
// an AWS signing identity is not something this box could collect, and pasting
// an OpenAI key into it would not be what the request signs with.
searchProviders("bedrock");
check(
  "asked a provider its machine credential authorizes for no key",
  app.state.providerName === "bedrock" &&
    elementFor("login-key-field").hidden === true &&
    elementFor("login-key-note").hidden === false &&
    String(elementFor("login-key-note").textContent).includes("no key needed"),
  `${elementFor("login-key-field").hidden} / ${elementFor("login-key-note").textContent}`,
);
// A server on this machine is asked for no credential either: the field goes
// and the sentence that says why takes its place.
searchProviders("ollama");
check(
  "asked a provider that needs no key for none",
  app.state.providerName === "ollama" &&
    elementFor("login-key-field").hidden === true &&
    elementFor("login-key-note").hidden === false &&
    String(elementFor("login-key-note").textContent).includes("no key needed"),
  `${elementFor("login-key-field").hidden} / ${elementFor("login-key-note").textContent}`,
);
// The row the reader clicks is the provider Save connects, with a key typed for
// the one before it dropped rather than sent to this one.
searchProviders("");
el("login-key").value = "sk-typed-for-openai";
listedProviders()[1].onclick();
check(
  "took the row the reader clicked and dropped the key typed for another",
  app.state.providerName === "anthropic" &&
    listedProviders()[1].className.includes("active") &&
    el("login-key").value === "" &&
    el("login-key").placeholder === "Paste your API key",
  `${app.state.providerName} / ${el("login-key").value} / ${el("login-key").placeholder}`,
);
calls.length = 0;
el("login-key").value = "sk-ant-test";
el("login-model").value = "claude-sonnet-4-5";
await app.saveConnect();
const loginCall = calls.find(([name]) => name === "login");
check(
  "connected the provider the search left selected",
  loginCall !== undefined &&
    loginCall[1].provider === "anthropic" &&
    loginCall[1].key === "sk-ant-test" &&
    loginCall[1].model === "claude-sonnet-4-5" &&
    elementFor("connect-modal").hidden === true &&
    el("login-key").value === "" &&
    status() === "Connected Anthropic",
  `${JSON.stringify(loginCall)} / ${status()}`,
);
// A filter that matches nothing leaves nothing to connect, and Save says so
// rather than connecting whatever was selected before the search.
calls.length = 0;
searchProviders("nothing here");
await app.saveConnect();
check(
  "refused to connect anything with no provider selected",
  calls.length === 0 && status() === "Choose a provider first.",
  `${JSON.stringify(calls.map(([name]) => name))} / ${status()}`,
);
searchProviders("");
el("connect-modal").hidden = true;
// The bridge section below has no provider table in front of it, the way a
// window that never opened this dialog has none.
providersAnswer = [];
app.state.providers = [];

// ---------- the bridge the window is reached through ----------

// Every command the page performs goes through the one command the app
// answers on its bridge, with the command's own name and arguments inside it:
// the page
// reaches nothing else directly, which is what keeps `pick_folder` and
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
