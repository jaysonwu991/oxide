// Git worktrees as the panel sees them: the CLI's own store, read through
// `oxide worktree` rather than by running git here.
//
// A worktree is another checkout of the project on its own branch
// (`oxide/<name>`), which a thread runs in while the folder the reader has open
// stays as it was. The panel lists the checkouts a task can happen in and starts
// a chat in the one that is picked; what the branch is called, where the
// checkout sits and which ones exist is the core's answer, so the terminal, the
// desktop app and this panel cannot disagree about the same repository.

export interface Worktree {
  name: string;
  branch: string;
  path: string;
  /// The project the checkout was made from.
  project: string;
}

const str = (value: unknown): string => (typeof value === "string" ? value : "");

/// `oxide worktree list --json --project <folder>`: the listing every front-end
/// draws.
export function worktreeListArgs(folder: string): string[] {
  return ["worktree", "list", "--project", folder, "--json"];
}

/// `oxide worktree create [<name>] --project <folder> --json`. A name that is
/// blank is left to the CLI, which derives one from the first free default.
export function worktreeCreateArgs(folder: string, name: string, options?: { base?: string; source?: string; copyChanges?: boolean; detached?: boolean }): string[] {
  const wanted = name.trim();
  return [
    "worktree",
    "create",
    ...(wanted ? [wanted] : []),
    ...(options?.base?.trim() ? ["--base", options.base.trim()] : []),
    ...(options?.source ? ["--source", options.source] : []),
    ...(options?.copyChanges ? ["--copy-changes"] : []),
    ...(options?.detached ? ["--detached"] : []),
    "--project",
    folder,
    "--json",
  ];
}

export function worktreeBranchArgs(folder: string, name: string, branch: string): string[] {
  return ["worktree", "branch", name, branch, "--project", folder, "--json"];
}

export function worktreeHandoffArgs(folder: string, source: string, id: string, toWorktree: boolean): string[] {
  return ["worktree", "handoff", id, "--source", source, ...(toWorktree ? ["--to-worktree"] : []), "--project", folder, "--json"];
}

/// `oxide worktree remove <name> --project <folder> --json`: the checkout goes,
/// its branch stays in the repository.
export function worktreeRemoveArgs(folder: string, name: string): string[] {
  return ["worktree", "remove", name, "--project", folder, "--json"];
}

/// One worktree, from what `oxide worktree create --json` prints: the command
/// answers with the checkout it made rather than with a listing.
export function parseWorktree(raw: unknown): Worktree | null {
  let value = raw;
  if (typeof value === "string") {
    try {
      value = JSON.parse(value);
    } catch {
      return null;
    }
  }
  if (!value || typeof value !== "object") return null;
  return parseWorktrees([value])[0] ?? null;
}

/// The worktrees one project holds, from the JSON listing. A row that names no
/// path or name is skipped rather than painted as a checkout that cannot be
/// opened; a listing that is not an array reads as none.
export function parseWorktrees(raw: unknown): Worktree[] {
  let value = raw;
  if (typeof value === "string") {
    try {
      value = JSON.parse(value);
    } catch {
      return [];
    }
  }
  if (!Array.isArray(value)) return [];
  const out: Worktree[] = [];
  for (const entry of value) {
    if (!entry || typeof entry !== "object") continue;
    const record = entry as Record<string, unknown>;
    const name = str(record.name);
    const path = str(record.path);
    if (!name || !path) continue;
    out.push({
      name,
      path,
      branch: str(record.branch),
      project: str(record.project),
    });
  }
  return out;
}

/// The worktree a folder is, when it is one of the listing's: a thread records
/// the checkout it ran in as its folder, which is how the panel knows to send its
/// next message there. `null` for the project's own folder.
export function worktreeAt(list: readonly Worktree[], folder: string | null): Worktree | null {
  if (!folder) return null;
  const needle = folder.replace(/[\\/]+$/, "");
  return (
    list.find((entry) => entry.path.replace(/[\\/]+$/, "") === needle) ?? null
  );
}

/// A resumed local thread clears the previous checkout just as a worktree
/// thread selects its own. An absent header cannot establish a new checkout.
export function sessionWorktree(folder: string, cwd: string | undefined): string | null {
  return cwd && cwd !== folder ? cwd : null;
}

/// The checkout a folder belongs to, itself included: a file or a folder inside
/// a worktree is that worktree's, the way a folder inside a clone is the
/// clone's.
export function worktreeHolding(
  list: readonly Worktree[],
  folder: string | null,
): Worktree | null {
  if (!folder) return null;
  const exact = worktreeAt(list, folder);
  if (exact) return exact;
  return (
    list.find((entry) => {
      const root = entry.path.replace(/[\\/]+$/, "");
      return folder === root || folder.startsWith(`${root}/`) || folder.startsWith(`${root}\\`);
    }) ?? null
  );
}
