// The chat webview: a themed transcript, a composer, and the bridge back to
// the controller. The webview is a dumb renderer — every event decision lives
// in `core/protocol.ts`, in the extension host.

import * as path from "node:path";
import * as vscode from "vscode";

import { ChatController } from "./chat";
import { isApprovalDecision } from "./core/approvals";
import { CHANGE_SCHEME, diffPlan, snapshotQuery, type DiffPlan } from "./core/changes";
import { questionAnswers } from "./core/questions";
import { CHAT_VIEW, CHAT_VIEW_SECONDARY } from "./core/views";

/// Inline icons for the view's buttons: the webview cannot load VS Code's
/// codicon font, so the handful of glyphs it needs are inlined SVG that inherit
/// `currentColor` like the rest of the chrome, matching the terminal and the
/// desktop mark rather than shipping a font.
const ICONS = {
  new: `<svg viewBox="0 0 16 16" width="15" height="15" aria-hidden="true"><path d="M8 3.6v8.8M3.6 8h8.8" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round"/></svg>`,
  resume: `<svg viewBox="0 0 16 16" width="15" height="15" aria-hidden="true"><circle cx="8" cy="8" r="5.2" fill="none" stroke="currentColor" stroke-width="1.5"/><path d="M8 5.2V8.2l2.1 1.3" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/></svg>`,
  attach: `<svg viewBox="0 0 24 24" width="16" height="16" aria-hidden="true"><path d="M21.44 11.05l-9.19 9.19a6 6 0 0 1-8.49-8.49l9.19-9.19a4 4 0 0 1 5.66 5.66l-9.2 9.19a2 2 0 0 1-2.83-2.83l8.49-8.48" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"/></svg>`,
  send: `<svg viewBox="0 0 16 16" width="15" height="15" aria-hidden="true"><path d="M8 13.4V3.4M4 7.4 8 3.4l4 4" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"/></svg>`,
  stop: `<svg viewBox="0 0 16 16" width="13" height="13" aria-hidden="true"><rect x="4" y="4" width="8" height="8" rx="1.7" fill="currentColor"/></svg>`,
  refresh: `<svg viewBox="0 0 24 24" width="14" height="14" aria-hidden="true"><path d="M23 4v6h-6" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"/><path d="M20.49 15a9 9 0 1 1-2.12-9.36L23 10" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"/></svg>`,
  close: `<svg viewBox="0 0 24 24" width="14" height="14" aria-hidden="true"><path d="M18 6 6 18M6 6l12 12" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round"/></svg>`,
  worktree: `<svg viewBox="0 0 16 16" width="14" height="14" aria-hidden="true"><rect x="1.9" y="3.1" width="7.2" height="8.4" rx="1.5" fill="none" stroke="currentColor" stroke-width="1.3"/><path d="M11.2 4.9h1.1a1.5 1.5 0 0 1 1.5 1.5v4.2a1.5 1.5 0 0 1-1.5 1.5h-1.1" fill="none" stroke="currentColor" stroke-width="1.3" stroke-linecap="round"/><path d="M3.8 6.2h3.4M3.8 8.5h2.3" fill="none" stroke="currentColor" stroke-width="1.3" stroke-linecap="round"/></svg>`,
  lock: `<svg viewBox="0 0 16 16" width="15" height="15" aria-hidden="true"><path d="M5.6 7.4V5.6a2.4 2.4 0 0 1 4.8 0v1.8" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round"/><rect x="3.7" y="7.2" width="8.6" height="6.4" rx="1.6" fill="none" stroke="currentColor" stroke-width="1.4"/></svg>`,
  help: `<svg viewBox="0 0 16 16" width="15" height="15" aria-hidden="true"><circle cx="8" cy="8" r="5.6" fill="none" stroke="currentColor" stroke-width="1.4"/><path d="M6.4 6.3a1.65 1.65 0 1 1 2.3 1.55c-.5.22-.7.5-.7 1.05v.2" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round"/><circle cx="8" cy="11.2" r=".8" fill="currentColor"/></svg>`,
};

