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

  it("routes a question answer to the running turn", () => {
    // A question card that settled without a frame would leave the `ask` call
    // waiting for the answer until the broker's timeout, so the panel shows an
    // answered card while the agent sits there.
    const answer = chat.slice(
      chat.indexOf("answerQuestion(requestId"),
      chat.indexOf("private drainQueue"),
    );
    assert.ok(answer.includes("this.transcript.answerQuestion(requestId, answers)"));
    assert.ok(answer.includes("turn.answer(requestId, answers)"), "the frame is written too");
  });

  it("names the header's new-chat button the way the command is named", () => {
    // One action, two places that say its name: the palette entry and the
    // button's tooltip, which is also the name a screen reader reads out. The
    // palette keeps the title case VS Code expects and the panel's own labels
    // are sentence case, so the two agree on the words rather than the caps —
    // what must not happen is the button going on offering the old action.
    const command = manifest.contributes.commands.find(
      (entry) => entry.command === "oxide.newSession",
    );
    assert.equal(command?.title, "New Chat");
    const at = chatView.indexOf('id="new-session"');
    const button = chatView.slice(at, chatView.indexOf("</button>", at));
    for (const attribute of ["title", "aria-label"]) {
      const value = new RegExp(`${attribute}="([^"]+)"`).exec(button)?.[1];
      assert.equal(value?.toLowerCase(), command!.title.toLowerCase(), `${attribute} agrees`);
    }
    assert.ok(!chatView.includes("New session"), "the old name is gone from the header");
  });

  it("marks the open thread wherever the session listing is rebuilt", () => {
    // The listing says which thread you are in and offers to close it, so a
    // rebuild that composes it without that id — the one after a failed delete
    // did — drops the mark and turns the first row into one that promises a
    // fresh thread instead. It is composed in exactly one place, which supplies
    // it, and every redraw goes through that place.
    assert.equal(
      (chat.match(/showDialog\(\s*sessionDialog\(/g) ?? []).length,
      1,
      "the session listing is composed in one place",
    );
    const helper = chat.slice(
      chat.indexOf("private showSessions("),
      chat.indexOf("async resumeSession("),
    );
    assert.ok(
      helper.includes("sessionDialog(this.sessions, this.transcript.sessionId, note)"),
      "and that place passes the open thread's id",
    );
    // The sites that repaint it: the load, its failure, the listing itself, a
    // row while a turn runs, a delete while a turn runs, and a failed delete.
    assert.ok(
      (chat.match(/this\.showSessions\(/g) ?? []).length >= 6,
      "every redraw uses it rather than composing the dialog again",
    );
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
