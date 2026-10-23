import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { test } from "node:test";
import { runInNewContext } from "node:vm";
import * as ts from "typescript";
import { buildTurnArgs } from "../core/args";
import { openSession, type CliSession, type SessionCallbacks } from "../cli";

const source = ts.createSourceFile("chat.ts", readFileSync(resolve(__dirname, "../../src/chat.ts"), "utf8"), ts.ScriptTarget.Latest, true);
const declaration = source.statements.find((node): node is ts.ClassDeclaration => ts.isClassDeclaration(node) && node.name?.text === "ChatController")!;
const method = declaration.members.find((node): node is ts.MethodDeclaration => ts.isMethodDeclaration(node) && node.name.getText(source) === "startTurn")!;
const javascript = ts.transpileModule(`class Controller { ${method.getText(source)} }\nController;`, {
  compilerOptions: { target: ts.ScriptTarget.ES2022 },
}).outputText;

test("replaced process callbacks cannot end or remove its replacement", () => {
  const callbacks: SessionCallbacks[] = [];
  let spawned = 0;
  let closed = 0;
  let exits = 0;
  let ended = 0;
  let events = 0;
  let binary = "oxide";
  const Controller = runInNewContext(javascript, {
    buildTurnArgs, indent: (text: string) => text,
    openCliSession: (_command: string, _args: string[], _cwd: string, callback: SessionCallbacks): CliSession => {
      callbacks.push(callback);
      spawned++;
      return { alive: true, sessionId: `thread-${spawned}`, close() { closed++; }, start() { return {} as never; } };
    },
  });
  const controller = {
    cliSessions: new Map<string, CliSession>(), activeCliSession: null,
    folder: () => null, turnOptions: () => ({}), binary: () => binary,
    output: { appendLine() {} }, broadcastStatus() {},
    handleExit() { exits++; }, handleTurnEnd() { ended++; }, handleEvent() { events++; },
    run: null as { stderr: string[] } | null,
  };
  const prepared = { cwd: "/work/project", prompt: "hello", context: [], attachments: [] };
  const start = (id: string | null) => Controller.prototype.startTurn.call(controller, prepared, false, { sessionId: id });
  start(null);
  start("thread-2");
  assert.equal(spawned, 2);
  assert.equal(closed, 1);
  callbacks[0].onExit({ code: 0, signal: null });
  callbacks[0].onTurnEnd();
  callbacks[0].onEvent({ type: "agent_end" });
  callbacks[0].onStderr("stale error");
  assert.equal(exits, 0);
  assert.equal(ended, 0);
  assert.equal(events, 0);
  assert.equal(controller.run?.stderr.length, 0);
  assert.equal(controller.cliSessions.size, 1);
  start("thread-2");
  assert.equal(spawned, 2, "the replacement remains reusable");
  callbacks[1].onEvent({ type: "agent_end" });
  callbacks[1].onTurnEnd();
  assert.equal(events, 1);
  assert.equal(ended, 1);
  binary = "/new/oxide";
  start("thread-2");
  assert.equal(spawned, 3, "a changed binary starts a new process");
  callbacks[1].onExit({ code: 1, signal: null });
  assert.equal(exits, 0, "a parked process cannot settle the active turn");
  callbacks[2].onExit({ code: 1, signal: null });
  assert.equal(exits, 1);
});


test("a persistent CLI serves twelve turns without accumulating error listeners", { timeout: 10_000 }, async () => {
  const warnings: Error[] = [];
  const onWarning = (warning: Error) => warnings.push(warning);
  process.on("warning", onWarning);
  let turnEnd: () => void = () => {};
  let exited: () => void = () => {};
  const exit = new Promise<void>((resolve) => { exited = resolve; });
  const session = openSession(process.execPath, ["-e", `
    const lines = require("node:readline").createInterface({ input: process.stdin });
    lines.on("line", line => {
      const request = JSON.parse(line);
      if (request.type === "quit") process.exit(0);
      if (request.type === "prompt") console.log(JSON.stringify({ type: "agent_end" }));
    });
  `], process.cwd(), {
    onEvent() {}, onStderr() {}, onTurnEnd() { turnEnd(); }, onExit() { exited(); },
  });
  try {
    for (let index = 0; index < 12; index++) {
      const ended = new Promise<void>((resolve) => { turnEnd = resolve; });
      session.start({ prompt: "hello" });
      assert.throws(() => session.start({ prompt: "overlap" }), /active turn/);
      await ended;
    }
    await new Promise<void>((resolve) => setImmediate(resolve));
    assert.equal(warnings.filter((warning) => warning.name === "MaxListenersExceededWarning").length, 0);
  } finally {
    session.close();
    await exit;
    process.off("warning", onWarning);
  }
});


test("Stop preserves the warm process and stale turn controls cannot cancel the next turn", { timeout: 10_000 }, async () => {
  let ended: () => void = () => {};
  let exited: () => void = () => {};
  const exit = new Promise<void>((resolve) => { exited = resolve; });
  const events: string[] = [];
  const session = openSession(process.execPath, ["-e", `
    const lines = require("node:readline").createInterface({ input: process.stdin });
    lines.on("line", line => {
      const request = JSON.parse(line);
      if (request.type === "quit") process.exit(0);
      if (request.type === "abort") console.log(JSON.stringify({ type: "agent_end" }));
      if (request.type === "prompt" && request.message === "next") {
        setTimeout(() => { console.log(JSON.stringify({ type: "text_delta", delta: "next completed" })); console.log(JSON.stringify({ type: "agent_end" })); }, 40);
      }
    });
  `], process.cwd(), {
    onEvent(event) { if (event.type) events.push(event.type); }, onStderr() {}, onTurnEnd() { ended(); }, onExit() { exited(); },
  });
  try {
    const stopped = new Promise<void>((resolve) => { ended = resolve; });
    const first = session.start({ prompt: "wait" });
    first.cancel();
    first.cancel();
    await stopped;
    assert.ok(session.alive, "Stop must keep the process reusable");
    events.length = 0;
    const completed = new Promise<void>((resolve) => { ended = resolve; });
    session.start({ prompt: "next" });
    first.cancel();
    await completed;
    assert.ok(events.includes("text_delta"), "a stale Stop cannot end the replacement turn");
    assert.ok(session.alive);
  } finally {
    session.close();
    await exit;
  }
});
