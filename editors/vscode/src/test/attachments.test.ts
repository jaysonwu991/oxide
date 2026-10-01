// Attachment plumbing: what a pasted blob is, what it may be written as, and
// what the CLI will recognize once it is on disk. The CLI decides an
// attachment's type from the file extension (`oxide_core::media::image_mime`),
// so these shapes have to agree with it.

import assert from "node:assert/strict";
import { Buffer } from "node:buffer";
import * as fs from "node:fs";
import * as os from "node:os";
import * as path from "node:path";
import { describe, it } from "node:test";

import { AttachmentStore, previewForFile } from "../attachments";
import {
  attachmentExtension,
  attachmentFileName,
  attachmentId,
  attachmentKind,
  attachmentMimeForPath,
  attachmentRejection,
  dataUrlMime,
  decodeDataUrl,
  formatBytes,
  MAX_ATTACHMENT_BYTES,
  MAX_ATTACHMENTS,
  MAX_PREVIEW_CHARS,
} from "../core/attachments";

const PNG = "data:image/png;base64,iVBORw0KGgo=";

/// A file on disk, in a directory of its own so one test's sizes and mtimes
/// cannot be another's.
function tempFile(name: string, contents: Buffer): { dir: string; file: string } {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "oxide-vscode-preview-"));
  const file = path.join(dir, name);
  fs.writeFileSync(file, contents);
  return { dir, file };
}

const dataUrlFor = (contents: Buffer, mime = "image/png") =>
  `data:${mime};base64,${contents.toString("base64")}`;

describe("dataUrlMime", () => {
  it("reads the MIME type of a data URL", () => {
    assert.equal(dataUrlMime("data:image/png;base64,AAAA"), "image/png");
    assert.equal(dataUrlMime("data:APPLICATION/PDF;base64,AAAA"), "application/pdf");
    assert.equal(dataUrlMime("data:text/plain,hello"), "text/plain");
  });

  it("reads anything that is not a data URL as no type at all", () => {
    assert.equal(dataUrlMime(""), "");
    assert.equal(dataUrlMime("not-a-data-url"), "");
    assert.equal(dataUrlMime(undefined as unknown as string), "");
    assert.equal(dataUrlMime("data:text/plain,hello"), "text/plain");
  });
});

describe("attachment kinds", () => {
  it("accepts the rich-media formats Oxide can send", () => {
    assert.equal(attachmentKind("image/png"), "image");
    assert.equal(attachmentKind("image/jpeg"), "image");
    assert.equal(attachmentKind("image/bmp"), "image");
    assert.equal(attachmentKind("application/pdf"), "pdf");
  });

  it("refuses anything a provider cannot read as media", () => {
    assert.equal(attachmentKind("text/plain"), null);
    assert.equal(attachmentKind("application/octet-stream"), null);
    assert.equal(attachmentKind(""), null);
  });

  it("maps a type onto the extension the CLI sniffs", () => {
    assert.equal(attachmentExtension("image/png"), "png");
    assert.equal(attachmentExtension("image/jpeg"), "jpg");
    assert.equal(attachmentExtension("application/pdf"), "pdf");
    // A type without a recognized extension is not an attachment the CLI reads,
    // however image-like it looks.
    assert.equal(attachmentExtension("image/svg+xml"), null);
    assert.equal(attachmentExtension("image/tiff"), null);
  });
});

describe("attachmentMimeForPath", () => {
  it("reads the type the CLI would read from the extension", () => {
    assert.equal(attachmentMimeForPath("shot.PNG"), "image/png");
    assert.equal(attachmentMimeForPath("docs/spec.pdf"), "application/pdf");
    assert.equal(attachmentMimeForPath("photo.jpeg"), "image/jpeg");
    assert.equal(attachmentMimeForPath("C:\\Users\\me\\a.jpg"), "image/jpeg");
  });

  it("treats a source file, a dotfile and an extensionless name as no attachment", () => {
    assert.equal(attachmentMimeForPath("src/main.rs"), null);
    assert.equal(attachmentMimeForPath("Makefile"), null);
    assert.equal(attachmentMimeForPath(".png"), null);
    assert.equal(attachmentMimeForPath(""), null);
  });
});

describe("decodeDataUrl", () => {
  it("decodes a base64 attachment payload", () => {
    const decoded = decodeDataUrl(PNG);
    assert.ok(decoded);
    assert.equal(decoded.mime, "image/png");
    assert.equal(decoded.kind, "image");
    assert.equal(decoded.bytes.toString("base64"), "iVBORw0KGgo=");
  });

  it("tolerates whitespace inside the payload", () => {
    const decoded = decodeDataUrl("data:application/pdf;base64,AA\nBB");
    assert.ok(decoded);
    assert.equal(decoded.bytes.toString("base64"), "AABB");
  });

  it("refuses a type it cannot attach", () => {
    assert.equal(decodeDataUrl("data:text/plain;base64,aGk="), null);
    assert.equal(decodeDataUrl("data:image/tiff;base64,aGk="), null);
  });

  it("refuses a payload that is not base64", () => {
    assert.equal(decodeDataUrl("data:image/png,notbase64"), null);
    assert.equal(decodeDataUrl("data:image/png;base64,<script>"), null);
    assert.equal(decodeDataUrl("data:image/png;base64,"), null);
    assert.equal(decodeDataUrl("nonsense"), null);
  });
});

