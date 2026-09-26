// Checks the desktop front-end without a window: `ui/app.js` is loaded against
// a stubbed DOM and Tauri bridge, then driven the way the composer drives it.
//
//   node crates/desktop/ui/check-app.mjs
//
// It covers what `cargo test` cannot reach — the `/mcps` dialog (its listing,
// the toggle, and the no-project and failure paths), the Add-project dialog's
// call into the core, and the `/` menu's dispatch of every built-in the shared
// catalog offers — since a Rust test never runs the app's own JavaScript. The
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
  querySelector() {
    return null;
  }
  querySelectorAll() {
    return [];
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

const invoke = async (command, args = {}) => {
  calls.push([command, args]);
  switch (command) {
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
globalThis.window = {
  __TAURI__: { core: { invoke }, event: { listen: async () => {} } },
  innerWidth: 1280,
  innerHeight: 900,
  addEventListener: () => {},
  matchMedia: () => ({ matches: false, addEventListener: () => {} }),
  requestAnimationFrame: (callback) => setTimeout(callback, 0),
  localStorage: { getItem: () => null, setItem: () => {}, removeItem: () => {} },
};
globalThis.localStorage = globalThis.window.localStorage;
globalThis.requestAnimationFrame = globalThis.window.requestAnimationFrame;
globalThis.matchMedia = globalThis.window.matchMedia;
globalThis.addEventListener = () => {};

const source = readFileSync(`${here}app.js`, "utf8");
vm.runInThisContext(
  source +
    "\nglobalThis.__app = { send, runSlashCommand, state, createProjectState," +
    " openCreateProject, addCreateProjectTypedPath, saveCreateProject, refreshPaletteEntries, el };\n",
);

const app = globalThis.__app;
const status = () => String(elementFor("status-text").textContent);
const projectCalls = (command) => calls.filter(([name]) => name === command);

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
check(
  "offered Disable for a running server and Enable for a stopped one",
  opened.includes("mcp-toggle: Disable") && opened.includes("mcp-toggle: Enable"),
  opened,
);

// The toggle writes through the core and redraws from its answer.
const rowFor = (name) =>
  elementFor("mcp-list").children.find((row) =>
    row.children[0].children[0].textContent === name,
  );
check("a row exposes its name, status and toggle", Boolean(rowFor("filesystem")));
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
  after.includes("mcp-status state-disabled: Disabled") && after.includes("mcp-toggle: Enable"),
  after,
);
check("reported the toggle", status() === "Ready", status());

// A failing core call is shown, not swallowed into an empty dialog.
mcpsError = "connection refused";
await driveMcps();
const failed = elementFor("mcp-list").innerHTML;
check("showed why the listing failed", String(failed).includes("connection refused"), String(failed));
mcpsError = null;

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
