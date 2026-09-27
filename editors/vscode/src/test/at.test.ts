// The composer's `@path` completion, as the terminal resolves the same token:
// a reference typed into the message box has to offer the project's own files
// and folders, and hand the caret on so the path can be narrowed inside a
// folder.

import assert from "node:assert/strict";
import { describe, it } from "node:test";

import { atSuggestions, atToken, type AtToken } from "../core/at";

/// The token a value and caret stand for, asserted whole so a shifted index
/// cannot pass.
function token(value: string, caret = value.length): AtToken | null {
  return atToken(value, caret);
}

describe("the @path token at the caret", () => {
  it("reads a reference typed at the start of the box", () => {
    assert.deepEqual(token("@"), { start: 0, end: 1, query: "" });
    assert.deepEqual(token("@src"), { start: 0, end: 4, query: "src" });
    assert.deepEqual(token("@crates/core/src/agent.rs"), {
      start: 0,
      end: 25,
      query: "crates/core/src/agent.rs",
    });
  });

  it("reads a reference typed after other words", () => {
    assert.deepEqual(token("look at @src/ma"), {
      start: 8,
      end: 15,
      query: "src/ma",
    });
    assert.deepEqual(token("and\n@docs"), { start: 4, end: 9, query: "docs" });
  });

  it("completes the whole token when the caret sits inside one", () => {
    // Half of a path typed, then the caret walked back into it: the reference
    // being completed is the one the caret is in, not the part behind it.
    assert.deepEqual(token("@src/main.rs", 5), { start: 0, end: 12, query: "src/main.rs" });
    assert.deepEqual(token("see @src/main.rs and", 6), {
      start: 4,
      end: 16,
      query: "src/main.rs",
    });
  });

  it("ignores an @ that is not the start of a word", () => {
    // An address is not a path, and neither is an `@` the caret has left.
    assert.equal(token("mail me at a@b.com"), null);
    assert.equal(token("mail me at a@b.com", 13), null);
    assert.equal(token("@src "), null);
    assert.equal(token("@src", 0), null);
  });

  it("takes the token up to the next whitespace", () => {
    assert.deepEqual(token("@src/main.rs and docs", 12), {
      start: 0,
      end: 12,
      query: "src/main.rs",
    });
    // A value whose caret is at the end has no `@` in front of it at all.
    assert.equal(token("no reference here"), null);
    assert.equal(token(""), null);
  });
});

const paths = [
  "crates/core/src/",
  "crates/core/src/agent.rs",
  "crates/core/src/tools.rs",
  "crates/desktop/ui/app.js",
  "docs/",
  "docs/vscode.md",
  "editors/vscode/src/chat.ts",
  "src/main.rs",
];

function rows(value: string, caret = value.length) {
  return atSuggestions(paths, token(value, caret));
}

describe("the rows offered for a token", () => {
  it("leaves a file's row ready for the next word and a folder's open", () => {
    assert.deepEqual(rows("@docs/vscode.md").slice(0, 1), [
      { label: "docs/vscode.md", kind: "file", insert: "@docs/vscode.md " },
    ]);
    // A folder keeps the token open so the query goes on narrowing inside it.
    assert.deepEqual(rows("@docs").slice(0, 1), [
      { label: "docs/", kind: "folder", insert: "@docs/" },
    ]);
  });

  it("offers a folder beside the files under it", () => {
    assert.deepEqual(
      rows("@crates/core/").map((row) => row.label),
      ["crates/core/src/", "crates/core/src/agent.rs", "crates/core/src/tools.rs"],
    );
    // A folder the reference already spells is left out, so taking a row walks
    // into it instead of completing it to what is already typed.
    assert.deepEqual(
      rows("@crates/core/src/").map((row) => row.label),
      ["crates/core/src/agent.rs", "crates/core/src/tools.rs"],
    );
  });

  it("puts the nearest name first, then the path, then a mention", () => {
    // The nearest name first — the folder called `src` — then the path that
    // starts with the query, then the paths that only mention it.
    assert.deepEqual(
      rows("@src").map((row) => row.label),
      [
        "crates/core/src/",
        "src/main.rs",
        "crates/core/src/agent.rs",
        "crates/core/src/tools.rs",
        "editors/vscode/src/chat.ts",
      ],
    );
    // A query with a `/` in it is a path being walked, so the whole prefix is
    // what counts rather than the name at the end of it.
    assert.deepEqual(
      rows("@crates/core/src/ag").map((row) => row.label),
      ["crates/core/src/agent.rs"],
    );
  });

  it("matches case-insensitively", () => {
    assert.deepEqual(
      rows("@AGENT").map((row) => row.label),
      ["crates/core/src/agent.rs"],
    );
    assert.deepEqual(rows("@Docs/VsCode").map((row) => row.label), ["docs/vscode.md"]);
  });

  it("offers the whole list for a bare @, capped", () => {
    assert.equal(rows("@").length, paths.length);
    assert.deepEqual(
      rows("@").map((row) => row.label),
      [...paths].sort(),
    );
    assert.equal(rows("@", 1).length, paths.length, "the caret does not change which rows answer");
    assert.equal(atSuggestions(paths, token("@"), 3).length, 3);
    assert.equal(atSuggestions(paths, token("@"), 0).length, 0);
  });

  it("offers nothing for a path that is not in the project", () => {
    assert.deepEqual(rows("@nowhere/at/all.txt"), []);
    assert.deepEqual(atSuggestions(paths, null), [], "no token, no rows");
    // Stray punctuation is part of the token, as it is for the CLI's expansion,
    // so a reference followed by a comma simply matches nothing.
    assert.deepEqual(rows("@docs/vscode.md,"), []);
  });
});
