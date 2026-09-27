// The `@path` completion the composer offers while a reference is being typed.
//
// The terminal completes the same token from the same list —
// `oxide_core::tools::workspace_paths`, filtered by `active_file_query` in
// `crates/cli/src/tui/mod.rs` — so a reference is completed over the project's
// own files in either front-end. The paths are the one part the view cannot
// know; everything here is pure so what gets offered for a keystroke is
// assertable under node.
//
// Like the rest of `src/core/`, this imports nothing from `vscode`.

/// The `@path` token the caret sits in.
export interface AtToken {
  /// Where the `@` is in the composer's value.
  start: number;
  /// Just past the token: the caret, or the end of a token the caret is inside.
  end: number;
  /// What has been typed after the `@`.
  query: string;
}

/// One row the composer offers for a token.
export interface AtSuggestion {
  /// The workspace-relative path being offered, a folder ending in `/`.
  label: string;
  kind: "file" | "folder";
  /// What takes the token's place: `@path`, with a space after a file so the
  /// next word can be typed, and without one after a folder, so the query goes
  /// on narrowing inside it.
  insert: string;
}

/// The `@path` token the caret is in, or `null` when it is in none.
///
/// A reference starts at a word boundary and runs to the next whitespace, which
/// is how the CLI's own `@file` expansion and the terminal's completion both
/// read one: `mail me at a@b.com` names no file, and a caret parked inside a
/// half-typed `@src/ma|in.rs` completes the whole token rather than the part
/// behind it.
export function atToken(value: string, caret: number): AtToken | null {
  const at = Math.max(0, Math.min(caret, value.length));
  const before = value.slice(0, at);
  const typed = /\S*$/.exec(before)?.[0] ?? "";
  if (!typed.startsWith("@")) return null;
  const start = before.length - typed.length;
  const rest = /\S*/.exec(value.slice(at))?.[0] ?? "";
  const end = at + rest.length;
  return { start, end, query: value.slice(start + 1, end) };
}

/// The rows to offer for a token, best first and capped at `limit` so a bare
/// `@` in a large project does not hand the view the whole tree.
export function atSuggestions(
  paths: readonly string[],
  token: AtToken | null,
  limit = 200,
): AtSuggestion[] {
  if (!token || limit <= 0) return [];
  const query = token.query.toLowerCase();
  const ranked: { rank: number; path: string }[] = [];
  for (const path of paths) {
    const rank = rankOf(path, query);
    if (rank >= 0) ranked.push({ rank, path });
  }
  ranked.sort((a, b) => a.rank - b.rank || (a.path < b.path ? -1 : a.path > b.path ? 1 : 0));
  return ranked.slice(0, limit).map(({ path }) => {
    const folder = path.endsWith("/");
    return {
      label: path,
      kind: folder ? "folder" : "file",
      insert: folder ? `@${path}` : `@${path} `,
    };
  });
}

/// How well a path answers a query: the nearest name first (`agent.rs` for
/// `age`), then a path that starts with it, then one that mentions it anywhere.
/// A query already carrying a `/` is a path being walked, so the whole prefix
/// is what counts. `-1` is no match.
function rankOf(path: string, query: string): number {
  const whole = path.toLowerCase();
  // A folder the reference already spells exactly is what taking its row would
  // give back — the query itself. Leaving it out is what makes the row take the
  // reference *into* the folder and offer what is inside it, rather than
  // running a completion that changes nothing.
  if (whole.endsWith("/") && whole === query) return -1;
  const trimmed = whole.endsWith("/") ? whole.slice(0, -1) : whole;
  const name = trimmed.slice(trimmed.lastIndexOf("/") + 1);
  if (query.includes("/")) {
    if (whole.startsWith(query)) return 0;
    return whole.includes(query) ? 1 : -1;
  }
  if (name.startsWith(query)) return 0;
  if (whole.startsWith(query)) return 1;
  return whole.includes(query) ? 2 : -1;
}
