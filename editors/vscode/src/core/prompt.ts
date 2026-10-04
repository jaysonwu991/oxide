// Prompt assembly: the user's message plus any context the editor attached.
//
// Context is inlined into the prompt text in the same `--- path ---` shape the
// CLI's own `@file` expansion uses, so a run started from the extension reads
// the same way as one started from the terminal.

export interface ContextBlock {
  /// Workspace-relative path shown to the model.
  path: string;
  /// 1-based inclusive line range, when the block is an editor selection.
  startLine?: number;
  endLine?: number;
  text: string;
}

/// 1-based inclusive line range.
export interface LineRange {
  start: number;
  end: number;
}

export function contextHeader(block: ContextBlock): string {
  const range =
    block.startLine && block.endLine
      ? block.startLine === block.endLine
        ? `:${block.startLine}`
        : `:${block.startLine}-${block.endLine}`
      : "";
  return `--- ${block.path}${range} ---`;
}

/// The full prompt: context blocks, a blank line each, then the message.
export function buildPrompt(message: string, blocks: ContextBlock[] = []): string {
  const parts = blocks.map((block) => `${contextHeader(block)}\n${block.text}`);
  const body = message.trim();
  if (body) parts.push(body);
  return parts.join("\n\n");
}

/// Media extensions the CLI sends as attachments instead of text, mirroring
/// `oxide_core::media::is_attachment_path`. Anything listed here is passed as
/// `--image` rather than inlined into the prompt — including the formats the
/// CLI converts (a TIFF, a HEIC), which are media all the same.
const ATTACHMENT_EXTENSIONS = new Set([
  "png",
  "jpg",
  "jpeg",
  "gif",
  "webp",
  "bmp",
  "tif",
  "tiff",
  "heic",
  "heif",
  "avif",
  "pdf",
]);

export function isAttachmentPath(path: string): boolean {
  const name = path.replace(/\\/g, "/").split("/").pop() ?? path;
  const dot = name.lastIndexOf(".");
  if (dot <= 0) return false;
  return ATTACHMENT_EXTENSIONS.has(name.slice(dot + 1).toLowerCase());
}

/// The lines an editor selection covers, 1-based and inclusive, or null for a
/// caret with nothing selected — which is the whole file rather than line 1.
/// A selection ending where a line starts does not include that line, since the
/// reader stopped at the newline before it: dragging down a line gives one line,
/// not the two a raw `end.line` would name.
export function selectionLines(selection: {
  isEmpty: boolean;
  start: { line: number };
  end: { line: number; character: number };
}): LineRange | null {
  if (selection.isEmpty) return null;
  const endsAtLineStart =
    selection.end.character === 0 && selection.end.line > selection.start.line;
  return {
    start: selection.start.line + 1,
    end: endsAtLineStart ? selection.end.line : selection.end.line + 1,
  };
}

/// The `@path` reference the editor's insert shortcut writes, in the shape
/// Claude Code's uses: `@src/app.ts#5-10` for a selection, `@src/app.ts` for a
/// whole file, and a one-line range as the single number it is.
export function fileReference(path: string, lines?: LineRange): string {
  if (!lines || lines.start <= 0) return `@${path}`;
  const range = lines.end > lines.start ? `${lines.start}-${lines.end}` : `${lines.start}`;
  return `@${path}#${range}`;
}

/// The line range a reference names, or null when it names the whole file.
function referenceLines(reference: string): { path: string; start: number; end: number } | null {
  const match = /^(.*)#(\d+)(?:-(\d+))?$/.exec(reference);
  if (!match) return null;
  const start = Number(match[2]);
  const end = match[3] ? Number(match[3]) : start;
  if (!match[1] || start < 1 || end < start) return null;
  return { path: match[1], start, end };
}

/// The lines a range named. A range that starts past the end of the file reads
/// as unresolved rather than as an empty block.
export function sliceLines(text: string, start: number, end: number): string | null {
  const lines = text.split("\n");
  if (start > lines.length) return null;
  return lines.slice(start - 1, Math.min(end, lines.length)).join("\n");
}

