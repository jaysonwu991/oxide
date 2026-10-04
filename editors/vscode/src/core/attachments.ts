// Attachments for the composer: an image, a PDF or a text file a message
// carries.
//
// They travel to the CLI as `--image <path>`, which reads the type from the
// bytes and falls back to the file extension (`oxide_core::media::image_mime_of`
// / `is_pdf_path`, and the convertible-image extensions) — but a pasted
// screenshot only exists as a data URL inside the webview, so the host writes
// it to a temporary file first (`src/attachments.ts`). Everything that does not
// touch the filesystem lives here, where it is unit tested.

import { Buffer } from "node:buffer";

/// At most this many attachments per message, the cap the desktop composer
/// enforces too.
export const MAX_ATTACHMENTS = 8;

/// The longest edge a pasted image is downscaled to before it is written out,
/// matching `oxide_core::media`'s cap (and Anthropic's recommended maximum).
/// The webview does the resizing on a canvas.
export const MAX_IMAGE_EDGE = 1568;

/// The largest attachment that may be sent, matching
/// `oxide_core::media::MAX_ATTACHMENT_BYTES`. The CLI refuses one past this at
/// the far end; the checks here mean an over-large paste is refused before it
/// ever becomes a base64 string, a temp file or a view message.
export const MAX_ATTACHMENT_BYTES = 20 * 1024 * 1024;

/// The largest thumbnail sent to the webview: the chip falls back to a glyph
/// and its size past this, so a huge paste cannot bloat a view message. It has
/// to hold a real photo — a screenshot is megabytes — because the view shrinks
/// what it receives to the few KB it actually paints.
export const MAX_PREVIEW_CHARS = 6_000_000;

export type AttachmentKind = "image" | "pdf" | "text";

/// The extension the CLI recognizes per media MIME type, which is how it reads
/// the type of a file it is handed. Mirrors `media::image_mime_of` and the PDF
/// check: a TIFF, a HEIC and an AVIF are media too — the CLI converts them to a
/// format a provider takes rather than refusing them.
const EXTENSIONS: Record<string, string> = {
  "image/png": "png",
  "image/jpeg": "jpg",
  "image/gif": "gif",
  "image/webp": "webp",
  "image/bmp": "bmp",
  "image/tiff": "tiff",
  "image/heic": "heic",
  "image/heif": "heif",
  "image/avif": "avif",
  "application/pdf": "pdf",
};

/// The image formats this webview can draw, which are the ones a chip shows a
/// thumbnail for.
const PAINTABLE_IMAGE_MIMES = new Set([
  "image/png",
  "image/jpeg",
  "image/gif",
  "image/webp",
  "image/bmp",
]);

/// Text shapes a paste or a drop can carry. The CLI attaches any file that is
/// not media as its own text, so these are written with an extension the file
/// can be named by rather than being refused for having no picture in them.
const TEXT_EXTENSIONS: Record<string, string> = {
  "text/plain": "txt",
  "text/markdown": "md",
  "text/csv": "csv",
  "text/html": "html",
  "text/css": "css",
  "text/xml": "xml",
  "text/yaml": "yaml",
  "text/javascript": "js",
  "application/json": "json",
  "application/xml": "xml",
  "application/javascript": "js",
  "application/x-yaml": "yaml",
  "application/yaml": "yaml",
  "application/toml": "toml",
  "application/csv": "csv",
  "application/sql": "sql",
  "application/x-sh": "sh",
};

/// The reverse of `EXTENSIONS`, with the alternate spelling the CLI accepts.
/// Media only: a text file is inlined as context by the extension itself, so it
/// is not passed as an attachment path.
const MIMES: Record<string, string> = { jpeg: "image/jpeg" };
for (const [mime, extension] of Object.entries(EXTENSIONS)) MIMES[extension] = mime;

/// The MIME type of a data URL, lowercased, or `""` when it is not one.
export function dataUrlMime(dataUrl: string): string {
  const match = /^data:([^;,]*)/.exec(String(dataUrl || ""));
  return match ? match[1].trim().toLowerCase() : "";
}

/// Whether a chip draws the picture itself rather than a glyph. A TIFF or a
/// HEIC is an image the CLI sends but this browser cannot paint.
export function isPaintableImage(mime: string): boolean {
  return PAINTABLE_IMAGE_MIMES.has(mime);
}

