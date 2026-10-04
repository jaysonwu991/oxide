// Extension entry point: the commands, the editor context actions, and the
// status bar. All agent work goes through `ChatController`.

import * as fs from "node:fs";
import * as os from "node:os";
import * as path from "node:path";
import * as vscode from "vscode";

import { ChatController, type UpdateStore } from "./chat";
import { ChatViewProvider } from "./chatView";
import { CHANGE_SCHEME, parseSnapshotQuery } from "./core/changes";
import { isFile, exists, listMarkdown, readTextFile, realPath, resolveBinary } from "./cli";
import { configDir, parseConfigSummary } from "./core/config";
import { fileReference, selectionLines } from "./core/prompt";
import type { ProjectDeps } from "./core/project";
import type { ContextChip } from "./core/protocol";
import type { UpdateMemory } from "./core/updates";

/// Where the launch's own update check remembers what it did last time: when it
/// last asked, and the release an install put in VS Code. It is global state
/// rather than workspace state because the check is about the extension rather
/// than about the folder, and the next window may open a different project.
const UPDATE_STATE_KEY = "oxide.update";

export function activate(context: vscode.ExtensionContext): void {
  const output = vscode.window.createOutputChannel("Oxide");
  // The version this extension reports, which is what its own update check
  // compares a release against. Read from the manifest VS Code loaded it from,
  // so an installed release and a development build both answer for themselves.
  const version = String(context.extension.packageJSON.version ?? "");
  const controller = new ChatController(output, projectDeps(), version, updateMemory(context));

  // One provider serves both panes: the transcript and running turn live in the
  // controller, which broadcasts to every attached view. Queue/Steer messages
  // travel into that same turn over its RPC channel.
  const chatView = new ChatViewProvider(context.extensionUri, controller);
  const webviewOptions = { webviewOptions: { retainContextWhenHidden: true } };
  context.subscriptions.push(
    output,
    controller,
    vscode.window.registerWebviewViewProvider(ChatViewProvider.viewType, chatView, webviewOptions),
    vscode.window.registerWebviewViewProvider(
      ChatViewProvider.secondaryViewType,
      chatView,
      webviewOptions,
    ),
    // The baseline side of a change's diff. VS Code's diff editor asks for it by
    // URI, and the revision it names exists only inside the project's shadow
    // snapshot, so the controller reads it back through the CLI; the URI carries
    // the folder that project is (the card's own run root, which the window's
    // active folder need not be), and a URI with no revision is a side with no
    // content at all.
    vscode.workspace.registerTextDocumentContentProvider(CHANGE_SCHEME, {
      provideTextDocumentContent: async (uri) => {
        const { project, revision } = parseSnapshotQuery(uri.query);
        if (!revision) return "";
        const file = uri.path.startsWith("/") ? uri.path.slice(1) : uri.path;
        return (await controller.baselineText(file, revision, project || null)) ?? "";
      },
    }),
  );

  const status = vscode.window.createStatusBarItem(vscode.StatusBarAlignment.Right, 90);
  status.command = "oxide.openChat";
  context.subscriptions.push(status);

  const refreshStatus = (): void => {
    if (!vscode.workspace.workspaceFolders?.length) {
      status.hide();
      return;
    }
    const file = path.join(configFile(), "config.json");
    const summary = parseConfigSummary(readTextFile(file) ?? "");
    const model = setting<string>("model", "").trim() || summary?.model || "";
    const provider = summary?.provider ?? "";
    status.text = controller.running ? "$(sync~spin) Oxide" : "$(circuit-board) Oxide";
    const tooltip = new vscode.MarkdownString();
    tooltip.appendMarkdown(
      `**Oxide** — ${model ? `${provider ? `${provider} · ` : ""}${model}` : "no model configured"}\n\n`,
    );
    tooltip.appendMarkdown(`Binary: \`${binaryPath()}\`\n\n`);
    tooltip.appendMarkdown(`Config: \`${file}\`\n\n`);
    tooltip.appendMarkdown(
      "Click to open the chat. Choose a provider with the Connect Provider command, or `/connect` in the composer.",
    );
    status.tooltip = tooltip;
    status.show();
  };

  // A command that answers rather than reports — the update check returns what
  // it read so its own caller can name it — is still fire-and-forget here.
  const guard =
    <A extends unknown[]>(run: (...args: A) => Promise<unknown>) =>
    (...args: A): void => {
      void run(...args).catch((error: unknown) => {
        output.appendLine(`[error] ${String(error)}`);
        void vscode.window.showErrorMessage(`Oxide: ${String(error)}`);
      });
    };

  context.subscriptions.push(
    controller.onDidChange.event(() => refreshStatus()),
    vscode.workspace.onDidChangeConfiguration((event) => {
      if (!event.affectsConfiguration("oxide")) return;
      refreshStatus();
      // The footer's chips read the same settings the status bar does.
      controller.configurationChanged();
    }),
    // The composer's chip for the file being edited follows the editor, so the
    // file the user is working in rides along with the next message without
    // having to be attached by hand.
    vscode.window.onDidChangeActiveTextEditor(() => {
      refreshStatus();
      controller.syncActiveEditor();
    }),
    // A selection is context too: the same chip narrows to the lines the reader
    // selected, and back to the whole file when they let it go.
    vscode.window.onDidChangeTextEditorSelection(() => controller.syncActiveEditor()),
    // Bringing the chat forward leaves the caret in the composer: a click on the
    // toolbar mark or the status bar is a click to type in the panel, not only to
    // look at it, and focusing a webview view does not focus its own DOM.
    vscode.commands.registerCommand(
      "oxide.openChat",
      guard(async () => {
        await focusChat(controller);
        controller.focusComposer();
      }),
    ),
    // The editor's own focus toggle: from the editor the caret goes to the
    // composer, and from the composer back to the editor it came from.
    vscode.commands.registerCommand(
      "oxide.focusInput",
      guard(async () => {
        if (controller.chatFocused) {
          await vscode.commands.executeCommand("workbench.action.focusActiveEditorGroup");
          return;
        }
        await focusChat(controller);
        controller.focusComposer();
      }),
    ),
    // The file the editor has open, or the selection in it, as an `@path`
    // reference in the composer — the same reference the `@` completion writes,
    // with the lines it was read from when there was a selection.
    vscode.commands.registerCommand(
      "oxide.insertReference",
      guard(async () => {
        const editor = vscode.window.activeTextEditor;
        if (!editor || editor.document.uri.scheme !== "file") {
          void vscode.window.showInformationMessage("Oxide: open a file first.");
          return;
        }
        // The same rule the tracked chip reads a selection by, so a drag that
        // stopped where a line starts does not name that line.
        const reference = fileReference(
          controller.relativeTo(editor.document.uri.fsPath),
          selectionLines(editor.selection) ?? undefined,
        );
        await focusChat(controller);
        controller.insertReference(reference);
      }),
    ),
    vscode.commands.registerCommand("oxide.newSession", () => controller.newSession()),
    vscode.commands.registerCommand("oxide.resumeSession", guard(() => controller.resumeSession())),
    vscode.commands.registerCommand("oxide.continueSession", () => controller.continueSession()),
    vscode.commands.registerCommand("oxide.stop", () => controller.stop()),
    vscode.commands.registerCommand("oxide.addToChat", guard((uri?: vscode.Uri) => addToChat(controller, uri))),
    vscode.commands.registerCommand(
      "oxide.askAboutSelection",
      guard(async () => {
        const chip = await addSelection(controller);
        const question = await vscode.window.showInputBox({
          title: "Oxide: ask about the selection",
          prompt: "What do you want to know?",
          placeHolder: "e.g. why does this retry loop spin?",
        });
        if (!question?.trim()) {
          if (chip) controller.removeChip(chip.id);
          return;
        }
        await controller.send(question);
      }),
    ),
    vscode.commands.registerCommand(
      "oxide.explainSelection",
      guard(async () => {
        // No editor or no on-disk file means there is nothing attached, so the
        // instruction would refer to a selection the run never received.
        if (!(await addSelection(controller))) return;
        await controller.send(
          "Explain the attached selection: what it does, and anything surprising, risky, or subtly wrong about it.",
        );
      }),
    ),
    vscode.commands.registerCommand(
      "oxide.fixSelection",
      guard(async () => {
        if (!(await addSelection(controller))) return;
        await controller.send(
          "Fix the attached selection. Keep the change minimal and make it match the surrounding code.",
        );
      }),
    ),
    vscode.commands.registerCommand(
      "oxide.reviewChanges",
      guard(async () => {
        await focusChat(controller);
        await controller.send(
          "Review the uncommitted changes in this working tree. Summarize what changed, then flag anything wrong or risky. Do not modify files.",
        );
      }),
    ),
    vscode.commands.registerCommand("oxide.openTerminal", () => openTerminal(binaryPath())),
    vscode.commands.registerCommand("oxide.showOutput", () => output.show(true)),
    vscode.commands.registerCommand("oxide.setModel", guard(() => controller.setModel())),
    vscode.commands.registerCommand("oxide.setAgent", guard(() => controller.setAgent())),
    vscode.commands.registerCommand("oxide.setReasoning", guard(() => controller.setReasoning())),
    vscode.commands.registerCommand("oxide.setProjectTrust", guard(() => controller.setProjectTrust())),
    // The same provider table `/connect` opens in the composer, searched and
    // signed in through the CLI the panel drives: the credential lands where the
    // terminal's `/connect` puts it, so the login is shared rather than a second
    // one this extension keeps.
    vscode.commands.registerCommand(
      "oxide.connectProvider",
      guard(() => controller.openProviders()),
    ),
    // The same list `/mcp` opens in the composer, so the picker is reachable
    // from the palette whether or not the user knows the slash command.
    vscode.commands.registerCommand("oxide.mcpServers", guard(() => controller.showMcps())),
    // Checking for a newer release is the installed CLI's own check, answered in
    // the panel: the version the terminal's `oxide update` would install, and an
    // install row that runs it. The panel is not a place a binary is replaced
    // silently, so the check only reports and the install is a row the user
    // presses.
    vscode.commands.registerCommand("oxide.checkForUpdates", guard(() => controller.checkForUpdates())),
  );

  refreshStatus();
  // A file already open when the window started is tracked from the first
  // message, which is what the panel paints its chip from.
  controller.syncActiveEditor();
  // A newer release of this extension is noticed here rather than nowhere: VS
  // Code updates what it installed from the Marketplace, and a `.vsix` from the
  // release page is not one of those. It runs in the background — the launch
  // does not wait on the network, and nothing is painted over the panel — and
  // reports itself with a notification when there is something to do. A check
  // that failed is the output channel's alone, so an editor started without a
  // network does not open with an error about it.
  void controller.checkForUpdatesInBackground().catch((error: unknown) => {
    output.appendLine(`[update] ${String(error)}`);
  });
}