describe("attachmentFileName", () => {
  it("keeps the name and gives it the extension the CLI needs", () => {
    assert.equal(attachmentFileName("shot.png", "image/png"), "shot.png");
    assert.equal(attachmentFileName("shot.jpeg", "image/jpeg"), "shot.jpg");
    assert.equal(attachmentFileName("shot", "image/png"), "shot.png");
    assert.equal(attachmentFileName("report", "application/pdf"), "report.pdf");
  });

  it("names a nameless clipboard blob after its type", () => {
    assert.equal(attachmentFileName("", "image/png"), "image.png");
    assert.equal(attachmentFileName("", "application/pdf"), "document.pdf");
  });

  it("cannot escape the directory it is written to", () => {
    assert.equal(attachmentFileName("../../etc/passwd", "image/png"), "passwd.png");
    assert.equal(attachmentFileName("a/b\\c.png", "image/png"), "c.png");
    assert.equal(attachmentFileName(".hidden", "image/png"), "hidden.png");
    assert.equal(attachmentFileName("a\u0000b.png", "image/png"), "a-b.png");
  });

  it("keeps a readable name short", () => {
    const long = `${"x".repeat(300)}.png`;
    const name = attachmentFileName(long, "image/png");
    assert.equal(name.length, 84);
    assert.ok(name.endsWith(".png"));
  });
});

describe("attachmentId", () => {
  it("is stable for the same bytes and different for other ones", () => {
    const first = attachmentId(Buffer.from("hello"));
    assert.equal(attachmentId(Buffer.from("hello")), first);
    assert.notEqual(attachmentId(Buffer.from("hellp")), first);
    assert.notEqual(attachmentId(Buffer.from("hello!")), first);
  });
});

describe("attachmentRejection", () => {
  it("refuses a ninth attachment", () => {
    const full = Array.from({ length: MAX_ATTACHMENTS }, (_, i) => ({ key: `k${i}` }));
    assert.equal(attachmentRejection(full, "new"), "cap");
  });

  it("refuses a duplicate content address", () => {
    assert.equal(attachmentRejection([{ key: "abc" }], "abc"), "duplicate");
  });

  it("allows a new attachment below the cap", () => {
    assert.equal(attachmentRejection([], "abc"), null);
    assert.equal(attachmentRejection([{ key: "abc" }], "def"), null);
  });
});

describe("formatBytes", () => {
  it("reads as a size a tooltip can show", () => {
    assert.equal(formatBytes(512), "512 B");
    assert.equal(formatBytes(2048), "2 KB");
    assert.equal(formatBytes(1_500_000), "1.4 MB");
  });

  it("keeps a thumbnail to what one view message may carry", () => {
    assert.ok(MAX_PREVIEW_CHARS > 1_000_000, "a screenshot is not too large to preview");
    assert.ok(MAX_PREVIEW_CHARS <= MAX_ATTACHMENT_BYTES, "a thumbnail is smaller than an attachment");
  });
});

describe("previewForFile", () => {
  it("reads a file into a data URL", () => {
    const { dir, file } = tempFile("shot.png", Buffer.from("ok"));
    assert.equal(previewForFile(file, "image/png"), dataUrlFor(Buffer.from("ok")));
    fs.rmSync(dir, { recursive: true, force: true });
  });

  it("answers a file it has already read without reading it again", () => {
    const { dir, file } = tempFile("shot.png", Buffer.from("ok"));
    const first = previewForFile(file, "image/png");
    // A chip rides along with every state message, so the read has to happen
    // once: a file that cannot be read any more still answers from the cache.
    fs.chmodSync(file, 0o000);
    assert.equal(previewForFile(file, "image/png"), first);
    fs.chmodSync(file, 0o644);
    fs.rmSync(dir, { recursive: true, force: true });
  });

  it("reads a file that changed since the last look", () => {
    const { dir, file } = tempFile("shot.png", Buffer.from("ok"));
    previewForFile(file, "image/png");
    fs.writeFileSync(file, Buffer.from("changed"));
    assert.equal(previewForFile(file, "image/png"), dataUrlFor(Buffer.from("changed")));
    fs.rmSync(dir, { recursive: true, force: true });
  });

  it("gives a file too large for a view message no thumbnail", () => {
    const { dir, file } = tempFile("huge.png", Buffer.alloc(MAX_PREVIEW_CHARS / 2 + 1));
    assert.equal(previewForFile(file, "image/png"), null);
    fs.rmSync(dir, { recursive: true, force: true });
  });

  it("reports a file it cannot read as no thumbnail", () => {
    assert.equal(previewForFile(path.join(os.tmpdir(), "oxide-missing.png"), "image/png"), null);
  });
});

describe("AttachmentStore", () => {
  it("writes a blob to a file the CLI can read", () => {
    const store = new AttachmentStore();
    const written = store.write("shot.png", PNG);
    assert.ok(written, "the blob was written");
    assert.equal(path.extname(written.path), ".png");
    assert.equal(fs.readFileSync(written.path).toString("base64"), "iVBORw0KGgo=");
    store.dispose();
  });

  it("refuses a blob past the attachment limit", () => {
    const store = new AttachmentStore();
    const payload = Buffer.alloc(MAX_ATTACHMENT_BYTES + 1);
    assert.equal(store.write("huge.png", dataUrlFor(payload)), null);
    store.dispose();
  });
});
