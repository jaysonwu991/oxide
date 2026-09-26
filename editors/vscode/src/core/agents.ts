// Agents the footer can send a turn to. The names come from `--agent`'s own
// resolution in `oxide_core::ecosystem`: a markdown file's frontmatter `name`,
// else its file stem.

export interface AgentChoice {
  name: string;
  description: string;
}

/// One agent file: its contents and its file stem (the name without `.md`).
export interface AgentFile {
  stem: string;
  text: string;
}

const FRONTMATTER = /^---\r?\n([\s\S]*?)\r?\n---/;

/// A `key: value` pair from the leading frontmatter block, with the quotes a
/// hand-written file often carries stripped.
export function frontmatterValue(text: string, key: string): string {
  const block = FRONTMATTER.exec(text);
  if (!block) return "";
  for (const line of block[1].split(/\r?\n/)) {
    const trimmed = line.trim();
    // Only a top-level key counts: an indented `name:` belongs to a nested map.
    if (!trimmed.startsWith(`${key}:`) || /^\s/.test(line)) continue;
    return trimmed
      .slice(key.length + 1)
      .trim()
      .replace(/^["']|["']$/g, "")
      .trim();
  }
  return "";
}

/// Deduplicated agents in name order. The first file wins, so a project agent
/// shadows a global one of the same name — the precedence the ecosystem uses.
export function agentChoices(files: AgentFile[]): AgentChoice[] {
  const seen = new Map<string, AgentChoice>();
  for (const file of files) {
    const name = frontmatterValue(file.text, "name") || file.stem;
    if (!name || seen.has(name)) continue;
    seen.set(name, { name, description: frontmatterValue(file.text, "description") });
  }
  return [...seen.values()].sort((left, right) => left.name.localeCompare(right.name));
}
