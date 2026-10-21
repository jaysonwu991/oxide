import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { test } from "node:test";
import { runInNewContext } from "node:vm";
import * as ts from "typescript";
import { translate, type LanguageInfo } from "../core/i18n";
import { parseModelChoice } from "../core/models";
import { needsKey, parseLoginOutcome, providerLoginArgs } from "../core/providers";

const source = ts.createSourceFile("chat.ts", readFileSync(resolve(__dirname, "../../src/chat.ts"), "utf8"), ts.ScriptTarget.Latest, true);
const declaration = source.statements.find((node): node is ts.ClassDeclaration => ts.isClassDeclaration(node) && node.name?.text === "ChatController")!;

for (const method of ["connectProvider", "applyModel"]) {
  for (const language of ["en-GB", "zh-CN"]) {
    test(`${method} keeps its ${language} success notice when refreshing the catalog`, async () => {
      const node = declaration.members.find((member): member is ts.MethodDeclaration =>
        ts.isMethodDeclaration(member) && member.name.getText(source) === method)!;
      const javascript = ts.transpileModule(`class Controller { ${node.getText(source)} }\nController;`, {
        compilerOptions: { target: ts.ScriptTarget.ES2022 },
      }).outputText;
      const calls: string[][] = [];
      const Controller = runInNewContext(javascript, {
        vscode: { window: { setStatusBarMessage: () => {} } },
        needsKey, parseLoginOutcome, providerLoginArgs, parseModelChoice, LOGIN_TIMEOUT: 60000,
        runCapture: async (_binary: string, args: string[]) => {
          calls.push(args);
          return { code: 0, stdout: JSON.stringify({ provider: "original-provider", label: "Original Provider", model: "original-model" }) };
        },
      });
      const notices: string[] = [];
      let refreshes = 0;
      const controller = {
        languageInfo: { language, languages: [], strings: language === "zh-CN" ? {
          "surface.connectedValue": "已连接 {value}。", "surface.modelSetToValue": "模型已设为 {value}。",
        } : {} } as LanguageInfo | null,
        providers: [{ name: "original-provider", label: "Original Provider", local: true }],
        modelCatalog: [{ provider: "original-provider", model: "original-model" }],
        t(key: string, fallback: string, vars?: Record<string, unknown>) { return translate(this.languageInfo, key, fallback, vars); },
        cwd: () => "/work/project", binary: () => "oxide", closeDialog: () => {},
        refreshProject(force: boolean) {
          assert.equal(force, true);
          refreshes += 1;
          this.languageInfo = null; // The real refresh invalidates the catalog synchronously.
        },
        showNotice: (text: string) => notices.push(text),
        onDidChange: { fire: () => {} }, broadcast: () => {}, stateMessage: () => ({}),
      };
      await Controller.prototype[method].call(controller, method === "connectProvider" ? "original-provider" : "original-provider:original-model");
      assert.equal(refreshes, 1);
      assert.equal(controller.languageInfo, null);
      assert.deepEqual(notices, [method === "connectProvider"
        ? language === "zh-CN" ? "已连接 Original Provider。" : "Connected Original Provider."
        : language === "zh-CN" ? "模型已设为 original-model。" : "Model set to original-model."]);
      assert.equal(calls[0][0], method === "connectProvider" ? "login" : "select");
      assert.ok(calls[0].includes("original-provider"));
    });
  }
}
