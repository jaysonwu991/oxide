const { app, BrowserWindow, dialog, ipcMain, shell } = require("electron");
const path = require("node:path");
const { pathToFileURL } = require("node:url");
const { RustHost } = require("./host-client.cjs");

const ROOT = path.resolve(__dirname, "..");
const UI_FILE = path.join(ROOT, "ui", "index.html");
const HOST_NAME = process.platform === "win32" ? "oxide-desktop-host.exe" : "oxide-desktop-host";
const HOST_COMMANDS = new Set([
  "list_projects",
  "add_project",
  "create_project",
  "remove_project",
  "list_sessions",
  "all_sessions",
  "project_info",
  "set_project_trust",
  "mcp_servers",
  "set_mcp_server",
  "list_commands",
  "at_suggestions",
  "session_messages",
  "rename_session",
  "delete_session",
  "list_providers",
  "login",
  "logout",
  "send_prompt",
  "cancel_run",
  "steer_run",
  "undo_turn",
  "change_sides",
  "resolve_approval",
  "resolve_question",
  "list_approvals",
  "clear_approvals",
  "list_models",
  "set_model",
  "list_themes",
  "theme_colors",
  "set_theme",
]);

let mainWindow = null;
let host = null;

function hostPath() {
  if (process.env.OXIDE_DESKTOP_HOST) return path.resolve(process.env.OXIDE_DESKTOP_HOST);
  if (app.isPackaged) return path.join(process.resourcesPath, HOST_NAME);
  return path.resolve(ROOT, "..", "..", "target", "debug", HOST_NAME);
}

function trustedSender(event) {
  return (
    mainWindow &&
    !mainWindow.isDestroyed() &&
    event.sender === mainWindow.webContents &&
    event.senderFrame === mainWindow.webContents.mainFrame
  );
}

function webUrl(value) {
  const url = new URL(String(value || ""));
  if (url.protocol !== "https:" && url.protocol !== "http:") {
    throw new Error("Only http(s) links can be opened");
  }
  return url.href;
}

async function nativeCommand(command, args) {
  if (command === "pick_folder") {
    const answer = await dialog.showOpenDialog(mainWindow, {
      title: "Add a project to Oxide",
      properties: ["openDirectory", "createDirectory"],
    });
    return answer.canceled ? null : answer.filePaths[0] || null;
  }
  if (command === "open_url") {
    await shell.openExternal(webUrl(args?.url));
    return null;
  }
  return undefined;
}

function registerIpc() {
  ipcMain.handle("oxide:invoke", async (event, request) => {
    if (!trustedSender(event)) throw new Error("Rejected desktop command from an unknown frame");
    const command = request?.command;
    const args = request?.args || {};
    const native = await nativeCommand(command, args);
    if (command === "pick_folder" || command === "open_url") return native;
    if (!HOST_COMMANDS.has(command)) throw new Error(`Unknown desktop command: ${command}`);
    return host.invoke(command, args);
  });
}

function createWindow() {
  mainWindow = new BrowserWindow({
    title: "Oxide",
    width: 1200,
    height: 800,
    minWidth: 820,
    minHeight: 560,
    acceptFirstMouse: true,
    backgroundColor: "#181818",
    show: false,
    webPreferences: {
      preload: path.join(__dirname, "preload.cjs"),
      contextIsolation: true,
      sandbox: true,
      nodeIntegration: false,
      webSecurity: true,
    },
  });

  mainWindow.webContents.setWindowOpenHandler(({ url }) => {
    try {
      shell.openExternal(webUrl(url)).catch(() => {});
    } catch {
      // Invalid and non-web URLs are denied below like every other new window.
    }
    return { action: "deny" };
  });
  mainWindow.webContents.on("will-navigate", (event, url) => {
    if (url !== pathToFileURL(UI_FILE).href) event.preventDefault();
  });
  mainWindow.once("ready-to-show", () => mainWindow.show());
  mainWindow.on("closed", () => {
    mainWindow = null;
  });
  mainWindow.loadFile(UI_FILE);
}

app.whenReady().then(() => {
  host = new RustHost(hostPath());
  host.on("event", (message) => {
    if (mainWindow && !mainWindow.isDestroyed()) {
      mainWindow.webContents.send("oxide:event", message);
    }
  });
  host.on("host-error", (payload) => {
    if (mainWindow && !mainWindow.isDestroyed()) {
      mainWindow.webContents.send("oxide:event", { event: "host-error", payload });
    } else {
      dialog.showErrorBox("Oxide could not start", payload.message);
    }
  });
  registerIpc();
  createWindow();
  app.on("activate", () => {
    if (BrowserWindow.getAllWindows().length === 0) createWindow();
  });
});

app.on("window-all-closed", () => {
  if (process.platform !== "darwin") app.quit();
});

app.on("before-quit", () => {
  host?.close();
});
