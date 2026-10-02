// Fetching the release the check resolved, and knowing the download arrived
// whole.
//
// Which file belongs to this machine is `oxide_core::updates`'s answer, read
// through the CLI; what is left here is the transfer — GitHub's redirect to its
// object store, the checksum the release published beside the file, and a
// private directory to hold the file in until VS Code has read it. This module
// is deliberately free of `vscode`, so `node --test` can run it.
import * as crypto from "node:crypto";
import * as fs from "node:fs";
import * as http from "node:http";
import * as https from "node:https";
import * as os from "node:os";
import * as path from "node:path";
import type { UpdateAsset } from "./core/updates";

/// GitHub answers a release download with a redirect to its object store, which
/// can redirect again; more hops than this is a loop rather than a download.
const MAX_REDIRECTS = 5;

export interface Download {
  /// The private directory holding the file, removed by `removeDownload`.
  dir: string;
  /// The file itself, named so VS Code installs it from its own path.
  file: string;
  bytes: number;
}

/// Fetches the asset into a private directory of its own and verifies it
/// against the digest the release published. The directory is the caller's to
/// remove, since an installed VSIX has to stay on disk until VS Code has read
/// it; a download that fails is cleaned up here rather than left behind.
export async function downloadUpdate(asset: UpdateAsset): Promise<Download> {
  const url = new URL(asset.url);
  if (url.protocol !== "https:" && url.protocol !== "http:") {
    throw new Error(`Refusing to download ${asset.name} over ${url.protocol}`);
  }
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "oxide-update-"));
  fs.chmodSync(dir, 0o700);
  const file = path.join(dir, fileNameOf(asset.name));
  try {
    const bytes = await transfer(url, file, MAX_REDIRECTS);
    await verifyDigest(file, asset.digest);
    return { dir, file, bytes };
  } catch (error) {
    removeDownload({ dir, file, bytes: 0 });
    throw error;
  }
}

/// Removes the directory a download was written into. Best effort: a file
/// another process still holds is not something to fail an install over.
export function removeDownload(download: Download): void {
  try {
    fs.rmSync(download.dir, { recursive: true, force: true });
  } catch {
    /* the temp directory is the OS's to sweep up */
  }
}

/// Checks a downloaded file against the digest the release published
/// (`sha256:<hex>`, the form GitHub reports). A release that published none is
/// kept unverified, which is what the terminal's own update does with it.
export async function verifyDigest(file: string, digest: string): Promise<void> {
  const expected = digestHex(digest);
  if (!expected) return;
  const actual = await sha256(file);
  if (actual !== expected) {
    throw new Error(
      `The download does not match the release's checksum (expected ${expected}, got ${actual})`,
    );
  }
}

/// The hex digest of a `sha256:<hex>` value, or `""` for anything else — an
/// absent digest, or one in a form this panel cannot check.
export function digestHex(digest: string): string {
  const hex = digest.trim().replace(/^sha256:/i, "").toLowerCase();
  return /^[0-9a-f]{64}$/.test(hex) ? hex : "";
}

/// The SHA-256 of a file, streamed so a large download is not read into memory
/// twice.
export function sha256(file: string): Promise<string> {
  return new Promise((resolve, reject) => {
    const hash = crypto.createHash("sha256");
    const stream = fs.createReadStream(file);
    stream.on("data", (chunk) => hash.update(chunk));
    stream.on("error", reject);
    stream.on("end", () => resolve(hash.digest("hex")));
  });
}

/// Writes `url` to `file`, following the redirects GitHub's download URLs use.
function transfer(url: URL, file: string, redirects: number): Promise<number> {
  return new Promise((resolve, reject) => {
    const client = url.protocol === "https:" ? https : http;
    const request = client.get(
      url,
      {
        headers: {
          // GitHub refuses a request without one, and a release download is a
          // file rather than a page.
          "user-agent": "oxide-vscode",
          accept: "application/octet-stream",
        },
      },
      (response) => {
        const status = response.statusCode ?? 0;
        const location = response.headers.location;
        if (status >= 300 && status < 400 && location) {
          response.resume();
          if (redirects <= 0) {
            reject(new Error(`Too many redirects fetching ${url.href}`));
            return;
          }
          transfer(new URL(location, url), file, redirects - 1).then(resolve, reject);
          return;
        }
        if (status !== 200) {
          response.resume();
          const reason = response.statusMessage || "no reason given";
          reject(new Error(`HTTP ${status} ${reason} fetching ${url.href}`));
          return;
        }
        const sink = fs.createWriteStream(file, { mode: 0o600 });
        let bytes = 0;
        response.on("data", (chunk: Buffer) => {
          bytes += chunk.length;
        });
        response.on("error", reject);
        sink.on("error", reject);
        sink.on("finish", () => sink.close(() => resolve(bytes)));
        response.pipe(sink);
      },
    );
    request.on("error", reject);
  });
}

/// A name VS Code will install from: the release's own file name, or a plain
/// one when what the release announced is not a VSIX name — the check already
/// decided the artifact is one, and the name only has to keep the file it
/// installs recognizable.
export function fileNameOf(name: string): string {
  const base = path.basename(name || "");
  return /^[\w][\w.-]*\.vsix$/i.test(base) ? base : "oxide-vscode.vsix";
}
