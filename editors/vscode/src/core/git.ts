// The git branch shown in the footer. It is read out of `.git/HEAD` rather than
// through the Git extension, so the chip costs one file read and does not
// depend on another extension being installed.

import * as path from "node:path";

export interface GitDeps {
  /// File contents, or `null` for a missing path — and for a directory, which
  /// is how a `.git` directory is told apart from a `.git` pointer file.
  read: (file: string) => string | null;
}

/// `ref: refs/heads/main` → `main`. A detached HEAD has no branch, so the short
/// commit is reported instead of pretending there is one.
export function branchFromHead(text: string): string {
  const line = text.trim();
  const ref = /^ref:\s*(.+)$/.exec(line);
  if (ref) {
    const name = ref[1].trim();
    const heads = /^refs\/heads\/(.+)$/.exec(name);
    return (heads ? heads[1] : name).trim();
  }
  const sha = /^[0-9a-f]{7,40}$/.exec(line);
  return sha ? `detached ${sha[0].slice(0, 7)}` : "";
}

/// The `gitdir:` target of a `.git` file, which is how a worktree or submodule
/// points at the directory holding its real HEAD.
export function gitDirFromFile(text: string): string {
  const match = /^gitdir:\s*(.+)$/m.exec(text.trim());
  return match ? match[1].trim() : "";
}

/// The branch of `folder`, or `""` when it is not inside a repository (or the
/// HEAD cannot be read). The closest `.git` wins, so a folder inside a clone
/// reports that clone's branch, and a submodule reports its own.
export function gitBranch(folder: string, deps: GitDeps): string {
  let current: string | null = folder;
  while (current) {
    const dotGit = path.join(current, ".git");
    const head = deps.read(path.join(dotGit, "HEAD"));
    if (head !== null) return branchFromHead(head);

    const pointer = gitDirFromFile(deps.read(dotGit) ?? "");
    if (pointer) {
      // A worktree stores an absolute path; a submodule's is relative to `.git`.
      const dir = path.isAbsolute(pointer) ? pointer : path.join(current, pointer);
      return branchFromHead(deps.read(path.join(dir, "HEAD")) ?? "");
    }

    const parent = path.dirname(current);
    current = parent === current ? null : parent;
  }
  return "";
}
