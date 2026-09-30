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
