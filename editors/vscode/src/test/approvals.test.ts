// The approval path is the one the extension cannot test against a live model:
// the request a real run emits is parsed here, the title is what the card shows
// beside the tool name, and the frames are what the CLI reads to answer it.

import assert from "node:assert/strict";
import { describe, it } from "node:test";

import {
  approvalLabel,
  approvalRequest,
  approvalTitle,
  isApprovalDecision,
} from "../core/approvals";
import { approvalFrame, promptFrame, quitFrame } from "../core/rpc";

describe("approvalRequest", () => {
  it("reads the id, tool and detail the CLI emits", () => {
    assert.deepEqual(
      approvalRequest({ type: "approval_request", id: 3, toolName: "bash", detail: "rm -rf /tmp" }),
      { id: 3, tool: "bash", detail: "rm -rf /tmp" },
    );
  });

  it("keeps a request whose detail is missing", () => {
    assert.deepEqual(approvalRequest({ type: "approval_request", id: 1, toolName: "edit" }), {
      id: 1,
      tool: "edit",
      detail: "",
    });
  });

  // A card whose answer could never reach the broker would hold the turn until
  // the request times out, so a request without an id is dropped instead.
  it("refuses a request without a usable id", () => {
    for (const id of [undefined, null, 0, -1, "3", Number.NaN]) {
      assert.equal(approvalRequest({ type: "approval_request", id, toolName: "bash" }), null);
    }
  });

  it("names an unnamed tool", () => {
    assert.deepEqual(approvalRequest({ type: "approval_request", id: 2 }), {
      id: 2,
      tool: "tool",
      detail: "",
    });
  });
});

describe("approvalTitle", () => {
  it("names what the tool would do", () => {
    assert.equal(approvalTitle("bash"), "Run a shell command");
    assert.equal(approvalTitle("write"), "Write a file");
    assert.equal(approvalTitle("edit"), "Edit a file");
    // A saved rule may still name the legacy tool, so those read as well as the
    // Pi names do.
    assert.equal(approvalTitle("write_file"), "Write a file");
    assert.equal(approvalTitle("list_dir"), "List a directory");
  });

  it("names an MCP tool after the server it belongs to", () => {
    assert.equal(approvalTitle("files__read_file"), "Call read_file on files");
  });

  it("falls back to the tool's own name", () => {
    assert.equal(approvalTitle("semantic_search"), "Run semantic_search");
    assert.equal(approvalTitle("  "), "Run a tool");
  });
});

describe("approval answers", () => {
  it("accepts only the three decisions the CLI understands", () => {
    for (const decision of ["once", "always", "deny"]) {
      assert.equal(isApprovalDecision(decision), true);
    }
    for (const decision of ["allow", "yes", "", undefined, 1, null]) {
      assert.equal(isApprovalDecision(decision), false);
    }
  });

  it("labels each answer, naming the project for a saved rule", () => {
    assert.equal(approvalLabel("once"), "Allowed once");
    assert.equal(approvalLabel("always"), "Always allowed in this project");
    assert.equal(approvalLabel("deny"), "Denied");
  });
});

describe("rpc frames", () => {
  const parsed = (frame: string): unknown => JSON.parse(frame.trim());

  it("sends the prompt with its images on one line", () => {
    const frame = promptFrame("fix the build", ["/tmp/a.png", "/tmp/b.pdf"]);
    assert.equal(frame.endsWith("\n"), true);
    assert.equal(frame.indexOf("\n"), frame.length - 1);
    assert.deepEqual(parsed(frame), {
      type: "prompt",
      message: "fix the build",
      images: ["/tmp/a.png", "/tmp/b.pdf"],
    });
  });

  it("sends an empty image list rather than omitting it", () => {
    assert.deepEqual(parsed(promptFrame("hello")), {
      type: "prompt",
      message: "hello",
      images: [],
    });
  });

  it("answers an approval by id", () => {
    assert.deepEqual(parsed(approvalFrame(7, "always")), {
      type: "approval",
      id: 7,
      decision: "always",
    });
  });

  it("ends the session with a quit", () => {
    assert.deepEqual(parsed(quitFrame()), { type: "quit" });
  });
});