export function deactivate(): void {
  // Disposables registered in `context.subscriptions` are torn down by VS Code,
  // which cancels a running turn through `ChatController.dispose`.
}

function setting<T>(key: string, fallback: T): T {
  return vscode.workspace.getConfiguration("oxide").get<T>(key, fallback);
}

/// Brings the chat forward where the user keeps it. The secondary side bar's
/// pane is the default target because that is where chat lives in VS Code; a
/// build without that container has no such view to focus, so the activity-bar
/// pane takes over.
async function focusChat(controller: ChatController): Promise<void> {
  const visible = controller.visibleViewType();
  if (visible) {
    await vscode.commands.executeCommand(`${visible}.focus`);
    return;
  }
  try {
    await vscode.commands.executeCommand(`${ChatViewProvider.secondaryViewType}.focus`);
  } catch {
    await vscode.commands.executeCommand(`${ChatViewProvider.viewType}.focus`);
  }
}

function binaryPath(): string {
  return resolveBinary(setting<string>("binaryPath", "oxide"), {
    env: process.env,
    platform: process.platform,
    exists: isFile,
  });
}

/// Everything the footer reads off disk: the shared Oxide config directory, the
/// workspace's own `.oxide/` files, and the git branch the session runs on.
function projectDeps(): ProjectDeps {
  return {
    read: readTextFile,
    list: listMarkdown,
    exists,
    realpath: realPath,
    configDir: configFile(),
    home: os.homedir(),
    env: process.env,
  };
}