/// `image`, `pdf` or `text` for what the CLI can be handed, `null` for a type
/// it has no use for (a video, a tarball).
export function attachmentKind(mime: string): AttachmentKind | null {
  if (mime === "application/pdf") return "pdf";
  if (EXTENSIONS[mime]) return "image";
  return attachmentExtension(mime) ? "text" : null;
}

/// The extension to write a blob with, or `null` for a type the CLI cannot
/// take. A file without a recognized extension is not an attachment as far as
/// the CLI is concerned, so this is also the guard on what may be attached.
export function attachmentExtension(mime: string): string | null {
  if (EXTENSIONS[mime]) return EXTENSIONS[mime];
  if (TEXT_EXTENSIONS[mime]) return TEXT_EXTENSIONS[mime];
  return mime.startsWith("text/") ? "txt" : null;
}

/// Why another attachment cannot join the pending list, or `null` when it can:
/// the cap and the content-address duplicate rule both attachment paths share.
/// The controller checks this before writing a pasted blob, so a rejected paste
/// never leaves a temp file behind.
export function attachmentRejection(
  existing: readonly { key: string }[],
  key: string,
): "cap" | "duplicate" | null {
  if (existing.length >= MAX_ATTACHMENTS) return "cap";
  if (existing.some((entry) => entry.key === key)) return "duplicate";
  return null;
}

/// The extension the CLI would read from a path, or `null` when the path is not
/// media. Mirrors `media::is_attachment_path`, including its tolerance for a
/// Windows separator, and its set: a TIFF or a HEIC is media the CLI converts.
export function attachmentMimeForPath(file: string): string | null {
  const name = String(file || "").replace(/\\/g, "/").split("/").pop() ?? "";
  const dot = name.lastIndexOf(".");
  if (dot <= 0) return null;
  return MIMES[name.slice(dot + 1).toLowerCase()] ?? null;
}

export interface DecodedAttachment {
  mime: string;
  kind: AttachmentKind;
  bytes: Buffer;
}

/// Turns a data URL into the bytes to write, or `null` when it is not an
/// attachable type or the payload is not base64.
export function decodeDataUrl(dataUrl: string): DecodedAttachment | null {
  const mime = dataUrlMime(dataUrl);
  const kind = attachmentKind(mime);
  if (!kind || !attachmentExtension(mime)) return null;
  const comma = String(dataUrl).indexOf(",");
  if (comma === -1) return null;
  if (!/;base64$/i.test(String(dataUrl).slice(0, comma).trim())) return null;
  const payload = String(dataUrl).slice(comma + 1).replace(/\s+/g, "");
  if (!payload || !/^[A-Za-z0-9+/]*={0,2}$/.test(payload)) return null;
  const bytes = Buffer.from(payload, "base64");
  if (!bytes.length) return null;
  return { mime, kind, bytes };
}

/// A safe file name for a blob: the name the clipboard or the drop supplied,
/// with anything that could escape the directory it is written to stripped, and
/// the extension the CLI needs. An empty name falls back to `image`,
/// `document` or `file`.
export function attachmentFileName(name: string, mime: string): string {
  const extension = attachmentExtension(mime) ?? "bin";
  const cleaned = String(name || "")
    .replace(/\\/g, "/")
    .split("/")
    .pop()!
    .replace(/[^A-Za-z0-9._ -]+/g, "-")
    .replace(/^[.\- ]+/, "")
    .trim();
  const fallback =
    attachmentKind(mime) === "image"
      ? "image"
      : attachmentKind(mime) === "pdf"
        ? "document"
        : "file";
  const stem = (cleaned.slice(0, 80) || fallback).replace(/\.[^.]+$/, "");
  return `${stem || fallback}.${extension}`;
}

/// A content address for a pasted blob, so the same screenshot pasted twice is
/// one attachment — the same dedupe the terminal and the desktop do.
export function attachmentId(bytes: Buffer): string {
  let hash = 0x811c9dc5;
  for (const byte of bytes) {
    hash ^= byte;
    hash = Math.imul(hash, 0x01000193) >>> 0;
  }
  return `${bytes.length.toString(16)}-${hash.toString(16)}`;
}

/// A size for a chip's tooltip (`240 KB`, `1.2 MB`).
export function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${Math.round(bytes / 1024)} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}