interface WebviewMessage {
  k?: string;
  text?: string;
  mode?: "queue" | "steer";
  id?: number;
  /// The approval card's own fields: the broker's request id and the answer.
  requestId?: number;
  decision?: string;
  /// A question card's own field: the answers, each echoing the question it
  /// belongs to. An empty list is a dismissal.
  answers?: { question?: string; values?: unknown }[];
  url?: string;
  path?: string;
  line?: number;
  control?: string;
  /// A click inside a dialog: the action the row or its button carries, and the
  /// value it is about (a server name, a session id).
  action?: string;
  value?: string;
  /// The name and `data:` URL of a pasted or dropped blob.
  name?: string;
  data?: string;
  /// Absolute paths of dropped or picked files.
  paths?: string[];
  /// The composer's `@path` completion: what has been typed, where the caret
  /// is, and the sequence number the answer is labelled with.
  caret?: number;
  seq?: number;
  /// A change card's own field: which row of the listing was clicked.
  index?: number;
  direction?: number;
}

export class ChatViewProvider implements vscode.WebviewViewProvider {
  static readonly viewType = CHAT_VIEW;
  static readonly secondaryViewType = CHAT_VIEW_SECONDARY;

  constructor(
    private readonly extensionUri: vscode.Uri,
    private readonly controller: ChatController,
  ) {}

  resolveWebviewView(view: vscode.WebviewView): void {
    const media = vscode.Uri.joinPath(this.extensionUri, "media");
    view.webview.options = {
      enableScripts: true,
      localResourceRoots: [media],
    };
    view.webview.html = this.html(view.webview);
    view.webview.onDidReceiveMessage((message: WebviewMessage) => {
      void this.handle(message, view);
    });
    this.controller.attach(view);
  }

  private async handle(message: WebviewMessage, view: vscode.WebviewView): Promise<void> {
    switch (message.k) {
      case "ready":
        await this.paint(view);
        // A shortcut may have opened the chat and asked for something in the
        // composer before this pane had anything listening.
        this.controller.noteReady(view);
        return;
      case "focus":
        // The pane took the keyboard, which is what the editor's focus shortcut
        // toggles away from; `blur` is clicking back into the editor.
        this.controller.noteViewFocus(view, true);
        return;
      case "blur":
        this.controller.noteViewFocus(view, false);
        return;
      case "send":
        await this.controller.send(message.text ?? "", message.mode);
        return;
      case "queueAction":
        await this.controller.queueAction(
          message.action ?? "",
          message.index ?? -1,
          message.direction ?? 0,
          message.text ?? "",
        );
        return;
      case "stop":
        this.controller.stop();
        return;
      case "newSession":
        this.controller.newSession();
        return;
      case "resumeSession":
        await this.controller.resumeSession();
        return;
      case "attach":
        this.controller.addAttachment(message.data ?? "", message.name ?? "");
        return;
      case "attachClipboard":
        // The webview could not read a pasted file itself (macOS refuses a read
        // in a protected folder); the host reads the clipboard through the CLI,
        // which attaches what the pasteboard carries.
        await this.controller.attachClipboard();
        return;
      case "attachFiles":
        // A drop of files that exist on disk: the host decides whether each is
        // an image/PDF (media), text (context) or neither.
        for (const file of message.paths ?? []) this.controller.addFile(file);
        return;
      case "pickFiles":
        await this.controller.pickFiles();
        return;
      case "removeChip":
        if (typeof message.id === "number") this.controller.removeChip(message.id);
        return;
      case "approval":
        if (typeof message.requestId === "number" && isApprovalDecision(message.decision)) {
          this.controller.approve(message.requestId, message.decision);
        }
        return;
      case "question":
        // The view sends what the user typed and ticked; anything malformed is
        // dropped here rather than sent to the CLI as an answer to nothing.
        if (typeof message.requestId === "number" && Array.isArray(message.answers)) {
          this.controller.answerQuestion(message.requestId, questionAnswers(message.answers));
        }
        return;
      case "completeAt": {
        // The view posts what is in the composer and where the caret is; the
        // host decides what token that is and which paths answer it, so a
        // reference completes the way it does in the terminal. Only the pane
        // that asked is answered at all: the rows replace a token in its own
        // box, not in another one's.
        const answer = await this.controller.completeAt(
          message.text ?? "",
          message.caret ?? 0,
          message.seq ?? 0,
        );
        await view.webview.postMessage(answer);
        return;
      }
      case "completePalette": {
        // The `/` palette, on the same terms: the host owns the catalog (the
        // CLI's own) and answers only the pane that asked, because the rows
        // replace the value in that pane's box.
        const answer = await this.controller.completePalette(
          message.text ?? "",
          message.seq ?? 0,
          message.caret,
        );
        await view.webview.postMessage(answer);
        return;
      }
      case "clearChips":
        this.controller.clearChips();
        return;
      case "notice":
        this.controller.warn(message.text ?? "");
        return;
      case "control":
        await this.controller.control(message.control ?? "");
        return;
      case "dialogAction":
        // A row of the MCP or session dialog: the host decides what the action
        // means, so the view never has to know.
        await this.controller.dialogAction(message.action ?? "", message.value ?? "");
        return;
      case "dialogSearch":
        // Search/filter/custom input belongs to the open in-panel dialog; the
        // host rebuilds its rows, so the view never decides what matches.
        this.controller.searchDialog(message.text ?? "");
        return;
      case "openChangeDiff":
        // A row of a turn's change card. VS Code's
        // own diff editor is what draws it, against the baseline the run
        // recorded, so the panel renders no diff format of its own.
        if (typeof message.id === "number") {
          await this.openChange(message.id, message.index ?? 0);
        }
        return;
      case "openAllChanges":
        if (typeof message.id === "number") await this.openChange(message.id);
        return;
      case "undoChanges":
        // The Undo under a change card: the host asks before it restores, and
        // the restore itself is the CLI's (`changes undo`).
        if (typeof message.id === "number") this.controller.undoChanges(message.id);
        return;
      case "openUrl":
        await this.openUrl(message.url ?? "");
        return;
      case "reveal":
        await this.reveal(message.path ?? "", message.line);
        return;
      default:
        return;
    }
  }

