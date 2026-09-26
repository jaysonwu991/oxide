// The chat controller: it owns the transcript, the running turn, the queued
// follow-ups and the editor context, and broadcasts view updates to every
// attached webview. All CLI contact goes through here.

import * as path from "node:path";
import * as vscode from "vscode";

import {
  buildTurnArgs,
  sessionsListArgs,
  splitList,
  type TrustSetting,
  type TurnOptions,
} from "./core/args";
import {
  buildPrompt,
  contextLabel,
  expandAtReferences,
  isAttachmentPath,
  relativePath,
  type ContextBlock,
} from "./core/prompt";
import { parseSessionList } from "./core/sessions";
import { toolDiff } from "./core/preview";
import {
  Transcript,
  type AssistantItem,
  type ContextChip,
  type ViewMessage,
  type WireEvent,
} from "./core/protocol";
import {
  isFile,
  readTextFile,
  resolveBinary,
  runCapture,
  startTurn,
  type Turn,
} from "./cli";

interface Chip extends ContextChip {
  block: ContextBlock;
}

interface RunState {
  cancelled: boolean;
  sawEvent: boolean;
  stderr: string[];
  context: Chip[];
}

/// A follow-up queued while a turn runs. The context chips are snapshotted at
/// queue time so later edits to the composer's chips cannot change what the
/// queued message sends.
interface QueuedMessage {
  text: string;
  context: Chip[];
}

export class ChatController {
  private readonly transcript: Transcript;
  private readonly views = new Set<vscode.WebviewView>();
  private context: Chip[] = [];
  private nextChipId = 1;
  private turn: Turn | null = null;
  private run: RunState | null = null;
  private queue: QueuedMessage[] = [];
  private continueLast = false;
  private activeFolder: string | null = null;

  readonly onDidChange = new vscode.EventEmitter<void>();

  constructor(
    private readonly output: vscode.OutputChannel,
    contextWindow = 0,
  ) {
    this.transcript = new Transcript((name, args) => {
      const preview = toolDiff(name, args, (file) => this.readForPreview(file));
      return preview ? preview.diff : null;
    });
    this.transcript.contextWindow = contextWindow;
  }

  dispose(): void {
    this.turn?.cancel();
    this.onDidChange.dispose();
  }

  // ---------- views ----------

  attach(view: vscode.WebviewView): void {
    this.views.add(view);
    // The webview asks for state once its script is listening (`ready`), so a
    // repainted panel always restores the whole transcript.
    view.onDidDispose(() => this.views.delete(view));
  }

  private broadcast(message: ViewMessage): void {
    for (const view of this.views) void this.push(view, message);
  }

  private async push(view: vscode.WebviewView, message: ViewMessage): Promise<void> {
    try {
      await view.webview.postMessage(message);
    } catch {
      this.views.delete(view);
    }
  }

  stateMessage(): ViewMessage {
    const folder = this.folder();
    return {
      k: "state",
      ...this.transcript.state({
        queued: this.queue.length,
        context: this.chips(),
        folder: folder ? folder.name : "",
        model: this.modelLabel(),
        binary: this.binary(),
        showThinking: this.setting<boolean>("showThinking", true),
      }),
    };
  }

  private modelLabel(): string {
    const configured = this.setting<string>("model", "").trim();
    return configured || "config.json";
  }

  // ---------- settings ----------

  private setting<T>(key: string, fallback: T): T {
    return vscode.workspace.getConfiguration("oxide").get<T>(key, fallback);
  }

  private binary(): string {
    return resolveBinary(this.setting<string>("binaryPath", "oxide"), {
      env: process.env,
      platform: process.platform,
      exists: isFile,
    });
  }

  private trust(): TrustSetting {
    return this.setting<TrustSetting>("projectTrust", "default");
  }

  private turnOptions(): TurnOptions {
    return {
      model: this.setting<string>("model", "").trim(),
      agent: this.setting<string>("agent", "").trim(),
      reasoning: this.setting<string>("reasoning", "auto"),
      trust: this.trust(),
      tools: splitList(this.setting<string>("tools", "")),
      excludeTools: splitList(this.setting<string>("excludeTools", "")),
      extra: this.setting<string[]>("additionalArguments", []),
    };
  }

