// The footer's branch is read straight out of `.git`, so the parsing and the
// worktree indirection are the parts worth pinning down.

import assert from "node:assert/strict";
import * as path from "node:path";
import { describe, it } from "node:test";

import { branchFromHead, gitBranch, gitDirFromFile } from "../core/git";

/// A repository-shaped file map: `read` reports a directory (by returning
/// `null`, as the real reader does) as clearly as a missing file.
function reader(files: Record<string, string>) {
  return (file: string): string | null => files[file] ?? null;
}

describe("branchFromHead", () => {
  it("reads a branch, including one with slashes", () => {
    assert.equal(branchFromHead("ref: refs/heads/main\n"), "main");
    assert.equal(branchFromHead("ref: refs/heads/feat/footer\n"), "feat/footer");
  });

  it("reports a detached HEAD by its short commit", () => {
    assert.equal(branchFromHead("0123456789abcdef0123456789abcdef01234567\n"), "detached 0123456");
  });

  it("prefers the ref name for a ref that is not a local branch", () => {
    assert.equal(branchFromHead("ref: refs/remotes/origin/main\n"), "refs/remotes/origin/main");
  });

  it("treats an unreadable HEAD as no branch", () => {
    assert.equal(branchFromHead(""), "");
    assert.equal(branchFromHead("not a ref\n"), "");
  });
});

describe("gitDirFromFile", () => {
  it("reads the pointer of a worktree or submodule", () => {
    assert.equal(gitDirFromFile("gitdir: /repo/.git/worktrees/w\n"), "/repo/.git/worktrees/w");
    assert.equal(gitDirFromFile("gitdir: ../.git/modules/x\n"), "../.git/modules/x");
  });

  it("returns nothing for a normal clone's directory", () => {
    assert.equal(gitDirFromFile(""), "");
    assert.equal(gitDirFromFile("ref: refs/heads/main\n"), "");
  });
});

describe("gitBranch", () => {
  const folder = path.join("/repo", "pkg");

  it("reads HEAD from a .git directory", () => {
    const deps = reader({ [path.join(folder, ".git", "HEAD")]: "ref: refs/heads/main\n" });
    assert.equal(gitBranch(folder, { read: deps }), "main");
  });

  it("follows a worktree's absolute gitdir pointer", () => {
    const deps = reader({
      [path.join(folder, ".git")]: "gitdir: /repo/.git/worktrees/pkg\n",
      [path.join("/repo/.git/worktrees/pkg", "HEAD")]: "ref: refs/heads/footer\n",
    });
    assert.equal(gitBranch(folder, { read: deps }), "footer");
  });

  it("resolves a submodule's relative gitdir pointer against the folder", () => {
    const deps = reader({
      [path.join(folder, ".git")]: "gitdir: ../.git/modules/pkg\n",
      [path.join("/repo/pkg", "..", ".git/modules/pkg", "HEAD")]: "ref: refs/heads/sub\n",
    });
    assert.equal(gitBranch(folder, { read: deps }), "sub");
  });

  it("finds the repository an ancestor of the folder", () => {
    const deps = reader({ [path.join("/repo", ".git", "HEAD")]: "ref: refs/heads/main\n" });
    assert.equal(gitBranch(folder, { read: deps }), "main");
  });

  it("prefers the closest repository", () => {
    const deps = reader({
      [path.join(folder, ".git", "HEAD")]: "ref: refs/heads/inner\n",
      [path.join("/repo", ".git", "HEAD")]: "ref: refs/heads/outer\n",
    });
    assert.equal(gitBranch(folder, { read: deps }), "inner");
  });

  it("reports nothing outside a repository", () => {
    assert.equal(gitBranch(folder, { read: reader({}) }), "");
    assert.equal(gitBranch(folder, { read: reader({ [path.join(folder, ".git")]: "junk\n" }) }), "");
  });
});
