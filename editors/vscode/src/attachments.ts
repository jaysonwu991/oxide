// The temporary files a pasted or dropped attachment is written to.
//
// The CLI takes a path (`--image <path>`), but a blob that arrived from the
// clipboard or a drop only exists as a data URL inside the webview, so it has
// to become a real file before the turn starts. The directory is one private
// temp directory per window, created on the first paste and removed when the
// window goes away, so a pasted screenshot never outlives the chat it was
// pasted into and nothing of the user's is touched.

import * as fs from "node:fs";
import * as os from "node:os";
import * as path from "node:path";

import {
  attachmentFileName,
  decodeDataUrl,
  formatBytes,
  MAX_PREVIEW_CHARS,
} from "./core/attachments";

/// A thumbnail is a data URL in a view message; a file large enough to blow up
/// that message gets a glyph chip instead.
const MAX_PREVIEW_BYTES = MAX_PREVIEW_CHARS / 2;

export interface WrittenAttachment {
  /// The absolute path passed to `--image`.
  path: string;
  /// How large the file is, for the chip's tooltip.
  detail: string;
}

export class AttachmentStore {
  private dir: string | null = null;
  private next = 1;

  /// Writes a data URL to a file and returns where it landed, or `null` when
  /// the data URL is not an attachment or cannot be written.
  write(name: string, dataUrl: string): WrittenAttachment | null {
    const decoded = decodeDataUrl(dataUrl);
    if (!decoded) return null;
    try {
      // The counter keeps two different blobs with the same name apart, so the
      // second paste cannot overwrite the first.
      const file = path.join(
        this.directory(),
        `${this.next++}-${attachmentFileName(name, decoded.mime)}`,
      );
      fs.writeFileSync(file, decoded.bytes);
      return { path: file, detail: formatBytes(decoded.bytes.length) };
    } catch {
      return null;
    }
  }

  dispose(): void {
    if (!this.dir) return;
    try {
      fs.rmSync(this.dir, { recursive: true, force: true });
    } catch {
      // A temp file that cannot be removed is the OS's to clean up.
    }
    this.dir = null;
  }

  private directory(): string {
    if (!this.dir) {
      this.dir = fs.mkdtempSync(path.join(os.tmpdir(), "oxide-vscode-"));
    }
    return this.dir;
  }
}

/// A data URL for a file on disk, for a chip's thumbnail, or `null` when it is
/// too large to send through a view message or cannot be read.
export function previewForFile(file: string, mime: string): string | null {
  try {
    const stat = fs.statSync(file);
    if (!stat.isFile() || stat.size > MAX_PREVIEW_BYTES) return null;
    const url = `data:${mime};base64,${fs.readFileSync(file).toString("base64")}`;
    return url.length > MAX_PREVIEW_CHARS ? null : url;
  } catch {
    return null;
  }
}

/// The thumbnail for a blob the webview already handed over as a data URL: the
/// same string comes back, unless it is too large to echo.
export function previewForDataUrl(dataUrl: string): string | null {
  return dataUrl.length <= MAX_PREVIEW_CHARS ? dataUrl : null;
}
