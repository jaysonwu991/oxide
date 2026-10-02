// Fetching the release the check resolved, and knowing the download arrived
// whole.
//
// This is the half of the update that runs no `oxide` and no VS Code: an HTTPS
// request, the checksum the release published beside the file, and a private
// directory the file sits in until the editor has read it. It is tested against
// a real socket here rather than a stub, because a redirect GitHub sends and a
// body that does not match its checksum are the two ways this can go wrong
// quietly.

import assert from "node:assert/strict";
import * as crypto from "node:crypto";
import * as fs from "node:fs";
import * as http from "node:http";
import * as os from "node:os";
import * as path from "node:path";
import { after, describe, it } from "node:test";

import { digestHex, downloadUpdate, fileNameOf, removeDownload, sha256, verifyDigest } from "../updates";

/// A server serving one body, with an optional redirect in front of it, so the
/// transfer can be driven without a network.
async function serve(body: Buffer, redirectFrom = ""): Promise<{ url: string; close: () => void }> {
  const server = http.createServer((request, response) => {
    if (redirectFrom && request.url === redirectFrom) {
      response.writeHead(302, { location: "/release.vsix" });
      response.end();
      return;
    }
    if (request.url === "/release.vsix") {
      response.writeHead(200, { "content-type": "application/octet-stream" });
      response.end(body);
      return;
    }
    if (request.url === "/missing.vsix") {
      response.writeHead(404, { "content-type": "text/plain" });
      response.end("Not Found");
      return;
    }
    response.writeHead(500);
    response.end();
  });
  await new Promise<void>((resolve) => server.listen(0, "127.0.0.1", resolve));
  const address = server.address();
  const port = typeof address === "object" && address ? address.port : 0;
  return {
    url: `http://127.0.0.1:${port}`,
    // `closeAllConnections` matters: keep-alive sockets would hold the process
    // open past the test that made them.
    close: () => {
      server.closeAllConnections();
      server.close();
    },
  };
}

const downloads: Array<{ dir: string; file: string; bytes: number }> = [];
after(() => {
  for (const download of downloads) removeDownload(download);
});

const vsix = Buffer.from("PK\u0003\u0004 an extension package, for the purposes of a checksum\n");
const digest = `sha256:${crypto.createHash("sha256").update(vsix).digest("hex")}`;

describe("update download", () => {
  it("fetches the artifact and checks it against the release's digest", async () => {
    const server = await serve(vsix);
    try {
      const download = await downloadUpdate({
        name: "oxide-vscode-0.34.0.vsix",
        url: `${server.url}/release.vsix`,
        digest,
      });
      downloads.push(download);
      assert.equal(download.bytes, vsix.length);
      assert.equal(fs.readFileSync(download.file).toString(), vsix.toString());
      // The file is named what it is, since VS Code installs it from its path.
      assert.equal(path.basename(download.file), "oxide-vscode-0.34.0.vsix");
      // And it sits in a directory of its own that only this user can read: an
      // extension package is not a file to leave in a shared temp directory.
      assert.equal(fs.statSync(download.dir).mode & 0o077, 0);
      assert.equal(fs.statSync(download.file).mode & 0o077, 0);
      removeDownload(download);
      assert.equal(fs.existsSync(download.dir), false, "the download is removed on request");
    } finally {
      server.close();
    }
  });

  it("follows the redirect a release download answers with", async () => {
    // GitHub never serves the bytes itself: the release URL redirects to its
    // object store, and a transfer that does not follow that is an empty file.
    const server = await serve(vsix, "/start.vsix");
    try {
      const download = await downloadUpdate({
        name: "oxide-vscode-0.34.0.vsix",
        url: `${server.url}/start.vsix`,
        digest,
      });
      downloads.push(download);
      assert.equal(download.bytes, vsix.length);
    } finally {
      server.close();
    }
  });

  it("refuses a body that does not match the release's checksum", async () => {
    // A truncated or substituted download is the one thing the checksum is for,
    // and it is refused before VS Code is handed the file.
    const server = await serve(Buffer.from("not the release at all\n"));
    try {
      await assert.rejects(
        downloadUpdate({
          name: "oxide-vscode-0.34.0.vsix",
          url: `${server.url}/release.vsix`,
          digest,
        }),
        /does not match the release's checksum/,
      );
    } finally {
      server.close();
    }
  });

  it("leaves nothing behind when the download fails", async () => {
    const scratchOf = () =>
      new Set(
        fs.readdirSync(os.tmpdir()).filter((entry) => entry.startsWith("oxide-update-")),
      );
    const before = scratchOf();
    const server = await serve(vsix);
    try {
      await assert.rejects(
        downloadUpdate({
          name: "oxide-vscode-0.34.0.vsix",
          url: `${server.url}/missing.vsix`,
          digest,
        }),
        /HTTP 404/,
      );
    } finally {
      server.close();
    }
    const left = [...scratchOf()].filter((entry) => !before.has(entry));
    assert.deepEqual(left, [], "no scratch directory outlives its failed download");
  });

  it("keeps a download a release published no checksum for", async () => {
    // A release that carries no `.sha256` is installed unverified rather than
    // refused, which is what the terminal's own update does with it.
    const server = await serve(vsix);
    try {
      const download = await downloadUpdate({
        name: "oxide-vscode-0.34.0.vsix",
        url: `${server.url}/release.vsix`,
        digest: "",
      });
      downloads.push(download);
      assert.equal(download.bytes, vsix.length);
    } finally {
      server.close();
    }
  });

  it("reads the digest forms GitHub publishes, and no other", () => {
    const hex = "e02cf08397c21fc2b6f35e1d1220ac1e68bc056dbdef2ae7eb181308146fc101";
    assert.equal(digestHex(`sha256:${hex}`), hex);
    assert.equal(digestHex(`SHA256:${hex.toUpperCase()}`), hex);
    assert.equal(digestHex(hex), hex);
    // Anything else is a digest this panel cannot check, so the file is kept
    // unverified rather than refused for a value nobody published.
    assert.equal(digestHex(""), "");
    assert.equal(digestHex("md5:0d7a1e"), "");
    assert.equal(digestHex("sha256:notahex"), "");
  });

  it("hashes a file the way the release's digest does", async () => {
    const dir = fs.mkdtempSync(path.join(os.tmpdir(), "oxide-test-"));
    const file = path.join(dir, "body");
    try {
      fs.writeFileSync(file, "abc");
      assert.equal(
        await sha256(file),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
        "the known SHA-256 of abc",
      );
      await assert.rejects(
        verifyDigest(
          file,
          "sha256:81a863ce5e6e98e67e81580d1ed2e11a7698d27c97e9fa1dffb69af5f940e3be",
        ),
        /does not match/,
      );
      await verifyDigest(file, `sha256:${await sha256(file)}`);
    } finally {
      fs.rmSync(dir, { recursive: true, force: true });
    }
  });

  it("installs from a name VS Code can read, and never from a path", () => {
    assert.equal(fileNameOf("oxide-vscode-0.34.0.vsix"), "oxide-vscode-0.34.0.vsix");
    // A release's file name is a file name: anything that looks like a path, or
    // that is not a VSIX at all, is replaced by a plain one rather than
    // followed out of the directory the download owns.
    assert.equal(fileNameOf("../../etc/passwd"), "oxide-vscode.vsix");
    assert.equal(fileNameOf("oxide-vscode-0.34.0.zip"), "oxide-vscode.vsix");
    assert.equal(fileNameOf(""), "oxide-vscode.vsix");
  });
});
