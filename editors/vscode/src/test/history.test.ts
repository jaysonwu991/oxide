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
  it("reads what was said and what was called, oldest first", () => {
    const history = parseSessionHistory(JSON.stringify(payload));
    assert.ok(history);
    assert.equal(history.id, "fe0031b1");
    assert.equal(history.name, "Fix the flaky test");
    assert.deepEqual(history.entries, [
      { kind: "user", text: "why is the test flaky?" },
      { kind: "tool", name: "read", args: "{}", output: "fn test() {}", isError: false },
      { kind: "assistant", text: "it polls the clock" },
    ]);
    assert.equal(history.total, 4);
    assert.equal(history.shown, 4);
  });

  it("keeps the media a stored message carried", () => {
    // `oxide sessions show --json` reports what a message carried under the same
    // two field names the desktop app's own view of a stored thread uses, so a
    // resumed conversation shows the pictures in it rather than a list of names.
    const history = parseSessionHistory(
      JSON.stringify({
        ...payload,
        messages: [
          {
            role: "user",
            content: "what is this?\n[image]",
            attachments: [
              { name: "image", dataUrl: "data:image/png;base64,QUJD" },
              { name: "spec.pdf", dataUrl: "data:application/pdf;base64,REVG" },
              { name: "scan.tiff", dataUrl: "data:image/tiff;base64,VEk=" },
            ],
          },
        ],
      }),
    );
    assert.ok(history);
    assert.deepEqual(history.entries, [
      {
        kind: "user",
        text: "what is this?\n[image]",
        attachments: [
          // Only a picture this webview can draw keeps its bytes: a PDF's and a
          // TIFF's data URL would be painted as a broken image where the chip
          // promises a name.
          { label: "image", preview: "data:image/png;base64,QUJD" },
          { label: "spec.pdf", preview: null },
          { label: "scan.tiff", preview: null },
        ],
      },
    ]);
  });

  it("reads an attachment with no picture as its name alone", () => {
    // A CLI that names the media without the bytes still says what was carried:
    // the chip is the name it reports rather than a chip with nothing in it.
    const history = parseSessionHistory(
      JSON.stringify({
        ...payload,
        messages: [
          { role: "user", content: "see this", attachments: [{ name: "chart.png" }] },
        ],
      }),
    );
    assert.ok(history);
    assert.deepEqual(history.entries, [
      { kind: "user", text: "see this", attachments: [{ label: "chart.png", preview: null }] },
    ]);
  });

  it("pairs each call with the result that answered it", () => {
    const history = parseSessionHistory(
      JSON.stringify({
        ...payload,
        messages: [
          {
            role: "assistant",
            content: "reading both",
            toolCalls: [
              { id: "c1", name: "read", arguments: '{"path":"a.rs"}' },
              { id: "c2", name: "grep", arguments: '{"pattern":"x"}' },
            ],
          },
          { role: "tool", content: "first", toolCallId: "c1" },
          { role: "tool", content: "second", toolCallId: "c2" },
        ],
      }),
    );
    assert.ok(history);
    // Text first, then the calls it made, each carrying its own result rather
    // than a card per stored message.
    assert.deepEqual(history.entries, [
      { kind: "assistant", text: "reading both" },
      { kind: "tool", name: "read", args: '{"path":"a.rs"}', output: "first", isError: false },
      { kind: "tool", name: "grep", args: '{"pattern":"x"}', output: "second", isError: false },
    ]);
  });

  it("drops a result whose call the tail left out", () => {
    const history = parseSessionHistory(
      JSON.stringify({
        id: "abc123",
        name: null,
        messageCount: 12,
        shown: 1,
        messages: [{ role: "tool", content: "output", toolCallId: "gone" }],
        usage: {},
      }),
    );
    assert.ok(history);
    // Nothing names the tool or holds its arguments, so there is no card.
    assert.deepEqual(history.entries, []);
    // The size still comes from the session, not from what was returned.
    assert.equal(history.total, 12);
    assert.equal(history.shown, 1, "the CLI sent one message, whatever it was");
    assert.equal(history.usage.contextTokens, 0);
    assert.equal(history.usage.cacheHit, null);
  });

  it("replays the stored compatibility marker as structured error state", () => {
    const history = parseSessionHistory(
      JSON.stringify({
        ...payload,
        messages: [
          { role: "assistant", content: "", toolCalls: [{ id: "c1", name: "edit", arguments: "{}" }] },
          { role: "tool", content: "error: oldText not found", toolCallId: "c1" },
        ],
      }),
    );
    assert.ok(history);
    assert.deepEqual(history.entries, [
      { kind: "tool", name: "edit", args: "{}", output: "oldText not found", isError: true },
    ]);
  });

  it("keeps a thread whose tail holds nothing paintable", () => {
    const history = parseSessionHistory(
      JSON.stringify({
        id: "abc123",
        name: null,
        messageCount: 4,
        shown: 2,
        messages: [
          { role: "assistant", content: "   ", toolCalls: [] },
          { role: "system", content: "note to self" },
        ],
        usage: {},
      }),
    );
    assert.ok(history);
    assert.deepEqual(history.entries, [], "an empty step and a system note paint nothing");
    assert.equal(history.total, 4);
    assert.equal(history.shown, 2);
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
    assert.deepEqual(history.entries, [
      { kind: "user", text: "why is the test flaky?" },
      { kind: "tool", name: "read", args: "{}", output: "fn test() {}", isError: false },
      { kind: "assistant", text: "it polls the clock" },
    ]);
  });

  it("reports the size of the tail it asks for", () => {
    assert.equal(HISTORY_MESSAGES, 60);
  });
});
