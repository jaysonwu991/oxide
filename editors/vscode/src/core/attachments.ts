// Attachments for the composer: an image or a PDF a message carries.
//
// They travel to the CLI as `--image <path>`, which sniffs the type from the
// file extension (`oxide_core::media::image_mime` / `is_pdf_path`) — but a
// pasted screenshot only exists as a data URL inside the webview, so the host
// writes it to a temporary file first (`src/attachments.ts`). Everything that
// does not touch the filesystem lives here, where it is unit tested.

import { Buffer } from "node:buffer";

/// At most this many attachments per message, the cap the desktop composer
/// enforces too.
export const MAX_ATTACHMENTS = 8;

/// The longest edge a pasted image is downscaled to before it is written out,
/// matching `oxide_core::media`'s cap (and Anthropic's recommended maximum).
/// The webview does the resizing on a canvas.
export const MAX_IMAGE_EDGE = 1568;

/// Above this, a thumbnail is not sent to the webview: the chip falls back to a
/// glyph and its size, so a huge paste cannot bloat a footer message.
export const MAX_PREVIEW_CHARS = 400_000;

export type AttachmentKind = "image" | "pdf";

/// The extension the CLI recognizes per MIME type, which is how it decides an
/// attachment's type. Mirrors `media::image_mime` plus the `pdf` check.
const EXTENSIONS: Record<string, string> = {
  "image/png": "png",
  "image/jpeg": "jpg",
  "image/gif": "gif",
  "image/webp": "webp",
  "image/bmp": "bmp",
  "application/pdf": "pdf",
};

/// The reverse of `EXTENSIONS`, with the alternate spelling the CLI accepts.
const MIMES: Record<string, string> = { jpeg: "image/jpeg" };
for (const [mime, extension] of Object.entries(EXTENSIONS)) MIMES[extension] = mime;

/// The MIME type of a data URL, lowercased, or `""` when it is not one.
export function dataUrlMime(dataUrl: string): string {
  const match = /^data:([^;,]*)/.exec(String(dataUrl || ""));
  return match ? match[1].trim().toLowerCase() : "";
}

/// `image` or `pdf` for the types a provider accepts, `null` for anything it
/// cannot take (text, a tarball, a video).
export function attachmentKind(mime: string): AttachmentKind | null {
  if (mime === "application/pdf") return "pdf";
  return mime.startsWith("image/") ? "image" : null;
}

/// The extension to write a blob with, or `null` for a type the provider cannot
/// take. A file without a recognized extension is not an attachment at all as
/// far as the CLI is concerned, so this is also the guard on what may be
/// attached.
export function attachmentExtension(mime: string): string | null {
  return EXTENSIONS[mime] ?? null;
}

/// The MIME type the CLI would read from a path, or `null` when the path is not
/// an attachment. Mirrors `media::is_attachment_path`, including its tolerance
/// for a Windows separator.
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
/// the extension the CLI needs. An empty name falls back to `image`/`document`.
export function attachmentFileName(name: string, mime: string): string {
  const extension = attachmentExtension(mime) ?? "bin";
  const cleaned = String(name || "")
    .replace(/\\/g, "/")
    .split("/")
    .pop()!
    .replace(/[^A-Za-z0-9._ -]+/g, "-")
    .replace(/^[.\- ]+/, "")
    .trim();
  const fallback = attachmentKind(mime) === "pdf" ? "document" : "image";
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
