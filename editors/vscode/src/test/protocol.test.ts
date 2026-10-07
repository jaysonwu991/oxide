import assert from "node:assert/strict";
import { describe, it } from "node:test";

import {
  drainLines,
  parseEvent,
  summarizeTitle,
  Transcript,
  type AssistantItem,
  type Item,
  type ToolItem,
  type ViewMessage,
} from "../core/protocol";
import type { FooterState } from "../core/footer";

/// The status message carries the footer the controller composed, so the checks
/// below need one; its contents are covered by `footer.test.ts`.
const footer: FooterState = { chips: [], info: "", usage: "", percent: null, level: "ok" };

/// `assert.ok` is the only node assertion with an assertion signature, so the
/// discriminated union is narrowed through it.
function push(message: ViewMessage | undefined): Item {
  assert.ok(message, "expected a message");
  assert.ok(message.k === "push", `expected a push message, got ${message.k}`);
  return message.item;
}

function status(message: ViewMessage) {
  assert.ok(message.k === "status", `expected a status message, got ${message.k}`);
  return message;
}

function appended(message: ViewMessage | undefined) {
  assert.ok(message, "expected a message");
  assert.ok(message.k === "append", `expected an append message, got ${message.k}`);
  return message;
}

function patched(message: ViewMessage | undefined) {
  assert.ok(message, "expected a message");
  assert.ok(message.k === "patch", `expected a patch message, got ${message.k}`);
  return message;
}

function usage(message: ViewMessage | undefined) {
  assert.ok(message, "expected a message");
  assert.ok(message.k === "usage", `expected a usage message, got ${message.k}`);
  return message;
}

function removed(message: ViewMessage | undefined) {
  assert.ok(message, "expected a message");
  assert.ok(message.k === "remove", `expected a remove message, got ${message.k}`);
  return message;
}

function card(transcript: Transcript, index = 0): ToolItem {
  const tools = transcript.items.filter((item): item is ToolItem => item.kind === "tool");
  assert.ok(tools[index], "expected a tool item");
  return tools[index];
}

function notice(item: Item) {
  assert.ok(item.kind === "notice", `expected a notice, got ${item.kind}`);
  return item;
}

describe("drainLines", () => {
  it("splits complete lines and keeps the remainder", () => {
    const first = drainLines("", '{"a":1}\n{"b":2}\n{"c"');
    assert.deepEqual(first.lines, ['{"a":1}', '{"b":2}']);
    assert.equal(first.rest, '{"c"');

    const second = drainLines(first.rest, ':3}\n');
    assert.deepEqual(second.lines, ['{"c":3}']);
    assert.equal(second.rest, "");
  });

  it("strips carriage returns so CRLF output parses", () => {
    assert.deepEqual(drainLines("", "one\r\ntwo\r\n").lines, ["one", "two"]);
  });

  it("returns nothing for a chunk without a newline", () => {
    const result = drainLines("", "partial");
    assert.deepEqual(result.lines, []);
    assert.equal(result.rest, "partial");
  });
});

describe("parseEvent", () => {
  it("ignores blank lines and non-object payloads", () => {
    assert.equal(parseEvent(""), null);
    assert.equal(parseEvent("   "), null);
    assert.equal(parseEvent("not json"), null);
    assert.equal(parseEvent("[1,2]"), null);
    assert.equal(parseEvent('"text"'), null);
    assert.equal(parseEvent("null"), null);
  });

  it("parses an object line", () => {
    assert.deepEqual(parseEvent('{"type":"session","id":"abc"}'), { type: "session", id: "abc" });
  });
});

