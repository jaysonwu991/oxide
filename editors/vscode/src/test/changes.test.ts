// A finished turn's changed files, and the diff plan the panel opens them with.
//
// The listing is the CLI's own — built in `oxide_core::changes` from the
// project's shadow snapshot, so a file a shell command or a formatter wrote is
// listed the same as an edited one — and these tests hold the extension to
// reading that frame, arranging it into rows, and asking VS Code's own diff
// editor for the file rather than rendering a diff format of its own.

import assert from "node:assert/strict";
import { describe, it } from "node:test";

import {
  CHANGE_SCHEME,
  CHANGES_VISIBLE,
  changeArgs,
  changeDetail,
  changeLetter,
  changeRows,
  changesMore,
  changesTitle,
  changesTotals,
  diffPlan,
  parseChanges,
  parseSnapshotQuery,
  snapshotQuery,
  turnChanges,
  undoArgs,
  type ChangedFile,
} from "../core/changes";
import { Transcript, type WireEvent } from "../core/protocol";

const noState = {
  queued: 0,
  context: [],
  attachments: [],
  title: "oxide",
  folder: "/work/oxide",
  binary: "oxide",
  showThinking: true,
  footer: {
    chips: [],
    info: "",
    usage: "",
    percent: 0,
    level: "ok" as const,
  },
  run: null,
};

function file(over: Partial<ChangedFile> = {}): ChangedFile {
  return {
    path: "src/main.rs",
    status: "modified",
    added: 3,
    removed: 1,
    binary: false,
    ...over,
  };
}

function frame(
  changes: unknown,
  baseline = "abc123",
  project = "/tmp/project",
  after = "def456",
): WireEvent {
  return { type: "turn_changes", baseline, project, after, changes };
}

describe("a turn's changed files", () => {
  it("reads the CLI's own frame", () => {
    const changes = turnChanges(
      frame({
        files: [
          { path: "src/main.rs", status: "modified", added: 12, removed: 3, binary: false },
          { path: "docs/new.md", status: "added", added: 40, removed: 0, binary: false },
        ],
      }),
    );
    assert.ok(changes);
    assert.equal(changes.baseline, "abc123");
    // The folder the run was in travels with the listing, so a row opened later
    // reads its file and its snapshot from the project it belongs to.
    assert.equal(changes.project, "/tmp/project");
    assert.deepEqual(changes.files.map((entry) => entry.path), ["src/main.rs", "docs/new.md"]);
    // The card's own total is the listing's, not a count of files.
    assert.equal(changes.added, 52);
    assert.equal(changes.removed, 3);
  });

  it("adds the totals up from the files, not from a field that might drift", () => {
    const changes = parseChanges(
      {
        added: 999,
        removed: 999,
        files: [file({ added: 2, removed: 0 }), file({ path: "b.rs", added: 0, removed: 5 })],
      },
      "head",
      "/tmp/project",
    );
    assert.equal(changes?.added, 2);
    assert.equal(changes?.removed, 5);
  });

  it("ignores the preview a frame carries, since the diff drawn is VS Code's", () => {
    // The frame brings the CLI's own per-file preview along, which the desktop
    // card paints. This panel does not: its review opens the file in the
    // editor's diff, so the listing keeps only what a row says.
    const changes = turnChanges(
      frame({
        files: [
          {
            path: "src/main.rs",
            status: "modified",
            added: 1,
            removed: 1,
            binary: false,
            diff: "   1   1  fn main() {\n-  2      old();\n+  2   2  new();",
          },
          { path: "logo.png", status: "added", added: 0, removed: 0, binary: true, diff: "" },
        ],
      }),
    );
    assert.deepEqual(changes?.files, [
      { path: "src/main.rs", status: "modified", added: 1, removed: 1, binary: false },
      { path: "logo.png", status: "added", added: 0, removed: 0, binary: true },
    ]);
  });

  it("names a file whose kind it does not know as modified", () => {
    const changes = parseChanges({ files: [{ path: "a.rs", status: "renamed" }] }, "head", "");
    assert.deepEqual(changes?.files[0], {
      path: "a.rs",
      status: "modified",
      added: 0,
      removed: 0,
      binary: false,
    });
  });

  it("draws no card for a turn that changed nothing", () => {
    assert.equal(turnChanges(frame({ files: [] })), null);
    assert.equal(turnChanges(frame({})), null);
    assert.equal(turnChanges(frame(null)), null);
    // A frame that is not this one is not a listing either.
    assert.equal(turnChanges({ type: "agent_end" }), null);
  });

  it("drops an entry with no path instead of half a row", () => {
    const changes = parseChanges({ files: [{ status: "added" }, file()] }, "head", "");
    assert.equal(changes?.files.length, 1);
  });
});

