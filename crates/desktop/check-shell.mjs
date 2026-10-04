// Checks the desktop shell without a window: the command the page reaches, the
// channel it reaches it over, and the configuration the window is built from.
//
//   node crates/desktop/check-shell.mjs
//
// `cargo test` reaches the command layer and `check-app.mjs` drives the page
// against a stubbed bridge, but neither can see whether the two halves still
// agree: a command `ui/app.js` performs that `src/commands.rs` no longer answers
// is a button that fails in the window, and an event the page waits for that
// nothing emits is a turn that never ends. This file is that join — the packet
// one half writes and the other reads — plus the window Electrobun is asked to
// build and the migration's own end: no Tauri, no bundler, no node_modules.
import { readdirSync, readFileSync, statSync } from "node:fs";
import { fileURLToPath } from "node:url";

const here = fileURLToPath(new URL(".", import.meta.url));
const root = fileURLToPath(new URL("../../", import.meta.url));
const read = (path) => readFileSync(`${here}${path}`, "utf8");
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
const commands = read("src/commands.rs");
const shell = read("src/main.rs");
const bridge = read("src/bridge.rs");
const manifest = read("Cargo.toml");
const config = read("electrobun.config.ts");
const hutch = read("hutch.config.ts");

// The build config is TypeScript, and a shell check does not run a bundler to
// read it: an object is found by the name it is given and read to its matching
// brace, so what is asserted here is the shape a build would actually get.
const balanced = (text, open) => {
  let depth = 0;
  for (let index = open; index < text.length; index += 1) {
    if (text[index] === "{") depth += 1;
    else if (text[index] === "}") {
      depth -= 1;
      if (depth === 0) return text.slice(open + 1, index);
    }
  }
  return "";
};
const objectOf = (source, key) => {
  const match = new RegExp(`(?:^|\\n)\\s*${key}:\\s*\\{`).exec(source);
  return match ? balanced(source, match.index + match[0].length - 1) : "";
};
const literalOf = (source, key) => {
  const match = new RegExp(`(?:^|\\n)\\s*${key}:\\s*("[^"]*"|'[^']*'|[\\w.]+)`).exec(source);
  return match ? match[1].replace(/^["']|["']$/g, "") : "";
};
// A negative assertion is about what a file *does*, so it is read with its
// documentation taken off: these files explain what they replaced by name.
const code = (source) => source.replace(/^\s*(\/\/|\/\*|\*).*$/gm, "");

const appConfig = objectOf(config, "app");
const build = objectOf(config, "build");
const rust = objectOf(build, "rust");
const copy = objectOf(build, "copy");
const mac = objectOf(build, "mac");
const runtime = objectOf(config, "runtime");

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
    .match(/\.(?:emit|announce_launch_update)\(\s*"([a-z-]+)"/g)
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

// ---------- the channel between them ----------
//
// One JSON packet travels both ways: the page writes it, the main process reads
// it, and an answer or an announcement comes back over the same shapes. Each
// half is asserted on its own keys below, because a field renamed on one side is
// a window that answers nothing, and nothing else in either suite would notice.

console.log("the channel the page reaches");
// The page's one call to the host, read out of the page: a request that carries
// the command's name and arguments under the envelope the main process reads.
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

// The packet the page writes and the packet the main process reads are one
// packet: these are the paths both halves name.
check(
  "reads the packet the page writes",
  /packet\["type"\] != "request"/.test(shell) &&
    /packet\["id"\]\.as_u64\(\)/.test(shell) &&
    /packet\["params"\]\["command"\]/.test(shell) &&
    /packet\["params"\]\["args"\]/.test(shell),
  "",
);
check(
  "reads it off the channel the core queues for this process",
  /pop_next_queued_host_message_string\(\)/.test(shell) &&
    /spawn_host_message_drain/.test(shell) &&
    /event_bridge: Some\(event_bridge_message\)/.test(shell),
);
check(
  "hands the request to the command layer",
  /use commands::\{dispatch, DesktopState\}/.test(shell) &&
    /let answer = dispatch\(Arc::clone\(&state\), &host, &command, args\)\.await;/.test(shell),
);
check(
  "carries the app's state into the window it opened",
  /let webview_id = match core\.create_webview\(webview\)/.test(shell) &&
    /Host::new\(core, webview_id\)/.test(shell) &&
    /STATE\.set\(/.test(code(shell)) &&
    /DesktopManager::load_lossy/.test(shell),
);
check(
  "ends with the window it opened",
  /fn window_closed/.test(shell) && /stop_event_loop\(\)/.test(shell),
);

// An answer is the page's own promise: the id it sent, whether it worked, and
// either the value or the reason. The event channel is the same envelope with an
// event name where the id goes.
check(
  "answered the packet on the same channel",
  /send_host_message_to_webview_json/.test(bridge) &&
    /"type": "response", "id": id, "success": true, "payload": payload/.test(bridge) &&
    /"type": "response", "id": id, "success": false, "error": error/.test(bridge) &&
    /packet\.type === "response"/.test(app) &&
    /packet\.success/.test(app),
);
check(
  "announced an event as its own packet",
  /"type": "message", "id": event, "payload": payload/.test(bridge) &&
    /packet\.type === "message"/.test(app) &&
    /listeners\.get\(packet\.id\)/.test(app),
);
// What the host opens the page with is what the page has to be able to reach:
// the preload's own fallback evaluates the page's receiver by name, so the name
// it installs is the one the two halves have to agree on.
check(
  "answers over the name the preload's fallback calls",
  /window\.__electrobun\.receiveMessageFromHost = receiveMessageFromHost/.test(app) &&
    /window\.__electrobun\.receiveMessageFromBun = receiveMessageFromHost/.test(app),
  "",
);

check(
  "opens the folder chooser the app's own window provides",
  /OpenFileDialogOptions/.test(bridge) &&
    /can_choose_directory: true/.test(bridge) &&
    /host\.pick_folder\(\)\.await/.test(commands) &&
    !/osascript|zenity/.test(code(bridge)),
  "",
);
// One call into the host, which asks the core to pick the platform's handler:
// a URL reaches that handler as its own argument, so no branch here is one
// platform's and no shell reads the URL as text.
const openUrl = commands.slice(commands.indexOf("pub fn open_url"), commands.indexOf("pub async fn dispatch"));
check(
  "handed a link to the machine's own handler rather than a program chosen per target",
  /host\.open_url\(url\)/.test(openUrl) &&
    !/#\[cfg\(/.test(code(openUrl)) &&
    /is_openable_url\(url\)/.test(bridge) &&
    /open_external\(url\)/.test(bridge),
  openUrl.replace(/\s+/g, " ").trim(),
);

// ---------- the window the page runs in ----------

console.log("the window");
check(
  "runs this package as the app's own main process",
  literalOf(build, "mainProcess") === "rust" &&
    literalOf(rust, "manifest") === "Cargo.toml" &&
    literalOf(rust, "binary") === "oxide-desktop",
  `${literalOf(build, "mainProcess")} / ${literalOf(rust, "manifest")} / ${literalOf(rust, "binary")}`,
);
// One section of the manifest, so a name is asserted where it is declared
// rather than anywhere the file happens to mention it.
const section = (source, header) => {
  const start = source.indexOf(header);
  if (start < 0) return "";
  const end = source.indexOf("\n[", start);
  return source.slice(start, end < 0 ? source.length : end);
};
const binarySection = section(manifest, "[[bin]]");
check(
  "builds the binary this package's manifest declares",
  new RegExp(`name = "${literalOf(rust, "binary")}"`).test(binarySection),
  binarySection.replace(/\s+/g, " ").trim(),
);
// The page is copied where the window loads it from: the view name in the
// `views://` URL and the folder the copy destinations share are one name.
const loadUrl = /"views:\/\/([^/]+)\/([^"]+)"/.exec(shell);
const destinations = [...copy.matchAll(/^\s*"([^"]+)":\s*"([^"]+)"/gm)].map(([, from, to]) => ({
  from,
  to,
}));
check(
  "loads the page out of the folder the build copies it into",
  Boolean(loadUrl) &&
    destinations.length > 0 &&
    destinations.every(({ to }) => to.startsWith(`views/${loadUrl[1]}/`)) &&
    destinations.some(({ to }) => to === `views/${loadUrl[1]}/${loadUrl[2]}`) &&
    destinations.every(({ from }) => exists(from)),
  `${loadUrl?.[0]} / ${destinations.map(({ to }) => to).join(", ")}`,
);
check(
  "copies it and everything it loads",
  /<script src="app\.js"><\/script>/.test(html) &&
    /href="style\.css"/.test(html) &&
    destinations.some(({ to }) => to.endsWith("/app.js")) &&
    destinations.some(({ to }) => to.endsWith("/style.css")),
  "",
);
check(
  "gives the policy to the page that carries it rather than a window it is injected into",
  /http-equiv="Content-Security-Policy"/.test(html) &&
    /content="default-src 'none'; script-src 'self'/.test(html) &&
    /style-src 'self' 'unsafe-inline'/.test(html) &&
    /connect-src 'none'/.test(html) &&
    !config.includes("csp"),
  "",
);
check(
  "keeps the page's markup free of inline script",
  !/<script(?![^>]*\bsrc=)[^>]*>/.test(html) && !/\son[a-z]+=/i.test(html),
);
check(
  "ends the process with the window it opened",
  literalOf(runtime, "exitOnLastWindowClosed") === "true",
  literalOf(runtime, "exitOnLastWindowClosed"),
);

// ---------- the bundle and the build that produces it ----------

console.log("the bundle");
const iconset = literalOf(mac, "icons");
const iconsetFiles = iconset && exists(iconset) ? readdirSync(`${here}${iconset}`).sort() : [];
check(
  "bundles the icons the build names",
  iconsetFiles.length === 10 &&
    ["icon_16x16.png", "icon_128x128@2x.png", "icon_512x512@2x.png"].every((name) =>
      iconsetFiles.includes(name),
    ) &&
    exists(literalOf(objectOf(build, "win"), "icon")) &&
    exists(literalOf(objectOf(build, "linux"), "icon")),
  `${iconsetFiles.join(", ")}`,
);
// Signing is the release pipeline's to decide: the two switches follow an
// identity and notary credentials named in the environment, so a local build
// stays unsigned and a released bundle is signed and notarized. The
// entitlements are a record in the config rather than a file beside it.
check(
  "signs and notarizes only when the pipeline names the credentials",
  /codesign: identity !== ""/.test(mac) &&
    /notarize: notary/.test(mac) &&
    /env\.ELECTROBUN_DEVELOPER_ID/.test(config) &&
    /env\.ELECTROBUN_APPLEAPIKEY/.test(config) &&
    !exists("entitlements.plist"),
  "",
);
// The entitlements are a record in the config rather than a file beside it: a
// JIT-ing webview needs them, and a release build that names a missing path
// fails at the very end of a bundle.
check(
  "entitles the bundle from the config it is built with",
  /"com.apple.security.cs.allow-jit": true/.test(objectOf(mac, "entitlements")) &&
    /"com.apple.security.network.client": true/.test(objectOf(mac, "entitlements")),
  "",
);
// The app compares a release against the version it was built with, and the
// bundler writes the one in this config: two halves of one number, both written
// by the release's own script.
const packageVersion = /\[package\][\s\S]*?^version = "([^"]+)"/m.exec(manifest)?.[1];
const setVersion = readFileSync(`${root}scripts/set-version.sh`, "utf8");
check(
  "reports the version the app compares a release against",
  literalOf(appConfig, "version") === packageVersion &&
    /desktop_manifest = f"\{root\}\/crates\/desktop\/Cargo\.toml"/.test(setVersion) &&
    setVersion.includes('version:\\s*")[^\"]*(\")'),
  `${literalOf(appConfig, "version")} / ${packageVersion}`,
);

