// Checks the desktop shell without a window: the command the page reaches, the
// configuration the window is built from, and the capability that bounds it.
//
//   node crates/desktop/check-shell.mjs
//
// `cargo test` reaches the command layer and `check-app.mjs` drives the page
// against a stubbed bridge, but neither can see whether the two halves still
// agree: a command `ui/app.js` performs that `src/commands.rs` no longer answers
// is a button that fails in the window, and an event the page waits for that
// nothing emits is a turn that never ends. This file is that join, plus the
// migration's own end — no Electron packaging, no preload, no node_modules.
import { readdirSync, readFileSync, statSync } from "node:fs";
import { fileURLToPath } from "node:url";

const here = fileURLToPath(new URL(".", import.meta.url));
const root = fileURLToPath(new URL("../../", import.meta.url));
const read = (path) => readFileSync(`${here}${path}`, "utf8");
const load = (path) => JSON.parse(read(path));

const failures = [];
const check = (name, condition, detail = "") => {
  if (condition) {
    console.log(`  ok   ${name}`);
    return;
  }
  console.log(`  FAIL ${name}${detail ? ` — ${detail}` : ""}`);
  failures.push(name);
};

const app = read("ui/app.js");
const html = read("ui/index.html");
const commands = read("src/commands.rs");
const shell = read("src/main.rs");
const manifest = read("Cargo.toml");
const config = load("tauri.conf.json");
const capability = load("capabilities/default.json");

// ---------- the commands and events the two halves share ----------

