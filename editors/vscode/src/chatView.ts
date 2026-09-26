// The chat webview: a themed transcript, a composer, and the bridge back to
// the controller. The webview is a dumb renderer — every event decision lives
// in `core/protocol.ts`, in the extension host.

import * as path from "node:path";
import * as vscode from "vscode";

import { ChatController } from "./chat";
import { isApprovalDecision } from "./core/approvals";
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
};

interface WebviewMessage {
  k?: string;
  text?: string;
  id?: number;
  /// The approval card's own fields: the broker's request id and the answer.
  requestId?: number;
  decision?: string;
  url?: string;
  path?: string;
  line?: number;
  control?: string;
  /// The name and `data:` URL of a pasted or dropped blob.
  name?: string;
  data?: string;
  /// Absolute paths of dropped or picked files.
  paths?: string[];
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
        await view.webview.postMessage(this.controller.stateMessage());
        return;
      case "send":
        await this.controller.send(message.text ?? "");
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
      case "clearChips":
        this.controller.clearChips();
        return;
      case "notice":
        this.controller.warn(message.text ?? "");
        return;
      case "control":
        await this.controller.control(message.control ?? "");
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
    return `<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="UTF-8">
<meta name="viewport" content="width=device-width, initial-scale=1.0">
<meta http-equiv="Content-Security-Policy" content="default-src 'none'; style-src ${webview.cspSource}; script-src 'nonce-${nonce}'; img-src ${webview.cspSource} data:; font-src ${webview.cspSource};">
<link href="${style}" rel="stylesheet">
<title>Oxide</title>
</head>
<body>
<header id="header">
  <div id="identity">
    <span id="folder">New chat</span>
    <span id="model"></span>
  </div>
  <div id="actions">
    <button id="new-session" class="icon" title="New session" aria-label="New session">${ICONS.new}</button>
    <button id="resume-session" class="icon" title="Resume a session" aria-label="Resume a session">${ICONS.resume}</button>
  </div>
</header>
<main id="transcript" tabindex="0">
  <div id="empty" class="empty">
    <p>Ask Oxide to make a change, explain code, or run something.</p>
    <p class="hint">Runs use the same configuration, sessions and project trust as the terminal: <code>oxide</code> starts a turn with <code>--mode rpc</code>, so a tool that needs your approval waits for an answer here.</p>
  </div>
</main>
<footer>
  <div id="meta" class="meta"></div>
  <div id="composer">
    <div id="chips" class="chips" hidden></div>
    <textarea id="input" rows="2" spellcheck="false"
      placeholder="Ask Oxide…  Enter to send · Shift+Enter for a newline · paste or drop an image"></textarea>
    <div id="bar">
      <button id="attach" class="icon" title="Attach images, PDFs or files (paste or drop them here too)" aria-label="Attach">${ICONS.attach}</button>
      <span id="status">Idle</span>
      <span id="elapsed" hidden></span>
      <span class="spacer"></span>
      <button id="stop" class="icon" hidden title="Stop the running turn (Esc)" aria-label="Stop">${ICONS.stop}</button>
      <button id="send" class="icon primary" disabled title="Send (Enter)" aria-label="Send">${ICONS.send}</button>
    </div>
    <div id="dropzone" hidden><span>Drop files to attach</span></div>
  </div>
  <div id="footline">
    <span id="usage" class="usage"><span id="usage-text"></span></span>
    <span id="gauge" class="gauge" hidden><span id="gauge-fill"></span></span>
    <span id="branch"></span>
  </div>
</footer>
<script nonce="${nonce}" src="${script}"></script>
</body>
</html>`;
  }
}

function nonceValue(): string {
  const alphabet = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
  let value = "";
  for (let i = 0; i < 32; i += 1) {
    value += alphabet.charAt(Math.floor(Math.random() * alphabet.length));
  }
  return value;
}
