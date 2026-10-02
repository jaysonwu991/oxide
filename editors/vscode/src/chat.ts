// The chat controller: it owns the transcript, the running turn and the editor
// context, and broadcasts view updates to every
// attached webview. All CLI contact goes through here.

import * as fs from "node:fs";
import * as os from "node:os";
import * as path from "node:path";
import * as vscode from "vscode";

import {
  buildTurnArgs,
  sessionDeleteArgs,
  sessionShowArgs,
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
  MAX_ATTACHMENT_BYTES,
  MAX_ATTACHMENTS,
  type AttachmentKind,
} from "./core/attachments";
import { AttachmentStore, previewForDataUrl, previewForFile } from "./attachments";
import { atSuggestions, atToken } from "./core/at";
import { isApprovalDecision, type ApprovalDecision } from "./core/approvals";
import { modelsForProvider } from "./core/config";
import type { QuestionAnswer } from "./core/questions";
import {
  agentDialog,
  APPLY_AGENT,
  APPLY_MODEL,
  APPLY_REASONING,
  APPLY_TRUST,
  CHANGES_UNDO_CONFIRM,
  CLOSE_DIALOG,
  CONTINUE_SESSION,
  deleteSessionDialog,
  MCP_REFRESH,
  MCP_TOGGLE,
  NEW_SESSION,
  mcpDialog,
  modelDialog,
  OPEN_SESSION,
  SESSION_DELETE,
  SESSION_DELETE_CONFIRM,
  sessionDialog,
  reasoningDialog,
  trustDialog,
  undoChangesDialog,
  UPDATE_INSTALL,
  UPDATE_NOTES,
  updateDialog,
  type DialogState,
  type LiveSession,
  type ModelChoice,
} from "./core/dialogs";
import { isMcpCommand, mcpListArgs, mcpToggleArgs, parseMcpList, type McpServerView } from "./core/mcps";
import {
  parseUpdateCheck,
  rejectsJson,
  updateCheckArgs,
  updateInstallArgs,
  type UpdateCheck,
} from "./core/updates";
import { modelsListArgs, parseModelCatalog } from "./core/models";
import { changeArgs, diffPlan, undoArgs, type DiffPlan } from "./core/changes";
import {
  commandRows,
  parseCommandList,
  routeCommand,
  type CommandEntry,
  type PanelAction,
} from "./core/palette";
import { footerState, REASONING_LEVELS, type FooterState } from "./core/footer";
import { projectInfo, type ProjectDeps, type ProjectInfo } from "./core/project";
import {
  buildPrompt,
  contextLabel,
  expandAtReferences,
  isAttachmentPath,
  relativePath,
  selectionLines,
  sliceLines,
  type ContextBlock,
  type LineRange,
} from "./core/prompt";
import { HISTORY_MESSAGES, parseSessionHistory } from "./core/history";
import { isSessionCommand, parseSessionList, type SessionEntry } from "./core/sessions";
import { toolDiff } from "./core/preview";
import {
  Transcript,
  type AttachmentChip,
  type ChangesItem,
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

/// How many paths the composer's completion walks before it gives up. A very
/// large project is offered what the search provider returns first rather than
/// holding the list until the whole tree has been read.
const MAX_WORKSPACE_PATHS = 10_000;

/// The update check is one HTTPS request to GitHub, made by the CLI rather than
/// by the extension host: short enough that an answer is expected, long enough
/// that a slow link is not read as a failure. The install downloads a release
/// archive and replaces the installed binary, so its budget is minutes.
const UPDATE_CHECK_TIMEOUT = 60_000;
const UPDATE_INSTALL_TIMEOUT = 600_000;

/// What `/help` says in the panel: the commands it answers itself, since every
/// other command and skill is one the CLI expands. The terminal and the desktop
/// app list the whole catalog, which is why this is not built from it.
const PANEL_HELP =
  "Panel commands: /model /reasoning /agent /trust (the footer's chips), /mcps, /session, /new, /attach, /usage. " +
  "A project's commands and skills are completed from `/` and run by the CLI — pick a skill to load its instructions.";

interface Chip extends ContextChip {
  block: ContextBlock;
}

/// The file the editor has open, tracked as a path rather than as a copy of its
/// text: a chip that has been sitting in the composer since before the last
/// save has to send what the buffer says when the message goes, not what it
/// said when the panel first looked at it.
interface AutoContext {
  id: number;
  /// The absolute path of the file being edited.
  file: string;
  /// The lines selected in it, when the reader has some: what is attached is
  /// the selection rather than the whole file, and the chip says which lines.
  selection: LineRange | null;
}

/// Whether two tracked selections are the same lines: the caret moving inside
/// one is not something the composer has to repaint for.
function sameRange(a: LineRange | null, b: LineRange | null): boolean {
  if (!a || !b) return a === b;
  return a.start === b.start && a.end === b.end;
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

/// One message assembled from the composer, ready to run: the prompt text, the
/// media paths, the chips that name what it carries, and the raw chips a failed
/// start restores. A message queued or steered while a turn runs keeps its own
/// assembled copy, so clearing the composer cannot change the context/media
/// already handed to the active RPC process.
interface PreparedSend {
  /// The trimmed message text, for the transcript bubble.
  message: string;
  /// The full prompt, context blocks included.
  prompt: string;
  /// Absolute image/PDF paths carried in the RPC frame.
  images: string[];
  /// The chips the user bubble names under the message.
  labels: ContextChip[];
  /// The raw context chips and attachments, restored if the turn fails to start.
  context: Chip[];
  attachments: Attachment[];
  cwd: string;
}

export class ChatController {
  private readonly transcript: Transcript;
  private readonly views = new Set<vscode.WebviewView>();
  private readonly store = new AttachmentStore();
  private context: Chip[] = [];
  private attachments: Attachment[] = [];
  /// Messages whose active-turn delivery lost a race with process completion.
  /// They start normally once that process exits, preserving the composed copy.
  private queue: PreparedSend[] = [];
  /// The editor's own file chip, and whether the user removed it: a file they
  /// took out stays out until they open another one.
  private auto: AutoContext | null = null;
  private autoHidden = false;
  /// The project's own paths, for the composer's `@path` completion: every file
  /// with each directory above it (`oxide_core::tools::workspace_paths`, which
  /// is what the terminal completes from). Read once per folder and reused,
  /// because walking a project is not something to repeat per keystroke.
  private pathCache: { root: string; paths: string[] } | null = null;
  /// A read already in flight, so a burst of keystrokes shares one walk.
  private pathLoad: Promise<string[]> | null = null;
  /// The CLI's own catalog for this project (`oxide commands --json`) — the
  /// built-in commands and the project's own commands, prompt templates and
  /// skills. It is what the terminal's `/` menu and the desktop app's palette
  /// are built from, so the panel's palette offers what the CLI resolves, and a
  /// row that is taken is a message the CLI expands rather than a prompt the
  /// model has to make sense of. `null` before the first read, and keyed by the
  /// folder it was read in, because the active editor decides which project the
  /// panel is on and one window can hold more than one.
  private commandCache: { root: string; entries: CommandEntry[] } | null = null;
  private commandLoad: Promise<CommandEntry[]> | null = null;
  private nextChipId = 1;
  private turn: Turn | null = null;
  private run: RunState | null = null;
  private continueLast = false;
  /// The dialog the panel paints over the transcript — the MCP server list or
  /// the session history — and the rows it was composed from. Held here rather
  /// than in a view: both panes show the same dialog, and a re-listing repaints
  /// whichever ones are attached.
  private dialog: DialogState | null = null;
  private servers: McpServerView[] = [];
  /// The newest `/mcps` probe. A listing opened while an earlier probe is still
  /// running supersedes it, so an answer that arrives afterwards is dropped
  /// instead of painting a list the newer probe has already replaced.
  private mcpProbe = 0;
  /// The active provider's complete catalog, read through the CLI so endpoint,
  /// authentication and provider-specific fallbacks stay shared with `/models`.
  private modelCatalog: ModelChoice[] = [];
  private modelQuery = "";
  private modelNote = "";
  private modelProbe = 0;
  private sessions: SessionEntry[] = [];
  /// The filter the open session listing is showing. The rows are the store's
  /// own answer, so the search box only decides which of them are painted —
  /// typing never spawns the CLI again — and the value is held here so a redraw
  /// under the reader (a store resync while a turn runs) keeps their filter.
  private sessionQuery = "";
  /// The newest session listing asked for. The store gains a thread as the turn
  /// it belongs to runs, so the same listing is read twice — once when the run
  /// names its session, again when it ends — and the two reads overlap. The
  /// earlier one can answer last (it started before the file was written), so it
  /// is dropped rather than repainting the rows the newer read has replaced.
  private sessionsSync = 0;
  /// The check the update dialog is showing, held so its rows act on what was
  /// reported: the install row installs the release the check resolved, and a
  /// row a stale dialog post back cannot install anything else.
  private update: UpdateCheck | null = null;
  /// Whether the CLI answered that it does not know `--json` (`rejectsJson`),
  /// which is the one case with no check to hold and an install still worth
  /// offering: `oxide update` is the command that replaces that older binary.
  private legacyCli = false;
  /// Whether an install is in flight. The install takes as long as a download,
  /// so the command can be asked again while one runs.
  private installing = false;
  /// The name of the thread when the CLI knows one (a resumed session keeps its
  /// picker label); otherwise the header falls back to the first message.
  private sessionTitle: string | null = null;
  private activeFolder: string | null = null;
  /// The pane the caret is in, if any. The chat has a pane in the activity bar
  /// and one in the secondary side bar, and each holds its own composer — the
  /// text in the box belongs to the view, not to the controller — so a message
  /// that is about the composer goes only to the pane being typed in.
  private focusedView: vscode.WebviewView | null = null;
  /// A pane that has asked for state, so its listener is up. A webview is built
  /// asynchronously, and a message posted into one still being built is dropped.
  private readonly readyViews = new Set<vscode.WebviewView>();
  /// Messages about the composer that arrived before any pane was listening.
  private pendingComposer: ViewMessage[] = [];
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
    view.onDidDispose(() => {
      this.views.delete(view);
      this.readyViews.delete(view);
      if (this.focusedView === view) this.focusedView = null;
    });
  }

  /// Whether the caret is in the chat. The editor's focus shortcut toggles
  /// between the editor and the composer, so it has to know which side it is
  /// on: the panel reports its own window focus either way.
  get chatFocused(): boolean {
    return this.focusedView !== null;
  }

  /// The pane with `focus` took the keyboard, or the one with `blur` gave it
  /// up — which is also what clicking back into the editor does.
  noteViewFocus(view: vscode.WebviewView, focused: boolean): void {
    if (focused) this.focusedView = view;
    else if (this.focusedView === view) this.focusedView = null;
  }

  /// Puts the caret in the composer of the pane being looked at, which is what
  /// bringing the chat forward should leave behind.
  focusComposer(): void {
    this.postToPanel({ k: "focusComposer" });
  }

  /// Writes an `@path` reference into the composer at the caret — the file and
  /// selection the editor's insert shortcut read, in the shape `core/prompt.ts`
  /// resolves back into a context block.
  insertReference(reference: string): void {
    if (!reference) return;
    this.postToPanel({ k: "insert", text: reference });
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

  /// A message about the composer rather than the transcript: the pane with the
  /// caret, else the pane on screen — and only one that is listening, since what
  /// is posted into a webview still being built is dropped (a shortcut can open
  /// the chat and post into it in the same breath). Anything else waits for the
  /// first pane to say it is ready. The transcript itself is broadcast, since
  /// both panes show the same thread.
  private postToPanel(message: ViewMessage): void {
    const listening = (view: vscode.WebviewView) => this.readyViews.has(view);
    const target =
      this.focusedView && listening(this.focusedView)
        ? this.focusedView
        : [...this.views].find((view) => view.visible && listening(view));
    if (!target) {
      this.pendingComposer.push(message);
      return;
    }
    void this.push(target, message);
  }

  /// A pane is listening: what was asked for while the panel was still being
  /// built goes to it now, in the order it was asked for.
  noteReady(view: vscode.WebviewView): void {
    this.readyViews.add(view);
    const pending = this.pendingComposer;
    this.pendingComposer = [];
    for (const message of pending) void this.push(view, message);
  }

  private async push(view: vscode.WebviewView, message: ViewMessage): Promise<void> {
    try {
      await view.webview.postMessage(message);
    } catch {
      this.views.delete(view);
    }
  }

  stateMessage(): ViewMessage {
    this.refreshProject();
    return {
      k: "state",
      ...this.transcript.state({
        queued: this.queue.length,
        context: this.chips(),
        attachments: this.attachmentChips(),
        title: this.threadTitle(),
        binary: this.binary(),
        showThinking: this.setting<boolean>("showThinking", true),
        footer: this.footer(),
      }),
    };
  }

  /// The dialog the open panes are showing, for one that has just been created:
  /// a `state` message paints the transcript, this paints the list over it.
  /// `null` when no dialog is open.
  dialogMessage(): ViewMessage | null {
    return this.dialog ? { k: "dialog", dialog: this.dialog } : null;
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
  /// configuration is re-read, which `stateMessage` does. `autoContext` is the
  /// one setting that changes the composer's own chips (it decides whether the
  /// editor's file is tracked at all), so the chip is brought back in step
  /// before the state is painted. The completion's own paths follow the
  /// exclude settings, so the walk is dropped and taken again.
  configurationChanged(): void {
    this.pathCache = null;
    // The catalog follows the project too: a setting change is the one signal
    // there is that the trust decision or the CLI's configuration may have
    // moved, and a skill the menu lists is loaded by the CLI at the far end.
    this.commandCache = null;
    this.syncActiveEditor();
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
      branch: project?.branch ?? "",
      autoCompact: project?.autoCompact ?? true,
      usage: this.transcript.usage,
    });
  }

  /// The thread's title: a known session name, else a one-line summary of the
  /// first message the user sent. Empty for a thread that has not started; the
  /// view shows a neutral placeholder for that, and the notification falls back
  /// to a plain "finished" message. Claude Code names a conversation the same
  /// way, so the panel says what the thread is about instead of repeating the
  /// folder name.
  private threadTitle(): string {
    return this.sessionTitle || this.transcript.title();
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
      askApprovals: this.setting<boolean>("askApprovals", true),
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

  /// The folder a turn runs in. The change card carries the folder its own run
  /// started in (see `ChangesItem.project`), so this is the fallback: the active
  /// editor's folder, else the first one.
  workspaceRoot(): string | null {
    return this.folder()?.uri.fsPath ?? null;
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

  // ---------- `@path` completion ----------

  /// The rows of the composer's `@path` completion for a value and a caret.
  ///
  /// Returned rather than broadcast: the rows replace the token in the box the
  /// caret was read from, so a second pane must not be handed another pane's
  /// token and splice it into a value of its own. `core/at.ts` decides the token
  /// and the rows; this supplies the project's paths, which is the one part the
  /// view cannot know. `seq` comes back with the answer, so a list that arrives
  /// after the reader has typed on is dropped instead of replacing the rows
  /// under a caret that has moved.
  async completeAt(value: string, caret: number, seq: number): Promise<ViewMessage> {
    const token = atToken(value, caret);
    if (!token) return { k: "atSuggestions", seq, kind: "path", start: 0, end: 0, rows: [] };
    const rows = atSuggestions(await this.workspacePathList(), token);
    return { k: "atSuggestions", seq, kind: "path", start: token.start, end: token.end, rows };
  }

  /// The rows of the composer's `/` palette for a value: the commands, prompt
  /// templates and skills the CLI lists for this project, matched the way the
  /// terminal and the desktop app match them. Returned rather than broadcast,
  /// for the same reason as the `@path` rows — a palette row replaces the value
  /// in the box that asked.
  async completePalette(value: string, seq: number): Promise<ViewMessage> {
    const answer = commandRows(await this.commands(), value);
    return {
      k: "paletteRows",
      seq,
      kind: "command",
      start: answer?.start ?? 0,
      end: answer?.end ?? 0,
      rows: answer?.rows ?? [],
    };
  }

  /// The CLI's catalog for this project's folder, read once and reused: it is
  /// the listing a palette row is taken from, so it is the CLI's own answer
  /// rather than the extension's guess at what the project holds. A failed read
  /// is remembered as an empty catalog, so a broken CLI spawns once rather than
  /// on every keystroke; the next setting change or finished turn tries again,
  /// and so does a move to another folder — which is why the answer is kept
  /// with the root it came from rather than on its own.
  private async commands(): Promise<CommandEntry[]> {
    const root = this.folder()?.uri.fsPath ?? "";
    if (!root) return [];
    if (this.commandCache?.root === root) return this.commandCache.entries;
    if (!this.commandLoad) {
      this.commandLoad = this.readCommands(root).finally(() => {
        this.commandLoad = null;
      });
    }
    return this.commandLoad;
  }

  private async readCommands(root: string): Promise<CommandEntry[]> {
    if (this.folder()?.uri.fsPath !== root) return [];
    const result = await runCapture(this.binary(), ["commands", "--json"], root);
    if (result.error || result.code !== 0) {
      const detail = result.error || firstLine(result.stderr) || `exit ${result.code}`;
      this.output.appendLine(`commands --json failed: ${detail}`);
      // The empty catalog is this folder's answer, not a miss: a CLI that cannot
      // answer is asked once, not on every keystroke of `/`.
      this.commandCache = { root, entries: [] };
      return [];
    }
    const entries = parseCommandList(result.stdout);
    // The folder can move while the CLI runs: an answer for the project the user
    // has left is not this one's, so it is not returned or kept for it either.
    if (this.folder()?.uri.fsPath !== root) return [];
    this.commandCache = { root, entries };
    return entries;
  }

  /// The project's paths as the terminal lists them for its own `@` completion:
  /// every file, plus each directory above it with a trailing `/`, so a folder
  /// can be completed and the query narrowed inside it. The folders' own files
  /// come from the search provider, so the exclude settings (and the ignored
  /// files it knows about) leave build output out.
  private async workspacePathList(): Promise<string[]> {
    const root = this.folder()?.uri.fsPath ?? "";
    if (!root) return [];
    if (this.pathCache?.root === root) return this.pathCache.paths;
    if (!this.pathLoad) {
      this.pathLoad = this.readWorkspacePaths(root).finally(() => {
        this.pathLoad = null;
      });
    }
    return this.pathLoad;
  }

  private async readWorkspacePaths(root: string): Promise<string[]> {
    const folder = this.folder();
    if (!folder || folder.uri.fsPath !== root) return [];
    const found = await vscode.workspace.findFiles(
      new vscode.RelativePattern(folder, "**/*"),
      "**/{node_modules,.git}/**",
      MAX_WORKSPACE_PATHS,
    );
    const paths = new Set<string>();
    for (const uri of found) {
      if (uri.scheme !== "file") continue;
      const relative = relativePath(root, uri.fsPath);
      if (!relative || relative.startsWith("..") || relative.startsWith("/")) continue;
      paths.add(relative);
      let slash = relative.lastIndexOf("/");
      while (slash > 0) {
        paths.add(relative.slice(0, slash + 1));
        slash = relative.slice(0, slash).lastIndexOf("/");
      }
    }
    // A walk that outlived the folder it started in answers nothing: the list is
    // the project's, and the panel is no longer on that project.
    if (this.folder()?.uri.fsPath !== root) return [];
    this.pathCache = { root, paths: [...paths] };
    return this.pathCache.paths;
  }

  // ---------- context and attachments ----------

  /// Keeps the composer's chip for the file the editor has open in step with
  /// the editor — the file and the lines selected in it. It is the only place
  /// the tracked file is set — called when the active editor changes, when the
  /// selection does, when a setting changes and once at activation — so nothing
  /// else has to remember to keep it current.
  syncActiveEditor(): void {
    const editor = vscode.window.activeTextEditor;
    const document = editor?.document;
    const file = document && document.uri.scheme === "file" ? document.uri.fsPath : null;
    const wanted = file && this.setting<boolean>("autoContext", true) ? file : null;
    if (!wanted) {
      if (!this.auto) return;
      this.auto = null;
      this.autoHidden = false;
      this.broadcastChips();
      return;
    }
    const selection = editor ? selectionLines(editor.selection) : null;
    if (this.auto?.file === wanted && sameRange(this.auto.selection, selection)) return;
    // A chip is for one file: the next one opened brings the chip back, under an
    // id of its own, since the ✕ that took the last one out answered for it.
    if (this.auto?.file !== wanted) {
      this.autoHidden = false;
      this.auto = { id: this.nextChipId++, file: wanted, selection };
    } else {
      // The file already tracked, narrowed to different lines: the same chip
      // following the reader's selection, and one that was taken out of the
      // message stays out — moving the caret is not opening another file.
      this.auto.selection = selection;
    }
    this.broadcastChips();
  }

  /// The chip for the file the editor has open, painted after the ones the user
  /// attached: it is context they did not ask for, and its ✕ takes it out for
  /// as long as that file is the one being edited. A selection is named by the
  /// lines it covers — the chip is the only place the reader can see what the
  /// next message will carry — and the tooltip it carries says the same thing
  /// in words, since a hyphenated range is not what a screen reader reads out.
  private autoChip(): ContextChip | null {
    if (!this.auto || this.autoHidden) return null;
    const path = this.relativeTo(this.auto.file);
    if (!this.auto.selection) return { id: this.auto.id, label: path, auto: true };
    const { start, end } = this.auto.selection;
    const lines = end > start ? `${end - start + 1} lines` : "1 line";
    return {
      id: this.auto.id,
      label: contextLabel({ path, text: "", startLine: start, endLine: end }),
      auto: true,
      detail: `${lines} selected — sent with the next message`,
    };
  }

  /// The block the tracked chip stands for. The text is read here rather than
  /// when the chip was painted, so an unsaved edit is still what the run
  /// receives; a file too long for a prompt is trimmed like any other block,
  /// silently, since nothing was attached by hand to report on. A selection is
  /// sent as the lines it names and nothing else, the same slice a ranged `@`
  /// reference inlines.
  private autoBlock(): ContextBlock | null {
    const auto = this.auto;
    if (!auto || this.autoHidden) return null;
    const document = vscode.window.activeTextEditor?.document;
    const text =
      document && document.uri.scheme === "file" && document.uri.fsPath === auto.file
        ? document.getText()
        : readTextFile(auto.file);
    if (text === null || text.includes("\u0000")) return null;
    const path = this.relativeTo(auto.file);
    if (!auto.selection) return trimLines({ path, text }).block;
    const { start, end } = auto.selection;
    const slice = sliceLines(text, start, end);
    // A file that no longer holds those lines sends nothing rather than sending
    // something other than what the chip says it will.
    if (slice === null) return null;
    return trimLines({ path, text: slice, startLine: start, endLine: end }).block;
  }

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
    if (decoded.bytes.length > MAX_ATTACHMENT_BYTES) {
      this.showNotice(
        `${label} is ${formatBytes(decoded.bytes.length)}; the attachment limit is ${formatBytes(MAX_ATTACHMENT_BYTES)}.`,
        "warn",
      );
      return null;
    }
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
    const label = path.basename(file);
    let size: number;
    try {
      size = fs.statSync(file).size;
    } catch {
      this.showNotice(`${label} could not be read.`, "warn");
      return null;
    }
    // The CLI refuses one past the limit too, but a turn that fails halfway
    // says less than a chip that never appeared.
    if (size > MAX_ATTACHMENT_BYTES) {
      this.showNotice(
        `${label} is ${formatBytes(size)}; the attachment limit is ${formatBytes(MAX_ATTACHMENT_BYTES)}.`,
        "warn",
      );
      return null;
    }
    return this.pushAttachment({
      key: `file:${file}`,
      label,
      path: file,
      kind,
      preview: kind === "image" ? previewForFile(file, mime) : null,
      detail: `${formatBytes(size)} · ${this.relativeTo(file)}`,
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

  /// Removes one pending chip, whichever list it is in. Removing the tracked
  /// file's chip hides it rather than forgetting it, so it is painted again
  /// when another file is opened.
  removeChip(id: number): void {
    if (this.auto?.id === id) {
      this.autoHidden = true;
      this.broadcastChips();
      return;
    }
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

  /// Empties the composer. The tracked file goes with the rest — "remove
  /// everything pending" — and comes back when another file is opened.
  clearChips(): void {
    this.autoHidden = this.auto !== null;
    this.dropChips();
  }

  /// Drops the chips the user attached without touching the one for the file
  /// the editor has open: a new thread or a resumed one is about the
  /// conversation, not about what is being edited.
  private dropChips(): void {
    const tracked = this.auto !== null && !this.autoHidden;
    if (!this.context.length && !this.attachments.length && !tracked) return;
    this.context = [];
    this.attachments = [];
    this.broadcastChips();
  }

  /// What the composer is holding of its own. The file the editor has open is
  /// not counted: a run that carries only that has nothing asked of it, so an
  /// empty message stays empty.
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
    const tracked = this.autoChip();
    return [
      ...this.context.map((chip) => ({ id: chip.id, label: chip.label })),
      ...(tracked ? [tracked] : []),
    ];
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

  /// Sends a message, starting a turn or choosing how it joins a running one.
  async send(text: string, busyMode: "queue" | "steer" = "queue"): Promise<void> {
    const message = text.trim();
    if (!message && !this.contextCount) return;
    // `/mcps` and `/session` are the client's own commands: they open a dialog
    // here rather than being shipped to the model as a prompt, the way the
    // terminal and the desktop app answer them.
    if (this.contextCount === 0 && isMcpCommand(message)) {
      await this.showMcps();
      return;
    }
    if (this.contextCount === 0 && isSessionCommand(message)) {
      await this.openSessions();
      return;
    }
    // The rest of the built-ins are the panel's own draws too — the footer's
    // chips under another name — and a client command it has no action for is
    // answered here rather than shipped to the model as a prompt. A configured
    // command or a skill is deliberately left alone: the CLI expands it, which
    // is what loads the skill.
    if (this.contextCount === 0) {
      const route = routeCommand(await this.commands(), message);
      if (route?.kind === "action") {
        await this.runPanelCommand(route.action);
        return;
      }
      if (route?.kind === "refused") {
        this.showNotice(`/${route.name} is not one this panel runs — use the terminal.`);
        return;
      }
    }

    const prepared = this.prepareSend(message);
    if (!prepared) return;
    if (this.turn) {
      const followUp = busyMode === "queue";
      // The active RPC process owns both queues, like the desktop and terminal:
      // a follow-up waits for the current answer, while steering is read before
      // the next model step. Keeping both in this process also preserves its
      // in-memory tool and verification state.
      const accepted = await this.turn.steer(prepared.prompt, prepared.images, followUp);
      if (!accepted) {
        this.queue.push(prepared);
        this.dropComposerChips();
        this.showNotice("The active response finished; queued this message for the next turn.");
        this.broadcastStatus();
        return;
      }
      this.dropComposerChips();
      this.broadcastItem(this.transcript.pushUser(prepared.message, prepared.labels));
      this.showNotice(
        `${followUp ? "Queued" : "Steering"}: ${firstLine(prepared.message) || "an attachment"}`,
      );
      this.broadcastStatus();
      return;
    }
    this.dropComposerChips();
    this.startTurn(prepared, true);
  }

  /// Assembles one message from the composer into everything the turn needs.
  /// Returns `null` after reporting when there is no folder, or when there is
  /// neither prompt text nor media to send — a truly empty message — so an
  /// empty send is refused here rather than queued and then refused later.
  private prepareSend(message: string): PreparedSend | null {
    const cwd = this.cwd();
    if (!cwd) {
      this.showNotice("Open a folder to run Oxide: sessions and context are per project.", "error");
      return null;
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
    // The tracked file is read now rather than from the copy its chip was
    // painted with, so the run receives the buffer as it stands — unsaved edits
    // included. A file the message already carries, attached by hand or named
    // with `@path`, is not sent twice.
    const tracked = this.autoBlock();
    const carried = new Set([...blocks, ...expanded.blocks].map((block) => block.path));
    const active = tracked && !carried.has(tracked.path) ? tracked : null;
    const carriedBlocks = active ? [active, ...blocks] : blocks;
    // An `@path` reference to an image or PDF is media, not prompt text — the
    // same split the CLI's own `@file` expansion makes.
    const referenced = carriedBlocks
      .filter((block) => isAttachmentPath(block.path))
      .map((block) => path.resolve(cwd, block.path))
      .concat(expanded.attachments);
    const prompt = buildPrompt(expanded.inlined, carriedBlocks.filter((block) => !isAttachmentPath(block.path)));
    const images = [...attached.map((chip) => chip.path), ...referenced];
    if (!prompt && images.length === 0) {
      this.showNotice(
        "The message is empty once its references are attached; add a question next to them.",
        "warn",
      );
      return null;
    }
    return {
      message,
      prompt,
      images,
      labels: [
        ...(active ? [{ id: 0, label: contextLabel(active) }] : []),
        ...chips.map((chip) => ({ id: chip.id, label: chip.label })),
        ...attached.map((chip) => ({ id: chip.id, label: chip.label })),
        ...expanded.blocks.map((block) => ({ id: 0, label: contextLabel(block) })),
      ],
      context: chips,
      attachments: attached,
      cwd,
    };
  }

  /// Starts the CLI turn for an assembled message. When the message was queued
  /// while a turn ran, its bubble is already in the transcript and the composer
  /// is already clear, so `showUser` is false and the composer is left alone.
  private startTurn(prepared: PreparedSend, showUser: boolean): void {
    const folder = this.folder();
    const args = buildTurnArgs({
      ...this.turnOptions(),
      session: this.transcript.sessionId,
      continueLast: this.continueLast && !this.transcript.sessionId,
    });
    this.continueLast = false;

    if (showUser) {
      this.broadcastItem(this.transcript.pushUser(prepared.message, prepared.labels));
    }

    const command = this.binary();
    this.output.appendLine(`\n$ ${command} ${args.join(" ")}`);
    if (folder) this.output.appendLine(`  cwd ${folder.uri.fsPath}`);
    this.output.appendLine(`  prompt:\n${indent(prepared.prompt)}`);

    this.transcript.busy = true;
    this.transcript.status = "Thinking…";
    this.run = {
      cancelled: false,
      sawEvent: false,
      stderr: [],
      context: prepared.context,
      attachments: prepared.attachments,
    };
    this.turn = startTurn(
      command,
      args,
      prepared.cwd,
      {
        prompt: prepared.prompt,
        // The paths travel with the prompt instead of in `--image` flags: in
        // rpc mode the prompt itself is a request frame.
        images: prepared.images,
      },
      {
        onEvent: (event) => this.handleEvent(event),
        onStderr: (line) => {
          this.run?.stderr.push(line.trim());
          this.output.appendLine(`[stderr] ${line}`);
        },
        onExit: (result) => this.handleExit(result),
      },
    );
    this.broadcastStatus();
  }

  /// Clears the chips the user attached from the composer. The tracked file's
  /// chip stays: it is the editor's, not something the message consumed.
  private dropComposerChips(): void {
    this.context = [];
    this.attachments = [];
    this.broadcastChips();
  }

  /// Answers the tool approval waiting behind `requestId`. The CLI holds the
  /// turn until it arrives, so this is the only way a gated tool ever runs.
  /// An `always` answer is remembered by the CLI's own broker, in the shared
  /// `approvals.json` the terminal and the desktop app read too.
  approve(requestId: number, decision: ApprovalDecision): void {
    if (!isApprovalDecision(decision)) return;
    const turn = this.turn;
    if (!turn) {
      this.showNotice("That approval request is no longer waiting.", "warn");
      return;
    }
    const messages = this.transcript.answerApproval(requestId, decision);
    if (!messages) return;
    turn.approve(requestId, decision);
    this.broadcastItem(messages);
    this.broadcastStatus();
  }

  /// Answers the question waiting behind `requestId`. The CLI holds the turn
  /// until it arrives, so this is how an `ask` call finishes; an empty `answers`
  /// list is a dismissal, which the model is told about rather than being left
  /// to wait out the timeout.
  answerQuestion(requestId: number, answers: readonly QuestionAnswer[]): void {
    const turn = this.turn;
    if (!turn) {
      this.showNotice("That question is no longer waiting.", "warn");
      return;
    }
    const messages = this.transcript.answerQuestion(requestId, answers);
    if (!messages) return;
    turn.answer(requestId, answers);
    this.broadcastItem(messages);
    this.broadcastStatus();
  }

  /// Starts a message whose active-turn delivery was rejected because that
  /// process had already finished. Its prompt/media were snapshotted before the
  /// composer cleared, so the retry is the exact message the user submitted.
  private drainQueue(): void {
    const next = this.queue.shift();
    if (!next || this.turn) return;
    this.startTurn(next, true);
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
    const id = this.transcript.sessionId;
    const messages = this.transcript.apply(event);
    this.broadcastItem(messages);
    this.broadcastStatus();
    // A session is written as the turn it belongs to runs, so the listing gains
    // its row when the header arrives rather than when the turn ends: a reader
    // who opened it while the run was starting sees the thread it is about.
    if (this.transcript.sessionId !== id) void this.syncSessions();
    this.onDidChange.fire();
  }

  private handleExit(result: { code: number | null; signal: string | null; error?: string }): void {
    const run = this.run;
    this.turn = null;
    this.run = null;
    this.transcript.busy = false;
    this.transcript.status = "Idle";
    // A card still waiting belongs to a request whose process is gone (a stop,
    // or a crash): leaving its buttons live would offer an answer nobody reads.
    this.broadcastItem(this.transcript.closeApprovals());
    this.broadcastItem(this.transcript.closeQuestions());

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
    // A turn is where files appear, so the completion's list of them is taken
    // again rather than answering out of what the project held when it started —
    // and the same goes for a command or a skill the agent wrote into `.oxide/`.
    this.pathCache = null;
    this.commandCache = null;
    this.broadcastStatus();
    // The view's status-bar spinner is driven by this event, and `handleExit`
    // runs after the last stream event, so refresh it here too.
    this.onDidChange.fire();
    // The run's own session is now in the store (or has just grown a message),
    // so a listing left open is painted again from it.
    void this.syncSessions();
    this.drainQueue();
  }

  /// A turn that finishes while the chat view is hidden is worth a toast — the
  /// CLI, desktop and extension notify on completion too, and all three honor
  /// the same switch: the extension's own `oxide.notifyOnFinish` and the shared
  /// `notifyOnComplete` the terminal's `/notify` writes. The toast names the
  /// thread, the same summarized title the panel's header shows, so it says
  /// which conversation finished.
  private notify(run: RunState | null): void {
    if (!run || this.views.size === 0) return;
    if (!this.setting<boolean>("notifyOnFinish", true)) return;
    if (!(this.project?.notifyOnComplete ?? true)) return;
    if ([...this.views].some((view) => view.visible)) return;
    const title = this.threadTitle();
    void vscode.window.showInformationMessage(title ? `Oxide: ${title}` : "Oxide finished.");
  }

  // ---------- sessions ----------

  /// Leaves the thread the panel is showing and puts it back on the new-chat
  /// page: the transcript is cleared, the thread's id is dropped (so the next
  /// message starts a session of its own rather than appending to the one that
  /// was open), and the view paints the welcome page it starts on.
  newSession(): void {
    if (this.turn) {
      // Resetting now would apply the running process's later events to the new
      // thread and drop the session id it is about to report.
      this.showNotice("A turn is running; stop it before starting a new session.", "warn");
      return;
    }
    this.closeDialog();
    this.transcript.reset();
    this.queue = [];
    this.sessionTitle = null;
    this.continueLast = false;
    this.dropChips();
    this.broadcast(this.stateMessage());
    this.showNotice("New chat: the next message starts a thread of its own.");
  }

  /// The session listing, painted from the store's last answer with the thread
  /// the panel has open marked, and `note` over it where there is something to
  /// say (a load in progress, a listing that failed, a turn holding the switch
  /// off). Every redraw goes through here rather than composing the dialog at
  /// the point it is shown, so the mark and the "New chat" row cannot go
  /// missing from a rebuild — closing the open thread is only safe to offer
  /// while the listing knows which one that is.
  private showSessions(note = ""): void {
    this.showDialog(
      sessionDialog(
        this.sessions,
        this.transcript.sessionId,
        note,
        this.liveSession(),
        this.sessionQuery,
      ),
    );
  }

  /// The thread the panel has open, as the listing needs it: the title the
  /// header shows, so a session the store has not named yet is not the one row
  /// written as a bare id.
  private liveSession(): LiveSession | null {
    const id = this.transcript.sessionId;
    return id ? { id, label: this.threadTitle() } : null;
  }

  /// The session listing painted again from a fresh read, without the note a
  /// listing that was asked for carries. A turn is where a thread is written, so
  /// the list the reader is looking at when one ends is repainted from what the
  /// store now holds — otherwise the thread that just finished is missing from
  /// it until the listing is closed and opened again. Only the session listing is
  /// repainted: a confirmation or the MCP list is not about threads.
  private async syncSessions(): Promise<void> {
    if (this.dialog?.kind !== "sessions") return;
    const cwd = this.cwd();
    if (!cwd) return;
    const sync = ++this.sessionsSync;
    const result = await runCapture(this.binary(), sessionsListArgs(), cwd);
    if (result.error || result.code !== 0) return;
    // A read that started earlier can answer later — the header's, taken before
    // the store had the thread, against the exit's, taken after — and painting it
    // would put the just-created row back out of the listing. Only the newest
    // read is applied, and only for the folder it was taken in.
    if (sync !== this.sessionsSync || cwd !== this.cwd()) return;
    this.sessions = parseSessionList(result.stdout);
    if (this.dialog?.kind !== "sessions") return;
    this.showSessions();
  }

  /// The header's history button and the footer's session chip: one action that
  /// swaps rather than a second button beside the first. A listing already up is
  /// what it closes — the click that opened it closes it, which is what the
  /// button's own `aria-expanded` says — and otherwise it opens, so the panel's
  /// history control is a toggle that always has something to toggle.
  async resumeSession(): Promise<void> {
    if (this.dialog?.kind === "sessions") {
      this.closeDialog();
      return;
    }
    await this.openSessions();
  }

  /// Opens the session history in the panel, or paints it again from a fresh
  /// read if it is already up: the threads the CLI lists for this project, so the
  /// dialog and the terminal agree on what exists. The listing is shown as soon
  /// as it arrives; a row either resumes a session, leaves the current one for a
  /// fresh chat, or continues the newest thread. `/session` comes here rather
  /// than through the toggle above, because a command that *asks* for the
  /// listing should not answer by closing it.
  ///
  /// Reading the store is all this does — switching threads is the rows' own
  /// action, and each of those refuses while a turn runs — so a listing opened
  /// mid-turn is something to read rather than a switch waiting to be taken.
  async openSessions(): Promise<void> {
    const cwd = this.cwd();
    if (!cwd) {
      this.showNotice("Open a folder first.", "error");
      return;
    }
    // The listing being opened starts unfiltered: the query belongs to the one
    // that was on screen, not to the next one.
    this.sessionQuery = "";
    this.showSessions("Loading sessions…");
    const sync = ++this.sessionsSync;
    const result = await runCapture(this.binary(), sessionsListArgs(), cwd);
    // A read that started earlier — the refresh a turn's own start or end asked
    // for — is superseded by this one, and a project that changed under the read
    // is no longer the one it was about, so neither paints.
    if (sync !== this.sessionsSync || cwd !== this.cwd()) return;
    // Closed while the read was in flight: the button toggles, so the answer is
    // dropped rather than reopening the listing the reader put away.
    if (this.dialog?.kind !== "sessions") return;
    if (result.error || result.code !== 0) {
      const detail = result.error || firstLine(result.stderr) || `exit ${result.code}`;
      const failed = `Could not list sessions: ${detail}`;
      this.showNotice(failed, "error");
      this.showSessions(failed);
      return;
    }
    this.sessions = parseSessionList(result.stdout);
    this.showSessions();
  }

  /// The listing's search box: the query narrows the rows the store has already
  /// answered, so typing filters the list without spawning the CLI again. The
  /// redraw goes through `showSessions` like every other, which is what keeps
  /// the count beside the title and the empty-list note in step with it.
  searchSessions(text: string): void {
    if (this.dialog?.kind !== "sessions" || text === this.sessionQuery) return;
    this.sessionQuery = text;
    this.showSessions();
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

  /// A row of the session dialog: one of its own two entries, or the id of a
  /// session to resume from its stored context. Resuming paints the thread's
  /// stored conversation before anything is sent, so the panel shows what the
  /// next message continues from rather than an empty transcript.
  private async openSession(value: string): Promise<void> {
    if (this.turn) {
      // The dialog stays open with the reason in place rather than closing over
      // a notice painted behind it.
      this.showSessions("A turn is running; stop it before switching sessions.");
      return;
    }
    this.closeDialog();
    if (value === NEW_SESSION) {
      this.newSession();
      return;
    }
    if (value === CONTINUE_SESSION) {
      this.continueSession();
      return;
    }
    const session = this.sessions.find((entry) => entry.id === value);
    if (!session) return;
    const label = session.label || value;
    this.transcript.reset();
    this.transcript.sessionId = value;
    this.sessionTitle = session.label || null;
    this.dropChips();
    this.broadcast(this.stateMessage());
    await this.loadHistory(value, label);
  }

  /// The stored conversation of the thread being resumed, read by the CLI from
  /// its own session file (`oxide sessions show --json`) and pushed into the
  /// transcript as finished items — what was said, and the calls the thread
  /// made with the results that answered them, so a thread that is mostly tool
  /// steps does not reopen as a bubble or two. The totals come with it, so the
  /// footer's usage line and context gauge describe the thread that was reopened
  /// instead of starting from zero; the thread is still resumable for sending
  /// when the history cannot be read, which is why a failure is a warning rather
  /// than a refused resume.
  private async loadHistory(id: string, label: string): Promise<void> {
    const cwd = this.cwd();
    if (!cwd) return;
    const result = await runCapture(this.binary(), sessionShowArgs(id, HISTORY_MESSAGES), cwd);
    if (result.error || result.code !== 0) {
      const detail = result.error || firstLine(result.stderr) || `exit ${result.code}`;
      this.showNotice(`${label} is resumed, but its history could not be read: ${detail}`, "warn");
      return;
    }
    const history = parseSessionHistory(result.stdout);
    if (!history) {
      this.showNotice(`${label} is resumed, but its history could not be read.`, "warn");
      return;
    }
    // Pushed into the transcript without painting each one: the view rebuilds
    // the whole list from the one `state` message below, so sending a message
    // per stored turn would repaint the panel sixty times over.
    this.transcript.replay(history.entries);
    this.transcript.usage = history.usage;
    if (history.name) this.sessionTitle = history.name;
    this.broadcast(this.stateMessage());
    const tail =
      history.shown < history.total
        ? ` (the newest ${history.shown} of ${history.total} messages)`
        : "";
    this.showNotice(
      `Resumed ${this.threadTitle() || label}${tail} — the next message continues this thread.`,
    );
  }

  /// The trash on a session row opens the confirmation rather than deleting: a
  /// thread's file cannot be recovered once it is gone, and the desktop asks
  /// the same way.
  private confirmDeleteSession(id: string): void {
    // The running turn owns the session file and appends to it as it works, so
    // deleting it here would pull the file out from under the process — the
    // next append fails with `No such file or directory` and the turn is lost.
    if (this.turn) {
      this.showSessions("A turn is running; stop it before deleting a thread.");
      return;
    }
    const session = this.sessions.find((entry) => entry.id === id);
    if (!session) return;
    this.showDialog(
      deleteSessionDialog({
        id: session.id,
        label: session.label,
        detail: `${session.age} · ${session.messages} message${
          session.messages === 1 ? "" : "s"
        } · ${session.id}`,
      }),
    );
  }

  /// Deletes one thread through the CLI (`sessions delete --force`, since the
  /// confirmation was taken here) and paints the listing again. A thread that
  /// was open in the panel is closed with it: the session file the next turn
  /// would have continued is gone.
  private async deleteSession(id: string): Promise<void> {
    // The confirmation could have been open when a turn started, and a queued
    // message can begin one between the click and here, so the guard is
    // repeated at the point that writes.
    if (this.turn) {
      this.showNotice("Stop the running turn before deleting a thread.", "warn");
      return;
    }
    const cwd = this.cwd();
    if (!cwd) {
      this.showNotice("Open a folder first.", "error");
      return;
    }
    const label = this.sessions.find((entry) => entry.id === id)?.label || id;
    const result = await runCapture(this.binary(), sessionDeleteArgs(id), cwd);
    if (result.error || result.code !== 0) {
      const detail = result.error || firstLine(result.stderr) || `exit ${result.code}`;
      const failed = `Could not delete ${label}: ${detail}`;
      this.showNotice(failed, "error");
      this.showSessions(failed);
      return;
    }
    if (this.transcript.sessionId === id) {
      this.transcript.reset();
      this.sessionTitle = null;
      this.continueLast = false;
      this.broadcast(this.stateMessage());
    }
    this.showNotice(`Deleted ${label}.`);
    await this.openSessions();
  }

  // ---------- dialogs ----------

  /// Paints a dialog in every attached pane. The rows are composed by
  /// `core/dialogs.ts`, so the view paints them and posts back the action one
  /// carries; the dialog itself is the controller's.
  private showDialog(dialog: DialogState): void {
    this.dialog = dialog;
    this.broadcast({ k: "dialog", dialog });
  }

  private closeDialog(): void {
    if (!this.dialog) return;
    this.dialog = null;
    this.broadcast({ k: "dialog", dialog: null });
  }

  /// The `/mcps` listing: every configured server with the state the core
  /// probed, painted in the panel's own dialog rather than a QuickPick that
  /// takes over the window. A row's button turns the server off (or back on) in
  /// the file that defines it, and the list is painted again from a fresh probe
  /// — the same open, inspect, toggle flow the terminal's `/mcps` offers.
  async showMcps(): Promise<void> {
    const cwd = this.cwd();
    // The listing and a toggle act on the files that define a project's
    // servers, and an empty path would resolve to whatever directory the
    // extension host was launched from, so the workspace is required first.
    if (!cwd) {
      this.showNotice("Open a folder first.", "error");
      return;
    }
    // Every server is started or reached to learn its state, which is quick
    // when they answer and up to the command timeout when they do not, so the
    // wait is shown rather than looking like nothing happened.
    const probe = ++this.mcpProbe;
    void vscode.window.setStatusBarMessage("Oxide: checking MCP servers…", 20_000);
    this.showDialog(mcpDialog(this.servers, "Checking servers…"));
    const result = await runCapture(this.binary(), mcpListArgs(), cwd);
    // Recheck stays available while a probe runs, and a toggle re-lists when it
    // is done, so two probes can be in flight at once: the older answer is
    // dropped rather than replacing the state the newer one has already set.
    if (probe !== this.mcpProbe) return;
    if (result.error || result.code !== 0) {
      const detail = result.error || firstLine(result.stderr) || `exit ${result.code}`;
      const failed = `Could not list MCP servers: ${detail}`;
      this.showNotice(failed, "error");
      this.showDialog(mcpDialog(this.servers, failed));
      return;
    }
    this.servers = parseMcpList(result.stdout);
    this.showDialog(mcpDialog(this.servers));
  }

  // ---------- updates ----------

  /// The newest released oxide, checked by the installed CLI and reported in the
  /// panel: `oxide update --check --json` is the same check the terminal's
  /// `oxide update --check` runs, so the release the panel offers is the one the
  /// CLI would install, verified against the same manifest.
  ///
  /// `headline` names what the check is about when it is not the question "is
  /// there an update" — the re-check a finished install runs, which reports the
  /// version now on disk.
  async checkForUpdates(headline = ""): Promise<void> {
    // A check is one request to GitHub, and it runs about the installation
    // rather than about the project, so no folder is required; a window with
    // none open still has a CLI to ask.
    const cwd = this.cwd() ?? os.homedir();
    this.showDialog(updateDialog({ k: "checking" }));
    const result = await runCapture(
      this.binary(),
      updateCheckArgs(),
      cwd,
      UPDATE_CHECK_TIMEOUT,
    );
    this.update = null;
    this.legacyCli = false;
    if (result.error || result.code !== 0) {
      // A CLI older than this panel does not know `--json`. There is no release
      // to report, but there is still something to do about it, so the dialog
      // offers the update that works on every version rather than the CLI's
      // refusal alone.
      if (rejectsJson(result.stderr)) {
        this.legacyCli = true;
        this.showDialog(
          updateDialog({
            k: "legacy",
            text: result.stderr.trim() || `exit ${result.code}`,
            path: this.binary(),
          }),
        );
        return;
      }
      const detail = result.error || firstLine(result.stderr) || `exit ${result.code}`;
      this.showDialog(updateDialog({ k: "failed", stage: "check", message: detail }));
      return;
    }
    const check = parseUpdateCheck(result.stdout);
    if (!check) {
      this.update = null;
      this.showDialog(
        updateDialog({
          k: "failed",
          stage: "check",
          message: `The CLI did not answer with a check: ${firstLine(result.stdout) || "no output"}`,
        }),
      );
      return;
    }
    this.update = check;
    if (check.updateAvailable) {
      this.showNotice(
        `Oxide ${check.latest} is available (installed: ${check.current}).`,
        "info",
      );
    }
    this.showDialog(updateDialog({ k: "ready", check, headline }));
  }

  /// The install row: the release the check resolved, installed by the CLI that
  /// resolved it. A check that could not be run, an installation the CLI leaves
  /// to Homebrew, and a second click while one is running all do nothing, so the
  /// only way in is a row that was offered — which includes the one row a CLI
  /// too old to be checked is offered, since `oxide update` is what replaces it.
  private async installUpdate(): Promise<void> {
    const check = this.update;
    const legacy = this.legacyCli;
    if (this.installing) return;
    if (!legacy && (!check || !check.updateAvailable || !check.installable)) return;
    this.installing = true;
    this.showDialog(updateDialog({ k: "installing", what: check ? `Oxide ${check.latest}` : "the newest CLI" }));
    const cwd = this.cwd() ?? os.homedir();
    const result = await runCapture(this.binary(), updateInstallArgs(), cwd, UPDATE_INSTALL_TIMEOUT);
    this.installing = false;
    if (result.error || result.code !== 0) {
      const detail = result.error || firstLine(result.stderr) || `exit ${result.code}`;
      this.showDialog(updateDialog({ k: "failed", stage: "install", message: detail }));
      return;
    }
    this.showNotice(
      // The CLI's own outcome line rather than a claim this side makes: an
      // installation that is already current answers an `oxide update` with
      // `Already up to date`, and the last line it printed is what happened.
      legacy ? lastLine(result.stdout) || "Ran oxide update." : `Installed Oxide ${check!.latest}.`,
    );
    // Read once more rather than reporting the version that was asked for: what
    // is on disk is the CLI's answer, and the dialog shows it beside the
    // headline saying what just happened.
    await this.checkForUpdates(legacy ? "Installed the newest oxide CLI" : `Installed Oxide ${check!.latest}`);
    if (legacy && this.legacyCli) {
      // The re-check came back with the same older CLI, so the report of the
      // install itself is the answer: what it printed is the only thing that can
      // say whether a release went on, since an installation already current
      // answers `Already up to date` rather than installing anything.
      const report = result.stdout.trim() || lastLine(result.stderr) || "Ran oxide update.";
      this.showDialog(
        updateDialog({
          k: "legacy",
          text: report,
          path: this.binary(),
          headline: "Ran oxide update",
        }),
      );
    }
  }

  /// The release page, in the user's browser: a webview cannot navigate, and the
  /// URL is the CLI's own, so it is checked rather than trusted blindly.
  private openRelease(url: string): void {
    if (!url) return;
    let parsed: vscode.Uri;
    try {
      parsed = vscode.Uri.parse(url, true);
    } catch {
      return;
    }
    if (parsed.scheme !== "http" && parsed.scheme !== "https") return;
    void vscode.env.openExternal(parsed);
  }

  // ---------- changes ----------

  /// One row of a turn's change card, as VS Code's diff editor needs it: the
  /// file in the project and the revision its left side is read from. `null`
  /// for a card or a row the transcript no longer holds, so a stale click from
  /// the other pane opens nothing.
  changeTarget(id: number, index: number): DiffPlan | null {
    const card = this.transcript.changes(id);
    const row = card?.rows[index];
    return card && row ? diffPlan(card.baseline, row) : null;
  }

  /// The whole card, for the multi-file diff VS Code draws from the turn.
  changeCard(id: number): ChangesItem | null {
    return this.transcript.changes(id);
  }

  /// The Undo under a card: it asks first, since what it takes away is the
  /// turn's own work, and the dialog names what the turn wrote. The restore
  /// itself is the confirmation's row, so nothing is put back by a stray click.
  undoChanges(id: number): void {
    const card = this.transcript.changes(id);
    if (!card || card.undone || !card.undoable) return;
    // A running turn owns the files an undo would move under it — the same
    // reason a thread cannot be deleted mid-turn — so it is refused rather than
    // racing the run's own writes.
    if (this.turn) {
      this.showNotice("Stop the running turn before undoing a turn.", "warn");
      return;
    }
    if (!card.baseline) return;
    this.showDialog(
      undoChangesDialog({
        id,
        detail: [card.title, card.totals].filter(Boolean).join(" · "),
      }),
    );
  }

  /// Puts a turn back through the CLI's own restore (`changes undo`), which is
  /// the snapshot the desktop app's Undo and the terminal's `/undo` use, and
  /// reports the card as undone. A refusal — a change made after the turn — is
  /// the CLI's answer rather than a silent no-op, so it is shown.
  private async restoreTurn(id: string): Promise<void> {
    const card = this.transcript.changes(Number(id));
    if (!card || card.undone) return;
    if (this.turn) {
      this.closeDialog();
      this.showNotice("Stop the running turn before undoing a turn.", "warn");
      return;
    }
    const root = card.project || this.folder()?.uri.fsPath;
    const cwd = this.cwd();
    if (!root || !cwd) {
      this.closeDialog();
      this.showNotice("Open a folder before undoing a turn.", "error");
      return;
    }
    const result = await runCapture(
      this.binary(),
      undoArgs(root, card.baseline, card.after),
      cwd,
    );
    if (result.error || result.code !== 0) {
      const detail = firstLine(result.stderr) || result.error || `exit ${result.code}`;
      this.closeDialog();
      this.showNotice(`Could not undo the turn: ${detail}`, "error");
      return;
    }
    this.closeDialog();
    this.settleUndone(Number(id));
  }

  /// Marks one card undone and says so, which is what a click on its Undo does
  /// once the restore has landed: the card stays as the listing of what the turn
  /// did, with the action it already paid spent.
  private settleUndone(id: number): void {
    const messages = this.transcript.markUndone(id);
    if (!messages) return;
    for (const message of messages) this.broadcast(message);
    this.showNotice("Put the turn's files back.");
  }

  /// One file as the run's baseline recorded it, read by the CLI out of the
  /// project's shadow snapshot — the left side of a diff, which is the one side
  /// that is nowhere on disk. `project` is the folder the card's run started in
  /// (the frame names it), so a window that has moved the active editor to
  /// another root since still reads the snapshot the card belongs to. `null`
  /// when there is no project, no baseline, or the read fails, which leaves the
  /// diff editor's side empty rather than failing the open.
  async baselineText(
    file: string,
    baseline: string,
    project?: string | null,
  ): Promise<string | null> {
    const root = project ?? this.folder()?.uri.fsPath;
    if (!root || !file || !baseline) return null;
    const result = await runCapture(this.binary(), changeArgs(file, baseline, root), root);
    if (result.error || result.code !== 0) return null;
    return result.stdout;
  }

  /// A click inside a dialog: the action a row or its trailing button carries.
  async dialogAction(action: string, value: string): Promise<void> {
    switch (action) {
      case MCP_REFRESH:
        return this.showMcps();
      case MCP_TOGGLE:
        return this.toggleMcp(value);
      case UPDATE_INSTALL:
        return this.installUpdate();
      case UPDATE_NOTES:
        return this.openRelease(value);
      case OPEN_SESSION:
        return this.openSession(value);
      case SESSION_DELETE:
        return this.confirmDeleteSession(value);
      case SESSION_DELETE_CONFIRM:
        return this.deleteSession(value);
      case CHANGES_UNDO_CONFIRM:
        return this.restoreTurn(value);
      case APPLY_MODEL:
        return this.applyDialogSetting("model", value);
      case APPLY_AGENT:
        return this.applyDialogSetting("agent", value);
      case APPLY_REASONING:
        return this.applyDialogSetting("reasoning", value);
      case APPLY_TRUST:
        return this.applyDialogSetting("projectTrust", value);
      case CLOSE_DIALOG:
        return this.closeDialog();
      default:
        return;
    }
  }

  /// Turns one server off (or back on) in the file that defines it, then paints
  /// the list again. A server that cannot be reached is still worth turning
  /// over: an unanswered probe may be exactly why the list was opened.
  private async toggleMcp(name: string): Promise<void> {
    const cwd = this.cwd();
    const server = this.servers.find((entry) => entry.name === name);
    if (!cwd || !server) return;
    const enabling = !server.enabled;
    const result = await runCapture(this.binary(), mcpToggleArgs(server, enabling), cwd);
    if (result.error || result.code !== 0) {
      const detail = result.error || firstLine(result.stderr) || `exit ${result.code}`;
      const failed = `Could not ${enabling ? "enable" : "disable"} ${server.name}: ${detail}`;
      this.showNotice(failed, "error");
      this.showDialog(mcpDialog(this.servers, failed));
      return;
    }
    this.showNotice(
      `${server.name} ${enabling ? "enabled" : "disabled"} in its ${server.source} config.`,
    );
    await this.showMcps();
  }

  // ---------- settings commands ----------

  /// A click on a footer chip, or the command that names the same action. Both
  /// entry points share one implementation, so a chip and its command cannot
  /// drift apart.
  async control(id: string): Promise<void> {
    switch (id) {
      case "model":
        return this.setModel();
      case "reasoning":
        return this.setReasoning();
      case "agent":
        return this.setAgent();
      case "access":
        return this.setProjectTrust();
      default:
        return;
    }
  }

  /// A built-in command the panel answers itself: `/model` and `/trust` are the
  /// footer's chips, `/mcps`, `/session`, `/new` and `/attach` are the panel's
  /// own dialogs and pickers, and `/help` and `/usage` are what it can say
  /// about itself. Anything the panel has no action for (`/permissions`, the
  /// desktop's own `/theme`) is refused in `send` rather than sent on.
  private async runPanelCommand(action: PanelAction): Promise<void> {
    switch (action) {
      case "help":
        this.showNotice(PANEL_HELP);
        return;
      case "mcp":
        return this.showMcps();
      case "session":
        // Opens rather than toggles: the header's button is the control that
        // swaps, and a command that names the history should not answer by
        // taking it away.
        return this.openSessions();
      case "new":
        return this.newSession();
      case "attach":
        return this.pickFiles();
      case "usage": {
        const usage = this.footer().usage;
        this.showNotice(usage || "No usage reported yet — the footer fills in per turn.");
        return;
      }
      case "model":
      case "reasoning":
      case "agent":
        return this.control(action);
      case "trust":
        return this.control("access");
    }
  }

  /// The model picker lives in the panel. Its search box also accepts a custom
  /// model id, so no part of the flow escapes into VS Code's command palette.
  async setModel(): Promise<void> {
    this.refreshProject();
    this.modelQuery = "";
    this.modelCatalog = modelsForProvider(this.project, this.project?.provider ?? "");
    const cwd = this.cwd();
    if (!cwd) {
      this.modelNote = "Open a folder to load the provider's model catalog.";
      this.showModelDialog();
      return;
    }
    const probe = ++this.modelProbe;
    this.modelNote = "Loading the full model catalog…";
    this.showModelDialog();
    const result = await runCapture(this.binary(), modelsListArgs(), cwd, 30_000);
    if (probe !== this.modelProbe) return;
    const catalog = result.code === 0 && !result.error ? parseModelCatalog(result.stdout) : null;
    if (catalog) {
      this.modelCatalog = mergeModels(catalog.models, this.modelCatalog);
      this.modelNote = catalog.error
        ? `Could not refresh models: ${firstLine(catalog.error)}`
        : this.modelCatalog.length
          ? ""
          : "The active provider returned no models; enter a model ID below.";
    } else {
      const detail = result.error || firstLine(result.stderr) || "the CLI returned no catalog";
      this.modelNote = `Could not load the full catalog: ${detail}. Remembered models are still available.`;
    }
    if (this.dialog?.kind === "model") this.showModelDialog();
  }

  private showModelDialog(): void {
    const current = this.setting<string>("model", "").trim();
    const configured = this.project?.model ?? "";
    const provider = this.project?.provider ?? "";
    this.showDialog(
      modelDialog(configured, provider, current, this.modelCatalog, this.modelQuery, this.modelNote),
    );
  }

  /// Picks the agent inside the panel from the same set `--agent` resolves.
  async setAgent(): Promise<void> {
    this.refreshProject();
    this.showAgentDialog("");
  }

  private showAgentDialog(query: string): void {
    const current = this.setting<string>("agent", "").trim();
    const agents = this.project?.agents ?? [];
    this.showDialog(agentDialog(agents, current, query));
  }

  async setReasoning(): Promise<void> {
    this.showDialog(
      reasoningDialog(this.setting<string>("reasoning", "auto"), REASONING_LEVELS),
    );
  }

  async setProjectTrust(): Promise<void> {
    this.showDialog(trustDialog(this.trust()));
  }

  /// Search belongs to whichever in-panel listing is open. For model and agent
  /// it both filters known rows and offers the typed value as a custom choice.
  searchDialog(text: string): void {
    switch (this.dialog?.kind) {
      case "sessions":
        return this.searchSessions(text);
      case "model":
        this.modelQuery = text;
        return this.showModelDialog();
      case "agent":
        return this.showAgentDialog(text);
    }
  }

  private async applyDialogSetting(key: string, value: string): Promise<void> {
    this.closeDialog();
    await this.updateSetting(key, value.trim());
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
    const status = this.transcript.statusMessage(this.queue.length, this.footer()) as Extract<
      ViewMessage,
      { k: "status" }
    >;
    this.broadcast({ ...status, title: this.threadTitle() });
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

function firstLine(text: string): string {
  const line = text.split("\n").find((entry) => entry.trim());
  return line ? line.trim() : "";
}

/// The CLI's report ends with its outcome (`Already up to date; rerun with
/// --force…`, `Installed v0.34.0.`), which is the line worth repeating.
function lastLine(text: string): string {
  const lines = text.split("\n").filter((entry) => entry.trim());
  return lines.length ? lines[lines.length - 1].trim() : "";
}

function mergeModels(primary: readonly ModelChoice[], fallback: readonly ModelChoice[]): ModelChoice[] {
  const seen = new Set<string>();
  return [...primary, ...fallback].filter((entry) => {
    const key = `${entry.provider.toLowerCase()}\0${entry.model}`;
    if (!entry.model || seen.has(key)) return false;
    seen.add(key);
    return true;
  });
}