  // ---------- workspace ----------

  private folder(): vscode.WorkspaceFolder | undefined {
    const active = vscode.window.activeTextEditor?.document.uri;
    if (active) {
      const folder = vscode.workspace.getWorkspaceFolder(active);
      if (folder) return folder;
    }
    return vscode.workspace.workspaceFolders?.[0];
  }

  private cwd(): string | null {
    const folder = this.folder();
    if (!folder) return null;
    if (this.activeFolder && this.activeFolder !== folder.uri.fsPath) {
      // Sessions are per project, so a conversation does not follow the user
      // across folders.
      const previous = path.basename(this.activeFolder);
      this.transcript.reset();
      this.nextChipId = 1;
      this.context = [];
      this.broadcast(this.stateMessage());
      this.showNotice(
        `Switched to ${folder.name}: the conversation in ${previous} keeps its own session.`,
      );
    }
    this.activeFolder = folder.uri.fsPath;
    return folder.uri.fsPath;
  }

  /// The workspace-relative path the model sees for a file in this workspace.
  relativeTo(file: string): string {
    const folder = this.folder();
    return folder ? relativePath(folder.uri.fsPath, file) : file;
  }

  /// A file read for a diff preview: inside the workspace, text, and small.
  private readForPreview(file: string): string | null {
    const folder = this.folder();
    if (!folder) return null;
    const resolved = path.isAbsolute(file) ? file : path.join(folder.uri.fsPath, file);
    const root = folder.uri.fsPath.endsWith(path.sep)
      ? folder.uri.fsPath
      : folder.uri.fsPath + path.sep;
    if (!resolved.startsWith(root) && resolved !== folder.uri.fsPath) return null;
    return readTextFile(resolved);
  }

  // ---------- context ----------

  addContext(block: ContextBlock): ContextChip {
    const chip: Chip = { id: this.nextChipId++, label: contextLabel(block), block };
    this.context.push(chip);
    this.broadcast({ k: "context", context: this.chips() });
    return { id: chip.id, label: chip.label };
  }

  get contextCount(): number {
    return this.context.length;
  }

  removeContext(id: number): void {
    this.context = this.context.filter((chip) => chip.id !== id);
    this.broadcast({ k: "context", context: this.chips() });
  }

  clearContext(): void {
    if (!this.context.length) return;
    this.context = [];
    this.broadcast({ k: "context", context: this.chips() });
  }

  private chips(): ContextChip[] {
    return this.context.map((chip) => ({ id: chip.id, label: chip.label }));
  }

  // ---------- turns ----------

