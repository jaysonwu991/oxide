"use strict";
var __importDefault = (this && this.__importDefault) || function (mod) {
    return (mod && mod.__esModule) ? mod : { "default": mod };
};
Object.defineProperty(exports, "__esModule", { value: true });
const strict_1 = __importDefault(require("node:assert/strict"));
const node_test_1 = require("node:test");
const protocol_1 = require("../core/protocol");
/// `assert.ok` is the only node assertion with an assertion signature, so the
/// discriminated union is narrowed through it.
function push(message) {
    strict_1.default.ok(message, "expected a message");
    strict_1.default.ok(message.k === "push", `expected a push message, got ${message.k}`);
    return message.item;
}
function status(message) {
    strict_1.default.ok(message.k === "status", `expected a status message, got ${message.k}`);
    return message;
}
function appended(message) {
    strict_1.default.ok(message, "expected a message");
    strict_1.default.ok(message.k === "append", `expected an append message, got ${message.k}`);
    return message;
}
function patched(message) {
    strict_1.default.ok(message, "expected a message");
    strict_1.default.ok(message.k === "patch", `expected a patch message, got ${message.k}`);
    return message;
}
function usage(message) {
    strict_1.default.ok(message, "expected a message");
    strict_1.default.ok(message.k === "usage", `expected a usage message, got ${message.k}`);
    return message;
}
function removed(message) {
    strict_1.default.ok(message, "expected a message");
    strict_1.default.ok(message.k === "remove", `expected a remove message, got ${message.k}`);
    return message;
}
function card(transcript, index = 0) {
    const tools = transcript.items.filter((item) => item.kind === "tool");
    strict_1.default.ok(tools[index], "expected a tool item");
    return tools[index];
}
function notice(item) {
    strict_1.default.ok(item.kind === "notice", `expected a notice, got ${item.kind}`);
    return item;
}
(0, node_test_1.describe)("drainLines", () => {
    (0, node_test_1.it)("splits complete lines and keeps the remainder", () => {
        const first = (0, protocol_1.drainLines)("", '{"a":1}\n{"b":2}\n{"c"');
        strict_1.default.deepEqual(first.lines, ['{"a":1}', '{"b":2}']);
        strict_1.default.equal(first.rest, '{"c"');
        const second = (0, protocol_1.drainLines)(first.rest, ':3}\n');
        strict_1.default.deepEqual(second.lines, ['{"c":3}']);
        strict_1.default.equal(second.rest, "");
    });
    (0, node_test_1.it)("strips carriage returns so CRLF output parses", () => {
        strict_1.default.deepEqual((0, protocol_1.drainLines)("", "one\r\ntwo\r\n").lines, ["one", "two"]);
    });
    (0, node_test_1.it)("returns nothing for a chunk without a newline", () => {
        const result = (0, protocol_1.drainLines)("", "partial");
        strict_1.default.deepEqual(result.lines, []);
        strict_1.default.equal(result.rest, "partial");
    });
});
(0, node_test_1.describe)("parseEvent", () => {
    (0, node_test_1.it)("ignores blank lines and non-object payloads", () => {
        strict_1.default.equal((0, protocol_1.parseEvent)(""), null);
        strict_1.default.equal((0, protocol_1.parseEvent)("   "), null);
        strict_1.default.equal((0, protocol_1.parseEvent)("not json"), null);
        strict_1.default.equal((0, protocol_1.parseEvent)("[1,2]"), null);
        strict_1.default.equal((0, protocol_1.parseEvent)('"text"'), null);
        strict_1.default.equal((0, protocol_1.parseEvent)("null"), null);
    });
    (0, node_test_1.it)("parses an object line", () => {
        strict_1.default.deepEqual((0, protocol_1.parseEvent)('{"type":"session","id":"abc"}'), { type: "session", id: "abc" });
    });
});
(0, node_test_1.describe)("Transcript", () => {
    (0, node_test_1.it)("records the session id from the header", () => {
        const transcript = new protocol_1.Transcript();
        strict_1.default.deepEqual(transcript.apply({ type: "session", id: "s1", cwd: "/tmp" }), []);
        strict_1.default.equal(transcript.sessionId, "s1");
    });
    (0, node_test_1.it)("streams reasoning and text into separate items", () => {
        const transcript = new protocol_1.Transcript();
        strict_1.default.deepEqual(transcript.apply({ type: "thinking" }), []);
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
        strict_1.default.equal(push(messages[0][0]).kind, "thinking");
        appended(messages[0][1]);
        strict_1.default.equal(push(messages[1][0]).kind, "assistant");
        appended(messages[1][1]);
        appended(messages[2][0]);
        strict_1.default.deepEqual(transcript.items.map((item) => item.kind), ["thinking", "assistant"]);
        const [thinking, assistant] = transcript.items;
        strict_1.default.ok(thinking.kind === "thinking" && assistant.kind === "assistant");
        strict_1.default.equal(thinking.text, "because ");
        strict_1.default.equal(assistant.text, "Hello world");
        strict_1.default.equal(transcript.status, "Writing…");
    });
    (0, node_test_1.it)("leaves no empty thinking block when a turn reasons without a delta", () => {
        const transcript = new protocol_1.Transcript();
        transcript.apply({ type: "thinking" });
        transcript.apply({
            type: "message_update",
            assistantMessageEvent: { type: "text_delta", delta: "no reasoning here" },
        });
        strict_1.default.deepEqual(transcript.items.map((item) => item.kind), ["assistant"]);
    });
    (0, node_test_1.it)("starts a new assistant item after a tool call", () => {
        const transcript = new protocol_1.Transcript();
        transcript.apply({
            type: "message_update",
            assistantMessageEvent: { type: "text_delta", delta: "first" },
        });
        transcript.apply({ type: "tool_call", toolName: "read", arguments: '{"path":"a.rs"}' });
        transcript.apply({
            type: "message_update",
            assistantMessageEvent: { type: "text_delta", delta: "second" },
        });
        strict_1.default.deepEqual(transcript.items.map((item) => item.kind), ["assistant", "tool", "assistant"]);
        const texts = transcript.items
            .filter((item) => item.kind === "assistant")
            .map((item) => item.text);
        strict_1.default.deepEqual(texts, ["first", "second"]);
    });
    (0, node_test_1.it)("builds a tool card from the call and finishes it with the result", () => {
        const transcript = new protocol_1.Transcript((name) => (name === "edit" ? "diff-text" : null));
        const pushed = transcript.apply({
            type: "tool_call",
            toolName: "edit",
            arguments: '{"path":"src/a.rs","edits":[]}',
        });
        const item = push(pushed[0]);
        strict_1.default.equal(item.kind, "tool");
        strict_1.default.equal(card(transcript).running, true);
        strict_1.default.equal(card(transcript).diff, "diff-text");
        strict_1.default.equal(card(transcript).args, '{"path":"src/a.rs","edits":[]}');
        strict_1.default.equal(transcript.status, "Running edit…");
        const live = transcript.apply({
            type: "tool_execution_update",
            toolName: "edit",
            partialResult: "partial",
        });
        strict_1.default.deepEqual(live, [{ k: "append", id: card(transcript).id, field: "output", delta: "partial" }]);
        const ended = transcript.apply({
            type: "tool_execution_end",
            toolName: "edit",
            result: "Successfully replaced 1 block(s) in src/a.rs.",
            isError: false,
        });
        strict_1.default.deepEqual(patched(ended[0]).patch, {
            output: "Successfully replaced 1 block(s) in src/a.rs.",
            running: false,
            isError: false,
        });
        strict_1.default.equal(card(transcript).running, false);
        strict_1.default.equal(card(transcript).output, "Successfully replaced 1 block(s) in src/a.rs.");
    });
    (0, node_test_1.it)("marks a failed tool call and keeps its output", () => {
        const transcript = new protocol_1.Transcript();
        transcript.apply({ type: "tool_call", toolName: "bash", arguments: '{"command":"false"}' });
        transcript.apply({
            type: "tool_execution_end",
            toolName: "bash",
            result: "exit status 1",
            isError: true,
        });
        strict_1.default.equal(card(transcript).isError, true);
        strict_1.default.equal(card(transcript).output, "exit status 1");
    });
    (0, node_test_1.it)("matches parallel results to the call with the same name", () => {
        const transcript = new protocol_1.Transcript();
        transcript.apply({ type: "tool_call", toolName: "read", arguments: '{"path":"a"}' });
        transcript.apply({ type: "tool_call", toolName: "read", arguments: '{"path":"b"}' });
        transcript.apply({ type: "tool_call", toolName: "grep", arguments: '{"pattern":"x"}' });
        transcript.apply({ type: "tool_execution_end", toolName: "grep", result: "hit", isError: false });
        transcript.apply({ type: "tool_execution_end", toolName: "read", result: "first", isError: false });
        strict_1.default.equal(card(transcript, 0).output, "first");
        strict_1.default.equal(card(transcript, 0).running, false);
        strict_1.default.equal(card(transcript, 1).running, true);
        strict_1.default.equal(card(transcript, 2).output, "hit");
    });
    (0, node_test_1.it)("accumulates usage and tracks the latest context size", () => {
        const transcript = new protocol_1.Transcript();
        usage(transcript.apply({
            type: "usage",
            usage: { input: 100, output: 10, cacheRead: 40, cacheWrite: 5, cost: 0.002 },
        })[0]);
        usage(transcript.apply({
            type: "usage",
            usage: { input: 200, output: 20, cacheRead: 60, cacheWrite: 0, cost: 0.003 },
        })[0]);
        strict_1.default.deepEqual(transcript.usage, {
            input: 300,
            output: 30,
            cacheRead: 100,
            cacheWrite: 5,
            cost: 0.005,
            contextTokens: 260,
        });
    });
    (0, node_test_1.it)("reports retries as a status change", () => {
        const transcript = new protocol_1.Transcript();
        strict_1.default.deepEqual(transcript.apply({ type: "auto_retry_start", attempt: 2, maxAttempts: 5 }), []);
        strict_1.default.equal(transcript.status, "Retrying (2/5)…");
    });
    (0, node_test_1.it)("discards the partial attempt when a retry starts", () => {
        const transcript = new protocol_1.Transcript();
        const partial = push(transcript.apply({
            type: "message_update",
            assistantMessageEvent: { type: "text_delta", delta: "partial" },
        })[0]);
        const messages = transcript.apply({ type: "auto_retry_start", attempt: 2, maxAttempts: 5 });
        strict_1.default.deepEqual(messages.map((message) => removed(message).id), [partial.id]);
        strict_1.default.equal(transcript.items.length, 0);
        // The retry streams fresh: new text opens a new item rather than extending
        // the discarded one.
        const fresh = push(transcript.apply({
            type: "message_update",
            assistantMessageEvent: { type: "text_delta", delta: "done" },
        })[0]);
        strict_1.default.equal(fresh.kind, "assistant");
        strict_1.default.notEqual(fresh.id, partial.id);
        strict_1.default.equal(fresh.text, "done");
    });
    (0, node_test_1.it)("keeps a committed step when a later step retries", () => {
        const transcript = new protocol_1.Transcript();
        const reply = push(transcript.apply({
            type: "message_update",
            assistantMessageEvent: { type: "text_delta", delta: "summary" },
        })[0]);
        // The step commits, so the next step runs against the same transcript (the
        // hidden Definition-of-Done reminder is sent as another message).
        transcript.apply({ type: "thinking_done" });
        // The next step fails before emitting anything and retries: its discard
        // must not remove the committed reply.
        strict_1.default.deepEqual(transcript.apply({ type: "auto_retry_start", attempt: 1, maxAttempts: 3 }), []);
        strict_1.default.equal(transcript.items.length, 1);
        strict_1.default.equal(transcript.items[0].id, reply.id);
    });
    (0, node_test_1.it)("discards a streaming reasoning block when a retry starts", () => {
        const transcript = new protocol_1.Transcript();
        const thinking = push(transcript.apply({
            type: "message_update",
            assistantMessageEvent: { type: "thinking_delta", delta: "hmm" },
        })[0]);
        const messages = transcript.apply({ type: "auto_retry_start", attempt: 2, maxAttempts: 5 });
        strict_1.default.deepEqual(messages.map((message) => removed(message).id), [thinking.id]);
        strict_1.default.equal(transcript.items.length, 0);
    });
    (0, node_test_1.it)("notes a compaction and a stream error", () => {
        const transcript = new protocol_1.Transcript();
        const compacted = notice(push(transcript.apply({ type: "compaction", summarized: 12, tokensBefore: 1200 })[0]));
        strict_1.default.equal(compacted.text, "Compacted 12 earlier messages (~1.2k tokens)");
        strict_1.default.equal(compacted.tone, "info");
        const failure = notice(push(transcript.apply({ type: "error", message: "boom" })[0]));
        strict_1.default.equal(failure.text, "Error: boom");
        strict_1.default.equal(failure.tone, "error");
    });
    (0, node_test_1.it)("closes the stream on agent_end", () => {
        const transcript = new protocol_1.Transcript();
        transcript.busy = true;
        transcript.apply({
            type: "message_update",
            assistantMessageEvent: { type: "text_delta", delta: "done" },
        });
        transcript.apply({ type: "agent_end", messages: [] });
        transcript.busy = false;
        strict_1.default.equal(transcript.status, "Done");
        strict_1.default.deepEqual(status(transcript.statusMessage(0)), {
            k: "status",
            status: "Idle",
            busy: false,
            queued: 0,
        });
    });
    (0, node_test_1.it)("reports the live activity while a turn runs", () => {
        const transcript = new protocol_1.Transcript();
        transcript.busy = true;
        strict_1.default.deepEqual(status(transcript.statusMessage(2)), {
            k: "status",
            status: "Thinking…",
            busy: true,
            queued: 2,
        });
        transcript.apply({ type: "tool_call", toolName: "bash", arguments: "{}" });
        strict_1.default.equal(status(transcript.statusMessage(0)).status, "Running bash…");
    });
    (0, node_test_1.it)("ignores unknown events and tolerates missing fields", () => {
        const transcript = new protocol_1.Transcript();
        const messages = [
            ...transcript.apply({}),
            ...transcript.apply({ type: "assistant_message" }),
            ...transcript.apply({ type: "usage" }),
            ...transcript.apply({ type: "message_update" }),
            ...transcript.apply({ type: "tool_execution_end", toolName: "read" }),
        ];
        strict_1.default.equal(messages.length, 1);
        usage(messages[0]);
        strict_1.default.equal(transcript.usage.input, 0);
        strict_1.default.equal(transcript.items.length, 0);
    });
    (0, node_test_1.it)("ignores a tool result with no running call", () => {
        const transcript = new protocol_1.Transcript();
        strict_1.default.deepEqual(transcript.apply({ type: "tool_execution_end", toolName: "read", result: "x" }), []);
        strict_1.default.deepEqual(transcript.apply({ type: "tool_execution_update", toolName: "read", partialResult: "x" }), []);
    });
    (0, node_test_1.it)("records the user turn and the context it carried", () => {
        const transcript = new protocol_1.Transcript();
        const message = push(transcript.pushUser("fix this", [{ id: 1, label: "src/a.rs:10-12" }])[0]);
        strict_1.default.deepEqual(message, {
            id: 1,
            kind: "user",
            text: "fix this",
            context: ["src/a.rs:10-12"],
        });
    });
    (0, node_test_1.it)("starts a fresh thread when reset", () => {
        const transcript = new protocol_1.Transcript();
        transcript.apply({ type: "session", id: "s" });
        transcript.pushUser("hi", []);
        transcript.reset();
        strict_1.default.deepEqual(transcript.items, []);
        strict_1.default.equal(transcript.sessionId, null);
        strict_1.default.equal(transcript.usage.cost, 0);
        strict_1.default.equal(transcript.status, "Idle");
    });
    (0, node_test_1.it)("carries the current state for a repainted view", () => {
        const transcript = new protocol_1.Transcript();
        transcript.apply({ type: "session", id: "s" });
        transcript.pushUser("hi", []);
        const state = transcript.state({
            queued: 1,
            context: [{ id: 7, label: "a.rs" }],
            folder: "oxide",
            model: "deepseek-flash",
            binary: "/usr/local/bin/oxide",
            showThinking: false,
        });
        strict_1.default.equal(state.items.length, 1);
        strict_1.default.equal(state.sessionId, "s");
        strict_1.default.equal(state.queued, 1);
        strict_1.default.equal(state.showThinking, false);
        strict_1.default.equal(state.folder, "oxide");
    });
});
//# sourceMappingURL=protocol.test.js.map