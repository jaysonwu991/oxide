// Extension entry point: the commands, the editor context actions, and the
// status bar. All agent work goes through `ChatController`.

import * as fs from "node:fs";
import * as os from "node:os";
import * as path from "node:path";
import * as vscode from "vscode";

import { ChatController } from "./chat";
import { ChatViewProvider } from "./chatView";
import { isFile, exists, listMarkdown, readTextFile, realPath, resolveBinary } from "./cli";
import { configDir, parseConfigSummary } from "./core/config";
import type { ProjectDeps } from "./core/project";
import { isAttachmentPath, type ContextBlock } from "./core/prompt";

/// A whole-file context block is inlined into the prompt, so anything larger
/// than this is trimmed rather than shipped to the model in full.
const MAX_CONTEXT_LINES = 2_000;

export function activate(context: vscode.ExtensionContext): void {
  const output = vscode.window.createOutputChannel("Oxide");
  const controller = new ChatController(output, projectDeps());

  // One provider serves both panes: the transcript, the running turn and the
  // queued follow-ups live in the controller, which broadcasts to every
  // attached view.
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
      "Click to open the chat. Provider logins happen in the terminal: run **Oxide: Open Terminal** and `/login` there.",
    );
    status.tooltip = tooltip;
    status.show();
  };

  const guard =
    <A extends unknown[]>(run: (...args: A) => Promise<void>) =>
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
    vscode.window.onDidChangeActiveTextEditor(() => refreshStatus()),
    vscode.commands.registerCommand("oxide.openChat", guard(() => focusChat(controller))),
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
          if (chip) controller.removeContext(chip.id);
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
  );

  refreshStatus();
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

async function addToChat(controller: ChatController, uri?: vscode.Uri): Promise<void> {
  if (!uri || uri.scheme !== "file") {
    await addSelection(controller);
    return;
  }
  const relative = controller.relativeTo(uri.fsPath);
  if (isAttachmentPath(uri.fsPath)) {
    // Images and PDFs travel as media (`--image`), not as prompt text.
    add(controller, { path: relative, text: "" });
    return;
  }
  let text: string;
  try {
    text = fs.readFileSync(uri.fsPath, "utf8");
  } catch (error) {
    void vscode.window.showWarningMessage(`Oxide: could not read ${relative} (${String(error)}).`);
    return;
  }
  if (text.includes("\u0000")) {
    void vscode.window.showWarningMessage(`Oxide: ${relative} is not a text file.`);
    return;
  }
  add(controller, trimLines({ path: relative, text }));
}

function add(controller: ChatController, block: ContextBlock): { id: number } | null {
  const chip = controller.addContext(block);
  const pending = controller.contextCount;
  void vscode.window.setStatusBarMessage(
    `Oxide: attached ${chip.label}${pending > 1 ? ` (${pending} pending)` : ""} — send a message to include it.`,
    5_000,
  );
  return chip;
}

function trimLines(block: ContextBlock): ContextBlock {
  const lines = block.text.split("\n");
  if (lines.length <= MAX_CONTEXT_LINES) return block;
  const kept = lines.slice(0, MAX_CONTEXT_LINES).join("\n");
  void vscode.window.showWarningMessage(
    `Oxide: attached the first ${MAX_CONTEXT_LINES} lines of ${block.path}.`,
  );
  return { ...block, endLine: undefined, text: `${kept}\n… (truncated)` };
}

function openTerminal(binary: string): void {
  const editor = vscode.window.activeTextEditor;
  const folder = (editor && vscode.workspace.getWorkspaceFolder(editor.document.uri)) ||
    vscode.workspace.workspaceFolders?.[0];
  const terminal = vscode.window.createTerminal({ name: "Oxide", cwd: folder?.uri.fsPath });
  terminal.show();
  terminal.sendText(/[\s"']/.test(binary) ? `"${binary}"` : binary, true);
}