describe("what a change row says", () => {
  it("badges the status the way git spells it", () => {
    assert.equal(changeLetter("added"), "A");
    assert.equal(changeLetter("modified"), "M");
    assert.equal(changeLetter("deleted"), "D");
  });

  it("counts lines, and says so when there are none to count", () => {
    assert.equal(changeDetail(file({ added: 12, removed: 3 })), "+12 −3");
    assert.equal(changeDetail(file({ added: 0, removed: 4 })), "−4");
    assert.equal(changeDetail(file({ added: 0, removed: 0 })), "no line changes");
    assert.equal(changeDetail(file({ binary: true })), "binary");
  });

  it("names the card after how many files it lists", () => {
    assert.equal(changesTitle(1), "Edited 1 file");
    assert.equal(changesTitle(3), "Edited 3 files");
  });

  it("folds a listing longer than the card shows behind one row", () => {
    // Every file fits, so there is nothing to fold away.
    assert.equal(changesMore(CHANGES_VISIBLE), null);
    assert.equal(changesMore(0), null);
    const six = changesMore(CHANGES_VISIBLE + 1);
    assert.deepEqual(six, { visible: CHANGES_VISIBLE, closed: "+1 more file", open: "Show less" });
    // Singular where it should be, which is the row's own words rather than the
    // view's.
    assert.equal(changesMore(CHANGES_VISIBLE + 4)?.closed, "+4 more files");
  });

  it("leaves the total blank for a turn that changed no lines", () => {
    assert.equal(changesTotals(12, 3), "+12 −3");
    assert.equal(changesTotals(0, 4), "−4");
    assert.equal(changesTotals(0, 0), "");
  });

  it("composes the rows the webview paints, index and words included", () => {
    const rows = changeRows([
      file({ path: "src/main.rs", status: "modified", added: 2, removed: 1 }),
      file({ path: "docs/new.md", status: "added", added: 5, removed: 0, binary: true }),
    ]);
    assert.deepEqual(rows, [
      {
        path: "src/main.rs",
        status: "modified",
        letter: "M",
        detail: "+2 −1",
        title: "Show src/main.rs in VS Code's diff editor",
        index: 0,
      },
      {
        path: "docs/new.md",
        status: "added",
        letter: "A",
        detail: "binary",
        title: "Show docs/new.md in VS Code's diff editor",
        index: 1,
      },
    ]);
  });
});

describe("opening a change in VS Code's diff editor", () => {
  it("diffs a modified file against the run's baseline", () => {
    assert.deepEqual(diffPlan("abc123", file()), {
      title: "src/main.rs (modified)",
      path: "src/main.rs",
      baseline: "abc123",
      present: true,
    });
  });

  it("has nothing to compare a file the run added against", () => {
    const plan = diffPlan("abc123", file({ status: "added" }));
    assert.equal(plan.baseline, null);
    assert.equal(plan.present, true);
  });

  it("reads a file the run removed from the baseline, since it is gone", () => {
    const plan = diffPlan("abc123", file({ status: "deleted" }));
    assert.equal(plan.baseline, "abc123");
    assert.equal(plan.present, false);
  });

  it("reads the baseline side through the CLI, out of the shadow snapshot", () => {
    assert.deepEqual(changeArgs("src/main.rs", "abc123", "/tmp/project"), [
      "changes",
      "show",
      "src/main.rs",
      "--baseline",
      "abc123",
      "--project",
      "/tmp/project",
    ]);
    // The scheme the host's content provider serves that side over is the one
    // the plan's URIs are built with.
    assert.equal(CHANGE_SCHEME, "oxide-changes");
  });

  it("puts a turn back through the CLI's own restore", () => {
    assert.deepEqual(undoArgs("/tmp/project", "abc123", "def456"), [
      "changes",
      "undo",
      "--baseline",
      "abc123",
      "--project",
      "/tmp/project",
      "--after",
      "def456",
    ]);
    // A CLI too old to report the state it left behind still restores, taken at
    // its word rather than refusing every card.
    assert.deepEqual(undoArgs("/tmp/project", "abc123", ""), [
      "changes",
      "undo",
      "--baseline",
      "abc123",
      "--project",
      "/tmp/project",
    ]);
  });

  // The provider is handed the URI alone, so the project and revision have to
  // survive the round trip through its query: a card opened in a window that has
  // moved to another root still reads the snapshot its own run started from.
  it("round trips the project and revision a snapshot URI carries", () => {
    const query = snapshotQuery("/tmp/my project", "abc123");
    assert.deepEqual(parseSnapshotQuery(query), {
      project: "/tmp/my project",
      revision: "abc123",
    });
    assert.deepEqual(parseSnapshotQuery(snapshotQuery("/tmp/p", null)), {
      project: "/tmp/p",
      revision: "",
    });
    // A query that is not ours reads as a side with no content.
    assert.deepEqual(parseSnapshotQuery("abc123"), { project: "", revision: "" });
    assert.deepEqual(parseSnapshotQuery(""), { project: "", revision: "" });
  });
});

