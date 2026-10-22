// Copy the shared renderer and the pinned, self-contained Mermaid browser build
// into the front-end's packaged resources. No CDN or runtime network access.
import { copyFile, mkdir } from "node:fs/promises";
import { createRequire } from "node:module";
import { fileURLToPath } from "node:url";
import { join } from "node:path";

const root = fileURLToPath(new URL("../", import.meta.url));
const targets = { desktop: ["crates/desktop", "ui"], vscode: ["editors/vscode", "media"] };
const target = targets[process.argv[2]];
if (!target) throw new Error("Usage: node scripts/prepare-diagrams.mjs desktop|vscode");
const [project, resources] = target;
const require = createRequire(join(root, project, "package.json"));
const output = join(root, project, resources);
await mkdir(join(output, "vendor"), { recursive: true });
for (const file of ["diagrams.js", "diagrams.css"]) await copyFile(join(root, "shared", file), join(output, file));
await copyFile(require.resolve("mermaid/dist/mermaid.min.js"), join(output, "vendor/mermaid.min.js"));
await copyFile(require.resolve("mermaid/LICENSE"), join(output, "vendor/mermaid-LICENSE"));
