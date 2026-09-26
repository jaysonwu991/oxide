"use strict";
// Wire events from `oxide --mode json` and the transcript state machine the
// chat view renders. This module deliberately imports nothing from `vscode` so
// it can be unit tested under plain node (`node --test out/test/`).
//
// The extension owns the transcript; the webview is a dumb renderer that
// applies the `ViewMessage`s produced here. Every event-to-DOM decision is
// therefore testable without a webview.
Object.defineProperty(exports, "__esModule", { value: true });
exports.Transcript = void 0;
exports.emptyUsage = emptyUsage;
exports.drainLines = drainLines;
exports.parseEvent = parseEvent;
exports.formatTokens = formatTokens;
exports.parseArgs = parseArgs;
function emptyUsage() {
    return {
        input: 0,
        output: 0,
        cacheRead: 0,
        cacheWrite: 0,
        cost: 0,
        contextTokens: 0,
    };
}
/// Splits a chunk into complete lines, returning the unterminated remainder.
/// Mirrors `oxide_core::llm::drain_lines`: the buffer is compacted once per
/// chunk instead of once per line.
function drainLines(buffer, chunk) {
    const text = buffer + chunk;
    const lines = [];
    let start = 0;
    for (;;) {
        const end = text.indexOf("\n", start);
        if (end === -1)
            break;
        lines.push(text.slice(start, end).replace(/\r$/, ""));
        start = end + 1;
    }
    return { lines, rest: text.slice(start) };
}
/// Parses one line of the agent's JSONL stream, or `null` when the line is
/// blank or not a JSON object (a partial write, a stray log line).
function parseEvent(line) {
    const trimmed = line.trim();
    if (!trimmed)
        return null;
    try {
        const value = JSON.parse(trimmed);
        if (!value || typeof value !== "object" || Array.isArray(value))
            return null;
        return value;
    }
    catch {
        return null;
    }
}
const str = (value) => (typeof value === "string" ? value : "");
const num = (value) => typeof value === "number" && Number.isFinite(value) ? value : 0;
const obj = (value) => value && typeof value === "object" && !Array.isArray(value)
    ? value
    : {};