  /// A click on a turn's change card, opened in VS Code's own diff editor. The
  /// panel holds no diff of its own: the left side is the file as the run found
  /// it, which exists only in the project's shadow snapshot, so it is served by
  /// the `CHANGE_SCHEME` content provider out of the CLI's own read, and the
  /// right side is the file on disk — or an empty side for a file the run
  /// removed. With no row it is the whole turn, which VS Code draws as one
  /// multi-file diff whose rows are the listing the card just showed.
  private async openChange(id: number, index?: number): Promise<void> {
    const card = this.controller.changeCard(id);
    if (!card) {
      this.controller.warn("That turn's changes are no longer in the transcript.");
      return;
    }
    // The folder the card's run started in, not the active one: a multi-root
    // window can move the active editor to another root while the card stays in
    // the transcript, and the card's paths and baseline belong to the run.
    const root = card.project || this.controller.workspaceRoot();
    if (!root || !card.rows.length) return;
    if (index === undefined) {
      const resources = card.rows.map((row) =>
        this.changeUris(root, card.project, diffPlan(card.baseline, row)),
      );
      await vscode.commands.executeCommand("vscode.changes", card.title, resources);
      return;
    }
    const plan = this.controller.changeTarget(id, index);
    if (!plan) return;
    const [, baseline, current] = this.changeUris(root, card.project, plan);
    await vscode.commands.executeCommand("vscode.diff", baseline, current, plan.title, {
      preview: false,
      preserveFocus: false,
    });
  }

  /// What the diff editors want for one file, as the triple `vscode.changes`
  /// destructures — `[label, original, modified]`, the file's own URI first, then
  /// the side the run found and the side it is on now (`[left, right]`, which is
  /// the pair `vscode.diff` takes off the same triple). An added file has no left
  /// side and a deleted one no right, so that side is an empty document rather
  /// than a path that is not there.
  private changeUris(
    root: string,
    project: string,
    plan: DiffPlan,
  ): [vscode.Uri, vscode.Uri, vscode.Uri] {
    const current = vscode.Uri.file(path.join(root, plan.path));
    const baseline = snapshotUri(plan.path, plan.baseline, project);
    const empty = snapshotUri(plan.path, null, project);
    return [current, baseline, plan.present ? current : empty];
  }

