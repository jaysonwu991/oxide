// File-change previews for `write`/`edit`/`patch` tool calls.
//
// The JSON event stream does not carry `AgentEvent::ToolResult`'s `DiffPreview`
// (see `oxide_core::cli::event_json`), but the tool *arguments* are on the
// wire — so the extension builds the same preview locally, using the same
// algorithm as `oxide_core::diff`. The result is a compact line-numbered diff
// (`-  2      b`) with `⋯` marking the gaps between hunks.

const CONTEXT = 3;
const MAX_DIFF_LINES = 2_000;

type Kind = "context" | "add" | "remove";

interface Op {
  kind: Kind;
  old: number | null;
  next: number | null;
  text: string;
}

/// A line diff of `old` -> `new`, or `null` when the two are identical.
/// Oversized inputs fall back to a one-line summary instead of a huge diff.
export function diffPreview(oldText: string, newText: string): string | null {
  if (oldText === newText) return null;
  const oldLines = splitLines(oldText);
  const newLines = splitLines(newText);
  if (oldLines.length > MAX_DIFF_LINES || newLines.length > MAX_DIFF_LINES) {
    return `(diff omitted: ${oldLines.length} -> ${newLines.length} lines)`;
  }
  const ops = diffOps(oldLines, newLines);
  return render(ops, Math.max(oldLines.length, newLines.length));
}

/// `str::lines()`: no trailing empty element for text ending in a newline.
function splitLines(text: string): string[] {
  if (text === "") return [];
  const lines = text.split("\n");
  if (lines[lines.length - 1] === "") lines.pop();
  return lines.map((line) => line.replace(/\r$/, ""));
}

function diffOps(oldLines: string[], newLines: string[]): Op[] {
  const n = oldLines.length;
  const m = newLines.length;
  const width = m + 1;
  const dp = new Uint32Array((n + 1) * width);
  for (let i = n - 1; i >= 0; i -= 1) {
    for (let j = m - 1; j >= 0; j -= 1) {
      dp[i * width + j] =
        oldLines[i] === newLines[j]
          ? dp[(i + 1) * width + j + 1] + 1
          : Math.max(dp[(i + 1) * width + j], dp[i * width + j + 1]);
    }
  }

  const ops: Op[] = [];
  let i = 0;
  let j = 0;
  while (i < n && j < m) {
    if (oldLines[i] === newLines[j]) {
      ops.push({ kind: "context", old: i + 1, next: j + 1, text: oldLines[i] });
      i += 1;
      j += 1;
    } else if (dp[(i + 1) * width + j] >= dp[i * width + j + 1]) {
      ops.push({ kind: "remove", old: i + 1, next: null, text: oldLines[i] });
      i += 1;
    } else {
      ops.push({ kind: "add", old: null, next: j + 1, text: newLines[j] });
      j += 1;
    }
  }
  while (i < n) {
    ops.push({ kind: "remove", old: i + 1, next: null, text: oldLines[i] });
    i += 1;
  }
  while (j < m) {
    ops.push({ kind: "add", old: null, next: j + 1, text: newLines[j] });
    j += 1;
  }
  return ops;
}

function render(ops: Op[], total: number): string {
  const width = Math.max(3, String(total).length);
  const keep = new Array<boolean>(ops.length).fill(false);
  ops.forEach((op, index) => {
    if (op.kind === "context") return;
    const start = Math.max(0, index - CONTEXT);
    const end = Math.min(ops.length, index + CONTEXT + 1);
    for (let slot = start; slot < end; slot += 1) keep[slot] = true;
  });

  const out: string[] = [];
  let previous = false;
  ops.forEach((op, index) => {
    if (!keep[index]) {
      previous = false;
      return;
    }
    if (!previous && out.length > 0) {
      out.push(` ${"".padStart(width)} ${"".padStart(width)}  ⋯`);
    }
    out.push(formatOp(op, width));
    previous = true;
  });
  return out.join("\n");
}

function formatOp(op: Op, width: number): string {
  const marker = op.kind === "add" ? "+" : op.kind === "remove" ? "-" : " ";
  const old = op.old === null ? "" : String(op.old);
  const next = op.next === null ? "" : String(op.next);
  return `${marker}${old.padStart(width)} ${next.padStart(width)}  ${op.text}`;
}

export interface ToolDiff {
  path: string;
  diff: string;
}

interface Edit {
  old: string;
  next: string;
}

/// Canonical tool names, mirroring `oxide_core::tools::canonical_tool_name`.
export function canonicalTool(name: string): string {
  switch (name) {
    case "read":
    case "read_file":
      return "read_file";
    case "write":
    case "write_file":
      return "write_file";
    case "ls":
    case "list_dir":
      return "list_dir";
    case "find":
    case "glob":
      return "glob";
    default:
      return name;
  }
}

function record(value: unknown): Record<string, unknown> {
  return value && typeof value === "object" && !Array.isArray(value)
    ? (value as Record<string, unknown>)
    : {};
}

function text(value: unknown): string {
  return typeof value === "string" ? value : "";
}

/// The edits a call asks for, tolerating the shapes the tool itself accepts: an
/// array, a single object, a JSON string, or legacy top-level old/new text.
function parseEdits(args: Record<string, unknown>): Edit[] {
  let raw: unknown = args.edits;
  if (raw === undefined || raw === null) {
    if (typeof args.oldText === "string") {
      raw = [{ oldText: args.oldText, newText: args.newText }];
    } else {
      return [];
    }
  }
  if (typeof raw === "string") {
    raw = parseJsonLoose(raw);
  }
  const list = Array.isArray(raw) ? raw : [raw];
  return list.map((entry) => {
    const edit = record(entry);
    return { old: text(edit.oldText), next: text(edit.newText) };
  });
}

