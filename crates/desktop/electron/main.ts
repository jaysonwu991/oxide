/// The Oxide desktop app: the window the user launches.
///
/// The app is two processes, and this is the one that has a screen. Electron
/// owns the window, the menu bar, the folder chooser, the browser a link opens
/// in and the release the app was installed from; the harness beside it
/// (`crates/desktop/src`, the `oxide-desktop` binary) owns the projects, the
/// sessions and the turns, and is the only half that reaches `oxide-core`. What
/// travels between them is one JSON packet per line: the page's own requests
/// out on the harness's stdin, its answers and announcements back on the
/// harness's stdout.
///
/// The window is therefore a relay with a handful of jobs of its own. It reads
/// neither request nor answer — the page and the harness agree on the packet,
/// and a rule about a run belongs on the side that can enforce it — except for
/// the calls that are the window's because the operating system is: the folder
/// chooser and the browser a link opens in, which are both answered here, since
/// a native panel and the platform's own handler are things no child process
/// can be reached through, and the relaunch that runs an installed release, which
/// the harness asks for because it is the half that knows whether a turn is
/// running.
///
/// Nothing here is loaded from the network: the page is this crate's `ui/`
/// directory, read from disk (or from the app's own archive once packaged), and
/// the window may not navigate anywhere else.
import { app, BrowserWindow, Menu, dialog, ipcMain, shell } from "electron";
import { spawn, type ChildProcessWithoutNullStreams } from "node:child_process";
import { existsSync } from "node:fs";
import path from "node:path";

/// The packet channel's two ends: what the page sends on its way to the
/// harness, and what the harness sends on its way back to the page.
const TO_HOST = "oxide:to-host";
const TO_PAGE = "oxide:to-page";

/// The floor the layout is built for — the sidebar, the reply column and the
/// composer's own column — kept as the window's own minimum, so the frame the
/// reader leaves behind is one the app can still paint.
const MIN_SIZE = { width: 820, height: 560 };

/// Electron keeps its own browser state — the Chromium profile, the GPU cache,
/// the cookies of nothing at all — under `app.getPath("userData")`, which by
/// default is `<appData>/Oxide`: the very directory Oxide's own configuration
/// lives in (`config.json`, `auth.json`, `sessions/`, `snapshots/`). A `Cache/`
/// and a `Local Storage/` in the middle of a config directory the CLI reads is
/// somebody else's files in it, so the profile is moved into the desktop's own
/// directory, where `projects.json` already lives.
app.setPath(
  "userData",
  path.join(app.getPath("appData"), "Oxide", "desktop", "electron"),
);

/// The harness, once it is running, and the packet lines that arrived before it
/// was: a request written into a pipe nobody has opened yet would be lost.
let harness: ChildProcessWithoutNullStreams | undefined;
let queuedToHarness: string[] = [];

/// Whether this process is on its way out, so the harness going away with it is
/// not reported as the engine having died.
let shuttingDown = false;

app.whenReady().then(() => {
  // The app is one window, and it is the app: a menu bar with nothing in it
  // would be a row of dead items, so the platforms that show one get the app's
  // own and the rest get none (as this window had none before).
  installMenu();
  const window = createWindow();
  startHarness(window);
});

app.on("window-all-closed", () => app.quit());

app.on("before-quit", () => {
  shuttingDown = true;
  harness?.kill();
});

