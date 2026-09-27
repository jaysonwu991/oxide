// The `/` palette the composer offers while a slash command is being typed.
//
// The rows come from the CLI's own catalog — `oxide commands --json`, which is
// the same listing the terminal's `/` menu is built from and the desktop app's
// palette draws (`oxide_core::commands::palette`) — so the project's commands,
// prompt templates and skills are offered here by the name the CLI resolves,
// including the ones an installed plugin contributes. A row the user takes is
// sent as `/name`, which the CLI expands against the same ecosystem before the
// turn starts: picking a skill is what activates it.
//
// The built-ins are the client's own draws. Only the ones this panel has an
// action for are offered (`panelCommand`), so nothing it cannot perform is
// inserted into a message the CLI would then hand the model as a prompt.
//
// Like the rest of `src/core/`, this imports nothing from `vscode`.

/// One row of `oxide commands --json`.
export interface CommandEntry {
  name: string;
  /// Alternative spellings, e.g. `mcps` for `mcp`.
  aliases: string[];
  description: string;
  /// An argument hint (`auto|off|low|medium|high`), or "" when it takes none.
  arguments: string;
  /// `client` for a command the panel draws itself, `prompt` for a configured
  /// command or template, `skill` for a skill.
  kind: string;
  /// `builtin`, `project` or `global`.
  source: string;
  /// A command only the desktop app can perform (its theme picker, provider
  /// login), which is left out of this panel's menu.
  desktopOnly: boolean;
}

/// One row the composer offers.
export interface CommandRow {
  /// The name the row is listed and inserted under, without its slash.
  name: string;
  /// What takes the palette's place: `/name`, with a space after it when the
  /// command takes arguments to type after it.
  insert: string;
  arguments: string;
  description: string;
  kind: string;
  source: string;
}

/// The action the panel performs for a built-in command. The same actions the
/// footer's chips run, so a command and its chip stay one implementation.
export type PanelAction =
  | "help"
  | "mcp"
  | "model"
  | "reasoning"
  | "agent"
  | "trust"
  | "session"
  | "new"
  | "attach"
  | "usage";

/// How many rows a bare `/` offers. The catalog is small, but a project with
/// hundreds of commands should not hand the view every one of them.
export const MAX_COMMAND_ROWS = 200;

/// Parses the catalog. A CLI that predates `--json` prints its human table, so
/// a body that is not an array of objects yields nothing rather than throwing
/// in the middle of a keystroke.
export function parseCommandList(output: string): CommandEntry[] {
  const text = output.trim();
  if (!text) return [];
  let value: unknown;
  try {
    value = JSON.parse(text);
  } catch {
    return [];
  }
  if (!Array.isArray(value)) return [];
  const entries: CommandEntry[] = [];
  for (const item of value) {
    if (!item || typeof item !== "object") continue;
    const record = item as Record<string, unknown>;
    const name = stringOf(record.name).trim();
    if (!name) continue;
    entries.push({
      name,
      aliases: Array.isArray(record.aliases)
        ? record.aliases.filter((alias): alias is string => typeof alias === "string")
        : [],
      description: stringOf(record.description),
      arguments: stringOf(record.arguments),
      kind: stringOf(record.kind) || "prompt",
      source: stringOf(record.source),
      desktopOnly: record.desktop_only === true,
    });
  }
  return entries;
}

/// The query the composer is completing as, or `null` when it is not typing a
/// bare `/name`. A space ends the name, so `/model claude` is arguments being
/// typed and the menu closes rather than offering names under it.
export function commandQuery(value: string): string | null {
  if (!value.startsWith("/")) return null;
  const rest = value.slice(1);
  if (/\s/.test(rest)) return null;
  return rest;
}

