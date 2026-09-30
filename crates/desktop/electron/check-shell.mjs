import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const electronDir = path.dirname(fileURLToPath(import.meta.url));
const desktopDir = path.dirname(electronDir);
const read = (relative) => fs.readFileSync(path.join(desktopDir, relative), "utf8");

const main = read("electron/main.cjs");
const preload = read("electron/preload.cjs");
const renderer = read("ui/app.js");
const index = read("ui/index.html");
const manifest = JSON.parse(read("package.json"));
const forge = read("forge.config.cjs");
const cargo = read("Cargo.toml");

function stringSet(source, declaration) {
  const block = source.match(new RegExp(`const ${declaration} = new Set\\(\\[([\\s\\S]*?)\\]\\);`));
  assert.ok(block, `missing ${declaration}`);
  return new Set([...block[1].matchAll(/"([^"]+)"/g)].map((match) => match[1]));
}

assert.equal(manifest.main, "electron/main.cjs");
assert.match(manifest.devDependencies.electron, /^\d+\.\d+\.\d+$/);
assert.match(manifest.devDependencies["@electron-forge/cli"], /^\d+\.\d+\.\d+$/);
assert.match(manifest.packageManager || "", /^pnpm@\d+\.\d+\.\d+$/);

// CI and every clean contributor install from this lockfile, so each tarball has
// to come from the public registry: a mirror baked in by whoever regenerated it
// would make the build depend on a registry nobody else can reach. `.npmrc`
// pins that registry so regenerating the lockfile behind a corporate mirror
// does not quietly reintroduce one, and pnpm writes a `tarball:` URL only when
// it is not that registry, so anything but the public one is a mirror.
assert.equal(fs.existsSync(path.join(desktopDir, "package-lock.json")), false);
assert.match(read(".npmrc"), /^registry=https:\/\/registry\.npmjs\.org\/$/m);
const lockfile = read("pnpm-lock.yaml");
const foreign = [...lockfile.matchAll(/https?:\/\/[^\s,'"}]+/g)]
  .map((match) => match[0])
  .filter((url) => new URL(url).origin !== "https://registry.npmjs.org");
assert.deepEqual(
  foreign,
  [],
  `pnpm-lock.yaml resolves tarballs outside the public registry: ${foreign.join(", ")}`,
);

// pnpm runs no dependency build scripts it was not allowed to, and the Forge
// makers need theirs: Windows' installer maker selects its 7-Zip binary, and the
// macOS DMG maker loads two native modules. Allowed scripts are recorded in
// `pnpm-workspace.yaml`, so dropping an entry there breaks packaging rather than
// the install that follows it. The same file asks for the hoisted node_modules
// layout: Electron Forge loads its makers and the electron binary out of a flat
// tree and refuses to start on pnpm's isolated one, reading the setting back
// through `pnpm config get node-linker`.
const workspace = read("pnpm-workspace.yaml");
assert.match(workspace, /^nodeLinker: hoisted$/m);
for (const allowed of ["electron-winstaller", "fs-xattr", "macos-alias"]) {
  assert.match(
    workspace,
    new RegExp(`^\\s+${allowed}: true$`, "m"),
    `pnpm-workspace.yaml must allow ${allowed} to run its build script`,
  );
}

for (const setting of [
  "contextIsolation: true",
  "sandbox: true",
  "nodeIntegration: false",
  "webSecurity: true",
]) {
  assert.ok(main.includes(setting), `missing secure BrowserWindow setting: ${setting}`);
}
assert.match(main, /function trustedSender\(/);
assert.match(main, /setWindowOpenHandler/);
assert.match(main, /will-navigate/);
assert.match(main, /shell\.openExternal\(webUrl\(/);
// A run can only be watched, answered or stopped from the window that started
// it, so closing the last window ends the app (and with it the host) on every
// platform rather than leaving a turn streaming into nothing on macOS.
assert.match(main, /window-all-closed[\s\S]*?app\.quit\(\)/);
assert.doesNotMatch(main, /platform !== "darwin"/);
assert.doesNotMatch(main, /app\.on\("activate"/);
assert.match(main, /before-quit[\s\S]*?host\?\.close\(\)/);

// The host dying is fatal — nothing can answer a run, an approval or a question
// after it — while a frame it could not parse is not, and the renderer needs the
// difference to know whether to let go of the turn it is showing.
const hostClient = read("electron/host-client.cjs");
assert.match(hostClient, /fatal: true/);
assert.match(hostClient, /fatal: false/);
assert.match(renderer, /payload\.fatal[\s\S]*?hostStopped\(\)/);
assert.match(renderer, /function hostStopped\(/);

assert.match(preload, /contextBridge\.exposeInMainWorld\(/);
assert.doesNotMatch(preload, /exposeInMainWorld\([^,]+,\s*ipcRenderer/);
assert.match(preload, /const COMMANDS = new Set/);
assert.match(preload, /const EVENTS = new Set/);
const preloadCommands = stringSet(preload, "COMMANDS");
const mainCommands = stringSet(main, "HOST_COMMANDS");
mainCommands.add("pick_folder");
mainCommands.add("open_url");
assert.deepEqual(preloadCommands, mainCommands, "preload and main command allowlists diverged");
assert.match(renderer, /window\.__OXIDE__/);
assert.doesNotMatch(renderer, /__TAURI__/);
assert.doesNotMatch(renderer, /controlPress|pressedControl|heldRepaints|first_click/);

assert.match(index, /default-src 'none'/);
assert.match(index, /script-src 'self'/);
assert.match(index, /img-src 'self' data: blob:/);

assert.match(forge, /extraResource: \[host\]/);
for (const maker of ["maker-dmg", "maker-zip", "maker-squirrel", "maker-deb", "maker-rpm"]) {
  assert.ok(forge.includes(maker), `missing ${maker}`);
}
assert.doesNotMatch(cargo, /tauri/i);
assert.equal(fs.existsSync(path.join(desktopDir, "tauri.conf.json")), false);
assert.equal(fs.existsSync(path.join(desktopDir, "capabilities", "default.json")), false);

console.log("Electron shell checks passed");
