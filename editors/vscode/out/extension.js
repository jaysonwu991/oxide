"use strict";
// Extension entry point: the commands, the editor context actions, and the
// status bar. All agent work goes through `ChatController`.
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
exports.activate = activate;
exports.deactivate = deactivate;
const fs = __importStar(require("node:fs"));
const os = __importStar(require("node:os"));
const path = __importStar(require("node:path"));
const vscode = __importStar(require("vscode"));
const chat_1 = require("./chat");
const chatView_1 = require("./chatView");
const cli_1 = require("./cli");
const config_1 = require("./core/config");
const prompt_1 = require("./core/prompt");
/// A whole-file context block is inlined into the prompt, so anything larger
/// than this is trimmed rather than shipped to the model in full.
const MAX_CONTEXT_LINES = 2_000;
function activate(context) {
    const output = vscode.window.createOutputChannel("Oxide");
    const controller = new chat_1.ChatController(output, (0, config_1.contextWindowFromEnv)(process.env));
    context.subscriptions.push(output, controller, vscode.window.registerWebviewViewProvider(chatView_1.ChatViewProvider.viewType, new chatView_1.ChatViewProvider(context.extensionUri, controller), { webviewOptions: { retainContextWhenHidden: true } }));
    const status = vscode.window.createStatusBarItem(vscode.StatusBarAlignment.Right, 90);
    status.command = "oxide.openChat";
    context.subscriptions.push(status);
    const refreshStatus = () => {
        if (!vscode.workspace.workspaceFolders?.length) {
            status.hide();
            return;
        }
        const file = path.join(configFile(), "config.json");
        const summary = (0, config_1.parseConfigSummary)((0, cli_1.readTextFile)(file) ?? "");
        const model = setting("model", "").trim() || summary?.model || "";
        const provider = summary?.provider ?? "";
        status.text = controller.running ? "$(sync~spin) Oxide" : "$(circuit-board) Oxide";
        const tooltip = new vscode.MarkdownString();
        tooltip.appendMarkdown(`**Oxide** — ${model ? `${provider ? `${provider} · ` : ""}${model}` : "no model configured"}\n\n`);
        tooltip.appendMarkdown(`Binary: \`${binaryPath()}\`\n\n`);
        tooltip.appendMarkdown(`Config: \`${file}\`\n\n`);
        tooltip.appendMarkdown("Click to open the chat. Provider logins happen in the terminal: run **Oxide: Open Terminal** and `/login` there.");
        status.tooltip = tooltip;
        status.show();
    };
    const guard = (run) => (...args) => {
        void run(...args).catch((error) => {
            output.appendLine(`[error] ${String(error)}`);
            void vscode.window.showErrorMessage(`Oxide: ${String(error)}`);
        });
    };
    context.subscriptions.push(controller.onDidChange.event(() => refreshStatus()), vscode.workspace.onDidChangeConfiguration((event) => {
        if (event.affectsConfiguration("oxide"))
            refreshStatus();
    }), vscode.window.onDidChangeActiveTextEditor(() => refreshStatus()), vscode.commands.registerCommand("oxide.openChat", () => {
        void vscode.commands.executeCommand(`${chatView_1.ChatViewProvider.viewType}.focus`);
    }), vscode.commands.registerCommand("oxide.newSession", () => controller.newSession()), vscode.commands.registerCommand("oxide.resumeSession", guard(() => controller.resumeSession())), vscode.commands.registerCommand("oxide.continueSession", () => controller.continueSession()), vscode.commands.registerCommand("oxide.stop", () => controller.stop()), vscode.commands.registerCommand("oxide.addToChat", guard((uri) => addToChat(controller, uri))), vscode.commands.registerCommand("oxide.askAboutSelection", guard(async () => {
        const chip = await addSelection(controller);
        const question = await vscode.window.showInputBox({
            title: "Oxide: ask about the selection",
            prompt: "What do you want to know?",
            placeHolder: "e.g. why does this retry loop spin?",
        });
        if (!question?.trim()) {
            if (chip)
                controller.removeContext(chip.id);
            return;
        }
        await controller.send(question);
    })), vscode.commands.registerCommand("oxide.explainSelection", guard(async () => {
        await addSelection(controller);
        await controller.send("Explain the attached selection: what it does, and anything surprising, risky, or subtly wrong about it.");
    })), vscode.commands.registerCommand("oxide.fixSelection", guard(async () => {
        await addSelection(controller);
        await controller.send("Fix the attached selection. Keep the change minimal and make it match the surrounding code.");
    })), vscode.commands.registerCommand("oxide.reviewChanges", guard(async () => {
        await vscode.commands.executeCommand(`${chatView_1.ChatViewProvider.viewType}.focus`);
        await controller.send("Review the uncommitted changes in this working tree. Summarize what changed, then flag anything wrong or risky. Do not modify files.");
    })), vscode.commands.registerCommand("oxide.openTerminal", () => openTerminal(binaryPath())), vscode.commands.registerCommand("oxide.showOutput", () => output.show(true)), vscode.commands.registerCommand("oxide.setModel", guard(() => controller.setModel())), vscode.commands.registerCommand("oxide.setReasoning", guard(() => controller.setReasoning())), vscode.commands.registerCommand("oxide.setProjectTrust", guard(() => controller.setProjectTrust())));
    refreshStatus();
}
function deactivate() {
    // Disposables registered in `context.subscriptions` are torn down by VS Code,
    // which cancels a running turn through `ChatController.dispose`.
}
function setting(key, fallback) {
    return vscode.workspace.getConfiguration("oxide").get(key, fallback);
}
function binaryPath() {
    return (0, cli_1.resolveBinary)(setting("binaryPath", "oxide"), {
        env: process.env,
        platform: process.platform,
        exists: cli_1.isFile,
    });
}
function configFile() {
    return (0, config_1.configDir)({
        platform: process.platform,
        env: process.env,
        home: os.homedir(),
        exists: fs.existsSync,
    });
}
/// Adds the editor's selection, or the whole file, as context. Returns the
/// chip so a cancelled command can take it back.
async function addSelection(controller) {
    const editor = vscode.window.activeTextEditor;
    if (!editor) {
        void vscode.window.showInformationMessage("Oxide: open a file first.");
        return null;
    }
    const document = editor.document;
    if (document.uri.scheme !== "file") {
        void vscode.window.showInformationMessage("Oxide: only files on disk can be attached.");
        return null;
    }
    const selection = editor.selection;
    const selected = !selection.isEmpty;
    const block = trimLines({
        path: controller.relativeTo(document.uri.fsPath),
        startLine: selected ? selection.start.line + 1 : undefined,
        endLine: selected ? selection.end.line + 1 : undefined,
        text: selected ? document.getText(selection) : document.getText(),
    });
    return add(controller, block);
}
async function addToChat(controller, uri) {
    if (!uri || uri.scheme !== "file") {
        await addSelection(controller);
        return;
    }
    const relative = controller.relativeTo(uri.fsPath);
    if ((0, prompt_1.isAttachmentPath)(uri.fsPath)) {
        // Images and PDFs travel as media (`--image`), not as prompt text.
        add(controller, { path: relative, text: "" });
        return;
    }
    let text;
    try {
        text = fs.readFileSync(uri.fsPath, "utf8");
    }
    catch (error) {
        void vscode.window.showWarningMessage(`Oxide: could not read ${relative} (${String(error)}).`);
        return;
    }
    if (text.includes("\u0000")) {
        void vscode.window.showWarningMessage(`Oxide: ${relative} is not a text file.`);
        return;
    }
    add(controller, trimLines({ path: relative, text }));
}
function add(controller, block) {
    const chip = controller.addContext(block);
    const pending = controller.contextCount;
    void vscode.window.setStatusBarMessage(`Oxide: attached ${chip.label}${pending > 1 ? ` (${pending} pending)` : ""} — send a message to include it.`, 5_000);
    return chip;
}
function trimLines(block) {
    const lines = block.text.split("\n");
    if (lines.length <= MAX_CONTEXT_LINES)
        return block;
    const kept = lines.slice(0, MAX_CONTEXT_LINES).join("\n");
    void vscode.window.showWarningMessage(`Oxide: attached the first ${MAX_CONTEXT_LINES} lines of ${block.path}.`);
    return { ...block, endLine: undefined, text: `${kept}\n… (truncated)` };
}
function openTerminal(binary) {
    const editor = vscode.window.activeTextEditor;
    const folder = (editor && vscode.workspace.getWorkspaceFolder(editor.document.uri)) ||
        vscode.workspace.workspaceFolders?.[0];
    const terminal = vscode.window.createTerminal({ name: "Oxide", cwd: folder?.uri.fsPath });
    terminal.show();
    terminal.sendText(/[\s"']/.test(binary) ? `"${binary}"` : binary, true);
}
//# sourceMappingURL=extension.js.map