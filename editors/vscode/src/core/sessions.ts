// Parsing for `oxide sessions list` and `oxide --version`.
//
// The session picker reads the CLI's own listing rather than walking the
// session tree itself, so the extension never has to agree with the core about
// how a project's session directory is named.

export interface SessionEntry {
  id: string;
  age: string;
  messages: number;
  label: string;
}

/// One line of `oxide sessions list`:
///
/// ```text
/// 7c8031b1  just now       2 msg  say hi
/// ```
///
/// The label is free text and is always last, so the rest of the line is
/// matched and the label kept whole. Unrecognized lines (the "no sessions"
/// notice, or a future format) are skipped instead of mis-parsed.
const SESSION_LINE = /^([0-9a-fA-F-]+)\s+(just now|\d+(?:m|h|d) ago)\s+(\d+) msg\s*(.*)$/;

export function parseSessionList(output: string): SessionEntry[] {
  const entries: SessionEntry[] = [];
  for (const line of output.split("\n")) {
    const match = SESSION_LINE.exec(line.trimEnd());
    if (!match) continue;
    entries.push({
      id: match[1],
      age: match[2],
      messages: Number(match[3]),
      label: match[4].trim(),
    });
  }
  return entries;
}

/// The rows a search has left: the CLI's listing in its own order, without the
/// ones whose title and id both miss the query. Case-insensitive, and an empty
/// query (or one that is only spaces) keeps every row.
///
/// The filter is applied to the answer the store already gave rather than to a
/// fresh read, so typing in the panel's search box never spawns the CLI again:
/// the read belongs to the listing, and the query only decides which of its
/// rows are painted. A query nothing matches leaves an empty list rather than
/// falling back to the whole one — the note says so, and the count in the head
/// is what is left.
export function filterSessions(
  sessions: readonly SessionEntry[],
  query: string,
): SessionEntry[] {
  const needle = query.trim().toLowerCase();
  if (!needle) return [...sessions];
  return sessions.filter(
    (session) =>
      session.label.toLowerCase().includes(needle) ||
      session.id.toLowerCase().includes(needle),
  );
}

/// True when a composer line is the bare session command, which the panel
/// answers by opening the session history instead of sending the text on. The
/// two names are the CLI catalog's own (`session`, alias `sessions`), so a
/// client command cannot reach the model as prose. An argument (`/session
/// <id>`) is left to the prompt path.
export function isSessionCommand(text: string): boolean {
  return /^\/(session|sessions)$/i.test(text.trim());
}

/// The version from `oxide --version` (`oxide 0.1.2`).
export function parseVersion(output: string): string | null {
  const match = /(\d+\.\d+\.\d+(?:[-+][^\s]+)?)/.exec(output);
  return match ? match[1] : null;
}
