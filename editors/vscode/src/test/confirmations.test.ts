import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { test } from "node:test";
import { runInNewContext } from "node:vm";
import * as ts from "typescript";

import { translate, type LanguageInfo } from "../core/i18n";
import { sessionWorktree, worktreeAt, worktreeHandoffArgs, worktreeRemoveArgs } from "../core/worktrees";

const source = ts.createSourceFile("chat.ts", readFileSync(resolve(__dirname, "../../src/chat.ts"), "utf8"), ts.ScriptTarget.Latest, true);
const declaration = source.statements.find((node): node is ts.ClassDeclaration => ts.isClassDeclaration(node) && node.name?.text === "ChatController")!;
const chinese: LanguageInfo = { language: "zh-CN", languages: [], strings: {
  "ui.removeWorktree": "移除工作树", "ui.handOff": "移交",
} };

type Action = { title: string; action: string };

for (const method of ["removeWorktree", "handoffSession"]) {
  for (const confirmed of [true, false]) {
    test(`${method} ${confirmed ? "accepts" : "cancels"} a pending confirmation after its language changes`, async () => {
      const node = declaration.members.find((member): member is ts.MethodDeclaration => ts.isMethodDeclaration(member) && member.name.getText(source) === method)!;
      const javascript = ts.transpileModule(`class ReviewedController { ${node.getText(source)} }\nReviewedController;`, {
        compilerOptions: { target: ts.ScriptTarget.ES2022 },
      }).outputText;
      let item: Action | undefined;
      let answer: (value?: Action) => void = () => { throw new Error("No modal is open"); };
      const calls: string[][] = [];
      const ReviewedController = runInNewContext(javascript, {
        vscode: { window: { showWarningMessage: (_message: string, _options: unknown, action: Action) => {
          item = action;
          return new Promise<Action | undefined>((resolveAnswer) => { answer = resolveAnswer; });
        } } },
        worktreeAt, worktreeRemoveArgs, worktreeHandoffArgs, sessionWorktree,
        runCapture: async (_binary: string, args: string[]) => {
          calls.push(args);
          return { code: 0, stdout: '{"cwd":"/work/copy"}' };
        },
      });
      const controller = {
        languageInfo: null as LanguageInfo | null,
        worktrees: [{ name: "task", branch: "oxide/task", path: "/work/copy", project: "/work/project" }],
        worktree: null as string | null, turn: null, handingOff: false,
        transcript: { sessionId: "session-id" as string | null },
        folder: () => ({ uri: { fsPath: "/work/project" } }), cwd: () => "/work/project", binary: () => "oxide",
        t(key: string, fallback: string, vars?: Record<string, unknown>) { return translate(this.languageInfo, key, fallback, vars); },
        showNotice: () => {}, showWorktrees: async () => {}, refreshProject: () => {}, newSession: () => {},
        threadTitle: () => "Title", newTranscript: () => ({ sessionId: null as string | null }),
        loadHistory: async () => {}, broadcastStatus: () => {},
      };
      const result: Promise<void> = ReviewedController.prototype[method].call(controller, "/work/copy");
      assert.equal(item?.title, method === "removeWorktree" ? "Remove worktree" : "Hand off");
      controller.languageInfo = chinese;
      answer(confirmed ? item : undefined);
      await result;
      assert.equal(calls.length, confirmed ? 1 : 0);
      if (confirmed) assert.equal(calls[0][1], method === "removeWorktree" ? "remove" : "handoff");
    });
  }
}

for (const accept of [true, false]) {
  test(`localized update notification ${accept ? "restarts" : "dismisses"} after a language change`, async () => {
    const node = declaration.members.find((member): member is ts.MethodDeclaration =>
      ts.isMethodDeclaration(member) && member.name.getText(source) === "announceInstalled")!;
    const javascript = ts.transpileModule(`class UpdateController { ${node.getText(source)} }\nUpdateController;`, {
      compilerOptions: { target: ts.ScriptTarget.ES2022 },
    }).outputText;
    let answer!: (choice?: string) => void;
    let offered: string[] = [];
    let message = "";
    let restarts = 0;
    const UpdateController = runInNewContext(javascript, {
      vscode: { window: { showInformationMessage: (text: string, ...choices: string[]) => {
        message = text;
        offered = choices;
        return new Promise<string | undefined>((resolveAnswer) => { answer = resolveAnswer; });
      } } },
    });
    const controller = {
      languageInfo: { language: "zh-CN", languages: [], strings: {
        "extension.restartWindow": "重启窗口", "extension.later": "稍后",
        "extension.OxideValue0WasInstalledRestartTheWindowToRunIt": "已安装 Oxide {value0}，重启窗口即可使用。",
      } } as LanguageInfo | null,
      t(key: string, fallback: string, vars?: Record<string, unknown>) { return translate(this.languageInfo, key, fallback, vars); },
      reloadWindow: () => { restarts += 1; },
    };
    UpdateController.prototype.announceInstalled.call(controller, { latest: "1.2.3" });
    assert.equal(message, "已安装 Oxide 1.2.3，重启窗口即可使用。");
    assert.deepEqual(offered, ["重启窗口", "稍后"]);
    controller.languageInfo = null;
    answer(accept ? offered[0] : undefined);
    await Promise.resolve();
    assert.equal(restarts, accept ? 1 : 0);
  });
}