  /// The pane asks for everything it needs to paint itself once its script is
  /// listening, so a repainted panel restores the whole transcript — and a
  /// dialog the other pane opened, which both panes show.
  private async paint(view: vscode.WebviewView): Promise<void> {
    await view.webview.postMessage(this.controller.stateMessage());
    const dialog = this.controller.dialogMessage();
    if (dialog) await view.webview.postMessage(dialog);
  }

  /// Links in a reply open in the user's browser; a webview cannot navigate.
  private async openUrl(url: string): Promise<void> {
    let parsed: vscode.Uri;
    try {
      parsed = vscode.Uri.parse(url, true);
    } catch {
      return;
    }
    if (parsed.scheme !== "http" && parsed.scheme !== "https") return;
    await vscode.env.openExternal(parsed);
  }

  /// A path mentioned in a tool card opens in the editor. Only files inside the
  /// workspace are opened, so a path the model invented cannot probe the disk.
  private async reveal(target: string, line?: number): Promise<void> {
    if (!target) return;
    const root = vscode.workspace.workspaceFolders?.[0]?.uri.fsPath;
    const absolute = path.isAbsolute(target) ? target : path.join(root ?? "", target);
    const uri = vscode.Uri.file(absolute);
    if (vscode.workspace.workspaceFolders?.length && !vscode.workspace.getWorkspaceFolder(uri)) {
      return;
    }
    try {
      const stat = await vscode.workspace.fs.stat(uri);
      if (stat.type === vscode.FileType.Directory) return;
    } catch {
      return;
    }
    const document = await vscode.workspace.openTextDocument(uri);
    const editor = await vscode.window.showTextDocument(document, { preview: true });
    if (line && line > 0) {
      const position = new vscode.Position(Math.min(line - 1, document.lineCount - 1), 0);
      editor.selection = new vscode.Selection(position, position);
      editor.revealRange(new vscode.Range(position, position), vscode.TextEditorRevealType.InCenter);
    }
  }

