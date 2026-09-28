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
    configuration: { properties: Record<string, { default?: unknown }> };
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
      helper.includes("sessionDialog(this.sessions, this.transcript.sessionId, note, this.liveSession())"),
      "and that place passes the open thread's id and its live stand-in",
    );
    // The sites that repaint it: the load, its failure, the listing itself, a
    // row while a turn runs, a delete while a turn runs, and a failed delete.
    assert.ok(
      (chat.match(/this\.showSessions\(/g) ?? []).length >= 6,
      "every redraw uses it rather than composing the dialog again",
    );
  });

  it("keeps an open session listing in step with the store it lists", () => {
    // The store catches up to a thread while its turn runs, so a listing left
    // open has to be read again: without it the thread on screen is missing
    // from the list of threads until the listing is closed and opened again.
    const sync = chat.slice(
      chat.indexOf("private async syncSessions("),
      chat.indexOf("async resumeSession("),
    );
    assert.ok(
      sync.includes('this.dialog?.kind !== "sessions"'),
      "only a session listing is repainted",
    );
    assert.ok(
      sync.includes("parseSessionList(result.stdout)"),
      "and it is read the way the listing opened with it",
    );
    assert.ok(sync.includes("this.showSessions()"), "through the one place that composes it");
    assert.equal(
      (sync.match(/this\.dialog\?\.kind !== "sessions"/g) ?? []).length,
      2,
      "and a listing closed while the read was in flight is left closed",
    );

    // The two reads overlap — the header's, taken before the store has the
    // thread, and the exit's, taken after — so each takes the next token and
    // only the newest, for the folder it was taken in, is applied. Without it
    // the older answer can land last and hide the row again.
    assert.ok(
      sync.includes("const sync = ++this.sessionsSync"),
      "a refresh takes the next token",
    );
    assert.ok(
      sync.includes("if (sync !== this.sessionsSync || cwd !== this.cwd()) return;"),
      "and a superseded answer is dropped",
    );

    // The two moments a session is written: the header naming the thread, and
    // the turn ending.
    const events = chat.slice(
      chat.indexOf("private handleEvent("),
      chat.indexOf("private handleExit("),
    );
    assert.match(events, /if \(this\.transcript\.sessionId !== id\) void this\.syncSessions\(\)/);
    const exit = chat.slice(chat.indexOf("private handleExit("), chat.indexOf("async resumeSession("));
    assert.ok(exit.includes("void this.syncSessions()"), "and so does a finished turn");

    // Opening the listing is the same read, so it takes the next token too: one
    // still in flight from a turn that just ended is dropped instead of painting
    // over the rows the reader asked for.
    const opened = chat.slice(
      chat.indexOf("async resumeSession("),
      chat.indexOf("continueSession("),
    );
    assert.ok(opened.includes("++this.sessionsSync"), "the listing supersedes an older read");
    assert.ok(
      opened.includes("sync !== this.sessionsSync || cwd !== this.cwd()"),
      "and is dropped when it is superseded itself",
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

  it("tracks the file being edited, and reads it when the message goes", () => {
    // The chip belongs to the editor, so the editor is what keeps it in step: a
    // subscription that is dropped, or an activation that never syncs, leaves
    // the panel attributing a file nobody is editing to the next message.
    const subscription = extension.slice(extension.indexOf("onDidChangeActiveTextEditor"));
    assert.ok(subscription.includes("controller.syncActiveEditor()"), "the editor keeps it in step");
    assert.ok(
      extension
        .slice(extension.lastIndexOf("refreshStatus();"))
        .includes("controller.syncActiveEditor()"),
      "a file already open at activation is tracked too",
    );
    assert.ok(chat.includes("this.syncActiveEditor()"), "a setting change re-reads the setting");
    assert.equal(
      manifest.contributes.configuration.properties["oxide.autoContext"]?.default,
      true,
      "and the setting is the one it reads",
    );

    // The text is read when the message goes rather than painted into the chip,
    // so the run receives the buffer, unsaved edits included.
    const send = chat.slice(
      chat.indexOf("async send("),
      chat.indexOf("private ", chat.indexOf("async send(")),
    );
    assert.ok(send.includes("this.autoBlock()"), "the tracked file is read at send time");
    assert.ok(send.includes("carried.has(tracked.path)"), "and a file already carried is not sent twice");

    // The ✕ takes it out of the message without forgetting the file, so the
    // next one opened brings the chip back...
    const remove = chat.slice(chat.indexOf("removeChip(id: number)"), chat.indexOf("clearChips(): void"));
    assert.ok(remove.includes("this.autoHidden = true"), "the ✕ hides the tracked chip");
    const sync = chat.slice(chat.indexOf("syncActiveEditor()"), chat.indexOf("private autoChip()"));
    assert.ok(sync.includes("this.autoHidden = false"), "and another file brings it back");
    // ...while Clear takes it with the rest, and a new or resumed thread keeps
    // it, since it is not a chip attached to the conversation on screen.
    assert.ok(chat.includes("this.autoHidden = this.auto !== null;"), "Clear takes it with the rest");
    assert.ok(
      (chat.match(/this\.dropChips\(\);/g) ?? []).length >= 3,
      "a new thread drops only the chips the user attached",
    );
  });

  it("completes an @path from the project's own files, in the shared core", () => {
    // The token and the rows come from `core/at.ts` — the rules the terminal
    // filters its own list by — so a reference is completed from what the CLI
    // would offer, and the view only splices in the row it was handed.
    const complete = chat.slice(
      chat.indexOf("async completeAt("),
      chat.indexOf("private async readWorkspacePaths("),
    );
    assert.ok(complete.includes("atToken("), "the host reads the token, not the view");
    assert.ok(complete.includes("atSuggestions("), "and ranks the rows in the shared core");
    assert.ok(
      complete.includes("this.workspacePathList()"),
      "from the project's paths, the one part the view cannot know",
    );
    assert.ok(
      complete.includes('{ k: "atSuggestions", seq,'),
      "and labels the answer, so a list for a moved caret is dropped",
    );
    assert.equal(
      renderer.includes("atToken"),
      false,
      "the renderer never decides what a token is",
    );

    // The listing is the search provider's, so the exclude settings decide what
    // is offered, and it is read once per folder rather than per keystroke.
    const walk = chat.slice(chat.indexOf("private async readWorkspacePaths("));
    assert.ok(walk.includes("vscode.workspace.findFiles("), "the listing is the workspace's");
    assert.ok(walk.includes("this.pathCache = { root, paths"), "and is remembered per folder");
    assert.equal(
      (walk.match(/this\.folder\(\)\?\.uri\.fsPath !== root/g) ?? []).length,
      1,
      "a walk is not answered after the folder it started in has moved on",
    );
    assert.equal(
      (chat.match(/this\.pathCache = null;/g) ?? []).length,
      2,
      "a setting change and a finished turn are what re-read it",
    );
    assert.ok(
      chatView.includes('case "completeAt":'),
      "the webview's question reaches the controller",
    );
  });

  it("lists the project's commands and skills from the CLI's own catalog", () => {
    // The `/` palette is the CLI's listing — the one the terminal's menu and the
    // desktop app's palette draw — so a skill a project defines is offered with
    // its description and, once taken, is a message the CLI expands. The panel
    // does not walk the discovery directories itself.
    const complete = chat.slice(
      chat.indexOf("async completePalette("),
      chat.indexOf("private async readCommands("),
    );
    assert.ok(complete.includes("this.commands()"), "the rows come from the catalog");
    assert.ok(complete.includes("commandRows("), "and are ranked in the shared module");
    assert.ok(complete.includes('k: "paletteRows"'), "labelled with the question it answers");

    const load = chat.slice(
      chat.indexOf("private async commands("),
      chat.indexOf("private async readCommands("),
    );
    assert.ok(load.includes("this.readCommands(root)"), "the palette's read is shared");
    assert.ok(
      load.includes("if (this.commandCache?.root === root) return this.commandCache.entries;"),
      "an answer is kept with the folder it came from, so another one cannot borrow it",
    );

    const read = chat.slice(
      chat.indexOf("private async readCommands("),
      chat.indexOf("private async workspacePathList("),
    );
    assert.ok(read.includes('["commands", "--json"]'), "the catalog is the CLI's own answer");
    assert.ok(read.includes("parseCommandList("), "parsed into the panel's shape");
    assert.ok(read.includes("this.commandCache = { root, entries }"), "and remembered per project");
    assert.ok(
      read.includes("this.commandCache = { root, entries: [] }"),
      "a failed read is remembered too, so a broken CLI is spawned once",
    );
    assert.equal(
      (read.match(/this\.folder\(\)\?\.uri\.fsPath !== root/g) ?? []).length,
      2,
      "and an answer that outlived its folder is neither returned nor kept",
    );
    assert.equal(
      (chat.match(/this\.commandCache = null;/g) ?? []).length,
      2,
      "a setting change and a finished turn are what re-read it",
    );

    // The view draws the rows it is handed: it cannot know what the project
    // holds, and a second copy of the rules there would drift.
    assert.equal(
      renderer.includes("parseCommandList"),
      false,
      "the renderer never reads the catalog",
    );
    assert.ok(
      renderer.includes('{ k: "completePalette"'),
      "the view asks for the rows rather than listing its own",
    );
    assert.ok(
      chatView.includes('case "completePalette":'),
      "the webview's question reaches the controller",
    );

    // Taking a row is what activates a skill: `/name` is the message the CLI
    // expands, while a built-in is one the panel performs itself, and one it has
    // no action for is answered rather than shipped to the model as a prompt.
    const send = chat.slice(
      chat.indexOf("async send("),
      chat.indexOf("private ", chat.indexOf("async send(")),
    );
    assert.ok(
      send.includes("routeCommand(await this.commands(), message)"),
      "send routes a slash command",
    );
    assert.ok(send.includes("runPanelCommand("), "performing a built-in the panel owns");
    assert.ok(send.includes("is not one this panel runs"), "and saying so for one it cannot");
    assert.ok(
      send.indexOf("routeCommand(") < send.indexOf("this.queue.push"),
      "a built-in is not queued as a prompt either",
    );
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