function configFile(): string {
  return configDir({
    platform: process.platform,
    env: process.env,
    home: os.homedir(),
    exists: fs.existsSync,
  });
}

/// The update state the panel keeps between launches, as the store
/// `ChatController` reads it. A value from another version of this extension is
/// read field by field and anything missing falls back to the empty state, so a
/// stale key costs one extra check rather than a crash on activation.
function updateMemory(context: vscode.ExtensionContext): UpdateStore {
  return {
    read: () => {
      const stored: unknown = context.globalState.get(UPDATE_STATE_KEY);
      if (!stored || typeof stored !== "object" || Array.isArray(stored)) return null;
      const record = stored as Record<string, unknown>;
      return {
        checkedAt: typeof record.checkedAt === "number" ? record.checkedAt : 0,
        installedVersion:
          typeof record.installedVersion === "string" ? record.installedVersion : "",
      };
    },
    write: async (state: UpdateMemory) => {
      await context.globalState.update(UPDATE_STATE_KEY, state);
    },
  };
}

/// Adds the editor's selection, or the whole file, as context. Returns the
/// chip so a cancelled command can take it back.
async function addSelection(controller: ChatController): Promise<{ id: number } | null> {
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
  const lines = selectionLines(editor.selection);
  return add(
    controller,
    controller.addContext({
      path: controller.relativeTo(document.uri.fsPath),
      startLine: lines?.start,
      endLine: lines?.end,
      text: lines ? document.getText(editor.selection) : document.getText(),
    }),
  );
}

/// A file the explorer, a drop or a picker handed over: an image or PDF becomes
/// media, a text file is inlined as context.
async function addToChat(controller: ChatController, uri?: vscode.Uri): Promise<void> {
  if (!uri || uri.scheme !== "file") {
    await addSelection(controller);
    return;
  }
  const chip = controller.addFile(uri.fsPath);
  if (chip) add(controller, chip);
}

function add(controller: ChatController, chip: ContextChip): { id: number } | null {
  const pending = controller.contextCount;
  void vscode.window.setStatusBarMessage(
    `Oxide: attached ${chip.label}${pending > 1 ? ` (${pending} pending)` : ""} — send a message to include it.`,
    5_000,
  );
  return chip;
}

function openTerminal(binary: string): void {
  const editor = vscode.window.activeTextEditor;
  const folder = (editor && vscode.workspace.getWorkspaceFolder(editor.document.uri)) ||
    vscode.workspace.workspaceFolders?.[0];
  const terminal = vscode.window.createTerminal({ name: "Oxide", cwd: folder?.uri.fsPath });
  terminal.show();
  terminal.sendText(/[\s"']/.test(binary) ? `"${binary}"` : binary, true);
}
