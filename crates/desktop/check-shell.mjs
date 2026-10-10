// Checks the desktop shell without a window: the commands the page reaches, the
// two channels it reaches them over, and the configuration the app is built
// from.
//
//   node crates/desktop/check-shell.mjs
//
// The app is three programs in two languages — the page (`ui/`, plain
// JavaScript), the window (`electron/`, TypeScript) and the engine (`src/`,
// Rust) — and each pair of them agrees on something no single test can see.
// `cargo test` reaches the engine's command layer and `check-app.mjs` drives the
// page against a stubbed bridge, but neither can see whether the halves still
// agree: a command `ui/app.js` performs that `src/commands.rs` no longer answers
// is a button that fails in the window, an event the page waits for that nothing
// emits is a turn that never ends, a channel name renamed in the preload is a
// window that answers nothing at all, and an artifact renamed in
// `electron-builder.yml` is a release the updater cannot see. This file is those
// joins — the packet one half writes and the other reads — plus the window the
// build is asked to produce, and the migration's own end: no Electrobun, no
// Tauri, no bun.
import { readFileSync, statSync } from "node:fs";
import { fileURLToPath } from "node:url";

const here = fileURLToPath(new URL(".", import.meta.url));
const root = fileURLToPath(new URL("../../", import.meta.url));
// A check is about what a file says, not about how the machine that checked it
// out spells its newlines. A Windows checkout — and a Windows CI runner, where
// `core.autocrlf` is on by default — hands these files CRLF, and a guard that
// reads a line to its end (`targetsOf` below) has to see the same line there as
// it does here. Every read goes through this one place for that reason.
const text = (path) => readFileSync(path, "utf8").replace(/\r\n/g, "\n");
const read = (path) => text(`${here}${path}`);
const exists = (path) => Boolean(statSync(`${here}${path}`, { throwIfNoEntry: false }));

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
const shell = read("electron/main.ts");
const preload = read("electron/preload.ts");
const packaged = JSON.parse(read("package.json"));
const tsconfig = JSON.parse(read("tsconfig.json"));
const builder = read("electron-builder.yml");
const commands = read("src/commands.rs");
const engine = read("src/main.rs");
const bridge = read("src/bridge.rs");
const update = read("src/update.rs");
const manifest = read("Cargo.toml");
const coreUpdates = text(`${root}crates/core/src/updates.rs`);
const setVersion = text(`${root}scripts/set-version.sh`);
const ignores = text(`${root}.gitignore`);
// pnpm is this package's manager rather than npm, and it keeps its settings in
// `pnpm-workspace.yaml` instead of in an `.npmrc`. Read only if it is there, so
// its absence is the check that fails rather than this line.
const settings = exists("pnpm-workspace.yaml") ? read("pnpm-workspace.yaml") : "";

// A negative assertion is about what a file *does*, so it is read with its
// documentation taken off: these files name what they replaced.
const code = (source) => source.replace(/^\s*(\/\/|\/\*|\*).*$/gm, "");
// One section of a manifest, so a name is asserted where it is declared rather
// than anywhere the file happens to mention it.
const section = (source, header) => {
  const start = source.indexOf(header);
  if (start < 0) return "";
  const end = source.indexOf("\n[", start);
  return source.slice(start, end < 0 ? source.length : end);
};
// A slice of a source from one marker to the next, for asserting on one function
// rather than on the file it lives in.
const between = (source, from, to) => {
  const start = source.indexOf(from);
  const end = to ? source.indexOf(to, start) : -1;
  return start < 0 ? "" : source.slice(start, end < 0 ? source.length : end);
};

