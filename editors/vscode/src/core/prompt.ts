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

/// Image and PDF extensions the CLI sends as media instead of text, mirroring
/// `oxide_core::media::is_attachment_path`. Anything listed here is passed as
/// `--image` rather than inlined into the prompt.
const ATTACHMENT_EXTENSIONS = new Set([
  "png",
  "jpg",
  "jpeg",
  "gif",
  "webp",
  "bmp",
  "pdf",
]);

export function isAttachmentPath(path: string): boolean {
  const name = path.replace(/\\/g, "/").split("/").pop() ?? path;
  const dot = name.lastIndexOf(".");
  if (dot <= 0) return false;
  return ATTACHMENT_EXTENSIONS.has(name.slice(dot + 1).toLowerCase());
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
  /// The message with resolved references removed.
  message: string;
  /// Files inlined as context.
  blocks: ContextBlock[];
  /// Images and PDFs passed as `--image`.
  attachments: string[];
}

/// Expands the `@path` references in a message.
///
/// The CLI expands `@file` from its own arguments (`oxide_core::cli::expand_file_args`),
/// but a prompt sent on stdin is never scanned for them — so the extension
/// resolves them here instead, and a message reads the same either way. A
/// reference that does not resolve is left in the text rather than failing the
/// send, so a typo costs a round trip and not the message.
export function expandAtReferences(message: string, sources: AtReferenceSources): AtExpansion {
  const gone = "\u0000";
  const blocks: ContextBlock[] = [];
  const attachments: string[] = [];
  const seen = new Set<string>();
  const parts = message.split(/(\s+)/);
  const kept: string[] = [];

  for (const part of parts) {
    if (!part || /^\s+$/.test(part)) {
      kept.push(part);
      continue;
    }
    // Trailing punctuation is not part of a path: `see @a.rs, it …`.
    const match = /^(@[^\s]+?)([.,;:!?)\]]*)$/.exec(part);
    const reference = match ? match[1].slice(1) : part.slice(1);
    const tail = match ? match[2] : "";
    if (!part.startsWith("@") || !reference) {
      kept.push(part);
      continue;
    }
    const absolute = sources.resolve(reference);
    if (!absolute || seen.has(absolute)) {
      if (absolute) kept.push(gone + tail);
      else kept.push(part);
      continue;
    }
    if (isAttachmentPath(absolute)) {
      seen.add(absolute);
      attachments.push(absolute);
      kept.push(gone + tail);
      continue;
    }
    const text = sources.read(absolute);
    if (text === null) {
      kept.push(part);
      continue;
    }
    seen.add(absolute);
    blocks.push({ path: sources.label(absolute), text });
    kept.push(gone + tail);
  }

  return {
    message: kept
      .join("")
      .replace(/[ \t]*\u0000[ \t]*/g, " ")
      .replace(/[ \t]+([,.!?;:)])/g, "$1")
      .trim(),
    blocks,
    attachments,
  };
}