describe("Transcript", () => {
  it("records the session id from the header", () => {
    const transcript = new Transcript();
    assert.deepEqual(transcript.apply({ type: "session", id: "s1", cwd: "/tmp" }), []);
    assert.equal(transcript.sessionId, "s1");
  });

  it("streams reasoning and text into separate items", () => {
    const transcript = new Transcript();
    assert.deepEqual(transcript.apply({ type: "thinking" }), []);
    const messages = [
      transcript.apply({
        type: "message_update",
        assistantMessageEvent: { type: "thinking_delta", delta: "because " },
      }),
      transcript.apply({
        type: "message_update",
        assistantMessageEvent: { type: "text_delta", delta: "Hello " },
      }),
      transcript.apply({
        type: "message_update",
        assistantMessageEvent: { type: "text_delta", delta: "world" },
      }),
    ];
    // The first delta of a block pushes it, so the view has the id to append to.
    assert.equal(push(messages[0][0]).kind, "thinking");
    appended(messages[0][1]);
    assert.equal(push(messages[1][0]).kind, "assistant");
    appended(messages[1][1]);
    appended(messages[2][0]);

    assert.deepEqual(
      transcript.items.map((item) => item.kind),
      ["thinking", "assistant"],
    );
    const [thinking, assistant] = transcript.items;
    assert.ok(thinking.kind === "thinking" && assistant.kind === "assistant");
    assert.equal(thinking.text, "because ");
    assert.equal(assistant.text, "Hello world");
    assert.equal(transcript.status, "Writing…");
  });

  it("pushes an empty bubble before the first text delta so it is not painted twice", () => {
    const transcript = new Transcript();
    const messages = transcript.apply({
      type: "message_update",
      assistantMessageEvent: { type: "text_delta", delta: "Still" },
    });
    const pushed = push(messages[0]);
    assert.ok(pushed.kind === "assistant");
    assert.equal(pushed.text, "");
    appended(messages[1]);
    assert.deepEqual(
      transcript.items.map((item) => (item.kind === "assistant" ? item.text : "")),
      ["Still"],
    );
  });

  it("leaves no empty thinking block when a turn reasons without a delta", () => {
    const transcript = new Transcript();
    transcript.apply({ type: "thinking" });
    transcript.apply({
      type: "message_update",
      assistantMessageEvent: { type: "text_delta", delta: "no reasoning here" },
    });
    assert.deepEqual(
      transcript.items.map((item) => item.kind),
      ["assistant"],
    );
  });

  it("starts a new assistant item after a tool call", () => {
    const transcript = new Transcript();
    transcript.apply({
      type: "message_update",
      assistantMessageEvent: { type: "text_delta", delta: "first" },
    });
    transcript.apply({ type: "tool_call", toolName: "read", arguments: '{"path":"a.rs"}' });
    transcript.apply({
      type: "message_update",
      assistantMessageEvent: { type: "text_delta", delta: "second" },
    });
    assert.deepEqual(
      transcript.items.map((item) => item.kind),
      ["assistant", "tool", "assistant"],
    );
    const texts = transcript.items
      .filter((item): item is AssistantItem => item.kind === "assistant")
      .map((item) => item.text);
    assert.deepEqual(texts, ["first", "second"]);
  });

  it("builds a tool card from the call and finishes it with the result", () => {
    const transcript = new Transcript((name) => (name === "edit" ? "diff-text" : null));
    const pushed = transcript.apply({
      type: "tool_call",
      toolName: "edit",
      arguments: '{"path":"src/a.rs","edits":[]}',
    });
    const item = push(pushed[0]);
    assert.equal(item.kind, "tool");
    assert.equal(card(transcript).running, true);
    assert.equal(card(transcript).diff, "diff-text");
    assert.equal(card(transcript).args, '{"path":"src/a.rs","edits":[]}');
    assert.equal(transcript.status, "Running edit…");

    const live = transcript.apply({
      type: "tool_execution_update",
      toolName: "edit",
      partialResult: "partial",
    });
    assert.deepEqual(live, [{ k: "append", id: card(transcript).id, field: "output", delta: "partial" }]);

    const ended = transcript.apply({
      type: "tool_execution_end",
      toolName: "edit",
      result: "Successfully replaced 1 block(s) in src/a.rs.",
      isError: false,
    });
    assert.deepEqual(patched(ended[0]).patch, {
      output: "Successfully replaced 1 block(s) in src/a.rs.",
      running: false,
      isError: false,
    });
    assert.equal(card(transcript).running, false);
    assert.equal(card(transcript).output, "Successfully replaced 1 block(s) in src/a.rs.");
  });

  it("marks a failed tool call and keeps its output", () => {
    const transcript = new Transcript();
    transcript.apply({ type: "tool_call", toolName: "bash", arguments: '{"command":"false"}' });
    transcript.apply({
      type: "tool_execution_end",
      toolName: "bash",
      result: "exit status 1",
      isError: true,
    });
    assert.equal(card(transcript).isError, true);
    assert.equal(card(transcript).output, "exit status 1");
  });

  it("drops the diff a failed call was never going to make", () => {
    const transcript = new Transcript((name) => (name === "edit" ? "diff-text" : null));
    transcript.apply({ type: "tool_call", toolName: "edit", arguments: '{"path":"src/a.rs"}' });
    assert.equal(card(transcript).diff, "diff-text");
    const ended = transcript.apply({
      type: "tool_execution_end",
      toolName: "edit",
      result: "no match for oldText",
      isError: true,
    });
    // The diff comes from the call's own arguments, so a call that failed never
    // made that change: the card shows the error instead of counting lines.
    assert.equal(card(transcript).diff, null);
    assert.equal(ended.length, 1);
    assert.equal((ended[0] as { patch: { diff: null } }).patch.diff, null);
  });

  it("matches parallel results to the call with the same name", () => {
    const transcript = new Transcript();
    transcript.apply({ type: "tool_call", toolName: "read", arguments: '{"path":"a"}' });
    transcript.apply({ type: "tool_call", toolName: "read", arguments: '{"path":"b"}' });
    transcript.apply({ type: "tool_call", toolName: "grep", arguments: '{"pattern":"x"}' });
    transcript.apply({ type: "tool_execution_end", toolName: "grep", result: "hit", isError: false });
    transcript.apply({ type: "tool_execution_end", toolName: "read", result: "first", isError: false });

    assert.equal(card(transcript, 0).output, "first");
    assert.equal(card(transcript, 0).running, false);
    assert.equal(card(transcript, 1).running, true);
    assert.equal(card(transcript, 2).output, "hit");
  });

  it("drops progress when two running cards share a tool name", () => {
    const transcript = new Transcript();
    transcript.apply({ type: "tool_call", toolName: "read", arguments: '{"path":"a"}' });
    transcript.apply({ type: "tool_call", toolName: "read", arguments: '{"path":"b"}' });
    // Progress carries no call id, so it must not be guessed at either card.
    assert.deepEqual(
      transcript.apply({ type: "tool_execution_update", toolName: "read", partialResult: "x" }),
      [],
    );
    assert.equal(card(transcript, 0).output, "");
    assert.equal(card(transcript, 1).output, "");

    // Once only one card is running, its progress lands again.
    transcript.apply({ type: "tool_execution_end", toolName: "read", result: "first", isError: false });
    assert.deepEqual(
      transcript.apply({ type: "tool_execution_update", toolName: "read", partialResult: "y" }),
      [{ k: "append", id: card(transcript, 1).id, field: "output", delta: "y" }],
    );
  });

  it("commits the step on usage so a later retry keeps the reply", () => {
    // Older CLI builds do not send `thinking_done`, so the usage event is the
    // only step boundary before a no-tool step is followed by another request.
    const transcript = new Transcript();
    const reply = push(
      transcript.apply({
        type: "message_update",
        assistantMessageEvent: { type: "text_delta", delta: "summary" },
      })[0],
    );
    transcript.apply({
      type: "usage",
      usage: { input: 1, output: 1, cacheRead: 0, cacheWrite: 0, cost: 0 },
    });
    assert.deepEqual(
      transcript.apply({ type: "auto_retry_start", attempt: 1, maxAttempts: 3 }),
      [],
    );
    assert.equal(transcript.items.length, 1);
    assert.equal(transcript.items[0].id, reply.id);
  });

  it("keeps a re-checked answer in one bubble when the usage boundary is deferred", () => {
    // A summary whose Definition-of-Done re-check is pending streams both
    // answers before the CLI emits the usage event that commits the merged
    // turn, so no boundary may fall between them.
    const transcript = new Transcript();
    const reply = push(
      transcript.apply({
        type: "message_update",
        assistantMessageEvent: { type: "text_delta", delta: "Upgrade complete." },
      })[0],
    );
    assert.deepEqual(
      transcript.apply({
        type: "message_update",
        assistantMessageEvent: { type: "text_delta", delta: " Verified on disk." },
      }),
      [{ k: "append", id: reply.id, field: "text", delta: " Verified on disk." }],
    );
    // The deferred usage commits the single merged step afterwards, and the
    // step marker closes it rather than starting a second bubble.
    transcript.apply({
      type: "usage",
      usage: { input: 1, output: 1, cacheRead: 0, cacheWrite: 0, cost: 0 },
    });
    transcript.apply({ type: "thinking_done" });
    assert.equal(transcript.items.length, 1);
    const item = transcript.items[0];
    assert.equal(item.kind, "assistant");
    assert.equal((item as AssistantItem).text, "Upgrade complete. Verified on disk.");
  });

  it("accumulates usage and tracks the latest context size", () => {
    const transcript = new Transcript();
    usage(
      transcript.apply({
        type: "usage",
        usage: { input: 100, output: 10, cacheRead: 40, cacheWrite: 5, cost: 0.002 },
      })[0],
    );
    usage(
      transcript.apply({
        type: "usage",
        usage: { input: 200, output: 20, cacheRead: 60, cacheWrite: 0, cost: 0.003 },
      })[0],
    );
    assert.deepEqual(transcript.usage, {
      input: 300,
      output: 30,
      cacheRead: 100,
      cacheWrite: 5,
      cost: 0.005,
      contextTokens: 260,
      // The rate is measured against the latest request's own prompt.
      cacheHit: (60 / 260) * 100,
    });
  });

  it("keeps the last hit rate when a later request reads no cache", () => {
    const transcript = new Transcript();
    transcript.apply({
      type: "usage",
      usage: { input: 100, output: 10, cacheRead: 300, cacheWrite: 0, cost: 0 },
    });
    assert.equal(transcript.usage.cacheHit, 75);
    transcript.apply({
      type: "usage",
      usage: { input: 20, output: 10, cacheRead: 0, cacheWrite: 0, cost: 0 },
    });
    assert.equal(transcript.usage.cacheHit, 75);
    assert.equal(transcript.usage.contextTokens, 20);
  });

  it("reports retries as a status change", () => {
    const transcript = new Transcript();
    assert.deepEqual(transcript.apply({ type: "auto_retry_start", attempt: 2, maxAttempts: 5 }), []);
    assert.equal(transcript.status, "Retrying (2/5)…");
  });

  it("discards the partial attempt when a retry starts", () => {
    const transcript = new Transcript();
    const partial = push(
      transcript.apply({
        type: "message_update",
        assistantMessageEvent: { type: "text_delta", delta: "partial" },
      })[0],
    );
    const messages = transcript.apply({ type: "auto_retry_start", attempt: 2, maxAttempts: 5 });
    assert.deepEqual(messages.map((message) => removed(message).id), [partial.id]);
    assert.equal(transcript.items.length, 0);
    // The retry streams fresh: new text opens a new item rather than extending
    // the discarded one.
    const freshMessages = transcript.apply({
      type: "message_update",
      assistantMessageEvent: { type: "text_delta", delta: "done" },
    });
    const fresh = push(freshMessages[0]);
    assert.equal(fresh.kind, "assistant");
    assert.notEqual(fresh.id, partial.id);
    // The pushed item only creates the bubble; the first delta rides on the
    // append that follows it.
    assert.equal((fresh as AssistantItem).text, "");
    appended(freshMessages[1]);
    const freshItem = transcript.items.find((item) => item.id === fresh.id) as AssistantItem;
    assert.equal(freshItem.text, "done");
  });

  it("keeps a committed step when a later step retries", () => {
    const transcript = new Transcript();
    const reply = push(
      transcript.apply({
        type: "message_update",
        assistantMessageEvent: { type: "text_delta", delta: "summary" },
      })[0],
    );
    // The step commits, so the next step runs against the same transcript (the
    // hidden Definition-of-Done reminder is sent as another message).
    transcript.apply({ type: "thinking_done" });
    // The next step fails before emitting anything and retries: its discard
    // must not remove the committed reply.
    assert.deepEqual(
      transcript.apply({ type: "auto_retry_start", attempt: 1, maxAttempts: 3 }),
      [],
    );
    assert.equal(transcript.items.length, 1);
    assert.equal(transcript.items[0].id, reply.id);
  });

  it("discards a streaming reasoning block when a retry starts", () => {
    const transcript = new Transcript();
    const thinking = push(
      transcript.apply({
        type: "message_update",
        assistantMessageEvent: { type: "thinking_delta", delta: "hmm" },
      })[0],
    );
    const messages = transcript.apply({ type: "auto_retry_start", attempt: 2, maxAttempts: 5 });
    assert.deepEqual(messages.map((message) => removed(message).id), [thinking.id]);
    assert.equal(transcript.items.length, 0);
  });

  it("notes a compaction and a stream error", () => {
    const transcript = new Transcript();
    const compacted = notice(push(transcript.apply({ type: "compaction", summarized: 12, tokensBefore: 1200 })[0]));
    assert.equal(compacted.text, "Compacted 12 earlier messages (~1.2k tokens)");
    assert.equal(compacted.tone, "info");

    const failure = notice(push(transcript.apply({ type: "error", message: "boom" })[0]));
    assert.equal(failure.text, "Error: boom");
    assert.equal(failure.tone, "error");
  });

  it("closes the stream on agent_end", () => {
    const transcript = new Transcript();
    transcript.busy = true;
    transcript.apply({
      type: "message_update",
      assistantMessageEvent: { type: "text_delta", delta: "done" },
    });
    transcript.apply({ type: "agent_end", messages: [] });
    transcript.busy = false;
    assert.equal(transcript.status, "Done");
    assert.deepEqual(status(transcript.statusMessage(0, footer)), {
      k: "status",
      status: "Idle",
      busy: false,
      queued: 0,
      footer,
    });
  });

  it("reports the live activity while a turn runs", () => {
    const transcript = new Transcript();
    transcript.busy = true;
    assert.deepEqual(status(transcript.statusMessage(2, footer)), {
      k: "status",
      status: "Thinking…",
      busy: true,
      queued: 2,
      footer,
    });
    transcript.apply({ type: "tool_call", toolName: "bash", arguments: "{}" });
    assert.equal(status(transcript.statusMessage(0, footer)).status, "Running bash…");
  });

  it("ignores unknown events and tolerates missing fields", () => {
    const transcript = new Transcript();
    const messages = [
      ...transcript.apply({}),
      ...transcript.apply({ type: "assistant_message" }),
      ...transcript.apply({ type: "usage" }),
      ...transcript.apply({ type: "message_update" }),
      ...transcript.apply({ type: "tool_execution_end", toolName: "read" }),
    ];
    assert.equal(messages.length, 1);
    usage(messages[0]);
    assert.equal(transcript.usage.input, 0);
    assert.equal(transcript.items.length, 0);
  });

  it("ignores a tool result with no running call", () => {
    const transcript = new Transcript();
    assert.deepEqual(
      transcript.apply({ type: "tool_execution_end", toolName: "read", result: "x" }),
      [],
    );
    assert.deepEqual(transcript.apply({ type: "tool_execution_update", toolName: "read", partialResult: "x" }), []);
  });

  it("records the user turn and the context it carried", () => {
    const transcript = new Transcript();
    const message = push(transcript.pushUser("fix this", [{ label: "src/a.rs:10-12" }])[0]);
    assert.deepEqual(message, {
      id: 1,
      kind: "user",
      text: "fix this",
      context: [{ label: "src/a.rs:10-12", preview: null }],
    });
  });

  it("keeps the picture an attachment was sent with", () => {
    const transcript = new Transcript();
    const item = push(
      transcript.pushUser("what is this?", [
        { label: "screenshot.png", preview: "data:image/png;base64,AAAA" },
      ])[0],
    );
    assert.ok(item.kind === "user", "the message is a user turn");
    assert.deepEqual(item.context, [
      { label: "screenshot.png", preview: "data:image/png;base64,AAAA" },
    ]);
  });

  it("replays a stored thread's turns as finished items", () => {
    const transcript = new Transcript();
    transcript.replay([
      { kind: "user", text: "why is the test flaky?" },
      {
        kind: "tool",
        name: "read",
        args: '{"path":"src/a.rs"}',
        output: "fn test() {}",
        isError: false,
      },
      { kind: "assistant", text: "it polls the clock" },
    ]);
    // No message per entry: the controller repaints the whole list from one
    // `state` afterwards, so replaying forty stored turns is one paint.
    assert.deepEqual(transcript.items, [
      { id: 1, kind: "user", text: "why is the test flaky?", context: [] },
      {
        id: 2,
        kind: "tool",
        name: "read",
        args: '{"path":"src/a.rs"}',
        output: "fn test() {}",
        diff: null,
        running: false,
        isError: false,
        unknown: true,
      },
      { id: 3, kind: "assistant", text: "it polls the clock" },
    ]);
  });

  it("replays a stored tool failure without its compatibility marker", () => {
    const transcript = new Transcript();
    transcript.replay([
      {
        kind: "tool",
        name: "edit",
        args: '{"path":"src/a.rs","oldText":"missing","newText":"x"}',
        output: "oldText not found",
        isError: true,
      },
    ]);
    assert.deepEqual(transcript.items, [
      {
        id: 1,
        kind: "tool",
        name: "edit",
        args: '{"path":"src/a.rs","oldText":"missing","newText":"x"}',
        output: "oldText not found",
        diff: null,
        running: false,
        isError: true,
        unknown: false,
      },
    ]);
  });

  it("replays an edit with the change its own arguments describe", () => {
    const transcript = new Transcript(() => "a diff built from the file on disk now");
    transcript.replay([
      {
        kind: "tool",
        name: "edit",
        args: '{"path":"src/a.rs","edits":[{"oldText":"b","newText":"x"}]}',
        output: "Successfully replaced 1 block(s)",
        isError: false,
      },
      {
        kind: "tool",
        name: "write",
        args: '{"path":"src/a.rs","content":"x"}',
        output: "Wrote",
        isError: false,
      },
    ]);
    const [edit, write] = transcript.items;
    assert.ok(edit.kind === "tool" && write.kind === "tool");
    // Both sides of the replacement came with the call, so the card keeps the
    // change it made; the file as it stands now is never asked, since it is no
    // longer the state the call found.
    assert.match(edit.diff ?? "", /-\s+1\s+b/);
    assert.match(edit.diff ?? "", /\+\s+1\s+x/);
    // A `write` names only what it wrote, so that state is nowhere on disk any
    // more and the card shows the call and its result instead.
    assert.equal(write.diff, null);
    for (const item of [edit, write]) {
      assert.equal(item.running, false);
      assert.equal(item.isError, false, "the store does not say whether it applied");
      assert.equal(
        item.unknown,
        true,
        "a replayed call claims no state: a failed call stores what a landed one does",
      );
    }
  });

  it("starts a fresh thread when reset", () => {
    const transcript = new Transcript();
    transcript.apply({ type: "session", id: "s" });
    transcript.pushUser("hi", []);
    transcript.reset();
    assert.deepEqual(transcript.items, []);
    assert.equal(transcript.sessionId, null);
    assert.equal(transcript.usage.cost, 0);
    assert.equal(transcript.status, "Idle");
  });

  it("pushes an approval card that waits for an answer", () => {
    const transcript = new Transcript();
    const messages = transcript.apply({
      type: "approval_request",
      id: 12,
      toolName: "bash",
      detail: "rm -rf build",
    });
    const item = push(messages[0]);
    assert.ok(item.kind === "approval");
    assert.equal(item.requestId, 12);
    assert.equal(item.tool, "bash");
    assert.equal(item.title, "Run a shell command");
    assert.equal(item.detail, "rm -rf build");
    assert.equal(item.state, "pending");
    assert.equal(item.label, "");
    assert.equal(transcript.status, "Waiting for approval…");
  });

  it("ignores a request it could not answer", () => {
    const transcript = new Transcript();
    assert.deepEqual(transcript.apply({ type: "approval_request", toolName: "bash" }), []);
    assert.deepEqual(transcript.items, []);
  });

  it("records the answer on the card it belongs to", () => {
    const transcript = new Transcript();
    for (const id of [1, 2]) {
      transcript.apply({
        type: "approval_request",
        id,
        toolName: id === 1 ? "bash" : "edit",
        detail: "",
      });
    }
    const messages = transcript.answerApproval(2, "always");
    assert.ok(messages);
    assert.deepEqual(messages[0], {
      k: "approval",
      id: 2,
      state: "always",
      label: "Always allowed in this project",
    });
    const cards = transcript.items.filter((item) => item.kind === "approval");
    assert.deepEqual(
      cards.map((entry) => [entry.id, entry.state]),
      [
        [1, "pending"],
        [2, "always"],
      ],
    );
  });

  // The same answer arriving twice (a double click, or a replayed message) must
  // not send a second frame: the controller uses `null` to tell.
  it("answers a request once", () => {
    const transcript = new Transcript();
    transcript.apply({ type: "approval_request", id: 5, toolName: "bash", detail: "" });
    assert.ok(transcript.answerApproval(5, "once"));
    assert.equal(transcript.answerApproval(5, "deny"), null);
    assert.equal(transcript.answerApproval(99, "once"), null);
  });

  it("settles a card whose run ended before an answer", () => {
    const transcript = new Transcript();
    transcript.apply({ type: "approval_request", id: 1, toolName: "bash", detail: "" });
    transcript.apply({
      type: "message_update",
      assistantMessageEvent: { type: "text_delta", delta: "thinking again" },
    });
    transcript.apply({ type: "agent_end", messages: [] });
    const messages = transcript.closeApprovals();
    assert.deepEqual(messages, [
      { k: "approval", id: 1, state: "closed", label: "Not answered" },
    ]);
    assert.equal(transcript.answerApproval(1, "once"), null);
    // Nothing is waiting a second time.
    assert.deepEqual(transcript.closeApprovals(), []);
  });

  it("pushes a question card that waits for an answer", () => {
    const transcript = new Transcript();
    const messages = transcript.apply({
      type: "question_request",
      id: 7,
      questions: [
        {
          question: "Which database?",
          header: "Database",
          options: [{ label: "Postgres", description: "Relational" }],
          multiSelect: false,
        },
      ],
    });
    const item = push(messages[0]);
    assert.ok(item.kind === "question");
    assert.equal(item.requestId, 7);
    assert.equal(item.title, "Database");
    assert.equal(item.questions.length, 1);
    assert.equal(item.questions[0].question, "Which database?");
    assert.equal(item.state, "pending");
    assert.equal(item.label, "");
    assert.equal(transcript.status, "Waiting for your answer…");
  });

  // The same rule as an approval: a request whose answers could not reach the
  // broker would hold the turn until it times out.
  it("ignores a question it could not answer", () => {
    const transcript = new Transcript();
    assert.deepEqual(transcript.apply({ type: "question_request", id: 0 }), []);
    assert.deepEqual(transcript.items, []);
  });

  it("records the answers on the card they belong to", () => {
    const transcript = new Transcript();
    transcript.apply({ type: "question_request", id: 1, questions: [{ question: "Pick" }] });
    transcript.apply({ type: "question_request", id: 2, questions: [{ question: "Name it" }] });
    const messages = transcript.answerQuestion(2, [{ question: "Name it", values: ["oxide"] }]);
    assert.ok(messages);
    assert.deepEqual(messages[0], {
      k: "question",
      id: 2,
      state: "answered",
      label: "oxide",
    });
    const cards = transcript.items.filter((item) => item.kind === "question");
    assert.deepEqual(
      cards.map((entry) => [entry.id, entry.state]),
      [
        [1, "pending"],
        [2, "answered"],
      ],
    );
  });

  it("reads an empty answer as a dismissal", () => {
    const transcript = new Transcript();
    const item = push(
      transcript.apply({ type: "question_request", id: 3, questions: [{ question: "Pick" }] })[0],
    );
    assert.deepEqual(transcript.answerQuestion(3, []), [
      { k: "question", id: item.id, state: "dismissed", label: "Dismissed without an answer" },
    ]);
  });

  it("answers a question once", () => {
    const transcript = new Transcript();
    transcript.apply({ type: "question_request", id: 5, questions: [{ question: "Pick" }] });
    assert.ok(transcript.answerQuestion(5, [{ question: "Pick", values: ["a"] }]));
    assert.equal(transcript.answerQuestion(5, [{ question: "Pick", values: ["b"] }]), null);
    assert.equal(transcript.answerQuestion(99, []), null);
  });

  it("settles a question whose run ended before an answer", () => {
    const transcript = new Transcript();
    transcript.apply({ type: "question_request", id: 1, questions: [{ question: "Pick" }] });
    transcript.apply({ type: "agent_end", messages: [] });
    assert.deepEqual(transcript.closeQuestions(), [
      { k: "question", id: 1, state: "closed", label: "Not answered" },
    ]);
    assert.equal(transcript.answerQuestion(1, []), null);
    // Nothing is waiting a second time.
    assert.deepEqual(transcript.closeQuestions(), []);
  });

  // The CLI timed the request out while the run it belongs to kept going, so the
  // card stops taking an answer the broker no longer reads.
  it("settles a question the CLI gave up on, and leaves the others alone", () => {
    const transcript = new Transcript();
    const waiting = push(
      transcript.apply({ type: "question_request", id: 4, questions: [{ question: "Pick" }] })[0],
    );
    transcript.apply({ type: "question_request", id: 5, questions: [{ question: "Name it" }] });
    assert.deepEqual(transcript.apply({ type: "question_closed", id: 4 }), [
      { k: "question", id: waiting.id, state: "closed", label: "Not answered" },
    ]);
    assert.equal(transcript.status, "Thinking…", "the turn it belongs to is still running");
    assert.equal(transcript.answerQuestion(4, [{ question: "Pick", values: ["a"] }]), null);
    assert.ok(transcript.answerQuestion(5, [{ question: "Name it", values: ["oxide"] }]));
    // An id nothing is waiting on, or one already settled, changes nothing.
    assert.deepEqual(transcript.apply({ type: "question_closed", id: 4 }), []);
    assert.deepEqual(transcript.apply({ type: "question_closed", id: 99 }), []);
    assert.deepEqual(transcript.apply({ type: "question_closed" }), []);
  });

  it("titles the thread from the first message", () => {
    const transcript = new Transcript();
    assert.equal(transcript.title(), "", "an empty thread has no title");
    transcript.pushUser("  \nFix the build  \nplease", []);
    assert.equal(transcript.title(), "Fix the build", "the first non-empty line wins");
    transcript.pushUser("a later message", []);
    assert.equal(transcript.title(), "Fix the build", "the first message keeps the title");
  });

  it("names a media-only first message from its attachment", () => {
    const transcript = new Transcript();
    transcript.pushUser("", [{ label: "screenshot.png" }]);
    assert.equal(transcript.title(), "screenshot.png", "an image-only send names the file");
    transcript.pushUser("Explain this diagram", []);
    assert.equal(
      transcript.title(),
      "Explain this diagram",
      "the first message with prose names the thread",
    );
  });

  it("keeps a long title to one truncated line", () => {
    const transcript = new Transcript();
    transcript.pushUser("x".repeat(200), []);
    const title = transcript.title();
    assert.ok(title.length <= 64, title);
    assert.ok(title.endsWith("…"), title);
  });

  it("summarizes a title from prose rather than Markdown", () => {
    assert.equal(summarizeTitle("### Fix the **flaky** test"), "Fix the flaky test");
    assert.equal(
      summarizeTitle("See [the docs](https://example.com/x) and `cargo test`"),
      "See the docs and cargo test",
    );
    assert.equal(summarizeTitle("> quoted request"), "quoted request");
    assert.equal(summarizeTitle("- first item"), "first item");
    // A fence, a rule and a table border say nothing on their own, and a fenced
    // block is a listing rather than a title.
    assert.equal(summarizeTitle("```rust\nfn main() {}\n```\nRun this"), "Run this");
    assert.equal(summarizeTitle("---\n| a | b |\nWhitespace   collapsed"), "Whitespace collapsed");
    assert.equal(summarizeTitle("   "), "");
    // A long title is cut at a word boundary, never mid-word.
    const long = summarizeTitle("word ".repeat(30));
    assert.ok(long.length <= 64, long);
    assert.ok(long.endsWith("word…"), long);
  });

  it("carries the current state for a repainted view", () => {
    const transcript = new Transcript();
    transcript.apply({ type: "session", id: "s" });
    transcript.pushUser("hi", []);
    const state = transcript.state({
      queued: 1,
      context: [{ id: 7, label: "a.rs" }],
      attachments: [
        { id: 8, label: "shot.png", kind: "image", preview: "data:image/png;base64,AA", detail: "4 B · pasted" },
      ],
      title: "Fix the build",
      folder: "/work/oxide",
      binary: "/usr/local/bin/oxide",
      showThinking: false,
      footer,
      run: null,
    });
    assert.equal(state.items.length, 1);
    assert.equal(state.sessionId, "s");
    assert.equal(state.queued, 1);
    assert.equal(state.showThinking, false);
    assert.equal(state.title, "Fix the build");
    assert.equal(state.folder, "/work/oxide");
    assert.deepEqual(state.attachments, [
      { id: 8, label: "shot.png", kind: "image", preview: "data:image/png;base64,AA", detail: "4 B · pasted" },
    ]);
  });

  /// The controller keeps one `Transcript` per thread and swaps which one the
  /// panel is painting, so that the conversation a turn is writing into is not the
  /// one a reader scrolled to. Everything a thread holds — its items, its totals,
  /// its status — has to live in the object rather than in the controller, or an
  /// event applied to the run's thread would repaint the one on screen.
  it("keeps one thread's stream out of another thread's transcript", () => {
    const run = new Transcript();
    run.apply({ type: "session", id: "s1" });
    run.pushUser("fix the build", []);
    const other = new Transcript();
    other.apply({ type: "session", id: "s2" });
    other.replay([{ kind: "user", text: "what did we decide?" }]);

    run.busy = true;
    run.apply({ type: "message_update", assistantMessageEvent: { type: "text_delta", delta: "on it" } });
    run.apply({ type: "usage", usage: { input: 12, output: 3, cost: 0.5 } });

    assert.deepEqual(
      run.items.map((item) => item.kind),
      ["user", "assistant"],
      "the run's own thread holds what it streamed",
    );
    assert.deepEqual(
      other.items.map((item) => item.kind),
      ["user"],
      "and the thread being read is untouched",
    );
    assert.equal(run.sessionId, "s1");
    assert.equal(other.sessionId, "s2");
    assert.equal(run.busy, true);
    assert.equal(other.busy, false, "a phase belongs to the thread it is running in");
    assert.equal(other.usage.input, 0, "so do the totals");
    assert.equal(run.usage.input, 12);
    assert.equal(other.usage.cost, 0);
    assert.equal(run.usage.cost, 0.5);
  });
});