/// Renders a byte count for the footer (`1.2k`, `34`).
function formatTokens(count) {
    if (count < 1000)
        return String(count);
    if (count < 1_000_000)
        return `${(count / 1000).toFixed(1)}k`;
    return `${(count / 1_000_000).toFixed(2)}M`;
}
/// Turns one agent event into the messages the view applies.
class Transcript {
    lookupDiff;
    items = [];
    usage = emptyUsage();
    status = "Idle";
    busy = false;
    sessionId = null;
    contextWindow = 0;
    currentAssistant = null;
    currentThinking = null;
    nextId = 1;
    /// `lookupDiff` is how the transcript previews a file change. It is injected
    /// so this module stays free of filesystem access (and testable).
    constructor(lookupDiff = () => null) {
        this.lookupDiff = lookupDiff;
    }
    state(extra) {
        return {
            items: this.items,
            status: this.status,
            busy: this.busy,
            usage: this.usage,
            sessionId: this.sessionId,
            contextWindow: this.contextWindow,
            ...extra,
        };
    }
    reset() {
        this.items.length = 0;
        this.usage = emptyUsage();
        this.status = "Idle";
        this.busy = false;
        this.sessionId = null;
        this.currentAssistant = null;
        this.currentThinking = null;
    }
    pushUser(text, context) {
        this.currentAssistant = null;
        this.currentThinking = null;
        const item = {
            id: this.nextId++,
            kind: "user",
            text,
            context: context.map((chip) => chip.label),
        };
        this.items.push(item);
        return [{ k: "push", item }];
    }
    notice(text, tone = "info") {
        const item = { id: this.nextId++, kind: "notice", text, tone };
        this.items.push(item);
        return [{ k: "push", item }];
    }
    /// The current status/footer line. The controller sends this after applying
    /// a batch so the view never sees a stale busy flag or queue count.
    statusMessage(queued) {
        // While a turn runs the status names the activity; otherwise the agent is
        // idle and the outcome is in the transcript.
        const status = this.busy ? (this.status === "Idle" ? "Thinking…" : this.status) : "Idle";
        return { k: "status", status, busy: this.busy, queued };
    }
    /// Applies one wire event, returning the view updates it implies.
    apply(event) {
        switch (str(event.type)) {
            case "session":
                this.sessionId = str(event.id) || this.sessionId;
                return [];
            case "thinking":
                // The block itself is created by the first reasoning delta it carries:
                // a turn can start a thinking block and never fill it, and an empty
                // one would sit in the transcript for the rest of the session.
                this.closeAssistant();
                this.closeThinking();
                return [];
            case "thinking_done":
                // The model step finished streaming. Closing the in-progress pointers
                // here means a retry in a later step drops only its own attempt, not a
                // previous step's committed reply.
                this.closeAssistant();
                this.closeThinking();
                return [];
            case "message_update":
                return this.applyDelta(obj(event.assistantMessageEvent));
            case "tool_call":
                return this.startTool(str(event.toolName) || "tool", event.arguments);
            case "tool_execution_update":
                return this.appendToolOutput(str(event.toolName), str(event.partialResult));
            case "tool_execution_end":
                return this.finishTool(str(event.toolName), str(event.result), event.isError === true);
            case "usage": {
                const usage = obj(event.usage);
                const context = num(usage.input) + num(usage.cacheRead) + num(usage.cacheWrite);
                this.usage = {
                    input: this.usage.input + num(usage.input),
                    output: this.usage.output + num(usage.output),
                    cacheRead: this.usage.cacheRead + num(usage.cacheRead),
                    cacheWrite: this.usage.cacheWrite + num(usage.cacheWrite),
                    cost: this.usage.cost + num(usage.cost),
                    contextTokens: context,
                };
                return [{ k: "usage", usage: this.usage }];
            }
            case "auto_retry_start": {
                // A retry re-streams the response from the start, so the partial item a
                // failed attempt was streaming into is dropped rather than extended.
                const messages = this.discardAttempt();
                // The status line is also re-sent by the controller once per event
                // batch (it owns the queued count).
                const delay = num(event.delayMs);
                const wait = delay >= 1_000 ? ` in ${Math.round(delay / 1000)}s` : "";
                this.status = `Retrying (${num(event.attempt)}/${num(event.maxAttempts)})${wait}…`;
                return messages;
            }
            case "compaction":
                return this.notice(`Compacted ${num(event.summarized)} earlier messages (~${formatTokens(num(event.tokensBefore))} tokens)`);
            case "error":
                return this.notice(`Error: ${str(event.message)}`, "error");
            case "agent_end":
                this.closeAssistant();
                this.closeThinking();
                this.status = "Done";
                return [];
            default:
                return [];
        }
    }
    /// Drops the assistant or thinking item a failed attempt was still streaming
    /// into, so the retry's fresh output does not extend it.
    discardAttempt() {
        const messages = [];
        for (const item of [this.currentAssistant, this.currentThinking]) {
            if (!item)
                continue;
            const index = this.items.indexOf(item);
            if (index >= 0)
                this.items.splice(index, 1);
            messages.push({ k: "remove", id: item.id });
        }
        this.currentAssistant = null;
        this.currentThinking = null;
        return messages;
    }
    applyDelta(delta) {
        const text = str(delta.delta);
        switch (str(delta.type)) {
            case "thinking_delta": {
                if (!text)
                    return [];
                // A delta that opens a block has to push it before appending, or the
                // view has no entry for the id and drops the text.
                const created = this.currentThinking === null;
                const item = this.ensureThinking();
                item.text += text;
                const messages = created ? [{ k: "push", item }] : [];
                messages.push({ k: "append", id: item.id, field: "text", delta: text });
                return messages;
            }
            case "text_delta": {
                if (!text)
                    return [];
                const created = this.currentAssistant === null;
                const item = this.ensureAssistant();
                item.text += text;
                this.status = "Writing…";
                const messages = created ? [{ k: "push", item }] : [];
                messages.push({ k: "append", id: item.id, field: "text", delta: text });
                return messages;
            }
            default:
                return [];
        }
    }
    ensureAssistant() {
        if (this.currentAssistant)
            return this.currentAssistant;
        this.closeThinking();
        const item = { id: this.nextId++, kind: "assistant", text: "" };
        this.items.push(item);
        this.currentAssistant = item;
        return item;
    }
    ensureThinking() {
        if (this.currentThinking)
            return this.currentThinking;
        const item = { id: this.nextId++, kind: "thinking", text: "" };
        this.items.push(item);
        this.currentThinking = item;
        return item;
    }
    /// A tool call ends the current step: text after it starts a new bubble.
    closeAssistant() {
        this.currentAssistant = null;
    }
    closeThinking() {
        this.currentThinking = null;
    }
    startTool(name, args) {
        this.closeAssistant();
        this.closeThinking();
        const raw = typeof args === "string" ? args : JSON.stringify(args ?? {});
        const item = {
            id: this.nextId++,
            kind: "tool",
            name,
            args: raw,
            output: "",
            diff: this.lookupDiff(name, parseArgs(raw)),
            running: true,
            isError: false,
        };
        this.items.push(item);
        this.status = `Running ${name}…`;
        return [{ k: "push", item }];
    }
    /// Results are emitted in call order, so the oldest running card with a
    /// matching name owns the update.
    runningTool(name) {
        const tools = this.items.filter((item) => item.kind === "tool" && item.running);
        return tools.find((item) => item.name === name) ?? tools[0];
    }
    appendToolOutput(name, chunk) {
        if (!chunk)
            return [];
        const tool = this.runningTool(name);
        if (!tool)
            return [];
        tool.output += chunk;
        return [{ k: "append", id: tool.id, field: "output", delta: chunk }];
    }
    finishTool(name, result, isError) {
        const tool = this.runningTool(name);
        if (!tool)
            return [];
        if (result)
            tool.output = result;
        tool.running = false;
        tool.isError = isError;
        this.status = "Thinking…";
        return [
            {
                k: "patch",
                id: tool.id,
                patch: { output: tool.output, running: false, isError },
            },
        ];
    }
}
exports.Transcript = Transcript;
function parseArgs(raw) {
    try {
        return JSON.parse(raw || "{}");
    }
    catch {
        return {};
    }
}
//# sourceMappingURL=protocol.js.map