/// A workspace-relative path for display and for the model's context header.
export function relativePath(root: string, file: string): string {
  const normalize = (value: string) =>
    value.replace(/\\/g, "/").replace(/\/+$/, "").replace(/^\/\//, "/");
  const from = normalize(root);
  const to = normalize(file);
  if (!from || to === from) return to || ".";
  if (to.toLowerCase().startsWith(`${from.toLowerCase()}/`)) {
    return to.slice(from.length + 1);
  }
  return to;
}

/// The chip label shown in the composer for a context block.
export function contextLabel(block: ContextBlock): string {
  if (!block.startLine || !block.endLine) return block.path;
  return block.startLine === block.endLine
    ? `${block.path}:${block.startLine}`
    : `${block.path}:${block.startLine}-${block.endLine}`;
}

export interface AtReferenceSources {
  /// The absolute path a reference names, or `null` when it does not resolve.
  resolve: (reference: string) => string | null;
  /// The file's text, or `null` when it cannot be read.
  read: (absolute: string) => string | null;
  /// The path shown in the context header (workspace-relative).
  label: (absolute: string) => string;
}

export interface AtExpansion {
  /// The message with resolved references removed, for the transcript bubble
  /// and for building a prompt without the resolved blocks.
  message: string;
  /// Files inlined as context.
  blocks: ContextBlock[];
  /// Images and PDFs passed as `--image`.
  attachments: string[];
  /// The message with each resolved block written in place, the way the CLI's
  /// `@file` expansion writes it: a reference in the middle of a sentence stays
  /// in the middle, rather than every file being moved above the question.
  inlined: string;
}

/// Expands the `@path` references in a message.
///
/// The CLI expands `@file` from its own arguments (`oxide_core::cli::expand_file_args`),
/// but a prompt sent on stdin is never scanned for them — so the extension
/// resolves them here instead, and a message reads the same either way. A
/// reference that does not resolve is left in the text rather than failing the
/// send, so a typo costs a round trip and not the message. A reference may name
/// a line range (`@src/app.ts#5-10`, what the editor's own shortcut inserts),
/// which is inlined as that slice under the range's header.
export function expandAtReferences(message: string, sources: AtReferenceSources): AtExpansion {
  const gone = "\u0000";
  const blockMark = "\u0001";
  const blocks: ContextBlock[] = [];
  const attachments: string[] = [];
  const seen = new Set<string>();
  const parts = message.split(/(\s+)/);
  const kept: string[] = [];
  const inline: string[] = [];

  for (const part of parts) {
    if (!part || /^\s+$/.test(part)) {
      kept.push(part);
      inline.push(part);
      continue;
    }
    // Trailing punctuation is not part of a path: `see @a.rs, it …`.
    const match = /^(@[^\s]+?)([.,;:!?)\]]*)$/.exec(part);
    const reference = match ? match[1].slice(1) : part.slice(1);
    const tail = match ? match[2] : "";
    if (!part.startsWith("@") || !reference) {
      kept.push(part);
      inline.push(part);
      continue;
    }
    const lines = referenceLines(reference);
    const absolute = sources.resolve(lines ? lines.path : reference);
    // One text file at two ranges is two blocks, so a text reference's identity
    // carries the range it named — while an image or PDF travels whole whatever
    // follows its name, so two ranges of one attachment are one attachment.
    const key = absolute
      ? isAttachmentPath(absolute) || !lines
        ? absolute
        : `${absolute}#${lines.start}-${lines.end}`
      : "";
    if (!absolute || seen.has(key)) {
      if (absolute) {
        kept.push(gone + tail);
        inline.push(gone + tail);
      } else {
        kept.push(part);
        inline.push(part);
      }
      continue;
    }
    if (isAttachmentPath(absolute)) {
      seen.add(key);
      attachments.push(absolute);
      kept.push(gone + tail);
      inline.push(gone + tail);
      continue;
    }

    const text = sources.read(absolute);
    const slice = text === null ? null : lines ? sliceLines(text, lines.start, lines.end) : text;
    if (slice === null) {
      kept.push(part);
      inline.push(part);
      continue;
    }
    seen.add(key);
    const index = blocks.length;
    blocks.push({
      path: sources.label(absolute),
      text: slice,
      ...(lines ? { startLine: lines.start, endLine: lines.end } : {}),
    });
    kept.push(gone + tail);
    inline.push(`${blockMark}${index}${blockMark}${tail}`);
  }

  return {
    message: kept
      .join("")
      .replace(/[ \t]*\u0000[ \t]*/g, " ")
      .replace(/[ \t]+([,.!?;:)])/g, "$1")
      .trim(),
    blocks,
    attachments,
    inlined: inline
      .join("")
      .replace(/[ \t]*\u0001(\d+)\u0001[ \t]*/g, (_all, index: string) => {
        const block = blocks[Number(index)];
        const body = block.text.endsWith("\n") ? block.text : `${block.text}\n`;
        return `\n\n${contextHeader(block)}\n${body}`;
      })
      .replace(/[ \t]*\u0000[ \t]*/g, " ")
      .replace(/[ \t]+([,.!?;:)])/g, "$1")
      .trim(),
  };
}