// The names `ui/app.js` performs and the dispatch arms the app answers with,
// read out of the sources themselves rather than a hand-kept list.
const arms = new Set([...commands.matchAll(/^\s*"([a-z_]+)" =>/gm)].map(([, name]) => name));
const performed = new Set([...app.matchAll(/invoke\("([a-z_]+)"/g)].map(([, name]) => name));
const unanswered = [...performed].filter((name) => !arms.has(name));

console.log("the commands the page performs");
check(
  "answered every command the page performs",
  performed.size > 0 && unanswered.length === 0,
  `no arm in src/commands.rs for ${unanswered.join(", ")}`,
);
// Arms the window never performs are kept from the shell's own contract — the
// Electron preload could reach them and the Tauri app kept them — so an arm
// added here has to be one the page performs or a deliberate addition to this
// list.
const unused = [...arms].filter((name) => !performed.has(name)).sort();
check(
  "kept no arm the window cannot perform",
  JSON.stringify(unused) === JSON.stringify(["add_project", "list_sessions", "rename_session"]),
  unused.join(", "),
);

console.log("the events the two halves share");
const emitted = new Set(
  [...read("src/commands.rs") + read("src/approval.rs") + read("src/ask.rs") + read("src/turn.rs")]
    .join("")
    .match(/\.emit\(\s*"([a-z-]+)"/g)
    .map((call) => call.match(/"([a-z-]+)"/)[1]),
);
const listened = new Set([...app.matchAll(/listen\("([a-z-]+)"/g)].map(([, name]) => name));
check(
  "listens for exactly the events the app emits",
  listened.size > 0 &&
    [...listened].every((name) => emitted.has(name)) &&
    [...emitted].every((name) => listened.has(name)),
  `unemitted: ${[...listened].filter((name) => !emitted.has(name)).join(", ")} / unread: ${[...emitted]
    .filter((name) => !listened.has(name))
    .join(", ")}`,
);

// ---------- the bridge between them ----------

console.log("the bridge the page reaches");
check(
  "performs every command through the app's one command",
  app.includes("window.__TAURI__.core") &&
    app.includes('tauriInvoke("oxide_invoke", { command, args })') &&
    !app.includes("__OXIDE__"),
  app.slice(0, 0),
);
check(
  "took the events off the app's own channel",
  app.includes("const { listen } = window.__TAURI__.event") &&
    app.includes("event.payload"),
);
check(
  "registers that one command",
  /generate_handler!\[oxide_invoke\]/.test(shell) && /fn oxide_invoke\(/.test(shell),
);
check(
  "hands it to the command layer",
  /use commands::\{dispatch, DesktopState\}/.test(shell) &&
    /dispatch\(Arc::clone\(&state\), &app, &command, args\)/.test(shell),
);
check(
  "carries the app's state into the window",
  /\.manage\(/.test(shell) && /DesktopManager::load_lossy/.test(shell),
);
check(
  "ends with the window it opened",
  /CloseRequested/.test(shell) && /handle\.exit\(0\)/.test(shell),
);
check(
  "starts the dialog plugin the app calls itself",
  /tauri_plugin_dialog::init\(\)/.test(shell),
);

// ---------- the window the page runs in ----------

console.log("the window");
check(
  "hands the page the bridge globals",
  config.app?.withGlobalTauri === true,
  JSON.stringify(config.app?.withGlobalTauri),
);
check(
  "builds the window from the page's own directory",
  config.build?.frontendDist === "ui",
  String(config.build?.frontendDist),
);
check(
  "gives the policy to the window rather than the page",
  typeof config.app?.security?.csp === "string" &&
    config.app.security.csp.includes("default-src 'none'") &&
    config.app.security.csp.includes("style-src 'self' 'unsafe-inline'") &&
    config.app.security.csp.includes("connect-src ipc:") &&
    !/http-equiv="Content-Security-Policy"/.test(html),
  String(config.app?.security?.csp),
);
check(
  "loads the page's own script and stylesheet",
  /<script src="app\.js"><\/script>/.test(html) && /href="style\.css"/.test(html),
);
check(
  "keeps the page's markup free of inline script",
  !/<script(?![^>]*\bsrc=)[^>]*>/.test(html) && !/\son[a-z]+=/i.test(html),
);

const labels = (config.app?.windows || []).map((window) => window?.label);
check(
  "bounds exactly the window the configuration opens",
  capability.identifier === "default" &&
    JSON.stringify(capability.windows) === JSON.stringify(labels) &&
    labels.length === 1,
  `${JSON.stringify(capability.windows)} / ${JSON.stringify(labels)}`,
);
check(
  "grants the window no plugin's own IPC",
  JSON.stringify(capability.permissions) === JSON.stringify(["core:default"]),
  JSON.stringify(capability.permissions),
);

// ---------- the bundle and the build that produces it ----------

console.log("the bundle");
const icons = config.bundle?.icon || [];
check(
  "bundles the icons the configuration names",
  config.bundle?.active === true &&
    icons.length > 0 &&
    icons.every((path) => statSync(`${here}${path}`, { throwIfNoEntry: false })?.isFile()),
  icons.filter((path) => !statSync(`${here}${path}`, { throwIfNoEntry: false })?.isFile()).join(", "),
);
const workspaceVersion = readFileSync(`${root}Cargo.toml`, "utf8").match(
  /\[workspace\.package\][\s\S]*?^version = "([^"]+)"/m,
)?.[1];
check(
  "reports the workspace version the release script writes",
  config.version === workspaceVersion,
  `${config.version} / ${workspaceVersion}`,
);

console.log("the build");
check(
  "builds the binary only where there is a webview to put it in",
  /\[\[bin\]\][\s\S]*?name = "oxide-desktop"[\s\S]*?required-features = \["gui"\]/.test(manifest) &&
    /gui = \[\s*"dep:tauri",\s*"dep:tauri-plugin-dialog",\s*\]/.test(manifest),
  "",
);
check(
  "generates the tauri bindings behind that feature",
  /tauri_build::build\(\)/.test(read("build.rs")) &&
    /#\[cfg\(feature = "gui"\)\]/.test(read("build.rs")),
);

// ---------- nothing left of the shell this replaced ----------

console.log("the migration");
const files = [];
const walk = (dir) => {
  for (const entry of readdirSync(`${here}${dir}`, { withFileTypes: true })) {
    const path = `${dir}${entry.name}`;
    if (entry.isDirectory()) {
      if (!["target", "gen", "node_modules", ".git"].includes(entry.name)) walk(`${path}/`);
      continue;
    }
    if (/\.(js|cjs|mjs|ts)$/.test(entry.name)) files.push(path);
  }
};
walk("");
check(
  "keeps the front-end the only JavaScript, beside its own checks",
  JSON.stringify(files.sort()) ===
    JSON.stringify(["check-app.mjs", "check-shell.mjs", "ui/app.js"]),
  files.join(", "),
);
check(
  "left no Electron packaging behind",
  !files.some((path) => path.endsWith(".cjs")) &&
    ["electron/", "package.json", "forge.config.cjs", "pnpm-lock.yaml", "pnpm-workspace.yaml", ".npmrc"].every(
      (path) => !statSync(`${here}${path}`, { throwIfNoEntry: false }),
    ),
);

console.log(failures.length ? `\n${failures.length} failed` : "\nall checks passed");
process.exit(failures.length ? 1 : 0);
