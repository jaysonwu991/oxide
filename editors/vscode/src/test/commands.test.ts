// The manifest and the extension host have to agree: a contributed command with
// no handler, or a footer chip whose click the controller ignores, fails
// silently at the moment a user picks it.

import assert from "node:assert/strict";
import * as fs from "node:fs";
import * as path from "node:path";
import { describe, it } from "node:test";

import { footerState } from "../core/footer";
import { emptyUsage } from "../core/protocol";

/// The package root: this file compiles to `out/test/`.
const root = path.join(__dirname, "..", "..");

interface Manifest {
  contributes: {
    commands: { command: string; title: string; category?: string }[];
  };
}

const manifest = JSON.parse(fs.readFileSync(path.join(root, "package.json"), "utf8")) as Manifest;
const extension = fs.readFileSync(path.join(root, "src", "extension.ts"), "utf8");
const chat = fs.readFileSync(path.join(root, "src", "chat.ts"), "utf8");
const chatView = fs.readFileSync(path.join(root, "src", "chatView.ts"), "utf8");
const renderer = fs.readFileSync(path.join(root, "media", "main.js"), "utf8");
const dialogs = fs.readFileSync(path.join(root, "src", "core", "dialogs.ts"), "utf8");

describe("command contributions", () => {
  it("registers every command it contributes", () => {
    assert.ok(manifest.contributes.commands.length > 0);
    for (const { command, title, category } of manifest.contributes.commands) {
      assert.ok(title, `${command} has a title`);
      assert.equal(category, "Oxide", `${command} is grouped under Oxide`);
      // The registration may wrap onto its own line, so only the argument is
      // matched.
      const registered = new RegExp(`registerCommand\\(\\s*"${command.replace(/\./g, "\\.")}"`);
      assert.ok(registered.test(extension), `${command} has a registered handler`);
    }
  });

  it("handles every footer chip the controller paints", () => {
    const state = footerState({
      model: "",
      provider: "",
      contextWindow: 0,
      reasoning: "auto",
      agent: "",
      agentCount: 0,
      access: "untrusted",
      trustSetting: "default",
      defaultTrust: "ask",
      savedTrust: undefined,
      sessionId: null,
      branch: "",
      autoCompact: true,
      usage: emptyUsage(),
    });
    for (const chip of state.chips) {
      assert.ok(chat.includes(`case "${chip.id}":`), `the ${chip.id} chip is handled`);
    }
  });

  it("handles every message the webview sends", () => {
    // The webview is plain JavaScript with no type checking of its own, so a
    // mistyped `k` would silently do nothing: every kind it posts has to have a
    // case in the view's message switch.
    const kinds = new Set<string>();
    for (const match of renderer.matchAll(/postMessage\(\s*\{\s*k:\s*"([A-Za-z]+)"/g)) {
      kinds.add(match[1]);
    }
    assert.ok(kinds.size >= 8, `found the webview's messages (${[...kinds].join(", ")})`);
    for (const kind of kinds) {
      assert.ok(chatView.includes(`case "${kind}":`), `the host handles "${kind}"`);
    }
  });

  it("handles every action a dialog row can post", () => {
    // A row carries the action the panel performs for it, and the webview posts
    // that action back verbatim. An action with no case in the controller is a
    // button that silently does nothing — which is what a delete that never
    // deletes, or a session row that never opens, looks like.
    const actions = new Set<string>();
    for (const match of dialogs.matchAll(/(?:buttonAction|refreshAction|action): ([A-Z_]+)/g)) {
      actions.add(match[1]);
    }
    assert.ok(actions.size >= 5, `found the dialog actions (${[...actions].join(", ")})`);
    for (const action of actions) {
      assert.ok(chat.includes(`case ${action}:`), `${action} is handled by the controller`);
    }
    // The confirm row a delete opens has to lead somewhere, too.
    assert.ok(actions.has("SESSION_DELETE") && actions.has("SESSION_DELETE_CONFIRM"));
  });

  it("will not delete a thread while the running turn owns its file", () => {
    // The turn's CLI appends to the session file as it works, so removing it
    // from here pulls the file out from under the process and the next append
    // fails with `No such file or directory`. Both the confirmation and the
    // write itself refuse while a turn is up.
    const confirm = chat.slice(
      chat.indexOf("private confirmDeleteSession("),
      chat.indexOf("private async deleteSession("),
    );
    const remove = chat.slice(
      chat.indexOf("private async deleteSession("),
      chat.indexOf("private showDialog("),
    );
    assert.ok(confirm.includes("if (this.turn)"), "the confirmation checks for a running turn");
    assert.ok(remove.includes("if (this.turn)"), "the write checks again");
  });

  it("answers the client commands in the panel instead of prompting them", () => {
    // `/mcps` and `/session` are the commands a front-end performs itself, so a
    // message that reaches the prompt path would ask the model what a server
    // list is rather than showing it.
    const send = chat.slice(chat.indexOf("async send("), chat.indexOf("private ", chat.indexOf("async send(")));
    for (const command of ["isMcpCommand", "isSessionCommand"]) {
      assert.ok(send.includes(`${command}(message)`), `send consults ${command}`);
      assert.ok(
        send.indexOf(`${command}(message)`) < send.indexOf("this.queue.push"),
        `${command} is answered before a message is queued or prompted`,
      );
    }
  });
});
