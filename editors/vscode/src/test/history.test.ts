// `oxide sessions show <id> --json` is how the panel learns what a thread it is
// resuming already holds. The reader is tolerant like the others here: a field
// that is missing or of the wrong type reads as empty rather than throwing in
// the middle of a resume.

import assert from "node:assert/strict";
import { describe, it } from "node:test";

import { HISTORY_MESSAGES, parseSessionHistory } from "../core/history";

const payload = {
  id: "fe0031b1",
  name: "Fix the flaky test",
  cwd: "/repo",
  path: "/sessions/fe0031b1.jsonl",
  messageCount: 4,
  shown: 4,
  messages: [
    { role: "user", content: "why is the test flaky?", toolCalls: [], toolCallId: null },
    { role: "assistant", content: "", toolCalls: [{ id: "c1", name: "read", arguments: "{}" }] },
    { role: "tool", content: "fn test() {}", toolCallId: "c1" },
    { role: "assistant", content: "it polls the clock", toolCalls: [] },
  ],
  usage: {
    input: 100,
    output: 40,
    cacheRead: 900,
    cacheWrite: 10,
    cost: 0.25,
    cacheHitRate: 90,
    contextTokens: 1010,
    messageCount: 4,
  },
};

describe("stored session history", () => {
  it("reads what was said, oldest first", () => {
    const history = parseSessionHistory(JSON.stringify(payload));
    assert.ok(history);
    assert.equal(history.id, "fe0031b1");
    assert.equal(history.name, "Fix the flaky test");
    assert.deepEqual(history.messages, [
      { role: "user", text: "why is the test flaky?" },
      { role: "assistant", text: "it polls the clock" },
    ]);
    assert.equal(history.total, 4);
    assert.equal(history.shown, 4);
  });

  it("leaves out the parts of a thread that are not the conversation", () => {
    const history = parseSessionHistory(JSON.stringify(payload));
    assert.ok(history);
    // The tool result and the tool-only assistant step are not painted: they
    // would bury what the thread was about.
    assert.equal(history.messages.length, 2);
    assert.ok(!history.messages.some((message) => message.text.includes("fn test")));
  });

  it("carries the totals the footer shows, so a resumed thread is not zeroed", () => {
    const history = parseSessionHistory(JSON.stringify(payload));
    assert.ok(history);
    assert.deepEqual(history.usage, {
      input: 100,
      output: 40,
      cacheRead: 900,
      cacheWrite: 10,
      cost: 0.25,
      contextTokens: 1010,
      cacheHit: 90,
    });
  });

  it("says how much of a long thread it was given", () => {
    const history = parseSessionHistory(
      JSON.stringify({ ...payload, messageCount: 400, shown: 60, name: null }),
    );
    assert.ok(history);
    assert.equal(history.total, 400);
    assert.equal(history.shown, 60, "the tool steps it sent count as messages too");
    assert.equal(history.name, "");
  });

  it("keeps a thread whose tail holds nothing paintable", () => {
    const history = parseSessionHistory(
      JSON.stringify({
        id: "abc123",
        name: null,
        messageCount: 12,
        shown: 1,
        messages: [{ role: "tool", content: "output" }],
        usage: {},
      }),
    );
    assert.ok(history);
    assert.deepEqual(history.messages, [], "a tool result is not painted");
    // The size still comes from the session, not from what was returned.
    assert.equal(history.total, 12);
    assert.equal(history.shown, 1, "the CLI sent one message, whatever it was");
    assert.equal(history.usage.contextTokens, 0);
    assert.equal(history.usage.cacheHit, null);
  });

  it("returns nothing for output that is not a session", () => {
    assert.equal(parseSessionHistory(""), null);
    assert.equal(parseSessionHistory("no session `x` for this project"), null);
    assert.equal(parseSessionHistory("[]"), null);
    assert.equal(parseSessionHistory('{"error":"nope"}'), null);
  });

  it("reads a body that came back with Windows line endings", () => {
    const crlf = `${JSON.stringify(payload, null, 2).replace(/\n/g, "\r\n")}\r\n`;
    const history = parseSessionHistory(crlf);
    assert.ok(history);
    assert.deepEqual(history.messages, [
      { role: "user", text: "why is the test flaky?" },
      { role: "assistant", text: "it polls the clock" },
    ]);
  });

  it("reports the size of the tail it asks for", () => {
    assert.equal(HISTORY_MESSAGES, 60);
  });
});