  /// Sends a message, starting a turn or queueing a follow-up while one runs.
  async send(text: string): Promise<void> {
    const message = text.trim();
    if (!message && this.context.length === 0) return;
    if (this.turn) {
      this.queue.push({ text: message, context: this.context });
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
    const expanded = expandAtReferences(message, {
      resolve: (reference) => {
        const absolute = path.resolve(cwd, reference);
        return isFile(absolute) ? absolute : null;
      },
      read: (absolute) => readTextFile(absolute),
      label: (absolute) => relativePath(cwd, absolute),
    });

    const chips = this.context;
    const blocks = chips.map((chip) => chip.block);
    const attachments = blocks
      .filter((block) => isAttachmentPath(block.path))
      .map((block) => path.resolve(cwd, block.path))
      .concat(expanded.attachments);
    const prompt = buildPrompt(expanded.message, [
      ...blocks.filter((block) => !isAttachmentPath(block.path)),
      ...expanded.blocks,
    ]);
    if (!prompt) {
      this.showNotice(
        "The message is empty once its references are attached; add a question next to them.",
        "warn",
      );
      return;
    }

    const folder = this.folder();
    const args = buildTurnArgs({
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
    this.broadcastItem(
      this.transcript.pushUser(message, [
        ...chips.map((chip) => ({ id: chip.id, label: chip.label })),
        ...expanded.blocks.map((block) => ({ id: 0, label: contextLabel(block) })),
      ]),
    );

    const command = this.binary();
    this.output.appendLine(`\n$ ${command} ${args.join(" ")}`);
    if (folder) this.output.appendLine(`  cwd ${folder.uri.fsPath}`);
    this.output.appendLine(`  prompt:\n${indent(prompt)}`);

    this.transcript.busy = true;
    this.transcript.status = "Thinking…";
    this.run = { cancelled: false, sawEvent: false, stderr: [], context: chips };
    this.turn = startTurn(command, args, cwd, prompt, {
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
  private drainQueue(): void {
    const next = this.queue.shift();
    if (next === undefined) return;
    // Restore the chips the message was queued with, not whatever the composer
    // holds now.
    this.context = next.context;
    this.broadcast({ k: "context", context: this.chips() });
    void this.send(next.text);
  }

  stop(): void {
    if (!this.turn) {
      this.showNotice("Nothing is running.");
      return;
    }
    if (this.run) this.run.cancelled = true;
    this.turn.cancel();
    this.transcript.status = "Stopping…";
    this.broadcastStatus();
  }

  private broadcastItem(messages: ViewMessage[]): void {
    for (const message of messages) this.broadcast(message);
  }

  private handleEvent(event: WireEvent): void {
    if (!this.run) return;
    this.run.sawEvent = true;
    const messages = this.transcript.apply(event);
    this.broadcastItem(messages);
    this.broadcastStatus();
    this.onDidChange.fire();
  }

  private handleExit(result: { code: number | null; signal: string | null; error?: string }): void {
    const run = this.run;
    this.turn = null;
    this.run = null;
    this.transcript.busy = false;
    this.transcript.status = "Idle";

    if (run?.cancelled) {
      this.showNotice("Run stopped. The next message continues this session.");
    } else if (result.error) {
      for (const chip of run?.context ?? []) this.context.push(chip);
      this.broadcast({ k: "context", context: this.chips() });
      this.showNotice(
        `Could not run ${this.binary()}: ${result.error}. Set "oxide.binaryPath" to the oxide binary, then use "Oxide: Open Terminal" to connect a provider.`,
        "error",
      );
    } else if (result.code !== 0) {
      const detail = (run?.stderr ?? []).filter(Boolean).join(" ").trim();
      const hint = /no api key|not logged in|authenticate/i.test(detail)
        ? ' Run "Oxide: Open Terminal" and use /login there to connect a provider.'
        : "";
      this.showNotice(
        detail
          ? `${detail}${hint}`
          : `oxide exited with code ${result.code ?? "?"}.${hint}`,
        "error",
      );
    } else if (!run?.sawEvent) {
      this.showNotice("oxide produced no events; see the Oxide output channel.", "warn");
    } else {
      this.notify(run);
    }

    this.broadcastStatus();
    // The view's status-bar spinner is driven by this event, and `handleExit`
    // runs after the last stream event, so refresh it here too.
    this.onDidChange.fire();
    this.drainQueue();
  }

  /// A turn that finishes while the chat view is hidden is worth a toast — the
  /// CLI and desktop notify on completion too.
  private notify(run: RunState | null): void {
    if (!run || this.views.size === 0) return;
    if (!this.setting<boolean>("notifyOnFinish", true)) return;
    if ([...this.views].some((view) => view.visible)) return;
    const last = [...this.transcript.items]
      .reverse()
      .find((item): item is AssistantItem => item.kind === "assistant");
    const body = last ? firstLine(stripMarkdown(last.text)) : "";
    void vscode.window.showInformationMessage(
      body ? `Oxide: ${truncate(body, 120)}` : "Oxide finished.",
    );
  }

  // ---------- sessions ----------

  newSession(): void {
    if (this.turn) {
      // Resetting now would apply the running process's later events to the new
      // thread and drop the session id it is about to report.
      this.showNotice("A turn is running; stop it before starting a new session.", "warn");
      return;
    }
    this.transcript.reset();
    this.continueLast = false;
    this.queue = [];
    this.broadcast(this.stateMessage());
    this.showNotice("New session: the next message starts a fresh thread.");
  }

  /// Resumes a session picked from the CLI's own listing, so the picker and the
  /// terminal agree on what exists.
  async resumeSession(): Promise<void> {
    if (this.turn) {
      this.showNotice("A turn is running; stop it before switching sessions.", "warn");
      return;
    }
    const cwd = this.cwd();
    if (!cwd) {
      this.showNotice("Open a folder first.", "error");
      return;
    }
    const result = await runCapture(this.binary(), sessionsListArgs(), cwd);
    if (result.error || result.code !== 0) {
      const detail = result.error || firstLine(result.stderr) || `exit ${result.code}`;
      this.showNotice(`Could not list sessions: ${detail}`, "error");
      return;
    }
    const sessions = parseSessionList(result.stdout);
    type Pick = vscode.QuickPickItem & { sessionId?: string; startNew?: boolean };
    const items: Pick[] = [
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
    if (!picked) return;
    if (picked.startNew) {
      this.newSession();
      return;
    }
    if (picked.sessionId === "continue") {
      this.newSession();
      // `newSession` clears the flag, so set it afterwards.
      this.continueLast = true;
      this.showNotice("The next message continues the most recent session.");
      return;
    }
    this.transcript.reset();
    this.transcript.sessionId = picked.sessionId ?? null;
    this.queue = [];
    this.broadcast(this.stateMessage());
    this.showNotice(
      `Resuming ${picked.sessionId} — the thread continues from its stored context.`,
    );
  }

  continueSession(): void {
    if (this.turn) {
      this.showNotice("A turn is running; stop it before switching sessions.", "warn");
      return;
    }
    if (!this.cwd()) {
      this.showNotice("Open a folder first.", "error");
      return;
    }
    this.newSession();
    // `newSession` clears the flag, so set it afterwards.
    this.continueLast = true;
    this.showNotice("The next message continues the most recent session.");
  }

  // ---------- settings commands ----------

  async setModel(): Promise<void> {
    const config = vscode.workspace.getConfiguration("oxide");
    const current = config.get<string>("model", "");
    const value = await vscode.window.showInputBox({
      title: "Oxide: model",
      prompt: "Model passed with --model. Leave empty to use the model from the Oxide config.json.",
      value: current,
      placeHolder: "e.g. glm-4.6, claude-sonnet-4-5, deepseek-chat",
    });
    if (value === undefined) return;
    // `oxide.model` overrides the shared config.json for this workspace only.
    await config.update("model", value.trim(), vscode.ConfigurationTarget.Workspace);
  }

  async setReasoning(): Promise<void> {
    const levels = ["auto", "off", "low", "medium", "high"];
    const picked = await vscode.window.showQuickPick(levels, {
      title: "Oxide: reasoning effort",
      placeHolder: "Passed with --reasoning",
    });
    if (!picked) return;
    await vscode.workspace
      .getConfiguration("oxide")
      .update("reasoning", picked, vscode.ConfigurationTarget.Workspace);
  }

  async setProjectTrust(): Promise<void> {
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
    if (!picked) return;
    await vscode.workspace
      .getConfiguration("oxide")
      .update("projectTrust", picked.label, vscode.ConfigurationTarget.Workspace);
  }

  // ---------- notices ----------

  /// A line in the transcript. Used instead of a toast for anything the user
  /// should be able to read back later.
  notice(text: string, tone: "info" | "warn" | "error" = "info"): void {
    this.broadcastItem(this.transcript.notice(text, tone));
  }

  private showNotice(text: string, tone: "info" | "warn" | "error" = "info"): void {
    this.notice(text, tone);
    this.onDidChange.fire();
  }

  private broadcastStatus(): void {
    this.broadcast(this.transcript.statusMessage(this.queue.length));
  }

  get running(): boolean {
    return this.turn !== null;
  }
}

function indent(text: string): string {
  return text
    .split("\n")
    .map((line) => `  | ${line}`)
    .join("\n");
}

function firstLine(text: string): string {
  const line = text.split("\n").find((entry) => entry.trim());
  return line ? line.trim() : "";
}

function truncate(text: string, max: number): string {
  return text.length > max ? `${text.slice(0, max - 1)}…` : text;
}

/// The first non-empty line of a reply, with the Markdown markers the TUI also
/// strips for its notification body.
function stripMarkdown(text: string): string {
  return text
    .replace(/^#{1,6}\s+/gm, "")
    .replace(/[*_`>]/g, "")
    .trim();
}
