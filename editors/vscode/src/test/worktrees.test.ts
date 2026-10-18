// The worktree listing the panel draws and the CLI calls it makes. The store
// itself is the core's (`oxide worktree`), so what is pinned here is the shape
// of the arguments and the tolerance of the readers: a panel that showed a
// checkout it cannot open, or asked the CLI for a listing with the wrong
// arguments, would fail on a machine rather than in a test.

import assert from "node:assert/strict";
import { describe, it } from "node:test";

import { WORKTREE_CREATE, WORKTREE_REMOVE, WORKTREE_SELECT, worktreeDialog } from "../core/dialogs";
import {
  parseWorktree,
  parseWorktrees,
  worktreeAt,
  worktreeCreateArgs,
  worktreeHolding,
  worktreeListArgs,
  worktreeRemoveArgs,
} from "../core/worktrees";

const entry = {
  name: "fix-login-bug",
  branch: "oxide/fix-login-bug",
  path: "/home/dev/Oxide/worktrees/9c1f/fix-login-bug",
  project: "/home/dev/Projects/oxide",
  created_at: 1_700_000_100,
};

describe("worktree arguments", () => {
  it("asks for one project's listing as JSON", () => {
    assert.deepEqual(worktreeListArgs("/w/app"), [
      "worktree",
      "list",
      "--project",
      "/w/app",
      "--json",
    ]);
  });

  it("creates with the name it was given, and without one when blank", () => {
    assert.deepEqual(worktreeCreateArgs("/w/app", "fix the bug"), [
      "worktree",
      "create",
      "fix the bug",
      "--project",
      "/w/app",
      "--json",
    ]);
    assert.deepEqual(worktreeCreateArgs("/w/app", "   "), [
      "worktree",
      "create",
      "--project",
      "/w/app",
      "--json",
    ]);
  });

  it("removes a checkout by the name it was created under", () => {
    assert.deepEqual(worktreeRemoveArgs("/w/app", "fix-login-bug"), [
      "worktree",
      "remove",
      "fix-login-bug",
      "--project",
      "/w/app",
      "--json",
    ]);
  });
});

describe("parseWorktrees", () => {
  it("reads the listing the CLI prints", () => {
    assert.deepEqual(parseWorktrees(JSON.stringify([entry])), [
      {
        name: "fix-login-bug",
        branch: "oxide/fix-login-bug",
        path: "/home/dev/Oxide/worktrees/9c1f/fix-login-bug",
        project: "/home/dev/Projects/oxide",
      },
    ]);
  });

  it("skips a row it cannot open rather than painting it", () => {
    assert.deepEqual(parseWorktrees([{ name: "no-path" }, entry, null, 3]), [
      {
        name: "fix-login-bug",
        branch: "oxide/fix-login-bug",
        path: "/home/dev/Oxide/worktrees/9c1f/fix-login-bug",
        project: "/home/dev/Projects/oxide",
      },
    ]);
  });

  it("reads an answer that is not a listing as no checkouts", () => {
    assert.deepEqual(parseWorktrees("not json"), []);
    assert.deepEqual(parseWorktrees({ name: "one" }), []);
    assert.deepEqual(parseWorktrees(""), []);
  });

  it("reads the one worktree `create` prints", () => {
    assert.equal(parseWorktree(JSON.stringify(entry))?.name, "fix-login-bug");
    assert.equal(parseWorktree("{}"), null);
    assert.deepEqual(parseWorktrees([entry, { ...entry, name: "second", path: "/tmp/b" }]).length, 2);
  });
});

describe("worktreeAt", () => {
  const list = parseWorktrees([entry]);

  it("finds the checkout a folder is", () => {
    assert.equal(worktreeAt(list, entry.path)?.name, "fix-login-bug");
    // A trailing separator is the same folder.
    assert.equal(worktreeAt(list, `${entry.path}/`)?.name, "fix-login-bug");
    assert.equal(worktreeAt(list, "/home/dev/Projects/oxide"), null);
    assert.equal(worktreeAt(list, null), null);
  });

  it("finds the checkout a file inside it belongs to", () => {
    assert.equal(
      worktreeHolding(list, `${entry.path}/src/app.ts`)?.name,
      "fix-login-bug",
    );
    // A name that only starts the same way is a different folder.
    assert.equal(worktreeHolding(list, `${entry.path}-old`), null);
    assert.equal(worktreeHolding(list, "/home/dev/Projects/oxide"), null);
  });
});

describe("worktreeDialog", () => {
  const list = parseWorktrees([entry]);

  it("lists the folder itself, the checkouts, and the way to make one", () => {
    const dialog = worktreeDialog(list, null, "oxide");
    assert.equal(dialog.kind, "worktrees");
    assert.equal(dialog.pin, "footer");
    assert.deepEqual(dialog.rows.map((row) => row.label), [
      "Local checkout",
      "fix-login-bug",
      "New worktree…",
    ]);
    assert.deepEqual(dialog.rows.map((row) => row.action), [
      WORKTREE_SELECT,
      WORKTREE_SELECT,
      WORKTREE_CREATE,
    ]);
    // The row's value is the checkout it names, and the folder itself carries
    // none: that is how the controller tells "here" from "that checkout".
    assert.deepEqual(dialog.rows.map((row) => row.value), ["", entry.path, ""]);
    // The folder comes first and is the one marked while no checkout is picked.
    assert.equal(dialog.rows[0].current, true);
    assert.equal(dialog.rows[1].current, false);
    assert.equal(dialog.count, 1);
    assert.match(dialog.rows[1].detail, /oxide\/fix-login-bug/);
  });

  it("marks the checkout the chat is in, and leaves the folder unmarked", () => {
    const dialog = worktreeDialog(list, entry.path, "oxide");
    assert.equal(dialog.rows[0].current, false);
    assert.equal(dialog.rows[1].current, true);
    assert.equal(dialog.rows[1].status, "in use");
    // And the folder is no longer marked as the one in use.
    assert.equal(dialog.rows[0].status, "");
    assert.equal(dialog.rows[0].tone, "");
  });

  it("offers a remove on each checkout and none on the folder", () => {
    const dialog = worktreeDialog(list, null, "oxide");
    assert.equal(dialog.rows[0].buttons?.length ?? 0, 0);
    assert.deepEqual(dialog.rows[1].buttons, [
      {
        label: "Remove the worktree fix-login-bug",
        action: WORKTREE_REMOVE,
        icon: "close",
      },
    ]);
  });

  it("says what an empty listing means", () => {
    const dialog = worktreeDialog([], null, "oxide");
    assert.match(dialog.note, /No worktrees yet/);
    assert.equal(dialog.count, 0);
    assert.deepEqual(dialog.rows.map((row) => row.label), ["Local checkout", "New worktree…"]);
  });

  it("carries a note over the rows when the listing failed", () => {
    const dialog = worktreeDialog([], null, "oxide", "Could not read this project's worktrees: no git");
    assert.match(dialog.note, /no git/);
  });
});