console.log("the build");
// The devkit is projected into `.hutch/` by Hutch when it prepares this
// project, so the path dependency, the tsconfig and the pinned release all name
// the same directory: a build here is the SDK the hutch config asked for.
check(
  "links the SDK the devkit projects",
  new RegExp(`electrobun = \\{ path = "\\.hutch/devkit/rust-sdk"`).test(manifest) &&
    /"\.\/\.hutch\/devkit\/tsconfig\.json"/.test(read("tsconfig.json")) &&
    /electrobun: \{ version: "\d+\.\d+\.\d+" \}/.test(hutch),
  "",
);
// Hutch builds this package with its own Cargo invocation — the manifest and
// binary `electrobun.config.ts` names — so the SDK is an ordinary dependency of
// that binary rather than one behind a feature: a binary gated behind a feature
// Hutch does not pass is one Hutch's build would not produce.
check(
  "declares the SDK the binary Hutch builds from",
  /^electrobun = \{ path = "\.hutch\/devkit\/rust-sdk" \}$/m.test(manifest) &&
    !/required-features/.test(manifest) &&
    !/^gui = /m.test(manifest),
  `${binarySection.replace(/\s+/g, " ").trim()} / ${/^electrobun = .*$/m.exec(manifest)?.[0] ?? ""}`,
);
// The SDK is a path dependency inside this package's own directory, so Cargo
// would otherwise adopt it as a member and lint generated code with `-D
// warnings`; the package leaving the root workspace is what keeps every other
// cargo command in the repository working while `.hutch/` is unsynced.
check(
  "keeps the vendored SDK out of the workspace's own build",
  /\[workspace\][\s\S]*?exclude = \[".hutch"\]/.test(manifest) &&
    /exclude = \[[\s\S]{0,80}"crates\/desktop"/.test(readFileSync(`${root}Cargo.toml`, "utf8")),
  "",
);

// ---------- the menu bar ----------

console.log("the menu bar");
// The application menu is handed to Electrobun as raw JSON rather than through
// the SDK's typed builder, and its native side reads a missing `enabled` as
// `false`: an item spelled without it is drawn greyed out with nothing to
// click, and a disabled top-level item takes the whole submenu under it with it
// — the update item answered nothing until every item carried the field. Every
// item the menu is written from is therefore read back here, so one added
// without it fails in this check instead of in a reader's menu bar.
const menu = shell.slice(shell.indexOf("fn menu_json"), shell.indexOf("fn cstr"));
const menuItems = [...menu.matchAll(/\{([^{}]*)\}/g)].map(([, body]) => body.replace(/\s+/g, " ").trim());
const labelled = menuItems.filter((body) => body.includes('"label"'));
check(
  "spells every menu item enabled",
  labelled.length >= 10 && labelled.every((body) => body.includes('"enabled": true')),
  labelled.filter((body) => !body.includes('"enabled": true')).join(" | "),
);
// The three menus that carry a submenu are written one field per line, so they
// are held to the shape that names them and enables them in that order.
const submenus = [...menu.matchAll(/\{\s*"label": "([^"]+)",\s*"enabled": true,\s*"submenu":/g)].map(
  ([, label]) => label,
);
check(
  "offers the app, Edit and Window menus",
  submenus.join(", ") === "Oxide, Edit, Window",
  submenus.join(", "),
);
// An item that carries both a role and an action loses the action: the native
// side replaces the click selector with the role's own behaviour, so the menu
// item that was meant to announce something would silently do the role instead.
const dispatched = menuItems.filter((body) => body.includes('"action"'));
check(
  "leaves an action item to announce itself",
  dispatched.length > 0 && dispatched.every((body) => !body.includes('"role"')),
  dispatched.filter((body) => body.includes('"role"')).join(" | "),
);
// The menu is built against `NSApp`, which exists only once the native event
// loop is running: installing it from `run()` before `run_main_thread` is a null
// dereference at launch, so the install belongs to the window path that follows
// it.
check(
  "builds the menu bar once the application exists",
  code(shell).indexOf("set_application_menu_json") > code(shell).indexOf("fn create_window") &&
    code(shell).indexOf("fn create_window") > code(shell).indexOf("run_main_thread"),
  "",
);

// ---------- nothing left of the shell this replaced ----------

console.log("the migration");
const files = [];
const walk = (dir) => {
  for (const entry of readdirSync(`${here}${dir}`, { withFileTypes: true })) {
    const path = `${dir}${entry.name}`;
    if (entry.isDirectory()) {
      if (
        !["target", "gen", "node_modules", ".git", ".hutch", "build", "artifacts", ".cottontail-tmp"].includes(
          entry.name,
        )
      )
        walk(`${path}/`);
      continue;
    }
    if (/\.(js|cjs|mjs|ts)$/.test(entry.name)) files.push(path);
  }
};
walk("");
// The front-end is one plain script — no bundler, no preload, no imports to
// resolve — and the only other sources in the package are the two the build is
// configured by and the tsconfig, which is JSON and only points at the devkit's
// own.
const sources = [
  "check-app.mjs",
  "check-shell.mjs",
  "electrobun.config.ts",
  "hutch.config.ts",
  "tsconfig.json",
  "ui/app.js",
];
check(
  "keeps the front-end the only JavaScript, beside its own checks and build config",
  JSON.stringify(files.sort()) === JSON.stringify(sources.filter((path) => path !== "tsconfig.json").sort()),
  files.join(", "),
);
check(
  "left no packaging of the shell this replaced",
  ["tauri.conf.json", "capabilities", "build.rs", "gen", "package.json", "bun.lockb", "node_modules"].every(
    (path) => !exists(path),
  ) && !/tauri/i.test(manifest),
);
check(
  "keeps the build's own output out of the repository",
  [
    "crates/desktop/.hutch/",
    "crates/desktop/build/",
    "crates/desktop/artifacts/",
    "crates/desktop/.cottontail-tmp/",
  ].every((path) => readFileSync(`${root}.gitignore`, "utf8").includes(path)),
);

console.log(failures.length ? `\n${failures.length} failed` : "\nall checks passed");
process.exit(failures.length ? 1 : 0);