/// The one window the app is: the page, the bridge it speaks to this process
/// on, and the bounds that keep its layout readable.
function createWindow(): BrowserWindow {
  const window = new BrowserWindow({
    title: "Oxide",
    width: 1200,
    height: 800,
    minWidth: MIN_SIZE.width,
    minHeight: MIN_SIZE.height,
    center: true,
    // The page paints nothing outside the window's own frame, so the window is
    // shown once its first frame is there rather than as a white rectangle
    // while the transcript is built.
    show: false,
    // A press that activates the window is a press in the window: without this,
    // the first click on a control in a window that is not yet key is spent on
    // making it key, and the reader has to click twice.
    acceptFirstMouse: true,
    webPreferences: {
      // The page's bridge is the preload below, and the page is the app's own
      // script, so the two live in the same world: `ui/app.js` fills in its own
      // receive half on `window`, which a context-isolated preload could not be
      // reached through. `nodeIntegration` stays off all the same — nothing in
      // the page can require anything — and the sandbox is off for the same
      // reason the two worlds are joined.
      preload: path.join(__dirname, "preload.js"),
      contextIsolation: false,
      nodeIntegration: false,
      sandbox: false,
      spellcheck: true,
    },
  });

  // Codex's own window opens filling the screen rather than at a fixed size,
  // and this one follows it: maximized rather than the platform's own
  // kiosk-style fullscreen, so the traffic lights and the menu bar stay where
  // the reader expects them and the window still restores to the size above
  // when the reader un-maximizes it.
  window.once("ready-to-show", () => {
    window.maximize();
    window.show();
  });

  // The window shows the app's own page and nothing else. It never navigates:
  // a link in a reply is opened in the machine's browser through `open_url`,
  // which is this process's own to answer rather than the harness's, and a page
  // that tried to navigate this window away would be replacing the app.
  window.webContents.setWindowOpenHandler(() => ({ action: "deny" }));
  window.webContents.on("will-navigate", (event) => event.preventDefault());
  window.webContents.on("will-attach-webview", (event) => event.preventDefault());

  void window.loadFile(pagePath());
  return window;
}

/// The page this window is: this crate's `ui/` directory, which sits beside the
/// compiled `electron/dist/` both in a checkout and inside the packaged app's
/// archive.
function pagePath(): string {
  return path.join(__dirname, "..", "..", "ui", "index.html");
}

// ---------- the harness ----------

/// The harness binary: an environment override for a development run, the copy
/// packaged beside the app, or the one `cargo build` leaves in this crate's
/// `target/`.
function harnessPath(): string {
  const name = process.platform === "win32" ? "oxide-desktop.exe" : "oxide-desktop";
  const override = process.env.OXIDE_DESKTOP_HARNESS;
  if (override) return override;
  if (app.isPackaged) return path.join(process.resourcesPath, "harness", name);
  const profile = process.env.OXIDE_DESKTOP_PROFILE === "release" ? "release" : "debug";
  return path.join(__dirname, "..", "..", "target", profile, name);
}

/// Starts the harness and wires the channel to it in both directions.
function startHarness(window: BrowserWindow): void {
  const binary = harnessPath();
  if (!existsSync(binary)) {
    fail(
      `Oxide's engine is missing.\n\n${binary}\n\n` +
        "Build it with `cargo build` in crates/desktop, or point " +
        "OXIDE_DESKTOP_HARNESS at one.",
    );
    return;
  }

  const child = spawn(binary, [], { stdio: ["pipe", "pipe", "pipe"] });
  harness = child;
  for (const line of queuedToHarness) child.stdin.write(`${line}\n`);
  queuedToHarness = [];

  // Every line the harness writes is a packet: the page's own is forwarded
  // unchanged, and the one the window is the one to carry out is read here.
  readLines(child.stdout, (line) => {
    if (!window.isDestroyed()) window.webContents.send(TO_PAGE, line);
    relay(line);
  });
  // What is not a packet is the app's own log, which belongs in the terminal a
  // development run was started from.
  readLines(child.stderr, (line) => console.error(`[oxide] ${line}`));

  child.on("error", (error) => {
    fail(`Oxide's engine would not start.\n\n${error.message}`);
  });
  child.on("exit", (code, signal) => {
    harness = undefined;
    if (shuttingDown) return;
    fail(`Oxide's engine stopped (${signal ?? code ?? "unknown"}).`);
  });
}

/// Hands one request from the page to the harness, or holds it until the
/// harness is there to read it.
function sendToHarness(line: string): void {
  if (harness) harness.stdin.write(`${line}\n`);
  else queuedToHarness.push(line);
}

