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
import { modelsForProvider } from "./core/config";
import { footerState, nextReasoning, REASONING_LEVELS, type FooterState } from "./core/footer";
import { projectInfo, type ProjectDeps, type ProjectInfo } from "./core/project";
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
  /// The shared on-disk state the footer reports. Re-read when something the
  /// user or the agent could have changed it happens — not per stream event,
  /// which would stat a dozen files for every token.
  private project: ProjectInfo | null = null;

  readonly onDidChange = new vscode.EventEmitter<void>();

  constructor(
    private readonly output: vscode.OutputChannel,
    private readonly deps: ProjectDeps,
  ) {
    this.transcript = new Transcript((name, args) => {
      const preview = toolDiff(name, args, (file) => this.readForPreview(file));
      return preview ? preview.diff : null;
    });
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

  /// The type of the chat view the user is looking at, if any. The chat has a
  /// pane in the activity bar and one in the secondary side bar, so "open the
  /// chat" should bring forward the one already on screen.
  visibleViewType(): string | null {
    for (const view of this.views) {
      if (view.visible) return view.viewType;
    }
    return null;
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
    this.refreshProject();
    return {
      k: "state",
      ...this.transcript.state({
        queued: this.queue.length,
        context: this.chips(),
        folder: folder ? folder.name : "",
        model: this.modelLabel(),
        binary: this.binary(),
        showThinking: this.setting<boolean>("showThinking", true),
        footer: this.footer(),
      }),
    };
  }

  // ---------- footer ----------

  /// Re-reads the shared configuration the footer reports. Called when the
  /// panel is painted, when a turn starts or ends (the agent may have created a
  /// branch or written `.oxide/` files) and when a setting changes.
  private refreshProject(): void {
    const folder = this.folder();
    if (!folder) {
      this.project = null;
      return;
    }
    this.project = projectInfo(folder.uri.fsPath, this.trust(), this.deps);
  }

  /// A setting or the workspace changed: the chips are stale until the shared
  /// configuration is re-read, which `stateMessage` does.
  configurationChanged(): void {
    this.broadcast(this.stateMessage());
  }

  private footer(): FooterState {
    const project = this.project;
    const agent = this.setting<string>("agent", "").trim();
    return footerState({
      model: this.setting<string>("model", "").trim() || project?.model || "",
      provider: project?.provider ?? "",
      contextWindow: project?.contextWindow ?? 0,
      reasoning: this.setting<string>("reasoning", "auto"),
      agent,
      agentCount: project?.agents.length ?? 0,
      access: project?.access ?? "untrusted",
      trustSetting: this.trust(),
      defaultTrust: project?.defaultTrust ?? "ask",
      savedTrust: project?.savedTrust,
      sessionId: this.transcript.sessionId,
      branch: project?.branch ?? "",
      autoCompact: project?.autoCompact ?? true,
      usage: this.transcript.usage,
    });
  }

  private modelLabel(): string {
    const configured = this.setting<string>("model", "").trim();
    return configured || this.project?.model || "config.json";
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

  /// Writes a setting where a workspace is available, and to the user's own
  /// settings when there is none (a `.vscode/settings.json` needs a folder).
  private async updateSetting(key: string, value: string): Promise<void> {
    const target = vscode.workspace.workspaceFolders?.length
      ? vscode.ConfigurationTarget.Workspace
      : vscode.ConfigurationTarget.Global;
    await vscode.workspace.getConfiguration("oxide").update(key, value, target);
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
    // The agent may have written `.oxide/` files, committed, or the user may
    // have changed a setting since the panel was painted.
    this.refreshProject();

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
    for (const message of messages) {
      // A usage message is the one the transcript cannot complete on its own:
      // the usage line is composed by the controller, which knows the context
      // window and the settings the CLI resolved.
      this.broadcast(message.k === "usage" ? { ...message, footer: this.footer() } : message);
    }
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

    // The run may have created a branch, committed, or written `.oxide/` files.
    this.refreshProject();
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

  /// A click on a footer chip. The chips are the same actions the commands
  /// expose, so both entry points share one implementation.
  async control(id: string): Promise<void> {
    switch (id) {
      case "model":
        return this.setModel();
      case "reasoning":
        return this.cycleReasoning();
      case "agent":
        return this.setAgent();
      case "access":
        return this.setProjectTrust();
      case "session":
        return this.resumeSession();
      default:
        return;
    }
  }

  /// The model picker. The models a provider has been used with are remembered
  /// in `config.json`, so they are offered by name instead of asking for the id
  /// to be typed from memory — only the active provider's, because the id is
  /// sent to whichever provider the CLI has active and another provider's model
  /// would run against the wrong endpoint.
  async setModel(): Promise<void> {
    this.refreshProject();
    const current = this.setting<string>("model", "").trim();
    const configured = this.project?.model ?? "";
    const provider = this.project?.provider ?? "";
    type Pick = vscode.QuickPickItem & { model?: string; other?: boolean };
    const items: Pick[] = [
      {
        label: configured || "config.json",
        description: current ? "Oxide config" : "in use",
        detail: "Use the model stored in the Oxide config",
        model: "",
      },
      ...modelsForProvider(this.project, provider).map((remembered) => ({
        label: remembered.model,
        description: remembered.model === current ? "in use" : provider,
        detail: `Last used with ${remembered.provider}`,
        model: remembered.model,
      })),
      { label: "$(edit) Other model…", detail: "Type a model id to pass to --model", other: true },
    ];
    const picked = await vscode.window.showQuickPick(items, {
      title: "Oxide: model",
      placeHolder: `A turn passes --model; empty follows the Oxide config (${configured || "none"})`,
    });
    if (!picked) return;
    if (!picked.other) {
      await this.updateSetting("model", picked.model ?? "");
      return;
    }
    const value = await vscode.window.showInputBox({
      title: "Oxide: model",
      prompt: "Model passed with --model. Leave empty to use the model from the Oxide config.json.",
      value: current,
      placeHolder: "e.g. glm-4.6, claude-sonnet-4-5, deepseek-chat",
    });
    if (value === undefined) return;
    await this.updateSetting("model", value.trim());
  }

  /// Cycles the reasoning level the way the terminal's Shift+Tab and the
  /// desktop composer chip do.
  async cycleReasoning(): Promise<void> {
    const next = nextReasoning(this.setting<string>("reasoning", "auto"));
    await this.updateSetting("reasoning", next);
    this.showNotice(next === "auto" ? "Reasoning: auto (provider native)" : `Reasoning: ${next}`);
  }

  /// Picks the agent a chat runs with from the ones discovered on disk, which
  /// is the same set `--agent` resolves a name against.
  async setAgent(): Promise<void> {
    this.refreshProject();
    const current = this.setting<string>("agent", "").trim();
    const agents = this.project?.agents ?? [];
    type Pick = vscode.QuickPickItem & { agent?: string; other?: boolean };
    const items: Pick[] = [
      {
        label: "No agent",
        description: current ? "Oxide default" : "in use",
        detail: "Run the main agent, without a subagent prompt",
        agent: "",
      },
      ...agents.map((agent) => ({
        label: agent.name,
        description: agent.name === current ? "in use" : "",
        detail: agent.description,
        agent: agent.name,
      })),
      { label: "$(edit) Other agent…", detail: "Type an agent name", other: true },
    ];
    const picked = await vscode.window.showQuickPick(items, {
      title: "Oxide: agent",
      placeHolder: agents.length
        ? `${agents.length} agent${agents.length === 1 ? "" : "s"} discovered for this project and globally`
        : "No agents were found; type a name to pass to --agent",
    });
    if (!picked) return;
    if (!picked.other) {
      await this.updateSetting("agent", picked.agent ?? "");
      return;
    }
    const value = await vscode.window.showInputBox({
      title: "Oxide: agent",
      prompt: "Agent passed with --agent. Leave empty to run the main agent.",
      value: current,
      placeHolder: "e.g. planner, rust-reviewer",
    });
    if (value === undefined) return;
    await this.updateSetting("agent", value.trim());
  }

  async setReasoning(): Promise<void> {
    const levels = [...REASONING_LEVELS];
    const picked = await vscode.window.showQuickPick(levels, {
      title: "Oxide: reasoning effort",
      placeHolder: "Passed with --reasoning",
    });
    if (!picked) return;
    await this.updateSetting("reasoning", picked);
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
    await this.updateSetting("projectTrust", picked.label);
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
    this.broadcast(this.transcript.statusMessage(this.queue.length, this.footer()));
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