/// The entries the panel offers, best first, capped at `MAX_COMMAND_ROWS`.
///
/// Matching is the terminal's and the desktop app's: a case-insensitive
/// substring of the name or an alias, with the names that start with the query
/// first — so `/ox` offers `/oxide-architecture` before a command that merely
/// mentions it. The panel's own actions follow the project's own entries: a
/// skill or command the user is reaching for is what the menu is for.
export function paletteCommands(
  entries: readonly CommandEntry[],
  query: string,
): CommandEntry[] {
  const wanted = query.toLowerCase();
  const ranked: { rank: number; entry: CommandEntry }[] = [];
  for (const entry of entries) {
    if (!offered(entry)) continue;
    const rank = rankOf(entry, wanted);
    if (rank >= 0) ranked.push({ rank, entry });
  }
  ranked.sort((left, right) => left.rank - right.rank);
  return ranked.slice(0, MAX_COMMAND_ROWS).map(({ entry }) => entry);
}

/// Whether the panel offers an entry at all: every configured command and
/// skill, and the built-in commands it has an action for.
function offered(entry: CommandEntry): boolean {
  if (entry.kind !== "client") return true;
  if (entry.desktopOnly) return false;
  return panelCommand(entry.name) !== null;
}

/// How well an entry answers a query: a name that starts with it, then an alias
/// that does, then one that mentions it anywhere. `-1` is no match.
function rankOf(entry: CommandEntry, query: string): number {
  const name = entry.name.toLowerCase();
  if (name.startsWith(query)) return 0;
  const aliases = entry.aliases.map((alias) => alias.toLowerCase());
  if (aliases.some((alias) => alias.startsWith(query))) return 1;
  if (name.includes(query)) return 2;
  return aliases.some((alias) => alias.includes(query)) ? 3 : -1;
}

/// The rows for a composer value, and the range of the value they replace —
/// the whole value, since a palette is only up while a bare `/name` is being
/// typed. `null` when the value is not one the palette answers.
export function commandRows(
  entries: readonly CommandEntry[],
  value: string,
): { start: number; end: number; rows: CommandRow[] } | null {
  const query = commandQuery(value);
  if (query === null) return null;
  const rows = paletteCommands(entries, query).map((entry) => ({
    name: entry.name,
    insert: `/${entry.name}${entry.arguments ? " " : ""}`,
    arguments: entry.arguments,
    description: entry.description,
    kind: entry.kind,
    source: entry.source,
  }));
  return { start: 0, end: value.length, rows };
}

/// The action the panel performs for a built-in command, by name or alias, or
/// `null` for one it has none for. A leading slash is allowed, as the CLI's own
/// `builtin` lookup allows it.
export function panelCommand(name: string): PanelAction | null {
  switch (name.trim().replace(/^\//, "").toLowerCase()) {
    case "help":
      return "help";
    case "mcp":
    case "mcps":
      return "mcp";
    case "model":
      return "model";
    case "reasoning":
    case "thinking":
      return "reasoning";
    case "agent":
      return "agent";
    case "trust":
    case "access":
      return "trust";
    case "session":
    case "sessions":
      return "session";
    case "new":
    case "clear":
      return "new";
    case "attach":
      return "attach";
    case "usage":
    case "cost":
      return "usage";
    default:
      return null;
  }
}

/// What the panel does with a message before it is sent.
///
/// A bare `/name` naming a client command never reaches the model: the panel
/// performs it, or — for one it has no action for, like the terminal's
/// `/permissions` — says so, rather than sending text the CLI would hand the
/// agent as a prompt. `null` for everything else, which is what the CLI
/// expands: a configured command, a skill, or an ordinary message.
///
/// Only a bare name routes, so `/session auth is broken` stays a message the
/// agent has something to say about, as it does in the terminal.
export type CommandRoute =
  | { kind: "action"; action: PanelAction }
  | { kind: "refused"; name: string };

export function routeCommand(
  entries: readonly CommandEntry[],
  text: string,
): CommandRoute | null {
  const trimmed = text.trim();
  if (!trimmed.startsWith("/") || /\s/.test(trimmed)) return null;
  const name = trimmed.slice(1).toLowerCase();
  if (!name) return null;
  const action = panelCommand(name);
  if (action) return { kind: "action", action };
  const entry = entries.find(
    (candidate) =>
      candidate.kind === "client" &&
      (candidate.name.toLowerCase() === name ||
        candidate.aliases.some((alias) => alias.toLowerCase() === name)),
  );
  return entry ? { kind: "refused", name: entry.name } : null;
}

function stringOf(value: unknown): string {
  return typeof value === "string" ? value : "";
}