/// Carries out the packet that is the window's own because the operating system
/// is: the harness decides whether the app is in a state to be restarted — a
/// turn's tools write files and its stream is read there — and the window is the
/// only half that can start it again.
function relay(line: string): void {
  const packet = parsePacket(line);
  if (packet?.type !== "message") return;
  if (packet.id === "restart") restart();
}

/// Whether a URL is a page, which is the only thing this window will hand to the
/// platform's own handler: anything else — a `file:` path, a `javascript:`
/// payload — would open something that is not a page at all.
function isOpenableUrl(url: string): boolean {
  const scheme = url.trimStart().toLowerCase();
  return scheme.startsWith("http://") || scheme.startsWith("https://");
}

/// Runs the release an install has put in place: the process running is still
/// the build that started, so only a new one is the new version. The harness
/// has already refused a restart mid-turn, since a run's tools write files and
/// its stream is read there.
function restart(): void {
  shuttingDown = true;
  harness?.kill();
  // An AppImage runs from a copy the runtime unpacked into a temporary mount,
  // so `process.execPath` is not the file the reader has: the AppImage runtime
  // names its own path in the environment, and that file is the one to run.
  const image = process.env.APPIMAGE;
  if (image && existsSync(image)) app.relaunch({ execPath: image });
  else app.relaunch();
  app.exit(0);
}

/// Reports a failure that leaves the app with nothing to show and ends it: a
/// window with no engine behind it would paint a UI whose every click fails.
function fail(message: string): void {
  dialog.showErrorBox("Oxide", message);
  app.exit(1);
}

/// Reads a child's stream line by line, which is the shape of the channel.
function readLines(stream: NodeJS.ReadableStream, onLine: (line: string) => void): void {
  let pending = "";
  stream.setEncoding("utf8");
  stream.on("data", (chunk: string) => {
    pending += chunk;
    const lines = pending.split("\n");
    pending = lines.pop() ?? "";
    for (const line of lines) {
      const trimmed = line.endsWith("\r") ? line.slice(0, -1) : line;
      if (trimmed !== "") onLine(trimmed);
    }
  });
}

