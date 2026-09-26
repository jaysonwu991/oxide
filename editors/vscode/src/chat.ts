// The chat controller: it owns the transcript, the running turn, the queued
// follow-ups and the editor context, and broadcasts view updates to every
// attached webview. All CLI contact goes through here.

import * as fs from "node:fs";
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
  attachmentFileName,
  attachmentId,
  attachmentKind,
  attachmentMimeForPath,
  attachmentRejection,
  decodeDataUrl,
  formatBytes,
  MAX_ATTACHMENTS,
  type AttachmentKind,
} from "./core/attachments";
import { AttachmentStore, previewForDataUrl, previewForFile } from "./attachments";
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
  type AttachmentChip,
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

/// A whole-file context block is inlined into the prompt, so anything larger
/// than this is trimmed rather than shipped to the model in full.
const MAX_CONTEXT_LINES = 2_000;

interface Chip extends ContextChip {
  block: ContextBlock;
}

/// An image or PDF the composer is holding. A pasted blob has no path of its
/// own, so it is written to a temporary file the CLI can read.
interface Attachment {
  /// The chip id, shared with the context chips so one removal message
  /// addresses either list.
  id: number;
  /// The name shown on the chip.
  label: string;
  /// The absolute path passed to `--image`.
  path: string;
  kind: AttachmentKind;
  /// A content address, so the same screenshot pasted twice stays one chip.
  key: string;
  /// A data URL for the chip's thumbnail, when the picture is small enough.
  preview: string | null;
  /// The size and origin, for the chip's tooltip.
  detail: string;
}

interface RunState {
  cancelled: boolean;
  sawEvent: boolean;
  stderr: string[];
  context: Chip[];
  attachments: Attachment[];
}

/// A follow-up queued while a turn runs. The context chips and attachments are
/// snapshotted at queue time so later edits to the composer cannot change what
/// the queued message sends.
interface QueuedMessage {
  text: string;
  context: Chip[];
  attachments: Attachment[];
}

export class ChatController {
  private readonly transcript: Transcript;
  private readonly views = new Set<vscode.WebviewView>();
  private readonly store = new AttachmentStore();
  private context: Chip[] = [];
  private attachments: Attachment[] = [];
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
    this.store.dispose();
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
        attachments: this.attachmentChips(),
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

  // ---------- context and attachments ----------

  /// A file's text or an editor selection the message carries.
  addContext(block: ContextBlock): ContextChip {
    const { block: trimmed, cut } = trimLines(block);
    if (cut) {
      this.showNotice(`Attached the first ${MAX_CONTEXT_LINES} lines of ${trimmed.path}.`, "warn");
    }
    return this.pushContext(trimmed);
  }

  /// A file the user attached — from the explorer, a drop or the picker. An
  /// image or PDF travels as media, a text file is inlined as context, and
  /// anything else is refused rather than shipped as mojibake.
  addFile(file: string): ContextChip | null {
    const mime = attachmentMimeForPath(file);
    if (mime) return this.addAttachmentFile(file, mime);
    const label = this.relativeTo(file);
    const text = readTextFile(file);
    if (text === null || text.includes("\u0000")) {
      this.showNotice(`${label} is not a text file, an image or a PDF.`, "warn");
      return null;
    }
    return this.addContext({ path: label, text });
  }

  /// A pasted or dropped blob. The bytes are written to a temporary file
  /// because the CLI takes attachment paths (`--image`), not data URLs.
  addAttachment(dataUrl: string, name = ""): ContextChip | null {
    const decoded = decodeDataUrl(dataUrl);
    if (!decoded) {
      this.showNotice("Only images and PDFs can be attached.", "warn");
      return null;
    }
    const key = attachmentId(decoded.bytes);
    const label = attachmentFileName(name, decoded.mime);
    // Check the cap and the duplicate before the blob is written, so a
    // rejected paste never leaves a temp file behind.
    if (!this.canAddAttachment(key, label)) return null;
    const written = this.store.write(name, dataUrl);
    if (!written) {
      this.showNotice(`Could not write ${name || "the attachment"} to a file.`, "error");
      return null;
    }
    return this.pushAttachment({
      key,
      label,
      path: written.path,
      kind: decoded.kind,
      // Only an image gets a thumbnail: a PDF would echo its whole data URL
      // back to the view for nothing.
      preview: decoded.kind === "image" ? previewForDataUrl(dataUrl) : null,
      detail: `${written.detail} · pasted`,
    });
  }

  /// An image or PDF that is already on disk, addressed by an absolute path
  /// passed straight to `--image`.
  private addAttachmentFile(file: string, mime: string): ContextChip | null {
    const kind = attachmentKind(mime);
    if (!kind) return null;
    return this.pushAttachment({
      key: `file:${file}`,
      label: path.basename(file),
      path: file,
      kind,
      preview: kind === "image" ? previewForFile(file, mime) : null,
      detail: `${fileDetail(file)} · ${this.relativeTo(file)}`,
    });
  }

  /// The file picker behind the composer's attach button. An image or PDF
  /// becomes media and anything else is inlined as context, so a picked file
  /// reads the same as one attached from the editor.
  async pickFiles(): Promise<void> {
    const picked = await vscode.window.showOpenDialog({
      canSelectMany: true,
      openLabel: "Attach",
      title: "Oxide: attach files",
    });
    if (!picked?.length) return;
    for (const uri of picked) {
      if (uri.scheme === "file") this.addFile(uri.fsPath);
    }
  }

