// The saved "Always allow" rules, read through `oxide approvals`: the arguments
// name the folder rather than inheriting the process's own, so a listing always
// describes the project the panel is showing, and a malformed answer reads as no
// rules rather than throwing in the middle of a click.

import assert from "node:assert/strict";
import { describe, it } from "node:test";

import { approvalsArgs, approvalsClearArgs, parseApprovals } from "../core/permissions";

describe("approvalsArgs", () => {
  it("names the project it lists and the one it clears", () => {
    assert.deepEqual(approvalsArgs("/work/oxide"), [
      "approvals",
      "list",
      "--json",
      "--project",
      "/work/oxide",
    ]);
    assert.deepEqual(approvalsClearArgs("/work/oxide"), [
      "approvals",
      "clear",
      "--json",
      "--project",
      "/work/oxide",
    ]);
  });
});

describe("parseApprovals", () => {
  it("reads the tools a project allows without prompting", () => {
    const listing = parseApprovals(
      JSON.stringify({ project: "/work/oxide", tools: ["bash", "edit", "bash"] }),
    );
    assert.deepEqual(listing, { project: "/work/oxide", tools: ["bash", "edit", "bash"] });
  });

  it("reads anything else as no rules rather than throwing", () => {
    // A CLI too old to know the command, a partial write, and a project whose
    // rules were all cleared: an empty list, and the caller says why.
    for (const raw of ["", "error: unrecognized subcommand 'approvals'", "{}", "[]"]) {
      assert.deepEqual(parseApprovals(raw), { project: "", tools: [] });
    }
    assert.deepEqual(parseApprovals('{"tools":[1,null,"  "]}'), { project: "", tools: [] });
  });
});
