import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { test } from "node:test";
import { runInNewContext } from "node:vm";
import * as ts from "typescript";
import { parseReasoning, reasoningArgs } from "../core/reasoning";
import { reasoningChoices } from "../core/footer";

const source = ts.createSourceFile("chat.ts", readFileSync(resolve(__dirname, "../../src/chat.ts"), "utf8"), ts.ScriptTarget.Latest, true);
const declaration = source.statements.find((node): node is ts.ClassDeclaration => ts.isClassDeclaration(node) && node.name?.text === "ChatController")!;
const methods = ["refreshReasoning", "setReasoning"].map((name) => declaration.members.find((member): member is ts.MethodDeclaration =>
  ts.isMethodDeclaration(member) && member.name.getText(source) === name)!.getText(source));
const javascript = ts.transpileModule(`class Controller { ${methods.join("\n")} }\nController;`, {
  compilerOptions: { target: ts.ScriptTarget.ES2022 },
}).outputText;
const answer = (supported: boolean) => ({ code: 0, stdout: JSON.stringify({
  supportsReasoning: supported, reasoningLevels: supported ? ["low", "high"] : [],
}) });

function harness(read: (args: string[]) => Promise<ReturnType<typeof answer>>) {
  const calls: string[][] = [];
  const Controller = runInNewContext(javascript, {
    parseReasoning, reasoningArgs, reasoningChoices,
    reasoningDialog: () => ({ kind: "reasoning" }),
    runCapture: async (_binary: string, args: string[]) => { calls.push(args); return read(args); },
  });
  const controller = Object.assign(new Controller(), {
    reasoningProbe: 0, project: { model: "claude-sonnet-5" },
    reasoningLevels: [] as string[], supportsReasoning: undefined as boolean | undefined,
    cwd: () => "/work/project", binary: () => "oxide", broadcasts: 0, dialogs: 0,
    stateMessage: () => ({}), currentReasoning: () => "auto",
    broadcast() { this.broadcasts++; }, showDialog() { this.dialogs++; },
  });
  return { controller, calls };
}

test("a model switch publishes cached effort levels without a network refresh", async () => {
  const { controller, calls } = harness(async () => answer(true));
  await controller.refreshReasoning(true);
  assert.equal(calls.length, 1);
  assert.equal(calls[0].includes("--refresh"), false);
  assert.ok(calls[0].includes("claude-sonnet-5"));
  assert.deepEqual(controller.reasoningLevels, ["low", "high"]);
  assert.equal(controller.broadcasts, 1);
});

test("missing effort metadata is warmed after the cache read", async () => {
  const { controller, calls } = harness(async (args) => answer(args.includes("--refresh")));
  await controller.refreshReasoning(true);
  assert.equal(calls.length, 2);
  assert.equal(calls[0].includes("--refresh"), false);
  assert.equal(calls[1].includes("--refresh"), true);
  assert.equal(controller.supportsReasoning, true);
});

test("a stale cache read cannot publish levels or start a network refresh", async () => {
  let finish!: (value: ReturnType<typeof answer>) => void;
  const { controller, calls } = harness(() => new Promise((resolve) => { finish = resolve; }));
  const pending = controller.refreshReasoning(true);
  controller.reasoningProbe++;
  finish(answer(false));
  await pending;
  assert.equal(calls.length, 1);
  assert.equal(controller.broadcasts, 0);
});

test("opening the effort picker uses known levels immediately", async () => {
  const { controller, calls } = harness(async () => { throw new Error("unexpected CLI read"); });
  controller.reasoningLevels = ["low", "high"];
  controller.supportsReasoning = true;
  await controller.setReasoning();
  assert.equal(calls.length, 0);
  assert.equal(controller.dialogs, 1);
});