describe("the change card in the transcript", () => {
  const listing = {
    files: [
      { path: "src/main.rs", status: "modified", added: 2, removed: 1, binary: false },
      { path: "docs/old.md", status: "deleted", added: 0, removed: 8, binary: false },
    ],
  };

  it("pushes a card when the turn reports what it changed", () => {
    const transcript = new Transcript();
    const messages = transcript.apply(frame(listing));
    assert.equal(messages.length, 1);
    const message = messages[0] as unknown as { k: string; item: Record<string, unknown> };
    assert.equal(message.k, "push");
    assert.equal(message.item.kind, "changes");
    assert.equal(message.item.title, "Edited 2 files");
    assert.equal(message.item.totals, "+2 −9");
    assert.equal(message.item.baseline, "abc123");
    // The card carries the folder its run started in, so a row clicked after the
    // window moved to another root opens the right file and snapshot.
    assert.equal(message.item.project, "/tmp/project");
    assert.deepEqual((message.item.rows as { letter: string }[]).map((row) => row.letter), [
      "M",
      "D",
    ]);
    // The newest turn's card is the one that may be put back, and it carries the
    // state it left so the CLI can refuse a card the work tree has moved past.
    assert.equal(message.item.after, "def456");
    assert.equal(message.item.undoable, true);
    assert.equal(message.item.undone, false);
    // Two files fit, so nothing folds away behind a row of its own.
    assert.equal(message.item.more, null);
  });

  it("takes the Undo away from the cards a newer turn came after", () => {
    const transcript = new Transcript();
    const first = transcript.apply(frame(listing))[0] as unknown as { k: string; item: { id: number } };
    const messages = transcript.apply(frame(listing));
    // The new card is pushed, and every card before it is told it is no longer
    // the newest — an older restore would take this turn's work with it.
    assert.deepEqual(
      messages.map((message) => message.k),
      ["changes", "push"],
    );
    const settled = messages[0] as unknown as { id: number; undoable: boolean; undone: boolean };
    assert.equal(settled.id, first.item.id);
    assert.equal(settled.undoable, false);
    assert.equal(settled.undone, false, "it was not undone, it can no longer be");
  });

  it("settles a card whose turn was put back, and only once", () => {
    const transcript = new Transcript();
    transcript.apply(frame(listing));
    const card = transcript.items[0] as { id: number };
    assert.deepEqual(transcript.markUndone(card.id), [
      { k: "changes", id: card.id, undoable: false, undone: true },
    ]);
    // A card already put back is settled again without a second restore, which
    // is what a click from the other pane sends after the first one landed.
    assert.deepEqual(transcript.markUndone(card.id), [
      { k: "changes", id: card.id, undoable: false, undone: true },
    ]);
    // A card the transcript no longer holds is a stale click, not a restore.
    assert.equal(transcript.markUndone(card.id + 7), null);
  });

  it("pushes nothing for a turn that changed no files", () => {
    const transcript = new Transcript();
    assert.deepEqual(transcript.apply(frame({ files: [] })), []);
    assert.equal(transcript.items.length, 0);
  });

  it("finds the card a stale click names, and nothing for an id it never made", () => {
    const transcript = new Transcript();
    transcript.apply(frame(listing));
    const card = transcript.items[0];
    assert.equal(transcript.changes(card.id), card);
    assert.equal(transcript.changes(card.id + 7), null);
  });

  it("replays the card into a pane that attaches later", () => {
    const transcript = new Transcript();
    transcript.apply(frame(listing));
    const state = transcript.state(noState);
    assert.deepEqual(
      state.items.map((item) => item.kind),
      ["changes"],
    );
  });
});