  private html(webview: vscode.Webview): string {
    const nonce = nonceValue();
    const media = vscode.Uri.joinPath(this.extensionUri, "media");
    const script = webview.asWebviewUri(vscode.Uri.joinPath(media, "main.js"));
    const style = webview.asWebviewUri(vscode.Uri.joinPath(media, "style.css"));
    const diagrams = webview.asWebviewUri(vscode.Uri.joinPath(media, "diagrams.js"));
    const diagramStyle = webview.asWebviewUri(vscode.Uri.joinPath(media, "diagrams.css"));
    return `<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="UTF-8">
<meta name="viewport" content="width=device-width, initial-scale=1.0">
<meta http-equiv="Content-Security-Policy" content="default-src 'none'; style-src ${webview.cspSource} 'unsafe-inline'; script-src 'nonce-${nonce}'; img-src ${webview.cspSource} data:; font-src ${webview.cspSource};">
<link href="${style}" rel="stylesheet">
<link href="${diagramStyle}" rel="stylesheet">
<title>Oxide</title>
</head>
<body>
<header id="header">
  <span id="title" data-i18n="surface.newChat">New chat</span>
  <div id="actions">
    <button id="new-session" class="icon" title="New chat" aria-label="New chat" data-i18n-title="panel.newChat" data-i18n-aria="panel.newChat">${ICONS.new}</button>
    <button id="resume-session" class="icon" title="Session history" aria-label="Session history" aria-expanded="false" aria-controls="dialog" data-i18n-title="panel.sessionHistory" data-i18n-aria="panel.sessionHistory">${ICONS.resume}</button>
    <span class="actions-sep" aria-hidden="true"></span>
    <button id="approvals" class="icon" data-control="permissions" title="Tool approvals" aria-label="Tool approvals" data-i18n-title="composer.permissions" data-i18n-aria="composer.permissions">${ICONS.lock}</button>
    <button id="shortcuts" class="icon" data-control="shortcuts" title="Keyboard shortcuts" aria-label="Keyboard shortcuts" data-i18n-title="composer.help" data-i18n-aria="composer.help">${ICONS.help}</button>
  </div>
</header>
<section id="run-strip" hidden>
  <span id="run-strip-text"></span>
  <button id="run-open" class="run-open" type="button" data-i18n="surface.open">Open</button>
</section>
<section id="dialog" class="popover" hidden aria-labelledby="dialog-title">
  <div class="popover-head">
    <h2 id="dialog-title"></h2>
    <span id="dialog-count" class="popover-count" hidden></span>
    <div class="popover-actions">
      <button id="dialog-refresh" class="icon" hidden title="Recheck" aria-label="Recheck" data-i18n-title="surface.recheck" data-i18n-aria="surface.recheck">${ICONS.refresh}</button>
      <button id="dialog-close" class="icon" title="Close" aria-label="Close" data-i18n-title="surface.close" data-i18n-aria="surface.close">${ICONS.close}</button>
    </div>
  </div>
  <div id="dialog-search" class="popover-search" hidden>
    <input id="dialog-search-input" type="search" spellcheck="false" autocomplete="off" placeholder="Search sessions…" aria-label="Search sessions" data-i18n-placeholder="surface.searchSessionsPlaceholder" data-i18n-aria="surface.searchSessionsLabel">
    <button id="dialog-search-clear" class="icon" hidden title="Clear the search" aria-label="Clear the search" data-i18n-title="surface.clearTheSearch" data-i18n-aria="surface.clearTheSearch">${ICONS.close}</button>
  </div>
  <p id="dialog-sub" class="popover-sub"></p>
  <p id="dialog-note" class="popover-note" hidden></p>
  <div id="dialog-list" class="popover-list"></div>
</section>
<main id="transcript" tabindex="0">
  <div id="run-time" class="run-time" role="timer" hidden><span id="run-time-text"></span></div>
  <div id="empty" class="empty">
    <p id="empty-lead" data-i18n="surface.askOxideToMakeAChangeExplainCodeOrRunSomething">Ask Oxide to make a change, explain code, or run something.</p>
    <div id="empty-suggestions" class="suggestions" hidden></div>
    <p class="hint" data-i18n-code="panel.sharedRuntime">Runs use the same configuration, sessions and project trust as the terminal: \`oxide\` starts a turn with \`--mode rpc\`, so a tool that needs your approval waits for an answer here.</p>
  </div>
</main>
<button id="jump-latest" class="jump-latest" type="button" title="Jump to latest message" aria-label="Jump to latest message" data-i18n-title="topbar.jumpLatest" data-i18n-aria="topbar.jumpLatest" hidden><svg viewBox="0 0 24 24" width="20" height="20" aria-hidden="true"><path d="M12 4v16M5 13l7 7 7-7" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"/></svg></button>
<footer>
  <button id="live-changes" class="live-changes" type="button" hidden
    title="View changes" aria-label="View changes" data-i18n-title="topbar.viewChanges" data-i18n-aria="topbar.viewChanges"></button>
  <div id="strip" class="composer-strip">
    <div class="strip-facts">
      <span id="branch-wrap" class="branch-wrap" hidden><span class="foot-icon" aria-hidden="true"><svg viewBox="0 0 16 16"><circle cx="4" cy="3" r="1.5"/><circle cx="4" cy="13" r="1.5"/><circle cx="12" cy="5" r="1.5"/><path d="M4 4.5v7M5.5 11c4 0 6.5-1.5 6.5-4.5"/></svg></span><span id="branch"></span></span>
      <button id="worktree" class="fact fact-button" type="button" data-control="worktree"
        title="Worktree" aria-label="Worktree" aria-haspopup="dialog" aria-controls="dialog" data-i18n-title="composer.worktree" data-i18n-aria="composer.worktree" hidden><span class="foot-icon" aria-hidden="true">${ICONS.worktree}</span><span id="worktree-name" hidden></span></button>
    </div>
  </div>
  <div id="composer">
    <div id="queue" class="queued-messages" aria-label="Queued messages" hidden data-i18n-aria="surface.queuedMessages"></div>
    <div id="queue-menu" class="queued-message-menu" role="menu"
      aria-label="Queued message actions" hidden data-i18n-aria="surface.queuedMessageActions"></div>
    <div id="at" class="at-list" role="listbox" aria-label="Files and folders" hidden data-i18n-aria="surface.filesAndFolders"></div>
    <div id="chips" class="chips" hidden></div>
    <div id="input" class="markdown-input body" contenteditable="true" spellcheck="false"
      role="textbox" aria-multiline="true" aria-label="Message" data-i18n-aria="composer.message"
      data-placeholder="Ask Oxide…  Enter to send · Shift+Enter for a newline · paste or drop an image" data-i18n-placeholder="panel.placeholder"></div>
    <div id="bar">
      <button id="attach" class="icon" title="Attach images, PDFs or files (paste or drop them here too)" aria-label="Attach" data-i18n-title="panel.attachHint" data-i18n-aria="composer.attach">${ICONS.attach}</button>
      <div id="meta" class="meta" aria-label="Turn settings" data-i18n-aria="surface.turnSettings"></div>
      <span id="status" hidden></span>
      <span class="spacer"></span>
      <div id="model-settings" class="model-settings" aria-label="Model and reasoning" data-i18n-aria="surface.modelAndReasoning"></div>
      <div class="context-status">
      <span id="usage" class="usage" hidden><span id="usage-text" role="note" tabindex="0"></span></span>
      <span id="gauge" class="gauge" role="img" tabindex="0" hidden><svg viewBox="0 0 26 26" width="26" height="26" aria-hidden="true"><circle cx="13" cy="13" r="12.25" fill="none" stroke="currentColor" stroke-width="1.5" opacity="0.3"/><circle id="gauge-fill" cx="13" cy="13" r="12.25" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" transform="rotate(-90 13 13)" stroke-dasharray="0 76.97"/></svg><span id="context-percent" hidden></span></span>
      </div>
      <button id="stop" class="icon danger run-control composer-action" hidden title="Stop the running turn (Esc)" aria-label="Stop" data-i18n-title="composer.stop" data-i18n-aria="composer.stop"><span class="stop-icon" aria-hidden="true">${ICONS.stop}</span></button>
      <button id="send" class="icon primary composer-action" disabled title="Send (Enter)" aria-label="Send" data-i18n-title="composer.sendHint" data-i18n-aria="composer.send">${ICONS.send}</button>
    </div>
    <div id="dropzone" hidden><span data-i18n="surface.dropFilesToAttach">Drop files to attach</span></div>
  </div>
</footer>
<div id="image-view" class="overlay image-overlay" role="dialog" aria-modal="true" aria-label="Image preview" hidden data-i18n-aria="surface.imagePreview">
  <div class="image-frame">
    <div class="image-head">
      <span id="image-view-name" class="image-name"></span>
      <button id="image-view-close" class="icon danger" title="Close (Esc)" aria-label="Close" data-i18n-aria="surface.close" data-i18n-title="surface.closeEsc">✕</button>
    </div>
    <img id="image-view-img" alt="Attachment preview" data-i18n-alt="surface.attachmentPreview">
  </div>
</div>
<script nonce="${nonce}" src="${diagrams}"></script>
<script nonce="${nonce}" src="${script}"></script>
</body>
</html>`;
  }
}

/// The URI the baseline side of a diff is served by: the provider registered for
/// `CHANGE_SCHEME` reads the file out of the project's shadow snapshot, keyed by
/// the project and revision in the query (`snapshotQuery`). No revision is a side
/// with no content — a file the run added, or one it removed.
function snapshotUri(file: string, revision: string | null, project: string): vscode.Uri {
  return vscode.Uri.from({
    scheme: CHANGE_SCHEME,
    path: `/${file}`,
    query: snapshotQuery(project, revision),
  });
}

function nonceValue(): string {
  const alphabet = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
  let value = "";
  for (let i = 0; i < 32; i += 1) {
    value += alphabet.charAt(Math.floor(Math.random() * alphabet.length));
  }
  return value;
}