  /// Removes one pending chip, whichever list it is in.
  removeChip(id: number): void {
    const before = this.context.length + this.attachments.length;
    this.context = this.context.filter((chip) => chip.id !== id);
    this.attachments = this.attachments.filter((chip) => chip.id !== id);
    if (this.context.length + this.attachments.length !== before) this.broadcastChips();
  }

  /// A message from the webview that is worth showing in the transcript: a
  /// paste the view could not read, for instance.
  warn(text: string): void {
    if (text) this.notice(text, "warn");
  }

  clearChips(): void {
    if (!this.context.length && !this.attachments.length) return;
    this.context = [];
    this.attachments = [];
    this.broadcastChips();
  }

  get contextCount(): number {
    return this.context.length + this.attachments.length;
  }

  private pushContext(block: ContextBlock): ContextChip {
    const chip: Chip = { id: this.nextChipId++, label: contextLabel(block), block };
    this.context.push(chip);
    this.broadcastChips();
    return { id: chip.id, label: chip.label };
  }

  /// The cap and dedup guard both attachment paths share, checked before a
  /// pasted blob is written so a rejected paste leaves no temp file behind.
  private canAddAttachment(key: string, label: string): boolean {
    const rejection = attachmentRejection(this.attachments, key);
    if (rejection === "cap") {
      this.showNotice(`At most ${MAX_ATTACHMENTS} attachments per message.`, "warn");
      return false;
    }
    if (rejection === "duplicate") {
      this.showNotice(`${label} is already attached.`);
      return false;
    }
    return true;
  }

  private pushAttachment(attachment: Omit<Attachment, "id">): ContextChip | null {
    if (!this.canAddAttachment(attachment.key, attachment.label)) return null;
    const chip: Attachment = { id: this.nextChipId++, ...attachment };
    this.attachments.push(chip);
    this.broadcastChips();
    return { id: chip.id, label: chip.label };
  }

  /// Every list the composer paints changes together: they share one id space,
  /// so the view always receives both and can tell a removal where to land.
  private broadcastChips(): void {
    this.broadcast({ k: "context", context: this.chips(), attachments: this.attachmentChips() });
  }

  private chips(): ContextChip[] {
    return this.context.map((chip) => ({ id: chip.id, label: chip.label }));
  }

  private attachmentChips(): AttachmentChip[] {
    return this.attachments.map((chip) => ({
      id: chip.id,
      label: chip.label,
      kind: chip.kind,
      preview: chip.preview,
      detail: chip.detail,
    }));
  }

  // ---------- turns ----------

  /// Sends a message, starting a turn or queueing a follow-up while one runs.
  async send(text: string): Promise<void> {
    const message = text.trim();
    if (!message && !this.contextCount) return;
    if (this.turn) {
      // Snapshot the chips: the composer stays editable while the turn runs,
      // so a later chip must not join a message already queued.
      this.queue.push({
        text: message,
        context: [...this.context],
        attachments: [...this.attachments],
      });
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

    // Snapshot the composer: the running turn (or a failure restoring it)
    // owns these arrays, so chips added afterwards cannot leak into it.
    const chips = [...this.context];
    const attached = [...this.attachments];
    const blocks = chips.map((chip) => chip.block);
    // An `@path` reference to an image or PDF is media, not prompt text — the
    // same split the CLI's own `@file` expansion makes.
    const referenced = blocks
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
      attachments: [...attached.map((chip) => chip.path), ...referenced],
    });
    this.continueLast = false;

    this.context = [];
    this.attachments = [];
    this.broadcastChips();
    // The bubble names what was sent: the pending context and attachments plus
    // whatever `@path` references were resolved out of the message itself.
    this.broadcastItem(
      this.transcript.pushUser(message, [
        ...chips.map((chip) => ({ id: chip.id, label: chip.label })),
        ...attached.map((chip) => ({ id: chip.id, label: chip.label })),
        ...expanded.blocks.map((block) => ({ id: 0, label: contextLabel(block) })),
      ]),
    );

    const command = this.binary();
    this.output.appendLine(`\n$ ${command} ${args.join(" ")}`);
    if (folder) this.output.appendLine(`  cwd ${folder.uri.fsPath}`);
    this.output.appendLine(`  prompt:\n${indent(prompt)}`);

    this.transcript.busy = true;
    this.transcript.status = "Thinking…";
    this.run = {
      cancelled: false,
      sawEvent: false,
      stderr: [],
      context: chips,
      attachments: attached,
    };
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
    this.attachments = next.attachments;
    this.broadcastChips();
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
      // The turn never started, so hand the message's chips back to the
      // composer instead of losing them with the failed run.
      for (const chip of run?.context ?? []) this.context.push(chip);
      for (const chip of run?.attachments ?? []) this.attachments.push(chip);
      this.broadcastChips();
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
    this.clearChips();
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
    this.clearChips();
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

/// A context block capped at `MAX_CONTEXT_LINES`, so a huge file cannot fill the
/// prompt on its own. The flag lets the caller say it was cut.
function trimLines(block: ContextBlock): { block: ContextBlock; cut: boolean } {
  const lines = block.text.split("\n");
  if (lines.length <= MAX_CONTEXT_LINES) return { block, cut: false };
  return {
    block: {
      ...block,
      endLine: undefined,
      text: `${lines.slice(0, MAX_CONTEXT_LINES).join("\n")}\n… (truncated)`,
    },
    cut: true,
  };
}

/// A size for an attachment chip's tooltip, or `?` for a file that is no longer
/// there.
function fileDetail(file: string): string {
  try {
    return formatBytes(fs.statSync(file).size);
  } catch {
    return "missing";
  }
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
