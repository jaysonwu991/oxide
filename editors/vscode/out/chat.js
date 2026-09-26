"use strict";
// The chat controller: it owns the transcript, the running turn, the queued
// follow-ups and the editor context, and broadcasts view updates to every
// attached webview. All CLI contact goes through here.
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
exports.ChatController = void 0;
const path = __importStar(require("node:path"));
const vscode = __importStar(require("vscode"));
const args_1 = require("./core/args");
const prompt_1 = require("./core/prompt");
const sessions_1 = require("./core/sessions");
const preview_1 = require("./core/preview");
const protocol_1 = require("./core/protocol");
const cli_1 = require("./cli");
class ChatController {
    output;
    transcript;
    views = new Set();
    context = [];
    nextChipId = 1;
    turn = null;
    run = null;
    queue = [];
    continueLast = false;
    activeFolder = null;
    onDidChange = new vscode.EventEmitter();
    constructor(output, contextWindow = 0) {
        this.output = output;
        this.transcript = new protocol_1.Transcript((name, args) => {
            const preview = (0, preview_1.toolDiff)(name, args, (file) => this.readForPreview(file));
            return preview ? preview.diff : null;
        });
        this.transcript.contextWindow = contextWindow;
    }
    dispose() {
        this.turn?.cancel();
        this.onDidChange.dispose();
    }
    // ---------- views ----------
    attach(view) {
        this.views.add(view);
        // The webview asks for state once its script is listening (`ready`), so a
        // repainted panel always restores the whole transcript.
        view.onDidDispose(() => this.views.delete(view));
    }
    broadcast(message) {
        for (const view of this.views)
            void this.push(view, message);
    }
    async push(view, message) {
        try {
            await view.webview.postMessage(message);
        }
        catch {
            this.views.delete(view);
        }
    }
    stateMessage() {
        const folder = this.folder();
        return {
            k: "state",
            ...this.transcript.state({
                queued: this.queue.length,
                context: this.chips(),
                folder: folder ? folder.name : "",
                model: this.modelLabel(),
                binary: this.binary(),
                showThinking: this.setting("showThinking", true),
            }),
        };
    }
    modelLabel() {
        const configured = this.setting("model", "").trim();
        return configured || "config.json";
    }
    // ---------- settings ----------
    setting(key, fallback) {
        return vscode.workspace.getConfiguration("oxide").get(key, fallback);
    }
    binary() {
        return (0, cli_1.resolveBinary)(this.setting("binaryPath", "oxide"), {
            env: process.env,
            platform: process.platform,
            exists: cli_1.isFile,
        });
    }
    trust() {
        return this.setting("projectTrust", "default");
    }
    turnOptions() {
        return {
            model: this.setting("model", "").trim(),
            agent: this.setting("agent", "").trim(),
            reasoning: this.setting("reasoning", "auto"),
            trust: this.trust(),
            tools: (0, args_1.splitList)(this.setting("tools", "")),
            excludeTools: (0, args_1.splitList)(this.setting("excludeTools", "")),
            extra: this.setting("additionalArguments", []),
        };
    }
    // ---------- workspace ----------
    folder() {
        const active = vscode.window.activeTextEditor?.document.uri;
        if (active) {
            const folder = vscode.workspace.getWorkspaceFolder(active);
            if (folder)
                return folder;
        }
        return vscode.workspace.workspaceFolders?.[0];
    }
    cwd() {
        const folder = this.folder();
        if (!folder)
            return null;
        if (this.activeFolder && this.activeFolder !== folder.uri.fsPath) {
            // Sessions are per project, so a conversation does not follow the user
            // across folders.
            const previous = path.basename(this.activeFolder);
            this.transcript.reset();
            this.nextChipId = 1;
            this.context = [];
            this.broadcast(this.stateMessage());
            this.showNotice(`Switched to ${folder.name}: the conversation in ${previous} keeps its own session.`);
        }
        this.activeFolder = folder.uri.fsPath;
        return folder.uri.fsPath;
    }
    /// The workspace-relative path the model sees for a file in this workspace.
    relativeTo(file) {
        const folder = this.folder();
        return folder ? (0, prompt_1.relativePath)(folder.uri.fsPath, file) : file;
    }
    /// A file read for a diff preview: inside the workspace, text, and small.
    readForPreview(file) {
        const folder = this.folder();
        if (!folder)
            return null;
        const resolved = path.isAbsolute(file) ? file : path.join(folder.uri.fsPath, file);
        const root = folder.uri.fsPath.endsWith(path.sep)
            ? folder.uri.fsPath
            : folder.uri.fsPath + path.sep;
        if (!resolved.startsWith(root) && resolved !== folder.uri.fsPath)
            return null;
        return (0, cli_1.readTextFile)(resolved);
    }
    // ---------- context ----------
    addContext(block) {
        const chip = { id: this.nextChipId++, label: (0, prompt_1.contextLabel)(block), block };
        this.context.push(chip);
        this.broadcast({ k: "context", context: this.chips() });
        return { id: chip.id, label: chip.label };
    }
    get contextCount() {
        return this.context.length;
    }
    removeContext(id) {
        this.context = this.context.filter((chip) => chip.id !== id);
        this.broadcast({ k: "context", context: this.chips() });
    }
    clearContext() {
        if (!this.context.length)
            return;
        this.context = [];
        this.broadcast({ k: "context", context: this.chips() });
    }
    chips() {
        return this.context.map((chip) => ({ id: chip.id, label: chip.label }));
    }
    // ---------- turns ----------
    /// Sends a message, starting a turn or queueing a follow-up while one runs.
    async send(text) {
        const message = text.trim();
        if (!message && this.context.length === 0)
            return;
        if (this.turn) {
            this.queue.push(message);
            this.showNotice(`Queued: ${firstLine(message)}`);
            return;
        }
        const cwd = this.cwd();
        if (!cwd) {
            this.showNotice("Open a folder to run Oxide: sessions and context are per project.", "error");
            return;
        }
        // A prompt sent on stdin skips the CLI's own `@file` expansion, so the
        // references are resolved here and become ordinary context blocks.
        const expanded = (0, prompt_1.expandAtReferences)(message, {
            resolve: (reference) => {
                const absolute = path.resolve(cwd, reference);
                return (0, cli_1.isFile)(absolute) ? absolute : null;
            },
            read: (absolute) => (0, cli_1.readTextFile)(absolute),
            label: (absolute) => (0, prompt_1.relativePath)(cwd, absolute),
        });
        const chips = this.context;
        const blocks = chips.map((chip) => chip.block);
        const attachments = blocks
            .filter((block) => (0, prompt_1.isAttachmentPath)(block.path))
            .map((block) => path.resolve(cwd, block.path))
            .concat(expanded.attachments);
        const prompt = (0, prompt_1.buildPrompt)(expanded.message, [
            ...blocks.filter((block) => !(0, prompt_1.isAttachmentPath)(block.path)),
            ...expanded.blocks,
        ]);
        if (!prompt) {
            this.showNotice("The message is empty once its references are attached; add a question next to them.", "warn");
            return;
        }
        const folder = this.folder();
        const args = (0, args_1.buildTurnArgs)({
            ...this.turnOptions(),
            session: this.transcript.sessionId,
            continueLast: this.continueLast && !this.transcript.sessionId,
            attachments,
        });
        this.continueLast = false;
        this.context = [];
        this.broadcast({ k: "context", context: [] });
        // The bubble names what was sent: the pending chips plus whatever `@path`
        // references were resolved out of the message itself.
        this.broadcastItem(this.transcript.pushUser(message, [
            ...chips.map((chip) => ({ id: chip.id, label: chip.label })),
            ...expanded.blocks.map((block) => ({ id: 0, label: (0, prompt_1.contextLabel)(block) })),
        ]));
        const command = this.binary();
        this.output.appendLine(`\n$ ${command} ${args.join(" ")}`);
        if (folder)
            this.output.appendLine(`  cwd ${folder.uri.fsPath}`);
        this.output.appendLine(`  prompt:\n${indent(prompt)}`);
        this.transcript.busy = true;
        this.transcript.status = "Thinking…";
        this.run = { cancelled: false, sawEvent: false, stderr: [], context: chips };
        this.turn = (0, cli_1.startTurn)(command, args, cwd, prompt, {
            onEvent: (event) => this.handleEvent(event),
            onStderr: (line) => {
                this.run?.stderr.push(line.trim());
                this.output.appendLine(`[stderr] ${line}`);
            },
            onExit: (result) => this.handleExit(result),
        });
        this.broadcastStatus();
    }
    /// Sends queued follow-ups one at a time, in order.
    drainQueue() {
        const next = this.queue.shift();
        if (next === undefined)
            return;
        void this.send(next);
    }
    stop() {
        if (!this.turn) {
            this.showNotice("Nothing is running.");
            return;
        }
        if (this.run)
            this.run.cancelled = true;
        this.turn.cancel();
        this.transcript.status = "Stopping…";
        this.broadcastStatus();
    }
    broadcastItem(messages) {
        for (const message of messages)
            this.broadcast(message);
    }
    handleEvent(event) {
        if (!this.run)
            return;
        this.run.sawEvent = true;
        const messages = this.transcript.apply(event);
        this.broadcastItem(messages);
        this.broadcastStatus();
        this.onDidChange.fire();
    }
    handleExit(result) {
        const run = this.run;
        this.turn = null;
        this.run = null;
        this.transcript.busy = false;
        this.transcript.status = "Idle";
        if (run?.cancelled) {
            this.showNotice("Run stopped. The next message continues this session.");
        }
        else if (result.error) {
            for (const chip of run?.context ?? [])
                this.context.push(chip);
            this.broadcast({ k: "context", context: this.chips() });
            this.showNotice(`Could not run ${this.binary()}: ${result.error}. Set "oxide.binaryPath" to the oxide binary, then use "Oxide: Open Terminal" to connect a provider.`, "error");
        }
        else if (result.code !== 0) {
            const detail = (run?.stderr ?? []).filter(Boolean).join(" ").trim();
            const hint = /no api key|not logged in|authenticate/i.test(detail)
                ? ' Run "Oxide: Open Terminal" and use /login there to connect a provider.'
                : "";
            this.showNotice(detail
                ? `${detail}${hint}`
                : `oxide exited with code ${result.code ?? "?"}.${hint}`, "error");
        }
        else if (!run?.sawEvent) {
            this.showNotice("oxide produced no events; see the Oxide output channel.", "warn");
        }
        else {
            this.notify(run);
        }
        this.broadcastStatus();
        this.drainQueue();
    }
    /// A turn that finishes while the chat view is hidden is worth a toast — the
    /// CLI and desktop notify on completion too.
    notify(run) {
        if (!run || this.views.size === 0)
            return;
        if (!this.setting("notifyOnFinish", true))
            return;
        if ([...this.views].some((view) => view.visible))
            return;
        const last = [...this.transcript.items]
            .reverse()
            .find((item) => item.kind === "assistant");
        const body = last ? firstLine(stripMarkdown(last.text)) : "";
        void vscode.window.showInformationMessage(body ? `Oxide: ${truncate(body, 120)}` : "Oxide finished.");
    }
    // ---------- sessions ----------
    newSession() {
        this.transcript.reset();
        this.continueLast = false;
        this.queue = [];
        this.broadcast(this.stateMessage());
        this.showNotice("New session: the next message starts a fresh thread.");
    }
    /// Resumes a session picked from the CLI's own listing, so the picker and the
    /// terminal agree on what exists.
    async resumeSession() {
        const cwd = this.cwd();
        if (!cwd) {
            this.showNotice("Open a folder first.", "error");
            return;
        }
        const result = await (0, cli_1.runCapture)(this.binary(), (0, args_1.sessionsListArgs)(), cwd);
        if (result.error || result.code !== 0) {
            const detail = result.error || firstLine(result.stderr) || `exit ${result.code}`;
            this.showNotice(`Could not list sessions: ${detail}`, "error");
            return;
        }
        const sessions = (0, sessions_1.parseSessionList)(result.stdout);
        const items = [
            {
                label: "$(add) New session",
                detail: "Start a fresh thread",
                startNew: true,
            },
            {
                label: "$(history) Continue most recent session",
                detail: "Pick up the newest session for this project",
                sessionId: "continue",
            },
            ...sessions.map((session) => ({
                label: session.label || session.id,
                description: `${session.age} · ${session.messages} message${session.messages === 1 ? "" : "s"}`,
                detail: session.id,
                sessionId: session.id,
            })),
        ];
        const picked = await vscode.window.showQuickPick(items, {
            title: "Oxide: resume a session",
            placeHolder: sessions.length
                ? `${sessions.length} session${sessions.length === 1 ? "" : "s"} in this project`
                : "No sessions for this project yet",
        });
        if (!picked)
            return;
        if (picked.startNew) {
            this.newSession();
            return;
        }
        if (picked.sessionId === "continue") {
            this.continueLast = true;
            this.newSession();
            this.showNotice("The next message continues the most recent session.");
            return;
        }
        this.transcript.reset();
        this.transcript.sessionId = picked.sessionId ?? null;
        this.queue = [];
        this.broadcast(this.stateMessage());
        this.showNotice(`Resuming ${picked.sessionId} — the thread continues from its stored context.`);
    }
    continueSession() {
        if (!this.cwd()) {
            this.showNotice("Open a folder first.", "error");
            return;
        }
        this.continueLast = true;
        this.newSession();
        this.showNotice("The next message continues the most recent session.");
    }
    // ---------- settings commands ----------
    async setModel() {
        const config = vscode.workspace.getConfiguration("oxide");
        const current = config.get("model", "");
        const value = await vscode.window.showInputBox({
            title: "Oxide: model",
            prompt: "Model passed with --model. Leave empty to use the model from the Oxide config.json.",
            value: current,
            placeHolder: "e.g. glm-4.6, claude-sonnet-4-5, deepseek-chat",
        });
        if (value === undefined)
            return;
        // `oxide.model` overrides the shared config.json for this workspace only.
        await config.update("model", value.trim(), vscode.ConfigurationTarget.Workspace);
    }
    async setReasoning() {
        const levels = ["auto", "off", "low", "medium", "high"];
        const picked = await vscode.window.showQuickPick(levels, {
            title: "Oxide: reasoning effort",
            placeHolder: "Passed with --reasoning",
        });
        if (!picked)
            return;
        await vscode.workspace
            .getConfiguration("oxide")
            .update("reasoning", picked, vscode.ConfigurationTarget.Workspace);
    }
    async setProjectTrust() {
        const options = [
            {
                label: "default",
                detail: "Use the decision saved in trust.json (or defaultProjectTrust)",
            },
            { label: "always", detail: "Pass --approve: load this workspace's .oxide resources" },
            { label: "never", detail: "Pass --no-approve: ignore this workspace's own resources" },
        ];
        const picked = await vscode.window.showQuickPick(options, {
            title: "Oxide: project trust",
            placeHolder: 'Runs are non-interactive, so nothing is ever prompted; "default" follows trust.json',
        });
        if (!picked)
            return;
        await vscode.workspace
            .getConfiguration("oxide")
            .update("projectTrust", picked.label, vscode.ConfigurationTarget.Workspace);
    }
    // ---------- notices ----------
    /// A line in the transcript. Used instead of a toast for anything the user
    /// should be able to read back later.
    notice(text, tone = "info") {
        this.broadcastItem(this.transcript.notice(text, tone));
    }
    showNotice(text, tone = "info") {
        this.notice(text, tone);
        this.onDidChange.fire();
    }
    broadcastStatus() {
        this.broadcast(this.transcript.statusMessage(this.queue.length));
    }
    get running() {
        return this.turn !== null;
    }
}
exports.ChatController = ChatController;
function indent(text) {
    return text
        .split("\n")
        .map((line) => `  | ${line}`)
        .join("\n");
}
function firstLine(text) {
    const line = text.split("\n").find((entry) => entry.trim());
    return line ? line.trim() : "";
}
function truncate(text, max) {
    return text.length > max ? `${text.slice(0, max - 1)}…` : text;
}
/// The first non-empty line of a reply, with the Markdown markers the TUI also
/// strips for its notification body.
function stripMarkdown(text) {
    return text
        .replace(/^#{1,6}\s+/gm, "")
        .replace(/[*_`>]/g, "")
        .trim();
}
//# sourceMappingURL=chat.js.map