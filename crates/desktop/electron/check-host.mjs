import assert from "node:assert/strict";
import path from "node:path";
import { createRequire } from "node:module";
import { fileURLToPath } from "node:url";

const require = createRequire(import.meta.url);
const { RustHost } = require("./host-client.cjs");
const electronDir = path.dirname(fileURLToPath(import.meta.url));
const hostName = process.platform === "win32" ? "oxide-desktop-host.exe" : "oxide-desktop-host";
const hostPath = path.resolve(electronDir, "..", "..", "..", "target", "debug", hostName);
const host = new RustHost(hostPath);

const timeout = setTimeout(() => {
  host.close();
  throw new Error("desktop host protocol check timed out");
}, 15_000);

try {
  const projects = await host.invoke("list_projects");
  assert.ok(Array.isArray(projects), "list_projects should return an array");
  await assert.rejects(host.invoke("not_a_desktop_command"), /unknown desktop command/);
  console.log("Rust host protocol checks passed");
} finally {
  clearTimeout(timeout);
  host.close();
}