/// A stringified array closed with one brace too many is repaired before
/// parsing, matching the tolerance of the `edit` tool.
function parseJsonLoose(raw: string): unknown {
  try {
    return JSON.parse(raw);
  } catch {
    const repaired = raw.replace(/\}(\s*)\]/, "$1]");
    try {
      return JSON.parse(repaired);
    } catch {
      return [];
    }
  }
}

interface Located {
  start: number;
  end: number;
}

/// One line with its byte range, mirroring `oxide_core::tools::source_lines`.
function sourceLines(text: string): Array<{ start: number; end: number; text: string }> {
  const lines: Array<{ start: number; end: number; text: string }> = [];
  let start = 0;
  for (let i = 0; i < text.length; i += 1) {
    if (text[i] !== "\n") continue;
    lines.push({ start, end: i + 1, text: text.slice(start, i).replace(/\r$/, "") });
    start = i + 1;
  }
  if (start < text.length) lines.push({ start, end: text.length, text: text.slice(start) });
  return lines;
}

/// Splits a `12|text` or `12+|text` read line into its number and text.
function linePrefix(line: string): { number: number; rest: string } | null {
  const match = /^(\d+)\+?\|(.*)$/.exec(line);
  return match ? { number: Number(match[1]), rest: match[2] } : null;
}

/// Strips the `N|` / `N+|` prefixes `read` adds, when every non-blank line of
/// the block carries an ascending number.
function stripLinePrefixes(text: string): string | null {
  const lines = text.split("\n");
  if (lines.length < 2) return null;
  let previous = 0;
  let found = false;
  const stripped: string[] = [];
  for (const line of lines) {
    if (!line.trim()) {
      stripped.push(line);
      continue;
    }
    const prefix = linePrefix(line);
    if (!prefix || prefix.number <= previous) return null;
    previous = prefix.number;
    found = true;
    stripped.push(prefix.rest);
  }
  return found ? stripped.join("\n") : null;
}

function stripAllLinePrefixes(text: string): string {
  return text
    .split("\n")
    .map((line) => linePrefix(line)?.rest ?? line)
    .join("\n");
}

/// Locates `old` the way the core does: byte-exact first, then a line-wise
/// match that ignores trailing whitespace. Either must be unique.
function locate(base: string, old: string): Located | "ambiguous" | null {
  const exact: number[] = [];
  let index = base.indexOf(old);
  while (index !== -1) {
    exact.push(index);
    index = base.indexOf(old, index + old.length);
  }
  if (exact.length === 1) return { start: exact[0], end: exact[0] + old.length };
  if (exact.length > 1) return "ambiguous";

  const baseLines = sourceLines(base);
  const oldLines = sourceLines(old);
  if (oldLines.length === 0 || oldLines.length > baseLines.length) return null;
  const width = oldLines.length;
  const hits: Located[] = [];
  for (let i = 0; i + width <= baseLines.length; i += 1) {
    let matches = true;
    for (let k = 0; k < width; k += 1) {
      if (
        baseLines[i + k].text.replace(/\s+$/, "") !== oldLines[k].text.replace(/\s+$/, "")
      ) {
        matches = false;
        break;
      }
    }
    if (!matches) continue;
    const last = baseLines[i + width - 1];
    const end = old.endsWith("\n") ? last.end : last.start + last.text.length;
    hits.push({ start: baseLines[i].start, end });
  }
  if (hits.length === 1) return hits[0];
  if (hits.length > 1) return "ambiguous";
  return null;
}

/// Applies edits the way `oxide_core::tools::edit` does: each old text must
/// match the current content uniquely, tolerating trailing whitespace and the
/// `N|` prefixes a `read` result carries. Returns `null` when an edit does not
/// apply, in which case the call failed and its own error names the region.
function applyEdits(content: string, edits: Edit[]): string | null {
  let base = content.replace(/\r\n/g, "\n");
  for (const edit of edits) {
    const raw = edit.old.replace(/\r\n/g, "\n");
    const stripped = stripLinePrefixes(raw);
    const old = stripped ?? raw;
    if (!old.trim()) return null;
    const next = stripped
      ? stripAllLinePrefixes(edit.next.replace(/\r\n/g, "\n"))
      : edit.next.replace(/\r\n/g, "\n");
    const located = locate(base, old);
    if (!located || located === "ambiguous") return null;
    base = base.slice(0, located.start) + next + base.slice(located.end);
  }
  return base;
}

/// The diff to show for a tool call, or `null` when the call does not change a
/// file. `readFile` returns the current file content, or `null` when it cannot
/// be read (a new file, a binary, a path outside the workspace).
export function toolDiff(
  name: string,
  args: unknown,
  readFile: (path: string) => string | null,
): ToolDiff | null {
  const parsed = record(args);
  const canonical = canonicalTool(name);
  const path = text(parsed.path);

  if (canonical === "patch") {
    const diff = text(parsed.diff).trim();
    return diff ? { path: path || "patch", diff } : null;
  }
  if (!path) return null;

  if (canonical === "write_file") {
    const content = typeof parsed.content === "string" ? parsed.content : null;
    if (content === null) return null;
    const before = readFile(path);
    // A file that does not exist yet reads as empty, so the preview shows the
    // whole content as additions.
    const diff = diffPreview(before ?? "", content);
    return diff ? { path, diff } : null;
  }

  if (canonical === "edit") {
    const before = readFile(path);
    if (before === null) return null;
    const after = applyEdits(before, parseEdits(parsed));
    if (after === null) return null;
    const diff = diffPreview(before.replace(/\r\n/g, "\n"), after);
    return diff ? { path, diff } : null;
  }

  return null;
}