// `electron-builder.yml` is YAML, and this check does not run a YAML parser: a
// top-level block is read to the next line that starts at column zero, and a key
// inside it is read off its own line.
const block = (source, key) => {
  const lines = source.split("\n");
  const start = lines.findIndex((line) => new RegExp(`^${key}:`).test(line));
  if (start < 0) return "";
  let end = lines.length;
  for (let index = start + 1; index < lines.length; index += 1) {
    if (lines[index].trim() !== "" && !/^\s/.test(lines[index])) {
      end = index;
      break;
    }
  }
  return lines.slice(start, end).join("\n");
};
const scalar = (source, key) => {
  const match = new RegExp(`^\\s*${key}:[ \\t]*(.*)$`, "m").exec(source);
  return match ? match[1].replace(/^["']|["']$/g, "").trim() : "";
};

// ---------- the commands and events the two halves share ----------

// The names `ui/app.js` performs and the dispatch arms the engine answers with,
// read out of the sources themselves rather than a hand-kept list.
const arms = new Set([...commands.matchAll(/^\s*"([a-z_]+)" =>/gm)].map(([, name]) => name));
const performed = new Set([...app.matchAll(/invoke\("([a-z_]+)"/g)].map(([, name]) => name));
// A command the window performs itself never reaches the engine: the folder
// chooser is the platform's panel and a link is the platform's browser, and a
// child process can be reached through neither.
const performedByWindow = new Set(
  [...code(shell).matchAll(/params\?\.command === "([a-z_]+)"/g)].map(([, name]) => name),
);
const unanswered = [...performed].filter(
  (name) => !arms.has(name) && !performedByWindow.has(name),
);

console.log("the commands the page performs");
check(
  "answered every command the page performs",
  performed.size > 0 && unanswered.length === 0,
  `no arm in src/commands.rs for ${unanswered.join(", ")}`,
);
// Arms the window never performs are kept from the shell's own contract — a
// front-end that speaks this bridge could reach them — so an arm added here has
// to be one the page performs or a deliberate addition to this list.
const unused = [...arms].filter((name) => !performed.has(name)).sort();
check(
  "kept no arm the window cannot perform",
  JSON.stringify(unused) === JSON.stringify(["add_project", "list_sessions", "rename_session"]),
  unused.join(", "),
);

console.log("the events the two halves share");
// The launch's own install reports its steps through one call that both keeps
// them for a window that subscribed late and emits them, so that call site names
// the event the window listens for as surely as an `.emit` does.
const emitted = new Set(
  [...commands + read("src/approval.rs") + read("src/ask.rs") + read("src/turn.rs")]
    .join("")
    .match(/\.(?:emit|announce_launch_update)\(\s*"([a-z0-9_-]+)"/g)
    .map((call) => call.match(/"([a-z0-9_-]+)"/)[1]),
);
// Two of them the window raises on its own — the update check a menu item asks
// for — so the events a packet can carry are the engine's and the window's
// together.
for (const site of [...code(shell).matchAll(/webContents\.send\(([\s\S]*?)\);/g)].map(([, body]) => body)) {
  const name = /id: "([a-z-]+)"/.exec(site);
  if (name) emitted.add(name[1]);
}
const listened = new Set([...app.matchAll(/listen\("([a-z-]+)"/g)].map(([, name]) => name));
// One of them is the window's own rather than the page's — the restart that runs
// a release an install has put in place — so it is read where the window takes it
// off the packet channel. A link is not one of these at all: opening it is the
// window's own command, answered on the bridge before the engine ever sees it.
const windowHandled = new Set(
  [...code(shell).matchAll(/packet\.id === "([a-z-]+)"/g)].map(([, name]) => name),
);
check(
  "delivers every event the app emits to the half that reads it",
  listened.size > 0 &&
    windowHandled.size > 0 &&
    [...listened].every((name) => emitted.has(name)) &&
    [...emitted].every((name) => listened.has(name) || windowHandled.has(name)) &&
    [...windowHandled].every((name) => emitted.has(name) && !listened.has(name)),
  `unemitted: ${[...listened].filter((name) => !emitted.has(name)).join(", ")} / dropped: ${[...emitted]
    .filter((name) => !listened.has(name) && !windowHandled.has(name))
    .join(", ")}`,
);

// ---------- the channel between the page and the window ----------
//
// One JSON packet travels the whole way: the page writes it, the window passes
// it on, the engine reads it, and an answer or an announcement comes back over
// the same shapes. Each half is asserted on its own keys below, because a field
// renamed on one side is a window that answers nothing, and nothing else in
// either suite would notice.

console.log("the channel the page reaches");
// The page's one call to the process that shows it, read out of the page: a
// request carrying the command's name and arguments under the envelope the
// window and the engine both read.
const invokeCall = app.slice(app.indexOf("function invoke("), app.indexOf("async function listen("));
check(
  "performed every command through the app's one command",
  /hostSend\(\{ type: "request", id, method: "oxide_invoke", params: \{ command, args \} \}\)/.test(
    invokeCall,
  ) &&
    app.includes("window.__electrobunHostBridge") &&
    /userBridge\.postMessage\(JSON\.stringify\(packet\)\)/.test(app) &&
    !app.includes("__TAURI__") &&
    !app.includes("__OXIDE__"),
  invokeCall.replace(/\s+/g, " ").trim(),
);
check(
  "left the event bridge as the page's other way to the host",
  /eventBridge = window\.__electrobunSendToHost/.test(app) &&
    /return eventBridge\(packet\)/.test(app) &&
    /throw new Error\("This window has no channel/.test(app),
);
check(
  "took the events off the app's own channel",
  app.includes("window.__electrobun.receiveMessageFromHost = receiveMessageFromHost") &&
    app.includes("__electrobunPendingHostMessages") &&
    /listeners\.get\(packet\.id\)/.test(app),
);
// The page names four globals and defines one of them; the preload installs the
// rest. They are the names the shell this replaced used, which is how the page
// runs in this window unchanged — so the preload has to install exactly the
// names the page reads and no name it does not.
const pageGlobals = [...new Set([...app.matchAll(/__electrobun[A-Za-z]*/g)].map(([name]) => name))];
const installed = new Set([...preload.matchAll(/__electrobun[A-Za-z]*/g)].map(([name]) => name));
// The page hands a packet to the bridge it finds first, and the preload installs
// that one, so the two names it reads for the channel are the preload's — and
// the event bridge the page checks second is not installed, which is what makes
// the packet a `postMessage` in this window rather than an event.
check(
  "installed the globals the page reads and no others",
  ["__electrobunHostBridge", "__electrobunPendingHostMessages", "__electrobun"].every(
    (name) => pageGlobals.includes(name) && installed.has(name),
  ) &&
    [...installed].every((name) => pageGlobals.includes(name)) &&
    !installed.has("__electrobunSendToHost") &&
    /const userBridge = window\.__electrobunHostBridge/.test(app) &&
    /if \(userBridge\) return userBridge\.postMessage\(JSON\.stringify\(packet\)\)/.test(app),
  `page: ${pageGlobals.join(", ")} / preload: ${[...installed].join(", ")}`,
);
check(
  "answered over the name the preload's queue hands packets to",
  /typeof page\.__electrobun\?\.receiveMessageFromHost === "function"/.test(preload) &&
    /page\.__electrobun\.receiveMessageFromHost\(raw\)/.test(preload) &&
    /page\.__electrobunPendingHostMessages\.push\(raw\)/.test(preload) &&
    /window\.__electrobun\.receiveMessageFromHost = receiveMessageFromHost/.test(app) &&
    /window\.__electrobunPendingHostMessages = \[\]/.test(app) &&
    /setTimeout\(drainPendingHostMessages, 0\)/.test(app),
);

// ---------- the channel between the window and the engine ----------

console.log("the channel the window reaches");
// The preload's own end of the packet channel: one send, one subscription, and
// the two names both halves of the window have to spell the same way. A channel
// renamed in one of them is a window where every click does nothing.
const channelOf = (source) => ({
  host: /const TO_HOST = "([^"]+)"/.exec(source)?.[1],
  page: /const TO_PAGE = "([^"]+)"/.exec(source)?.[1],
});
const preloadChannel = channelOf(preload);
const windowChannel = channelOf(shell);
check(
  "spelled the two channel names the same in the preload and in the window",
  Boolean(preloadChannel.host) &&
    preloadChannel.host === windowChannel.host &&
    preloadChannel.page === windowChannel.page &&
    preloadChannel.host !== preloadChannel.page,
  `${preloadChannel.host}/${preloadChannel.page} vs ${windowChannel.host}/${windowChannel.page}`,
);
check(
  "carried the packet across the window's own boundary",
  /ipcRenderer\.send\(TO_HOST, message\)/.test(preload) &&
    /ipcRenderer\.on\(TO_PAGE, \(_event, raw: string\) => deliver\(raw\)\)/.test(preload) &&
    /ipcMain\.on\(TO_HOST, \(event, message: unknown\)/.test(shell) &&
    /webContents\.send\(TO_PAGE, line\)/.test(shell),
);
// The engine is a child process and a packet is one JSON line of it: the window
// writes each with its own newline and reads the answers the same way, so
// neither side buffers a packet into two or waits for a length prefix that never
// comes.
check(
  "started the engine and spoke to it one packet per line",
  /spawn\(binary, \[\], \{ stdio: \["pipe", "pipe", "pipe"\] \}\)/.test(shell) &&
    /harness\.stdin\.write\(`\$\{line\}\\n`\)/.test(shell) &&
    /readLines\(child\.stdout/.test(shell) &&
    /pending\.split\("\\n"\)/.test(shell),
);
// The engine's binary: packaged inside the app's own resources, or the one cargo
// left in this crate's `target/` for a development run, or whatever a developer
// pointed the app at. All three matter — a packaged app that looked for
// `target/` would have no engine at all.
const harnessPath = between(shell, "function harnessPath()", "function startHarness");
check(
  "found the engine where the build puts it",
  /app\.isPackaged\) return path\.join\(process\.resourcesPath, "harness", name\)/.test(
    harnessPath,
  ) &&
    /OXIDE_DESKTOP_HARNESS/.test(harnessPath) &&
    /path\.join\(__dirname, "\.\.", "\.\.", "target", profile, name\)/.test(harnessPath) &&
    /"oxide-desktop\.exe" : "oxide-desktop"/.test(harnessPath),
  harnessPath.replace(/\s+/g, " ").trim(),
);
// The engine's half of the protocol: a request read by its own fields, answered
// over the two envelopes the page and the window already agreed on.
check(
  "reads the packet the page writes",
  /packet\["type"\] != "request"/.test(engine) &&
    /packet\["id"\]\.as_u64\(\)/.test(engine) &&
    /packet\["params"\]\["command"\]/.test(engine) &&
    /packet\["params"\]\["args"\]/.test(engine),
);
check(
  "read it one line at a time off the pipe it was started with",
  /let stdin = std::io::stdin\(\)/.test(engine) &&
    /for line in stdin\.lock\(\)\.lines\(\)/.test(engine) &&
    /Host::stdout\(\)/.test(engine),
);
check(
  "handed the request to the command layer",
  /use commands::\{dispatch, DesktopState\}/.test(engine) &&
    /dispatch\(Arc::clone\(&state\), &host, &command, args\)\.await/.test(engine) &&
    /host\.respond\(id, answer\)/.test(engine) &&
    /DesktopManager::load_lossy/.test(engine) &&
    /auto_update\(Arc::clone\(&state\)\)/.test(engine) &&
    /auto_catalog\(Arc::clone\(&state\)\)/.test(engine),
);
// The launch that looks for the catalog is the launch that repaints what it
// changes: the window the model chip carries comes from the catalog, so the
// page is told an answer landed rather than waiting for the next folder open —
// and it repaints only that window, since a full reload would also hand back
// the level the reader picked while the lookup was in flight.
check(
  "told the window when the launch's own catalog lookup landed",
  /pub async fn auto_catalog\(state: Arc<DesktopState>\)/.test(commands) &&
    /catalog::launch_providers\(&active_provider\(\)\)/.test(commands) &&
    /catalog::refresh_launch\(&providers, None\)\.await\.is_some\(\)/.test(commands) &&
    /state\.events\.emit\("model-catalog", json!\(\{\}\)\)/.test(commands) &&
    /listen\("model-catalog", \(\) => refreshContextWindow\(\)\)/.test(app) &&
    !/listen\("model-catalog", \(\) => loadInfo\(\)\)/.test(app) &&
    /async function refreshContextWindow\(\)/.test(app) &&
    /state\.contextWindow = info\.contextWindow \|\| 0/.test(app),
);
// The provider the launch's own lookup covers is the one the turn runs on, so
// it is read the way the run reads it rather than from `config.json` alone.
check(
  "read the provider the launch's lookup covers the way a run reads it",
  /fn active_provider\(\) -> String \{/.test(commands) &&
    /std::env::var\("OXIDE_PROVIDER"\)/.test(commands) &&
    /std::fs::read_to_string\(Config::config_path\(\)\)/.test(commands),
);
check(
  "kept the answer and the announcement the same two shapes",
  /"type": "response", "id": id, "success": true, "payload": payload/.test(bridge) &&
    /"type": "response", "id": id, "success": false, "error": error/.test(bridge) &&
    /"type": "message", "id": event, "payload": payload/.test(bridge) &&
    /packet\.type === "response"/.test(app) &&
    /packet\.success/.test(app) &&
    /packet\.type === "message"/.test(app),
);
// A request the page sends before the engine is up is held rather than written
// into a pipe nobody has opened, and the window answers the one request the
// operating system's own panel has to: a folder chooser cannot be drawn from the
// engine, which has no window at all.
check(
  "held what the page sent before the engine was listening",
  /queuedToHarness\.push\(line\)/.test(shell) &&
    /for \(const line of queuedToHarness\) child\.stdin\.write/.test(shell),
);
check(
  "opened the folder chooser the window's own panel provides",
  /request\.params\?\.command === "pick_folder"/.test(shell) &&
    /dialog\.showOpenDialog\(window, options\)/.test(shell) &&
    /properties: \["openDirectory", "createDirectory"\]/.test(shell) &&
    /answerRequest\(window, id, answer\.canceled \? null : \(answer\.filePaths\[0\] \?\? null\)\)/.test(
      shell,
    ) &&
    /BrowserWindow\.fromWebContents\(event\.sender\)/.test(shell) &&
    /invoke\("pick_folder"/.test(app) &&
    !/osascript|zenity/.test(code(shell)),
);
// A link is handed to the machine's own handler, and the window is the half that
// has one: the page's request is answered there — before the engine ever sees it
// — the scheme is checked before the URL goes anywhere, and the answer carries
// whether the browser started, so a click that could not be carried out is one
// the reader is told about instead of one that quietly does nothing.
check(
  "handed a link to the machine's own handler and said whether it opened",
  /request\.params\?\.command === "open_url"/.test(shell) &&
    /await shell\.openExternal\(url\)/.test(shell) &&
    /isOpenableUrl\(url\)/.test(shell) &&
    /answerError\(window, id, describe\(error\)\)/.test(shell) &&
    /invoke\("open_url"/.test(app) &&
    !/xdg-open|rundll32|openPath|"open", "-a"/.test(code(shell)) &&
    !/open-url|is_openable_url/.test(commands),
);

// ---------- the window the page runs in ----------

console.log("the window");
check(
  "runs this package's own main process",
  packaged.main === "electron/dist/main.js" &&
    packaged.scripts.build === "tsc -p tsconfig.json" &&
    /tsc -p tsconfig\.json && cargo build --release && electron-builder --config electron-builder\.yml/.test(
      packaged.scripts.dist,
    ) &&
    exists("tsconfig.json") &&
    exists("electron-builder.yml") &&
    // Both workflows install with `pnpm install --frozen-lockfile`, which reads a
    // lockfile and fails without one: the app's own packages have to be pinned
    // in the tree. `package-lock.json` is npm's answer to the same question, and
    // the two together would describe the tree twice.
    exists("pnpm-lock.yaml") &&
    !exists("package-lock.json") &&
    packaged.packageManager === "pnpm@12.8.1",
  packaged.main,
);
// pnpm installs a package's tree the way its manifest declares it: what one
// dependency happens to carry is not reachable from this package. That is what
// makes the two settings below load-bearing — `electron`'s own install script
// downloads the browser the window runs in, and electron-builder names its
// `node-gyp` as a git URL at a pinned commit, which pnpm refuses under a
// dependency unless this file says otherwise — and it is why `@types/node` is
// declared here rather than borrowed: it is named in `tsconfig.json`'s `types`,
// so a copy that only arrived under a dependency would type-check against a flat
// tree and not in this one. The resolution mode is this compiler's: TypeScript 7
// removed `node10`, so the two move together.
check(
  "installed through the settings of the package manager the extension uses",
  settings !== "" &&
    scalar(settings, "blockExoticSubdeps") === "false" &&
    scalar(block(settings, "allowBuilds"), "electron") === "true" &&
    scalar(block(settings, "allowBuilds"), "electron-winstaller") === "false" &&
    Boolean(packaged.devDependencies["@types/node"]) &&
    /^7\./.test(packaged.devDependencies.typescript) &&
    tsconfig.compilerOptions.module === "nodenext" &&
    tsconfig.compilerOptions.moduleResolution === "nodenext",
  `${packaged.devDependencies.typescript} ${tsconfig.compilerOptions.moduleResolution}`,
);
// The workflows that build the app install the same tree this check reads, so a
// job that reached for npm would be one with no lockfile to install from while
// the compiler and the bundler it needs arrived from somewhere else. The pnpm
// they set up is the one `package.json` names, and each caches on this package's
// own lockfile rather than the tree beside it.
const withoutComments = (source) => source.replace(/^\s*#.*$/gm, "");
const ciWorkflow = text(`${root}.github/workflows/ci.yml`);
const desktopWorkflow = text(`${root}.github/workflows/desktop.yml`);
const workflowJobs = [
  between(ciWorkflow, "\n  desktop:", "\n  vscode:"),
  between(desktopWorkflow, "\n  build:", "\n  release:"),
];
const npmCommands = workflowJobs.flatMap(
  (job) => withoutComments(job).match(/\bnpm (?:ci|install|run)\b/g) ?? [],
);
check(
  "installed and built by the workflows the same way",
  workflowJobs.every(
    (job) =>
      job !== "" &&
      /pnpm\/action-setup@v4/.test(job) &&
      new RegExp(`version: ${packaged.packageManager.split("@")[1]}`).test(job) &&
      /pnpm install --frozen-lockfile/.test(job),
  ) &&
    npmCommands.length === 0 &&
    /cache-dependency-path: crates\/desktop\/pnpm-lock\.yaml/.test(workflowJobs[0]) &&
    /hashFiles\('crates\/desktop\/pnpm-lock\.yaml'\)/.test(workflowJobs[1]),
  npmCommands.join(", ") || "pnpm in both workflow jobs",
);
// The notarization key travels as three secrets that belong together — the
// encoded `.p8`, the key's own id, and the issuer it was made for — while
// electron-builder notarizes as soon as it finds this configuration at all. A
// step that wrote the file with a placeholder for the other two would fail
// every unsigned build instead of leaving it unsigned.
const keyStep = between(
  desktopWorkflow,
  "- name: Write App Store Connect API key",
  "- name: Build",
);
check(
  "wrote the notarization key only as a whole",
  keyStep !== "" &&
    /\[ -z "\$\{APPLE_API_KEY_P8:-\}" \] \|\| \[ -z "\$\{APPLE_API_KEY:-\}" \] \|\| \[ -z "\$\{APPLE_API_ISSUER:-\}" \]/.test(
      keyStep,
    ) &&
    /openssl base64 -d -A > "\$\{RUNNER_TEMP\}\/AuthKey\.p8"/.test(keyStep) &&
    /printf 'APPLE_API_KEY=%s\\n' "\$\{RUNNER_TEMP\}\/AuthKey\.p8"/.test(keyStep) &&
    !/\(unset\)/.test(keyStep),
  keyStep === "" ? "no key step" : "the key or nothing",
);
// `ui/app.js` fills the receive half of the bridge in on `window` itself, which
// a context-isolated preload could not be reached through — so the page and the
// preload share one world. What keeps the page from reaching Node is that
// `nodeIntegration` is off and the preload exposes one function and nothing
// else.
const prefs = between(shell, "webPreferences: {", "\n  });");
const exposed = [
  ...new Set(
    [...preload.matchAll(/require\("electron"\)|ipcRenderer\.(?:send|on|invoke)|contextBridge|process\.\w+/g)].map(
      ([name]) => name,
    ),
  ),
];
check(
  "loads the page's bridge into the page's own world, with Node out of the page",
  /preload: path\.join\(__dirname, "preload\.js"\)/.test(prefs) &&
    /contextIsolation: false/.test(prefs) &&
    /nodeIntegration: false/.test(prefs) &&
    /sandbox: false/.test(prefs),
  prefs.replace(/\s+/g, " ").trim(),
);
check(
  "exposed the packet channel to the page and nothing else",
  exposed.length > 0 &&
    exposed.every((name) => name === 'require("electron")' || name.startsWith("ipcRenderer.")) &&
    /ipcRenderer\.send\(TO_HOST, message\)/.test(preload) &&
    /ipcRenderer\.on\(TO_PAGE/.test(preload),
  exposed.join(", "),
);
const minimum = /const MIN_SIZE = \{ width: ([\d.]+), height: ([\d.]+) \}/.exec(shell);
check(
  "kept the layout's smallest window",
  Boolean(minimum) &&
    Number(minimum[1]) >= 820 &&
    Number(minimum[2]) >= 560 &&
    /minWidth: MIN_SIZE\.width/.test(shell) &&
    /minHeight: MIN_SIZE\.height/.test(shell),
  minimum ? `${minimum[1]}×${minimum[2]}` : "no minimum",
);
// A press that activates the window is a press in the window: without this the
// first click on a control in a window that is not yet key is spent on making it
// key, and the reader has to click twice. The window is also shown once its
// first frame is there rather than as a white rectangle while the transcript is
// read.
check(
  "took the first press as the reader's own rather than a press to activate with",
  /acceptFirstMouse: true/.test(shell) &&
    /show: false/.test(shell) &&
    /window\.once\("ready-to-show"/.test(shell),
);
const readyToShow = between(shell, 'window.once("ready-to-show", () => {', "\n  });");
check(
  "maximized the first frame before showing it",
  readyToShow.indexOf("window.maximize();") >= 0 &&
    readyToShow.indexOf("window.maximize();") < readyToShow.indexOf("window.show();"),
  readyToShow.replace(/\s+/g, " ").trim(),
);
// The page is this crate's `ui/` directory, both in a checkout and inside the
// app's own archive, and the build has to carry it there: a `files` list that
// forgot `ui/` is an app with no page.
const pagePath = between(shell, "function pagePath()", "function windowFor");
const files = block(builder, "files");
check(
  "loaded the page the build carries into the app",
  /path\.join\(__dirname, "\.\.", "\.\.", "ui", "index\.html"\)/.test(pagePath) &&
    /window\.loadFile\(pagePath\(\)\)/.test(shell) &&
    /^\s*- ui\/\*\*$/m.test(files) &&
    /^\s*- electron\/dist\/\*\*$/m.test(files),
  files.replace(/\s+/g, " ").trim(),
);
check(
  "copies it with everything it loads",
  /<script src="app\.js"><\/script>/.test(html) &&
    /href="style\.css"/.test(html) &&
    exists("ui/app.js") &&
    exists("ui/style.css"),
);
check(
  "gives the policy to the page that carries it rather than to a window it is injected into",
  /http-equiv="Content-Security-Policy"/.test(html) &&
    /content="default-src 'none'; script-src 'self'/.test(html) &&
    /style-src 'self' 'unsafe-inline'/.test(html) &&
    /connect-src 'none'/.test(html) &&
    !code(builder).includes("csp"),
);
check(
  "keeps the page's markup free of inline script",
  !/<script(?![^>]*\bsrc=)[^>]*>/.test(html) && !/\son[a-z]+=/i.test(html),
);
check(
  "ended the app with the window it opened",
  /app\.on\("window-all-closed", \(\) => app\.quit\(\)\)/.test(shell) &&
    /app\.on\("before-quit"/.test(shell) &&
    /harness\?\.kill\(\)/.test(shell),
);

// ---------- what the window is allowed to do ----------

console.log("the window's own rules");
// The window is the app's own page. It never navigates: a link in a reply is
// opened in the machine's browser by the packet above, so a page that navigated
// this window away would be replacing the app, a popup would be a window with no
// engine behind it, and a second webview would be a second page carrying the
// preload's channel.
check(
  "let the window show its own page and nothing else",
  /setWindowOpenHandler\(\(\) => \(\{ action: "deny" \}\)\)/.test(shell) &&
    /on\("will-navigate", \(event\) => event\.preventDefault\(\)\)/.test(shell) &&
    /on\("will-attach-webview", \(event\) => event\.preventDefault\(\)\)/.test(shell) &&
    !/webSecurity: false|allowRunningInsecureContent/.test(code(shell)) &&
    !/allow_all_navigation/.test(code(shell)),
);
// Electron keeps its own browser state — the Chromium profile, the GPU cache —
// under `app.getPath("userData")`, which by default is the very directory
// Oxide's own configuration lives in: a `Cache/` and a `Local Storage/` in the
// middle of a config directory the CLI reads is somebody else's files in it.
check(
  "kept Electron's own profile out of the config directory the CLI reads",
  /app\.setPath\(\s*"userData",\s*path\.join\(app\.getPath\("appData"\), "Oxide", "desktop", "electron"\)/.test(
    shell,
  ) &&
    /^app\.setPath\(/m.test(shell),
  between(shell, "app.setPath(", ");").replace(/\s+/g, " ").trim(),
);

// ---------- the menu bar ----------

console.log("the menu bar");
// The menu bar is the window's own. The one item on it that is not a platform
// role is the update check, and the page paints the dialog: the click reaches it
// as the event the page already listens for, so the menu item and the sidebar's
// own button end at the same dialog.
const menu = between(shell, "function installMenu()", "function announceCheckUpdates()");
check(
  "offered the app, Edit and Window menus where the platform has one",
  /Menu\.setApplicationMenu\(\s*Menu\.buildFromTemplate/.test(menu) &&
    /if \(process\.platform !== "darwin"\) \{\s*Menu\.setApplicationMenu\(null\);\s*return;/.test(
      menu,
    ) &&
    /label: "Oxide"/.test(menu) &&
    /label: "Edit"/.test(menu) &&
    /label: "Window"/.test(menu) &&
    /installMenu\(\);/.test(shell),
  menu.replace(/\s+/g, " ").trim().slice(0, 160),
);
check(
  "left the menu's own behaviour to the platform's roles",
  /role: "about"/.test(menu) &&
    /role: "quit"/.test(menu) &&
    /role: "copy"/.test(menu) &&
    /role: "minimize"/.test(menu) &&
    /role: "hideOthers"/.test(menu),
);
const clicks = [...menu.matchAll(/\{ label: "([^"]+)", click: /g)].map(([, label]) => label);
check(
  "asks the window to check for updates and no more",
  JSON.stringify(clicks) === JSON.stringify(["Check for Updates…"]) &&
    /label: "Check for Updates…", click: \(\) => announceCheckUpdates\(\)/.test(menu) &&
    menu.indexOf('role: "about"') < menu.indexOf("Check for Updates…"),
  clicks.join(", "),
);
check(
  "announced it over the channel a turn's events travel on",
  /window\.webContents\.send\(\s*TO_PAGE,\s*JSON\.stringify\(\{ type: "message", id: "check-updates", payload: \{\} \}\)/.test(
    shell,
  ) && /listen\("check-updates"/.test(app),
);

// ---------- the release the app installs itself ----------

console.log("the release");
// The artifact names are not decoration: the app updates itself from its own
// release, and it resolves the file a release carries by that name. So every
// name `oxide_core::updates` resolves for this component is checked against the
// one `electron-builder.yml` would write — the join a rename on either side
// breaks, and a rename anybody could make by hand.
const artifacts = [
  { platform: "mac", arch: "arm64", ext: "dmg", asset: "macos-arm64-Oxide.dmg" },
  { platform: "linux", arch: "x64", ext: "AppImage", asset: "linux-x64-Oxide-Setup.AppImage" },
  { platform: "linux", arch: "arm64", ext: "tar.gz", asset: "linux-arm64-Oxide-Setup.tar.gz" },
  { platform: "win", arch: "x64", ext: "exe", asset: "win-x64-Oxide-Setup.exe" },
];
const named = artifacts.map(({ platform, arch, ext, asset }) => {
  const template = scalar(block(builder, platform), "artifactName");
  const expanded = template
    .replace("${arch}", arch)
    .replace("${ext}", ext)
    .replace("${name}", "Oxide")
    .replace("${version}", packaged.version);
  return { asset, expanded, template };
});
const resolved = (asset) => new RegExp(`"${asset.replace(/[.$]/g, "\\$&")}"`).test(coreUpdates);
check(
  "named the files a release carries the way the updater resolves them",
  named.length === artifacts.length &&
    named.every(({ asset, expanded }) => expanded === asset && resolved(asset)),
  named
    .filter(({ asset, expanded }) => expanded !== asset || !resolved(asset))
    .map(({ template, expanded, asset }) => `${template} → ${expanded} ≠ ${asset}`)
    .join(" | "),
);
// A target list and the names are two halves of one release: a name for an
// archive nothing builds is a download that is never there.
// A target and the architectures it builds for, read as the pairs they are
// declared in: `- target: x` with the `arch:` list under it. A name for an
// archive nothing builds is a download that is never there, which is why the
// architectures are asserted per target rather than as one union.
const targetsOf = (platform) => {
  const lines = block(builder, platform).split("\n");
  const found = [];
  for (let index = 0; index < lines.length; index += 1) {
    const name = /^\s*- target: (\S+)$/.exec(lines[index]);
    if (!name) continue;
    const arch = [];
    for (let next = index + 1; next < lines.length; next += 1) {
      if (/^\s*- target: /.test(lines[next])) break;
      if (!/^\s*arch:\s*$/.test(lines[next])) continue;
      for (let entry = next + 1; entry < lines.length; entry += 1) {
        const match = /^(\s*)- (\S+)$/.exec(lines[entry]);
        if (!match || match[1].length <= 4) break;
        arch.push(match[2]);
      }
    }
    found.push({ target: name[1], arch });
  }
  return found;
};
check(
  "built every target those names belong to",
  targetsOf("mac").map(({ target }) => target).join(",") === "dmg" &&
    targetsOf("win").map(({ target }) => target).join(",") === "nsis" &&
    targetsOf("linux").map(({ target }) => target).join(",") === "AppImage,tar.gz",
  ["mac", "win", "linux"]
    .map((platform) => targetsOf(platform).map(({ target }) => target).join("+"))
    .join(" / "),
);
// macOS is arm64 only — the x64 build was never published — which is what the
// core's own platform list says too.
const archesOf = (platform, expected) =>
  targetsOf(platform).every(({ arch }) => arch.slice().sort().join(",") === expected);
check(
  "built the architectures the core offers a release for",
  archesOf("mac", "arm64") &&
    archesOf("win", "x64") &&
    archesOf("linux", "arm64,x64") &&
    /const SUPPORTED_PLATFORMS: &str = "darwin-arm64, darwin-x64, linux-x64, linux-arm64, win32-x64"/.test(
      coreUpdates,
    ),
  ["mac", "win", "linux"]
    .map((platform) =>
      targetsOf(platform)
        .map(({ target, arch }) => `${target}:${arch.join("+")}`)
        .join(" "),
    )
    .join(" / "),
);
// The engine is a child process the app starts itself, so it has to be inside
// the app: carried as a resource on each platform, and named as a binary so the
// macOS build signs it along with the bundle it runs from.
check(
  "carried the engine into every bundle the build makes",
  ["mac", "win", "linux"].every(
    (platform) =>
      /to: harness\/oxide-desktop(\.exe)?/.test(block(builder, platform)) &&
      /from: target\/release\/oxide-desktop/.test(block(builder, platform)),
  ) && /binaries:\s*\n\s*- Resources\/harness\/oxide-desktop/.test(block(builder, "mac")),
);
// The app and the engine report one version: the bundler writes `package.json`'s
// into the app, `current_version()` in the engine reads its own manifest's, and
// the release script writes both from the tag.
check(
  "reports the version the app compares a release against",
  /^version = "([^"]+)"/m.exec(section(manifest, "[package]"))?.[1] === packaged.version &&
    /env!\("CARGO_PKG_VERSION"\)/.test(update) &&
    /crates\/desktop\/Cargo\.toml/.test(setVersion) &&
    /crates\/desktop\/package\.json/.test(setVersion) &&
    !/electrobun/.test(setVersion),
  packaged.version,
);

// ---------- the update the app installs itself ----------

console.log("the installer");
// Each kind of installation is replaced the way it was installed: a macOS bundle
// from the release's disk image, a Linux AppImage by writing the release's own
// file over the one this process runs from, a Windows installation by starting
// the release's setup — which owns the files and waits for this app to be
// closed, so it is the one case the app cannot claim a version for.
const install = between(update, "fn install_downloaded", "/// Copies a downloaded setup program");
check(
  "replaced a copy in place for the two kinds the app owns",
  /Kind::Bundle\(bundle\) => \{\s*install_bundle\(download, bundle, work\)/.test(install) &&
    /Kind::AppImage\(image\) => \{\s*install_appimage\(download, image\)/.test(install) &&
    /matches!\(self\.kind, Kind::Bundle\(_\) \| Kind::AppImage\(_\)\)/.test(update),
  install.replace(/\s+/g, " ").trim().slice(0, 160),
);
check(
  "started the installer the release publishes rather than unpacking an archive",
  /Kind::Installer => \{[\s\S]*?let staged = stage_setup\(download, staging\)\?;[\s\S]*?Command::new\(&staged\)\s*\.spawn\(\)/.test(install) &&
    /pending: true/.test(install) &&
    !/unpack_setup|Command::new\("tar"\)/.test(code(update)),
  install.replace(/\s+/g, " ").trim().slice(0, 200),
);
// The program that is started is a copy in a directory of its own rather than
// the download itself: the run's scratch directory is removed as the install
// returns, so a setup started from there would be reading a file an unrelated
// cleanup is about to delete.
const staging = between(update, "fn stage_setup", "/// Where a downloaded setup is put");
check(
  "staged the setup it starts beside the app's own state",
  /fn stage_setup\(download: &Path, directory: &Path\) -> Result<PathBuf>/.test(staging) &&
    /remove\(directory\)/.test(staging) &&
    /fs::copy\(download, &staged\)/.test(staging) &&
    /from_mode\(0o755\)/.test(staging) &&
    /fn setup_staging\(\) -> PathBuf \{[\s\S]*?registry_path\(\)[\s\S]*?join\("installer"\)/.test(update) &&
    /install_downloaded\([\s\S]{0,160}?&setup_staging\(\)/.test(update),
  staging.replace(/\s+/g, " ").trim().slice(0, 200),
);
// The swap is a copy beside the installation followed by one rename, so the step
// that can fail happens before anything moves: an AppImage written over directly
// is a broken file if the download stops halfway.
const appimage = between(update, "fn install_appimage", "/// Replaces a macOS app bundle");
check(
  "put the new file in place in one step",
  /let fresh = sibling\(image, "new"\)/.test(appimage) &&
    /fs::copy\(download, &fresh\)/.test(appimage) &&
    /fs::rename\(&fresh, image\)/.test(appimage) &&
    /from_mode\(0o755\)/.test(appimage),
  appimage.replace(/\s+/g, " ").trim().slice(0, 160),
);
// A copy the app may not write over — one an administrator installed for every
// user, a distribution's package — is reported as advice rather than replaced,
// and a copy that is not an installation of this app at all is not touched.
check(
  "refused a copy that is not this app's to replace",
  /Kind::None => bail!\(\s*"\{\} cannot be replaced from inside the app"/.test(install) &&
    /replaceable: writable\(&root\)/.test(update) &&
    /fn writable\(/.test(update) &&
    /if !self\.replaceable \{/.test(update) &&
    /if !matches!\(self\.kind, Kind::None\)/.test(update),
);
// An installed copy is told from a checkout's build by what the installer left
// beside the program, and by the executable really being inside the directory:
// another application's uninstaller says nothing about this one.
check(
  "told an installed copy by the uninstaller its own setup left",
  /const INSTALL_MARKERS: \[&str; \d\] = [^;]*"Uninstall Oxide\.exe"/.test(update) &&
    /!executable\.starts_with\(&path\)/.test(update) &&
    /fn install_root_of\(/.test(update) &&
    /dirs::data_local_dir\(\)/.test(update),
);
// Linux is the one platform whose artifact is a file the user keeps: the
// AppImage runtime names the file it was started from in the environment, since
// the binary inside a mounted image is an unpacked copy whose own path says
// nothing about where the image is.
check(
  "found the AppImage this process was started from",
  /env::var_os\("APPIMAGE"\)/.test(update) &&
    /fn installation_of\(/.test(update) &&
    /if os == "macos"/.test(update) &&
    /installation_of\(\s*&executable,\s*std::env::consts::OS,\s*data_root\(\)\.as_deref\(\),\s*appimage\.as_deref\(\),\s*\)/.test(
      update,
    ) &&
    /label: "AppImage"/.test(update) &&
    /asset\.name\.ends_with\("\.AppImage"\)/.test(update),
);
// A launch installs on its own only where the app owns the copy it runs from —
// never a Windows installer, a distribution's package or a checkout's build,
// which the window's dialog offers instead.
check(
  "installed on its own only into a copy this app owns",
  /notice\.is_update_for\(current\) && installation\.replaces_itself\(\)/.test(update) &&
    /fn install_reporting\(/.test(update) &&
    /fn auto_update\(/.test(commands) &&
    /launch_installs\(/.test(commands),
);
// Every step of an install reports itself, and the page paints those stages as
// the dialog's download step: a stage renamed here is one the window never fills
// in, and a stage the window paints that nothing reports is a wait with nothing
// to say. The steps are written two ways — a struct literal for the download,
// which carries its own byte counts, and `Progress::step` for the ones that are
// just a stage — so both spellings are read.
const stages = [
  ...new Set(
    [...update.matchAll(/(?:stage: |Progress::step\()"([a-z]+)"/g)].map(([, name]) => name),
  ),
].sort();
check(
  "reported each step of an install to the window",
  JSON.stringify(stages) === JSON.stringify(["checking", "downloading", "installing", "verifying"]) &&
    /"stage": progress\.stage/.test(commands) &&
    /"received": progress\.received/.test(commands) &&
    /"total": progress\.total/.test(commands) &&
    /announce_launch_update\(\s*"update-progress"/.test(commands) &&
    /announce_launch_update\("update-ready"/.test(commands) &&
    /announce_launch_update\("update-failed"/.test(commands) &&
    /announce_launch_update\(\s*"update-available"/.test(commands) &&
    ["update-available", "update-progress", "update-ready", "update-failed"].every((name) =>
      app.includes(`listen("${name}"`),
    ) &&
    /fn launch_update\(/.test(commands),
  stages.join(", "),
);

// A cancel names the install it is about, and the name is the window's own: the
// reader can ask to stop an install in the moment between the request going out
// and the install starting, so a flag the next install clears would lose that
// request, and a name the two halves agree on is what a cancel is addressed by.
// The engine's own installs carry a name of their own, which the window learns
// from the stage it reports.
check(
  "named the install a cancel is about, both ways round",
  /static CANCEL_TOKEN: Mutex<Option<String>>/.test(update) &&
    /pub fn cancel\(token: &str\)/.test(update) &&
    /fn cancelled\(token: &str\) -> bool/.test(update) &&
    /pub fn new_token\(\) -> String/.test(update) &&
    /install_reporting\(&token, move \|progress\|/.test(commands) &&
    /"token": named/.test(commands) &&
    /install_update\(&state, optional_arg\(&args, "token"\)\?\)/.test(commands) &&
    /cancel_update\(arg\(&args, "token"\)\?\)/.test(commands) &&
    /invoke\("install_update", \{ token: install\.token \}\)/.test(app) &&
    /invoke\("cancel_update", \{ token \}\)/.test(app) &&
    /\(payload && payload\.token\)/.test(app),
  "the cancel's own name",
);

// ---------- the migration's own end ----------

console.log("the migration");
// The window this replaced is gone from the crate, not merely unused: a config
// file for it, a lockfile for a runtime the build no longer needs, or a Rust
// dependency on it is a build that goes on doing two things.
const strays = [
  "electrobun.config.ts",
  "hutch.config.ts",
  "build/dev-macos-arm64",
  "src-tauri",
  "bun.lockb",
  "node_modules/@electrobun",
  "scripts/hutch",
];
check(
  "left nothing of the shell this replaced in the crate",
  strays.every((path) => !exists(path)) &&
    !/electrobun|hutch/i.test(code(read("src/lib.rs")) + code(update) + code(bridge)) &&
    !/electrobun|hutch/i.test(manifest + JSON.stringify(packaged) + read("tsconfig.json")) &&
    !/\bbun\b/i.test(code(read("src/lib.rs")) + code(update)),
  strays.filter((path) => exists(path)).join(", "),
);
// What the crate does carry is the two things a release build needs on disk: a
// bundler that makes the platform's own artifact, and an icon for it.
check(
  "built the app with a bundler and an icon for each platform",
  exists("electron-builder.yml") &&
    exists("icons/icon.icns") &&
    exists("icons/icon.ico") &&
    exists("icons/icon.png") &&
    scalar(builder, "appId") === "dev.oxide.desktop" &&
    scalar(builder, "productName") === "Oxide" &&
    scalar(builder, "output") === "artifacts",
  `${scalar(builder, "appId")} / ${scalar(builder, "productName")}`,
);
// The macOS build is signed and notarized from the release workflow, and the
// hardened runtime it runs under needs the JIT entitlement the engine's own
// JavaScript-free binary does not: an unsigned bundle is one the reader has to
// walk around Gatekeeper to open.
check(
  "kept the hardened runtime's own entitlements",
  /hardenedRuntime: true/.test(block(builder, "mac")) &&
    /entitlements: build\/entitlements\.mac\.plist/.test(block(builder, "mac")) &&
    exists("build/entitlements.mac.plist") &&
    /com\.apple\.security\.cs\.allow-jit/.test(read("build/entitlements.mac.plist")),
);
// The window's build output, the bundler's output and the engine cargo builds
// are all generated: a release that committed one would ship a stale app, and a
// checkout that ignored none of them cannot tell a fresh build from an old one.
check(
  "kept the generated parts of the crate out of the tree",
  ["node_modules", "electron/dist", "artifacts", "target"].every((name) =>
    ignores.includes(`/crates/desktop/${name}/`),
  ) && exists("electron/main.ts") && exists("electron/preload.ts"),
);
// Every file above is read through one normaliser, because `targetsOf` reads a
// line to its end and a Windows checkout — and a Windows CI runner, where
// `core.autocrlf` is on by default — hands these files CRLF, where that parse
// finds no targets at all and the release reads as one nothing builds. That is
// how this file's release check failed on the Windows runner once, so both
// halves are held here: the one reader strips carriage returns, and nothing
// reads a file around it.
const selfSource = readFileSync(fileURLToPath(import.meta.url), "utf8");
check(
  "read every file as a Windows checkout spells it",
  /\.replace\(\/\\r\\n\/g, "\\n"\)/.test(selfSource) &&
    (selfSource.match(/readFileSync\(/g) ?? []).length === 2,
  `${(selfSource.match(/readFileSync\(/g) ?? []).length} read point(s)`,
);

console.log("");
if (failures.length > 0) {
  console.log(`${failures.length} check(s) failed`);
  for (const name of failures) console.log(`  - ${name}`);
  process.exit(1);
}
console.log("the desktop shell is the one the build makes");
