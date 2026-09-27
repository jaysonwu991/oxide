// Checks the desktop front-end without a window: `ui/app.js` is loaded against
// a stubbed DOM and Tauri bridge, then driven the way the composer drives it.
//
//   node crates/desktop/ui/check-app.mjs
//
// It covers what `cargo test` cannot reach — the `/mcps` dialog (its listing,
// the toggle, and the no-project and failure paths), the Add-project dialog's
// call into the core, the attachment chips (an image's thumbnail and the
// full-size preview it opens), the question dialog (its title, what a blank
// form sends, and the two ways a request goes away — the run ending, and the CLI
// giving up on it), and the `/` menu's dispatch of every built-in
// the shared catalog offers — since a Rust test never runs the app's own
// JavaScript. The
// catalog is read from the built CLI (`target/debug/oxide commands --json`)
// when that binary is present, so a client command the palette offers but the
// app cannot answer is caught here rather than in the window.
import { execFileSync } from "node:child_process";
import { existsSync, readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import vm from "node:vm";

const here = fileURLToPath(new URL(".", import.meta.url));
const root = fileURLToPath(new URL("../../../", import.meta.url));
const cli = `${root}target/debug/oxide`;

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
// fallback for a command the app cannot perform has nothing to look up.
let catalog = [];
let catalogSkipped = false;
if (existsSync(cli)) {
  catalog = JSON.parse(execFileSync(cli, ["commands", "--json"], { encoding: "utf8" }));
} else {
  catalogSkipped = true;
}

// ---------- the stubbed page ----------

class StubElement {
  constructor(tag = "div", id = "") {
    this.tagName = tag.toUpperCase();
    this.id = id;
    this.children = [];
    this.classList = {
      add() {},
      remove() {},
      toggle() {},
      contains: () => false,
    };
    this.style = {};
    this.dataset = {};
    this.hidden = true;
    this.value = "";
    this.textContent = "";
    this.className = "";
    this.disabled = false;
    this.checked = false;
    this.offsetParent = {};
    this.scrollHeight = 10;
    this.clientHeight = 10;
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
    return { left: 0, top: 0, width: 300, height: 40, bottom: 40 };
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
    return node;
  }

  addEventListener() {}
  removeEventListener() {}
  setAttribute() {}
  getAttribute() {
    return null;
  }
  focus() {}
  blur() {}
  click() {}
  scrollTo() {}

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
  closest() {
    return null;
  }
  remove() {}

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

const elements = new Map();
const elementFor = (id) => {
  if (!elements.has(id)) elements.set(id, new StubElement("div", id));
  return elements.get(id);
};

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

// The threads the sidebar groups by project and `/sessions` lists for the one
// that is selected, newest first, the way the core orders them.
const existing = [
  {
    id: "fe0031b1",
    name: "Fix the flaky test",
    cwd: "/Users/jayson/Projects/oxide",
    created_at: 1,
    modified_at: Math.floor(Date.now() / 1000) - 7 * 60,
    message_count: 195,
    preview: "the CI job fails one run in ten",
  },
  {
    id: "7c8031b1",
    name: null,
    cwd: "/Users/jayson/Projects/oxide",
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
// (most recently opened first), then projects discovered from sessions. The app
// opens on the first of them, so the stub carries two.
let projectRows = [
  {
    id: "/Users/jayson/Projects/oxide",
    path: "/Users/jayson/Projects/oxide",
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
  addEventListener: () => {},
  body: new StubElement("body"),
  documentElement: new StubElement("html"),
};

globalThis.document = document;
// The app listens for the events the Rust side emits; the handlers are kept so
// the checks can emit one the way a finished turn does.
const listeners = new Map();
globalThis.window = {
  __TAURI__: {
    core: { invoke },
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

/// The markup ships the composer disabled and the app enables it once a project
/// is selected, so the stub starts it the way the page does.
elementFor("prompt").disabled = true;

const source = readFileSync(`${here}app.js`, "utf8");
vm.runInThisContext(
  source +
    "\nglobalThis.__app = { send, runSlashCommand, state, createProjectState," +
    " openDefaultProject, openCreateProject, addCreateProjectTypedPath, saveCreateProject," +
    " refreshPaletteEntries, addAttachment, addAttachmentFiles, el, showQuestion, answerQuestion," +
    " collectAnswers };\n",
);

const app = globalThis.__app;
const status = () => String(elementFor("status-text").textContent);
const projectCalls = (command) => calls.filter(([name]) => name === command);
/// Delivers an event to the app the way Tauri does (`event.payload`).
const emit = async (name, payload) => {
  for (const handler of listeners.get(name) || []) await handler({ payload });
};

// ---------- the project it opens on ----------

console.log("default project");
// `init()` ran as the source loaded; its project list arrives on a promise, so
// let the startup chain settle before reading what it selected.
await new Promise((resolve) => setTimeout(resolve, 0));
check(
  "opened on the first project in the sidebar",
  app.state.project === projectRows[0].path,
  String(app.state.project),
);
const firstRow = elementFor("projects-tree").children[0]?.children[0];
check(
  "marked that row as the active one",
  String(firstRow?.className).includes("active"),
  String(firstRow?.className),
);
check("left the composer ready to type in", elementFor("prompt").disabled === false);
app.state.project = projectRows[1].path;
await app.openDefaultProject();
check(
  "left a project that was already selected alone",
  app.state.project === projectRows[1].path,
  String(app.state.project),
);
// With no project at all there is nothing to open, so the empty state stays
// rather than a task being started in a folder the user never picked.
const rows = projectRows;
app.state.project = null;
projectRows = [];
await app.openDefaultProject();
check("kept the empty state with no project to open", app.state.project === null, String(app.state.project));
// Back to what the app opened with, so the sections below start from a real
// startup rather than an empty sidebar.
projectRows = rows;
await app.openDefaultProject();

// ---------- the composer's popovers ----------

// The MCP and session listings are part of the composer rather than windows
// over the app: each is a sibling above `.composer` inside `.composer-wrap`, so
// it grows out of the composer's top edge and stays stuck to it.
console.log("popovers");
const shell = readFileSync(`${here}index.html`, "utf8");
const shellAt = (needle) => shell.indexOf(needle);
const buttonFor = (id) => {
  const at = shell.indexOf(`id="${id}"`);
  if (at < 0) return "";
  return shell.slice(shell.lastIndexOf("<button", at), shell.indexOf("</button>", at));
};
check(
  "attached the MCP and session popovers to the composer",
  shellAt('class="composer-wrap"') < shellAt('id="mcps-modal"') &&
    shellAt('id="mcps-modal"') < shellAt('id="sessions-modal"') &&
    shellAt('id="sessions-modal"') < shellAt('class="composer"'),
  `${shellAt('id="mcps-modal"')} / ${shellAt('id="sessions-modal"')} / ${shellAt('class="composer"')}`,
);
check(
  "left the popovers out of the overlays",
  !/<div id="(mcps|sessions)-modal" class="overlay"/.test(shell),
);
for (const [id, label] of [
  ["mcps-refresh", "Recheck the servers"],
  ["mcps-close", "Close"],
  ["sessions-new", "New thread"],
  ["sessions-close", "Close"],
]) {
  const button = buttonFor(id);
  check(
    `made ${id} an icon-only button`,
    button.includes(`title="${label}"`) &&
      button.includes(`aria-label="${label}"`) &&
      button.includes("<svg"),
    button,
  );
}

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

app.state.project = "/Users/jayson/Projects/oxide";
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
app.state.project = "/Users/jayson/Projects/oxide";
app.state.projects = [
  { id: "/Users/jayson/Projects/oxide", name: "oxide", path: "/Users/jayson/Projects/oxide" },
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
await app.runSlashCommand("/sessions");
check(
  "said so when the project has no threads",
  elementFor("sessions-list").innerHTML.includes("No threads for this project yet"),
  elementFor("sessions-list").innerHTML,
);

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
opener.onclick();
check(
  "opened the full-size preview",
  elementFor("image-view-img").src === shot && elementFor("image-modal").hidden === false,
  String(elementFor("image-modal").hidden),
);
elementFor("image-view-close").onclick();
check("closed it again", elementFor("image-modal").hidden === true);

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

// ---------- the question dialog ----------

console.log("questions");
app.state.project = "/Users/jayson/Projects/oxide";
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
check("opened the question dialog", elementFor("question").hidden === false);
check(
  "headed it with the question's own header",
  elementFor("question-title").textContent === "Database",
  elementFor("question-title").textContent,
);
check(
  "painted the question text and its options",
  dialog.outline().includes("Which database should the migration target?") &&
    dialog.outline().includes("The production store"),
  dialog.outline(),
);
const firstBlock = dialog.children[0];
const optionRows = firstBlock.children.filter((row) => row.tagName === "LABEL");
check(
  "offered a single answer's options as radios",
  optionRows.length === 2 && optionRows.every((row) => row.children[0].type === "radio"),
  dialog.outline(),
);
check(
  "preselected a single-answer question's first option",
  optionRows[0].children[0].checked === true && optionRows[1].children[0].checked === false,
  dialog.outline(),
);
const secondBlock = dialog.children[1];
check(
  "offered a multi-select question's options as checkboxes",
  secondBlock.querySelectorAll(".question-choice").every((input) => input.type === "checkbox"),
  dialog.outline(),
);
// The free-text field answers a question with no options at all, so it is part
// of every question rather than a fallback for the empty case.
check(
  "gave every question a field to type an answer in",
  dialog.querySelectorAll(".question-free").length === 2,
  dialog.outline(),
);

// Pick the second option, add text, tick one box on the multi-select question.
optionRows[0].children[0].checked = false;
optionRows[1].children[0].checked = true;
firstBlock.querySelector(".question-free").value = "temporary";
secondBlock.querySelectorAll(".question-choice")[1].checked = true;
await app.answerQuestion(false);
const answered = calls.find(([name]) => name === "resolve_question");
check("answered the question over the bridge", Boolean(answered), JSON.stringify(calls));
check(
  "sent what was picked and typed against the question it belongs to",
  answered &&
    answered[1].id === 42 &&
    answered[1].answers[0].question === "Which database should the migration target?" &&
    JSON.stringify(answered[1].answers[0].values) === '["SQLite","temporary"]' &&
    JSON.stringify(answered[1].answers[1].values) === '["Fixtures"]',
  JSON.stringify(answered && answered[1]),
);
check("closed the dialog once it was answered", elementFor("question").hidden === true);

// Skip answers with nothing, which is how the model is told the question was
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

// The dialog is named the way the core and the extension name it: the *first*
// question's own header, else that question itself.
app.showQuestion({
  id: 45,
  questions: [
    { question: "Proceed?", options: [{ label: "Yes" }] },
    { header: "Details", question: "Which ones?" },
  ],
});
check(
  "headed it with the first question, not the first header it finds",
  elementFor("question-title").textContent === "Proceed?",
  elementFor("question-title").textContent,
);

// Answering an empty form is the same dismissal as pressing Skip, so the agent
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

// ---------- the / menu ----------

console.log("slash commands");
if (catalogSkipped) {
  // Worth saying out loud rather than passing silently.
  console.log(`  skip the catalog check: ${cli} is not built`);
} else {
  const client = catalog.filter((entry) => entry.kind === "client");
  check("the catalog offers client commands", client.length > 0);
  app.state.project = "/Users/jayson/Projects/oxide";
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
}

console.log(failures.length ? `\n${failures.length} failed` : "\nall checks passed");
process.exit(failures.length ? 1 : 0);
