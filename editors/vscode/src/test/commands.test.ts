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
    commands: { command: string; title: string; category?: string; icon?: string }[];
    menus: Record<string, { command: string; when?: string; group?: string }[]>;
    keybindings: { command: string; key: string; mac?: string; when?: string }[];
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

  it("handles every message the host sends into the composer", () => {
    // The other direction of the same bargain: a host message with no case in
    // the renderer is an insert that silently never happens.
    for (const kind of ["insert", "focusComposer"]) {
      assert.ok(renderer.includes(`case "${kind}":`), `the view handles "${kind}"`);
    }
  });

  it("offers the editor's own chrome: the toolbar, the caret toggle and the insert key", () => {
    // Claude Code's editor chrome, in oxide's own words: the panel's icon on
    // the editor toolbar, a key that toggles the caret between the editor and
    // the composer, and a key that writes the file and the selection into the
    // composer as the `@path` reference the CLI expands.
    const open = manifest.contributes.commands.find((entry) => entry.command === "oxide.openChat");
    assert.ok(open?.icon, "Open Chat carries the icon a toolbar draws");
    const toolbar = manifest.contributes.menus["editor/title"] ?? [];
    assert.ok(
      toolbar.some(
        (entry) => entry.command === "oxide.openChat" && entry.when === "resourceScheme == file",
      ),
      "the editor toolbar offers it",
    );

    const bindings = new Map(
      manifest.contributes.keybindings.map((entry) => [entry.command, entry] as const),
    );
    assert.deepEqual(
      { key: bindings.get("oxide.focusInput")?.key, mac: bindings.get("oxide.focusInput")?.mac },
      { key: "ctrl+escape", mac: "cmd+escape" },
    );
    assert.deepEqual(
      {
        key: bindings.get("oxide.insertReference")?.key,
        when: bindings.get("oxide.insertReference")?.when,
      },
      { key: "alt+k", when: "editorTextFocus" },
    );
  });

  it("leaves the caret in the composer when the panel is brought forward", () => {
    // The toolbar mark, the status bar and the palette are one command, and a
    // click to chat is a click to type: focusing a webview view does not focus
    // its own DOM, so the caret has to be sent separately.
    const open = extension.slice(extension.indexOf('"oxide.openChat",'));
    assert.ok(
      open.slice(0, open.indexOf("oxide.newSession")).includes("controller.focusComposer()"),
      "raising the panel puts the caret in the message box",
    );
  });

  it("toggles the caret from what the pane reported, not from what is on screen", () => {
    // The composer's own text belongs to the pane, and there is a pane in the
    // activity bar and one in the secondary side bar: `Cmd+Esc` has to know
    // which side the caret is on, and the panel is the only thing that knows.
    const toggle = extension.slice(
      extension.indexOf('"oxide.focusInput"'),
      extension.indexOf('"oxide.insertReference"'),
    );
    assert.ok(toggle.includes("controller.chatFocused"), "it asks the controller");
    assert.ok(toggle.includes('"workbench.action.focusActiveEditorGroup"'), "and hands the caret back");
    assert.ok(toggle.includes("controller.focusComposer()"), "and puts it in the box");
    assert.ok(
      chat.includes('{ k: "focusComposer" }'),
      "the controller asks the pane it belongs to, since focusing a pane is not focusing its box",
    );
    // A pane that is still being built has no listener yet, so what was asked
    // for waits for the `ready` that says one is there.
    assert.ok(chat.includes("noteReady"), "the request is kept until a pane is listening");
    assert.ok(chatView.includes("this.controller.noteReady(view)"), "and applied then");
    assert.ok(
      /readyViews\.has\(view\)/.test(chat),
      "and only a pane that has said so is posted into at all",
    );
    assert.ok(chat.includes("this.pendingComposer.push(message)"), "the insert waits with it");
    assert.ok(chatView.includes('this.controller.noteViewFocus(view, true)'), "the pane reports `focus`");
    assert.ok(chatView.includes('this.controller.noteViewFocus(view, false)'), "and `blur`");
  });

  it("reads the file and the selection in the host, and only splices in the view", () => {
    const insert = extension.slice(
      extension.indexOf('"oxide.insertReference"'),
      extension.indexOf('"oxide.newSession"'),
    );
    assert.ok(insert.includes("vscode.window.activeTextEditor"), "the host reads the editor");
    assert.ok(insert.includes("fileReference("), "and builds the reference");
    assert.ok(insert.includes("controller.insertReference(reference)"), "and hands it over");
    assert.ok(
      chat.includes('{ k: "insert", text: reference }'),
      "the controller sends the reference, not the file's text",
    );
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
      chat.indexOf("stop():"),
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
      helper.replace(/\s+/g, " ").includes(
        "sessionDialog( this.sessions, this.transcript.sessionId, note, this.liveSession(), this.sessionQuery, )",
      ),
      "and that place passes the open thread's id, its live stand-in and the filter",
    );
    // The sites that repaint it: the load, its failure, the listing itself, a
    // row while a turn runs, a delete while a turn runs, a failed delete and a
    // keystroke in the search box.
    assert.ok(
      (chat.match(/this\.showSessions\(/g) ?? []).length >= 6,
      "every redraw uses it rather than composing the dialog again",
    );
  });

  it("toggles the session history from the header", () => {
    // One button for the listing the command names: the click that opens it is
    // the click that closes it, which is the one thing the header can offer
    // without a state for the button to contradict — so the panel's own history
    // control is a toggle rather than a button that reopens what is already up.
    const command = manifest.contributes.commands.find(
      (entry) => entry.command === "oxide.resumeSession",
    );
    assert.equal(command?.title, "Session History");
    const at = chatView.indexOf('id="resume-session"');
    const button = chatView.slice(at, chatView.indexOf("</button>", at));
    const label = "Session history";
    for (const attribute of ["title", "aria-label"]) {
      const value = new RegExp(`${attribute}="([^"]+)"`).exec(button)?.[1];
      assert.equal(value, label, `${attribute} names the button`);
    }
    // The panel and the palette name it with the same words — the palette title
    // case like the manifest's other entries, the panel sentence case like its
    // own chrome — so a reader who asked for one finds the other.
    assert.equal(label.toLowerCase(), command!.title.toLowerCase());
    // The state lives in the markup, and the element it acts on is named, so a
    // screen reader reads the same toggle the panel paints.
    assert.ok(button.includes('aria-expanded="false"'), "it says whether the listing is up");
    assert.ok(button.includes('aria-controls="dialog"'), "and which element it shows");
    assert.ok(
      renderer.includes(
        'historyButton.setAttribute("aria-expanded", dialog && dialog.kind === "sessions"',
      ),
      "the renderer keeps that state in step with what it paints",
    );

    const toggle = chat.slice(
      chat.indexOf("async resumeSession("),
      chat.indexOf("async openSessions("),
    );
    assert.ok(
      toggle.includes('if (this.dialog?.kind === "sessions")'),
      "a listing already up is what the click closes",
    );
    assert.ok(toggle.includes("this.closeDialog()"));
    assert.ok(toggle.includes("await this.openSessions()"), "and otherwise it opens");
    // Reading the store is all opening does: the rows refuse to switch threads
    // while a turn owns the session file, so the listing itself can be read
    // mid-turn rather than being a button that answers with a notice.
    const open = chat.slice(chat.indexOf("async openSessions("), chat.indexOf("searchSessions("));
    assert.equal(open.includes("this.turn"), false, "a listing is readable while a turn runs");
    assert.ok(
      open.includes('if (this.dialog?.kind !== "sessions") return;'),
      "and an answer to a listing closed while it was read is dropped",
    );
    // `/session` asks for the listing rather than toggling it, in both places it
    // is reached from: a command that names the history should not answer by
    // closing it.
    assert.equal(
      /isSessionCommand\(message\)\s*\)\s*\{\s*await this\.openSessions\(\)/.test(chat),
      true,
      "the command typed in the composer opens the history",
    );
    assert.equal(
      /isSessionCommand\(message\)\s*\)\s*\{\s*await this\.resumeSession\(\)/.test(chat),
      false,
      "and never the toggle",
    );
    const panel = chat.slice(
      chat.indexOf("private async runPanelCommand("),
      chat.indexOf('case "new":'),
    );
    assert.ok(
      panel.includes("return this.openSessions();"),
      "the palette's own `/session` row opens it too",
    );
  });

  it("filters the session listing in the host rather than the webview", () => {
    // The rows are the store's own answer, so the query is applied where they
    // are: the view asks, the host composes the narrowed listing — count and
    // note together — and the renderer never decides which thread matches.
    assert.ok(renderer.includes('vscode.postMessage({ k: "dialogSearch"'), "the box asks");
    assert.ok(
      chatView.includes('case "dialogSearch":'),
      "and the question reaches the controller",
    );
    const search = chat.slice(
      chat.indexOf("searchSessions(text: string)"),
      chat.indexOf("private async continueSession("),
    );
    assert.ok(search.includes("this.sessionQuery = text"), "the filter is the controller's");
    assert.ok(search.includes("this.showSessions()"), "and the redraw goes through one place");
    assert.ok(
      search.includes('this.dialog?.kind !== "sessions"'),
      "a keystroke that outlived its listing does nothing",
    );
    assert.ok(dialogs.includes("filterSessions(named, query)"), "filtered where the rows are");
    assert.equal(
      renderer.includes("filterSessions"),
      false,
      "the renderer never filters rows of its own",
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
    const prepare = chat.slice(
      chat.indexOf("private prepareSend("),
      chat.indexOf("private ", chat.indexOf("private prepareSend(") + 1),
    );
    assert.ok(prepare.includes("this.autoBlock()"), "the tracked file is read at send time");
    assert.ok(
      prepare.includes("carried.has(tracked.path)"),
      "and a file already carried is not sent twice",
    );

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

  it("narrows the tracked chip to a selection, and sends those lines", () => {
    // A selection is context the reader made, so the same chip follows it —
    // which needs the selection subscription, or the panel would go on holding
    // a whole file after the reader had selected three lines of it.
    assert.ok(
      extension
        .slice(extension.indexOf("onDidChangeTextEditorSelection"))
        .includes("controller.syncActiveEditor()"),
      "the selection keeps the chip in step too",
    );
    const sync = chat.slice(chat.indexOf("syncActiveEditor()"), chat.indexOf("private autoChip()"));
    assert.ok(sync.includes("selectionLines("), "the tracked state carries the lines");
    assert.ok(
      sync.includes("sameRange("),
      "and moving the caret inside them is not a change worth repainting for",
    );
    // The reference the insert shortcut writes is read by the same rule, so a
    // drag that stopped where a line starts does not name that line in either.
    const insert = extension.slice(
      extension.indexOf('"oxide.insertReference",'),
      extension.indexOf("oxide.newSession"),
    );
    assert.ok(insert.includes("selectionLines("), "the shortcut reads a selection the same way");

    // A chip removed for the file already tracked stays out: narrowing to a new
    // selection in it is the same chip following the reader, not the next file
    // the ✕ did not answer for, so it keeps its id and its hidden state.
    const same = sync.slice(sync.indexOf("this.auto?.file !== wanted"));
    assert.ok(
      same.indexOf("this.autoHidden = false") < same.indexOf("} else {"),
      "only another file brings the removed chip back",
    );
    assert.ok(same.includes("this.auto.selection = selection"), "and the chip keeps its id");

    // The chip is the only place the reader can see what the next message will
    // carry, so it names the lines and says them in words as well.
    const chip = chat.slice(chat.indexOf("private autoChip()"), chat.indexOf("private autoBlock()"));
    assert.ok(chip.includes("contextLabel("), "the label is the same path:5-10 spelling");
    assert.ok(chip.includes("selected — sent with the next message"), "with the words behind it");

    // And what goes is the slice the label named — the same rule a ranged `@`
    // reference inlines — not the whole file the lines came out of.
    const block = chat.slice(chat.indexOf("private autoBlock()"));
    assert.ok(block.includes("sliceLines(text, start, end)"), "the block is the selection");
    assert.ok(
      block.includes("startLine: start, endLine: end"),
      "under the range's own header, so the model is told which lines it read",
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
      send.indexOf("routeCommand(") < send.indexOf("this.turn.steer"),
      "a built-in is not queued or steered as a prompt either",
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
        send.indexOf(`${command}(message)`) < send.indexOf("this.turn.steer"),
        `${command} is answered before a message is queued, steered or prompted`,
      );
    }
  });

  it("sends an image on its own, and a busy message keeps its own chips", () => {
    // An image with no question is still a message the CLI accepts (`@shot.png`
    // with nothing typed is media, not an empty prompt), so the host must not
    // refuse it; it is only empty when there is neither text nor media.
    const prepare = chat.slice(
      chat.indexOf("private prepareSend("),
      chat.indexOf("private ", chat.indexOf("private prepareSend(") + 1),
    );
    assert.ok(
      prepare.includes("buildPrompt(expanded.inlined, carriedBlocks.filter("),
      "@path blocks land where they were typed, not above the message",
    );
    assert.ok(
      prepare.includes("if (!prompt && images.length === 0)"),
      "a message is only empty when it has neither prompt text nor media",
    );

    // A message sent while a turn runs hands its assembled prompt and media to
    // that RPC process before clearing the composer, so it keeps its chips and
    // the image is not left in the box to be sent twice.
    const send = chat.slice(
      chat.indexOf("async send("),
      chat.indexOf("private ", chat.indexOf("async send(")),
    );
    assert.ok(
      send.indexOf("this.turn.steer(prepared.prompt, prepared.images, followUp)") <
        send.indexOf("this.dropComposerChips()"),
      "a busy message is handed to the active turn before the composer is cleared",
    );
    assert.ok(
      send.includes("const accepted = await this.turn.steer") &&
        send.includes("if (!accepted)") &&
        send.includes("this.queue.push(prepared)"),
      "a busy message is retained when the active process cannot acknowledge delivery",
    );
    assert.ok(send.includes("this.startTurn(prepared, true)"), "an idle send starts its own turn");
    assert.ok(
      send.includes('`${followUp ? "Queued" : "Steering"}:'),
      "the selected busy behavior is announced",
    );
    assert.ok(
      send.includes("broadcastItem(this.transcript.pushUser(prepared.message"),
      "and its bubble appears while the active turn owns it",
    );

    assert.ok(send.includes('const followUp = busyMode === "queue"'));
  });

  it("names the thread as soon as the first message is sent", () => {
    // The header's title is computed from the first message, so it has to
    // travel with the status line the send emits: the next full `state`
    // message would repaint the whole view, and the header would lag behind
    // the bubble until then.
    const status = chat.slice(chat.indexOf("private broadcastStatus()"), chat.indexOf("get running"));
    assert.ok(status.includes("title: this.threadTitle()"), "the status carries the title");
    assert.ok(
      renderer.includes('titleLabel.textContent = message.title || "New chat"'),
      "and the webview paints it over the placeholder",
    );
  });
});
