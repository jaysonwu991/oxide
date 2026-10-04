/// The page's own side of the packet channel.
///
/// `ui/app.js` speaks to the process that shows it through three globals it
/// checks in a fixed order — the bridge's `postMessage`, the event bridge, and
/// a queue of messages that arrived before it was listening — so this installs
/// exactly those, in the same shapes the page has always read them in, and the
/// page needed no change to run in an Electron window.
///
/// The three are `__electrobunHostBridge` (what the page sends through),
/// `__electrobun.receiveMessageFromHost` (what this process calls to hand the
/// page a packet, which the page itself defines) and the
/// `__electrobunPendingHostMessages` queue in between: the window is loaded
/// before the page's own script has run, and a packet that arrived in that gap
/// would otherwise be dropped. Everything crosses as the string the page built,
/// unread in both directions, which is what makes the engine and the page the
/// only two parties to the protocol. The names are the ones the page has read
/// since it was written, so they are kept as they are: renaming one here would
/// mean editing the page it exists for.
const { ipcRenderer } = require("electron") as typeof import("electron");

/// The same two names `electron/main.ts` sends and listens on.
const TO_HOST = "oxide:to-host";
const TO_PAGE = "oxide:to-page";

interface PageGlobals {
  __electrobunHostBridge?: { postMessage: (message: string) => void };
  __electrobun?: { receiveMessageFromHost?: (raw: string) => void };
  __electrobunPendingHostMessages?: string[];
}

/// Hands one packet to the page, or holds it in the queue the page drains when
/// its own script is up. `ui/app.js` defines `receiveMessageFromHost` itself, so
/// until it has, the queue is the only place a packet can wait.
function deliver(raw: string): void {
  const page = window as unknown as PageGlobals;
  if (typeof page.__electrobun?.receiveMessageFromHost === "function") {
    page.__electrobun.receiveMessageFromHost(raw);
    return;
  }
  page.__electrobunPendingHostMessages = page.__electrobunPendingHostMessages ?? [];
  page.__electrobunPendingHostMessages.push(raw);
}

const page = window as unknown as PageGlobals;
page.__electrobun = page.__electrobun ?? {};
page.__electrobunPendingHostMessages = page.__electrobunPendingHostMessages ?? [];
page.__electrobunHostBridge = {
  postMessage: (message: string) => ipcRenderer.send(TO_HOST, message),
};

ipcRenderer.on(TO_PAGE, (_event, raw: string) => deliver(raw));