function describe(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

// ---------- the page's own calls ----------

interface PagePacket {
  type?: string;
  /// A request's own number, or — for an announcement — the event's name.
  id?: number | string;
  params?: { command?: string; args?: { url?: unknown } };
}

function parsePacket(raw: unknown): PagePacket | undefined {
  if (typeof raw !== "string") return undefined;
  try {
    const packet = JSON.parse(raw) as unknown;
    if (!packet || typeof packet !== "object") return undefined;
    return packet as PagePacket;
  } catch {
    return undefined;
  }
}

ipcMain.on(TO_HOST, (event, message: unknown) => {
  const request = parsePacket(message);
  if (!request) return;
  // The folder chooser is the window's own: a native panel cannot be drawn by
  // the harness, so the request is answered here — with the folder the reader
  // picked, or with nothing when they closed it — and never reaches the
  // harness, which has no way to ask.
  if (request.params?.command === "pick_folder" && typeof request.id === "number") {
    void answerPickFolder(BrowserWindow.fromWebContents(event.sender), request.id);
    return;
  }
  // Opening a link is the window's own for the same reason: the platform's
  // handler is this process's to start, and whether it started is something only
  // this process hears. A launch the page was told had succeeded, whose browser
  // then would not run, is a click that quietly did nothing.
  if (request.params?.command === "open_url" && typeof request.id === "number") {
    void answerOpenUrl(
      BrowserWindow.fromWebContents(event.sender),
      request.id,
      request.params.args,
    );
    return;
  }
  if (typeof message === "string") sendToHarness(message);
});

/// Asks for a project folder through the window's own panel and answers the
/// page's request with it. Adding a project does not have to mean typing an
/// absolute path from memory.
async function answerPickFolder(window: BrowserWindow | null, id: number): Promise<void> {
  const options: Electron.OpenDialogOptions = {
    title: "Choose a project folder",
    properties: ["openDirectory", "createDirectory"],
  };
  const answer = window
    ? await dialog.showOpenDialog(window, options)
    : await dialog.showOpenDialog(options);
  answerRequest(window, id, answer.canceled ? null : (answer.filePaths[0] ?? null));
}

/// Opens a link in the machine's browser and answers the page's request with what
/// happened. The transcript renders URLs as anchors, but the window cannot
/// navigate to a remote page, so a click is routed here instead of being left to
/// the page's own navigation — and the scheme is checked before the URL goes
/// anywhere, so a `file:` path or a `javascript:` payload never reaches the
/// platform's handler.
async function answerOpenUrl(
  window: BrowserWindow | null,
  id: number,
  args: { url?: unknown } | undefined,
): Promise<void> {
  const url = typeof args?.url === "string" ? args.url.trim() : "";
  if (!isOpenableUrl(url)) {
    answerError(window, id, "Only http(s) links can be opened");
    return;
  }
  try {
    await shell.openExternal(url);
    answerRequest(window, id, null);
  } catch (error: unknown) {
    answerError(window, id, describe(error));
  }
}

/// Answers one of the page's requests in the shape the harness answers in, so a
/// window-owned call settles the page's own promise either way.
function answerRequest(window: BrowserWindow | null, id: number, payload: unknown): void {
  respond(window, { type: "response", id, success: true, payload });
}

/// The same answer for a call that could not be carried out, which is what the
/// page's own `catch` reads.
function answerError(window: BrowserWindow | null, id: number, error: string): void {
  respond(window, { type: "response", id, success: false, error });
}

function respond(window: BrowserWindow | null, packet: Record<string, unknown>): void {
  if (!window || window.isDestroyed()) return;
  window.webContents.send(TO_PAGE, JSON.stringify(packet));
}

// ---------- the menu bar ----------

/// The menu bar the app is, on the platforms that have one. macOS is the only
/// one: everywhere else the window is the app, as it was before.
function installMenu(): void {
  if (process.platform !== "darwin") {
    Menu.setApplicationMenu(null);
    return;
  }
  Menu.setApplicationMenu(
    Menu.buildFromTemplate([
      {
        label: "Oxide",
        submenu: [
          { label: "About Oxide", role: "about" },
          { type: "separator" },
          // The app updates itself from its own release train, and this is the
          // one place the reader can ask it to look: the window paints the
          // dialog, so the click reaches it as the event it understands.
          { label: "Check for Updates…", click: () => announceCheckUpdates() },
          { type: "separator" },
          { label: "Hide Oxide", role: "hide" },
          { label: "Hide Others", role: "hideOthers" },
          { label: "Show All", role: "unhide" },
          { type: "separator" },
          { label: "Quit Oxide", role: "quit" },
        ],
      },
      {
        label: "Edit",
        submenu: [
          { label: "Undo", role: "undo" },
          { label: "Redo", role: "redo" },
          { type: "separator" },
          { label: "Cut", role: "cut" },
          { label: "Copy", role: "copy" },
          { label: "Paste", role: "paste" },
          { label: "Select All", role: "selectAll" },
        ],
      },
      {
        label: "Window",
        submenu: [
          { label: "Minimize", role: "minimize" },
          { label: "Zoom", role: "zoom" },
          { type: "separator" },
          { label: "Close", role: "close" },
        ],
      },
    ]),
  );
}

/// Asks the open window to check for updates and show what it found. The menu
/// item has no page of its own to paint into, so it asks the window through the
/// same channel a run's own events travel on, and the menu item and the
/// sidebar's button end at one dialog.
function announceCheckUpdates(): void {
  const window = BrowserWindow.getAllWindows()[0];
  if (!window || window.isDestroyed()) return;
  window.webContents.send(
    TO_PAGE,
    JSON.stringify({ type: "message", id: "check-updates", payload: {} }),
  );
}
