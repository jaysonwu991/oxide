// Copies Monaco's browser runtime into the page's ignored vendor directory.
// The Tauri page has no bundler, so this is the editor's equivalent of
// prepare-diagrams.mjs: the locked package supplies the files, not the repo.
import { cpSync, existsSync, mkdirSync, rmSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const desktop = dirname(here);
const source = join(desktop, "node_modules", "monaco-editor");
const vendor = join(desktop, "ui", "vendor", "monaco");
const target = join(vendor, "vs");

if (!existsSync(join(source, "min", "vs", "loader.js"))) {
  throw new Error("Monaco is not installed; run `pnpm install` in crates/desktop first.");
}

rmSync(target, { recursive: true, force: true });
mkdirSync(target, { recursive: true });
cpSync(join(source, "min", "vs"), target, { recursive: true });
for (const name of ["LICENSE", "ThirdPartyNotices.txt"]) {
  cpSync(join(source, name), join(vendor, name));
}
