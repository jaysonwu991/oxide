"use strict";
// The chat webview: a themed transcript, a composer, and the bridge back to
// the controller. The webview is a dumb renderer — every event decision lives
// in `core/protocol.ts`, in the extension host.
var __createBinding = (this && this.__createBinding) || (Object.create ? (function(o, m, k, k2) {
    if (k2 === undefined) k2 = k;
    var desc = Object.getOwnPropertyDescriptor(m, k);
    if (!desc || ("get" in desc ? !m.__esModule : desc.writable || desc.configurable)) {
      desc = { enumerable: true, get: function() { return m[k]; } };
    }
    Object.defineProperty(o, k2, desc);
}) : (function(o, m, k, k2) {
    if (k2 === undefined) k2 = k;
    o[k2] = m[k];
}));
var __setModuleDefault = (this && this.__setModuleDefault) || (Object.create ? (function(o, v) {
    Object.defineProperty(o, "default", { enumerable: true, value: v });
}) : function(o, v) {
    o["default"] = v;
});
var __importStar = (this && this.__importStar) || (function () {
    var ownKeys = function(o) {
        ownKeys = Object.getOwnPropertyNames || function (o) {
            var ar = [];
            for (var k in o) if (Object.prototype.hasOwnProperty.call(o, k)) ar[ar.length] = k;
            return ar;
        };
        return ownKeys(o);
    };
    return function (mod) {
        if (mod && mod.__esModule) return mod;
        var result = {};
        if (mod != null) for (var k = ownKeys(mod), i = 0; i < k.length; i++) if (k[i] !== "default") __createBinding(result, mod, k[i]);
        __setModuleDefault(result, mod);
        return result;
    };
})();
Object.defineProperty(exports, "__esModule", { value: true });
exports.ChatViewProvider = void 0;
const path = __importStar(require("node:path"));
const vscode = __importStar(require("vscode"));
class ChatViewProvider {
    extensionUri;
    controller;
    static viewType = "oxide.chat";
    constructor(extensionUri, controller) {
        this.extensionUri = extensionUri;
        this.controller = controller;
    }
    resolveWebviewView(view) {
        const media = vscode.Uri.joinPath(this.extensionUri, "media");
        view.webview.options = {
            enableScripts: true,
            localResourceRoots: [media],
        };
        view.webview.html = this.html(view.webview);
        view.webview.onDidReceiveMessage((message) => {
            void this.handle(message, view);
        });
        this.controller.attach(view);
    }
    async handle(message, view) {
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
            case "removeContext":
                if (typeof message.id === "number")
                    this.controller.removeContext(message.id);
                return;
            case "clearContext":
                this.controller.clearContext();
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
    async openUrl(url) {
        let parsed;
        try {
            parsed = vscode.Uri.parse(url, true);
        }
        catch {
            return;
        }
        if (parsed.scheme !== "http" && parsed.scheme !== "https")
            return;
        await vscode.env.openExternal(parsed);
    }
    /// A path mentioned in a tool card opens in the editor. Only files inside the
    /// workspace are opened, so a path the model invented cannot probe the disk.
    async reveal(target, line) {
        if (!target)
            return;
        const root = vscode.workspace.workspaceFolders?.[0]?.uri.fsPath;
        const absolute = path.isAbsolute(target) ? target : path.join(root ?? "", target);
        const uri = vscode.Uri.file(absolute);
        if (vscode.workspace.workspaceFolders?.length && !vscode.workspace.getWorkspaceFolder(uri)) {
            return;
        }
        try {
            const stat = await vscode.workspace.fs.stat(uri);
            if (stat.type === vscode.FileType.Directory)
                return;
        }
        catch {
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
    html(webview) {
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
    <span id="folder">Oxide</span>
    <span id="model"></span>
  </div>
  <div id="actions">
    <button id="new-session" class="ghost" title="New session">New</button>
    <button id="resume-session" class="ghost" title="Resume a session">Resume</button>
  </div>
</header>
<main id="transcript" tabindex="0">
  <div id="empty" class="empty">
    <p>Ask Oxide to make a change, explain code, or run something.</p>
    <p class="hint">Runs use the same configuration, sessions and project trust as the terminal: <code>oxide</code> starts a turn with <code>--mode json</code>.</p>
  </div>
</main>
<footer>
  <div id="usage" class="usage"></div>
  <div id="composer">
    <div id="chips" class="chips" hidden></div>
    <textarea id="input" rows="1" spellcheck="false"
      placeholder="Ask Oxide… (Enter to send, Shift+Enter for a newline)"></textarea>
    <div id="bar">
      <span id="status">Idle</span>
      <span class="spacer"></span>
      <button id="stop" class="ghost" hidden>Stop</button>
      <button id="send" class="primary" disabled>Send</button>
    </div>
  </div>
</footer>
<script nonce="${nonce}" src="${script}"></script>
</body>
</html>`;
    }
}
exports.ChatViewProvider = ChatViewProvider;
function nonceValue() {
    const alphabet = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
    let value = "";
    for (let i = 0; i < 32; i += 1) {
        value += alphabet.charAt(Math.floor(Math.random() * alphabet.length));
    }
    return value;
}
//# sourceMappingURL=chatView.js.map