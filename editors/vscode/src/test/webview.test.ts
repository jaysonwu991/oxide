// `media/main.js` is plain JavaScript with no type checking, and the footer and
// composer are the parts of it the host hands the most data to. Running the file
// against a small DOM stub makes the painting assertable under node: which chips
// appear, whether a label is split into a dim key and a value, whether an
// attachment renders a thumbnail or a glyph, when Send is enabled, and what a
// paste or a drop sends back to the host.

import assert from "node:assert/strict";
import * as fs from "node:fs";
import * as path from "node:path";
import { describe, it } from "node:test";
import * as vm from "node:vm";

const root = path.join(__dirname, "..", "..");
const renderer = fs.readFileSync(path.join(root, "media", "main.js"), "utf8");
const shell = fs.readFileSync(path.join(root, "src", "chatView.ts"), "utf8");
const style = fs.readFileSync(path.join(root, "media", "style.css"), "utf8");

/// Every element the HTML shell declares, with the `hidden` attribute it starts
/// with, so the stubbed document matches the real one and a renamed id fails
/// here.
function shellElements(): { id: string; hidden: boolean }[] {
  const elements = new Map<string, boolean>();
  for (const match of shell.matchAll(/<[a-z]+[^>]*\bid="([^"]+)"[^>]*>/g)) {
    elements.set(match[1], /\shidden[\s>/]/.test(match[0]));
  }
  return [...elements].map(([id, hidden]) => ({ id, hidden }));
}

class StubClassList {
  private readonly names = new Set<string>();

  add(...names: string[]): void {
    for (const name of names) this.names.add(name);
  }

  remove(...names: string[]): void {
    for (const name of names) this.names.delete(name);
  }

  toggle(name: string, force?: boolean): boolean {
    const on = force ?? !this.names.has(name);
    if (on) this.names.add(name);
    else this.names.delete(name);
    return on;
  }

  contains(name: string): boolean {
    return this.names.has(name);
  }

  /// In source order, which is what the class attribute holds.
  list(): string[] {
    return [...this.names];
  }

  replace(value: string): void {
    this.names.clear();
    for (const name of value.split(/\s+/)) if (name) this.names.add(name);
  }
}

class StubElement {
  readonly children: StubElement[] = [];
  readonly classList = new StubClassList();
  readonly dataset: Record<string, string> = {};
  readonly style: Record<string, string> = {};
  readonly listeners = new Map<string, ((event: unknown) => void)[]>();
  readonly attributes: Record<string, string> = {};
  parent: StubElement | null = null;
  hidden = false;
  disabled = false;
  value = "";
  checked = false;
  title = "";
  type = "";
  src = "";
  alt = "";
  placeholder = "";
  spellcheck = false;
  rows = 0;
  clientHeight = 0;
  private offset = 0;
  private text = "";

  constructor(readonly tagName: string, readonly id = "") {}

  /// A scroll reports itself to the element's own listeners, which is how the
  /// renderer learns that the reader left the bottom.
  get scrollTop(): number {
    return this.offset;
  }

  set scrollTop(value: number) {
    this.offset = value;
    for (const handler of this.listeners.get("scroll") ?? []) handler({ type: "scroll" });
  }

  /// Assigning the class attribute replaces the list, as it does in the DOM:
  /// the renderer sets `className = "chip attachment"` and matches on it later.
  get className(): string {
    return this.classList.list().join(" ");
  }

  set className(value: string) {
    this.classList.replace(value);
  }

  /// What the renderer reads back: with no layout engine, a height is modelled
  /// from the content — 40 characters to the line, 20px to the line — which is
  /// enough for the follow arithmetic to be asserted without a browser.
  get scrollHeight(): number {
    const own = Math.ceil(this.text.length / 40) * 20;
    const inner = this.children.reduce(
      (sum, child) => sum + (child.hidden ? 0 : child.scrollHeight),
      0,
    );
    return Math.max(this.clientHeight, own + inner);
  }

  get textContent(): string {
    return this.text;
  }

  /// Like the DOM: writing text replaces whatever was there.
  set textContent(value: string) {
    this.text = value;
    this.children.length = 0;
  }

  /// The renderer clears a container with `innerHTML = ""` and only ever sets
  /// HTML it builds itself, so a stub that keeps it as text is faithful enough.
  set innerHTML(html: string) {
    this.textContent = html;
  }

  get innerHTML(): string {
    return this.text;
  }

  append(...nodes: StubElement[]): void {
    for (const node of nodes) this.appendChild(node);
  }

  appendChild(node: StubElement): StubElement {
    node.parent = this;
    this.children.push(node);
    return node;
  }

  remove(): void {
    if (!this.parent) return;
    const at = this.parent.children.indexOf(this);
    if (at >= 0) this.parent.children.splice(at, 1);
    this.parent = null;
  }

  addEventListener(kind: string, handler: (event: unknown) => void): void {
    const list = this.listeners.get(kind) ?? [];
    list.push(handler);
    this.listeners.set(kind, list);
  }

  setAttribute(name: string, value: string): void {
    this.attributes[name] = value;
  }

  getAttribute(name: string): string | null {
    return this.attributes[name] ?? null;
  }

  /// Enough of `matches` for the renderer's own selectors: it walks up from a
  /// click target to find `[data-control]`, `[data-path]`, `a[href]`, `.thead`
  /// or the card a tool row belongs to, and the question card reads back the
  /// answers it painted with a compound of a class, a data attribute with a
  /// value and `:checked`.
  matches(selector: string): boolean {
    let rest = selector.trim();
    const tag = /^[a-z]+/.exec(rest);
    if (tag) {
      if (this.tagName !== tag[0]) return false;
      rest = rest.slice(tag[0].length);
    }
    const parts = rest.match(/\.[\w-]+|\[[^\]]+\]|:[\w-]+/g) ?? [];
    // Anything the loop does not consume (a combinator, an unknown part) is not
    // a selector the renderer uses.
    if (parts.join("") !== rest) return false;
    for (const part of parts) {
      if (part.startsWith(".")) {
        if (!this.classList.contains(part.slice(1))) return false;
      } else if (part === ":checked") {
        if (!this.checked) return false;
      } else if (part === "[href]") {
        if (this.getAttribute("href") === null) return false;
      } else {
        const attribute = /^\[data-([a-z-]+)(?:="([^"]*)")?\]$/.exec(part);
        if (!attribute) return false;
        const key = attribute[1].replace(/-(\w)/g, (_, letter: string) => letter.toUpperCase());
        const value = this.dataset[key];
        if (attribute[2] === undefined ? value === undefined : value !== attribute[2]) return false;
      }
    }
    return true;
  }

  /// The renderer's one descendant lookup, over the selectors `matches` knows.
  querySelectorAll(selector: string): StubElement[] {
    const found: StubElement[] = [];
    for (const child of this.children) {
      if (child.matches(selector)) found.push(child);
      found.push(...child.querySelectorAll(selector));
    }
    return found;
  }

  querySelector(selector: string): StubElement | null {
    return this.querySelectorAll(selector)[0] ?? null;
  }

  closest(selector: string): StubElement | null {
    let node: StubElement | null = this;
    while (node) {
      if (node.matches(selector)) return node;
      node = node.parent;
    }
    return null;
  }

  focus(): void {}

  fire(kind: string, event: unknown = {}): void {
    for (const handler of this.listeners.get(kind) ?? []) handler(event);
  }
}

/// A canvas the renderer can draw on: `toDataURL` reports the type it asked for,
/// which is how the downscale path is asserted.
class StubCanvas extends StubElement {
  width = 0;
  height = 0;
  drawn = 0;

  constructor() {
    super("canvas");
  }

  getContext(): { drawImage: () => void } {
    return {
      drawImage: () => {
        this.drawn += 1;
      },
    };
  }

  toDataURL(type: string): string {
    return `data:${type};base64,QUJD`;
  }
}

/// A clipboard image is a `data:` URL, which is what the CLI cannot take — the
/// host writes it to a file instead.
class StubFileReader {
  result = "";
  onload: (() => void) | null = null;
  onerror: (() => void) | null = null;

  readAsDataURL(file: { type: string }): void {
    this.result = `data:${file.type};base64,QUJD`;
    this.onload?.();
  }
}

/// An image reports its natural size once its `src` is set; the renderer only
/// downscales one whose longest edge is over 1568px.
class StubImage {
  onload: (() => void) | null = null;
  onerror: (() => void) | null = null;
  width = 10;
  height = 10;

  set src(value: string) {
    this.loads += 1;
    this.loaded = value;
    setTimeout(() => this.onload?.(), 0);
  }

  loaded = "";
  /// How many sources this image was told to load, so a test can see that a
  /// repaint reused the decode already in flight.
  loads = 0;
}

/// The renderer re-follows the bottom when the pane it paints into changes size.
class StubResizeObserver {
  static readonly observers: StubResizeObserver[] = [];

  constructor(readonly callback: () => void) {
    StubResizeObserver.observers.push(this);
  }

  observe(): void {}

  resize(): void {
    this.callback();
  }
}

interface Harness {
  byId: Map<string, StubElement>;
  posted: Record<string, unknown>[];
  send(message: unknown): void;
  fireDocument(kind: string, event: unknown): void;
  /// The canvas the last downscale drew on, if any.
  readable(): { image: StubImage; canvas: StubCanvas | null };
  /// A change to the pane's own size, which the renderer observes.
  resize(): void;
}

/// The first descendant with the given class, which is how the tests reach the
/// nodes the renderer builds itself (a tool card's body, for one).
function find(node: StubElement, className: string): StubElement | null {
  if (node.classList.contains(className)) return node;
  for (const child of node.children) {
    const hit = find(child, className);
    if (hit) return hit;
  }
  return null;
}

function loadRenderer(options: { image?: StubImage } = {}): Harness {
  const byId = new Map<string, StubElement>();
  for (const { id, hidden } of shellElements()) {
    const element = new StubElement("div", id);
    element.hidden = hidden;
    byId.set(id, element);
  }
  const posted: Record<string, unknown>[] = [];
  const documentListeners = new Map<string, ((event: unknown) => void)[]>();
  const image = options.image ?? new StubImage();
  let canvas: StubCanvas | null = null;

  const document = {
    getElementById: (id: string) => byId.get(id) ?? null,
    createElement: (tagName: string) => {
      if (tagName === "canvas") {
        canvas = new StubCanvas();
        return canvas;
      }
      return new StubElement(tagName);
    },
    addEventListener: (kind: string, handler: (event: unknown) => void) => {
      const list = documentListeners.get(kind) ?? [];
      list.push(handler);
      documentListeners.set(kind, list);
    },
    body: new StubElement("body"),
  };

  const context = vm.createContext({
    document,
    window: {
      addEventListener: document.addEventListener,
      matchMedia: () => ({ matches: false }),
      setTimeout,
      clearTimeout,
    },
    acquireVsCodeApi: () => ({ postMessage: (message: unknown) => posted.push(message as never) }),
    requestAnimationFrame: (callback: () => void) => setTimeout(callback, 0),
    setTimeout,
    clearTimeout,
    setInterval: () => 0,
    clearInterval: () => {},
    Element: StubElement,
    Image: function Image() {
      return image;
    },
    FileReader: StubFileReader,
    ResizeObserver: StubResizeObserver,
    console,
  });
  vm.runInContext(renderer, context, { filename: "main.js" });

  return {
    byId,
    posted,
    send: (message) => {
      for (const handler of documentListeners.get("message") ?? []) handler({ data: message });
    },
    fireDocument: (kind, event) => {
      for (const handler of documentListeners.get(kind) ?? []) handler(event);
    },
    readable: () => ({ image, canvas }),
    resize: () => {
      for (const observer of StubResizeObserver.observers) observer.resize();
    },
  };
}

const footer = {
  chips: [
    { id: "setModel", label: "model: glm-5 · 128.0k", title: "Switch the model" },
    { id: "cycleReasoning", label: "thinking: auto" },
    { id: "setAgent", label: "agent: review" },
    { id: "setProjectTrust", label: "access: trusted" },
    { id: "resumeSession", label: "session: 1a2b3c" },
  ],
  info: "main",
  usage: "↑1.2k · ↓340 · $0.01 · ctx 12%/128.0k (auto)",
  percent: 12,
  level: "ok",
};

function stateMessage(extra: Record<string, unknown> = {}): Record<string, unknown> {
  return {
    k: "state",
    items: [],
    context: [],
    attachments: [],
    showThinking: true,
    title: "oxide",
    status: "Idle",
    busy: false,
    queued: 0,
    footer,
    ...extra,
  };
}

/// The host's wait for a `postMessage` that follows an asynchronous read, and
/// for the frame that paints a deferred render (the stub runs one on a timer).
function nextTick(): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, 5));
}
/// A message that crossed the webview boundary, in this realm: the renderer runs
/// in its own context, so its objects are not reference-equal to a test's.
function shape<T>(value: T): T {
  return JSON.parse(JSON.stringify(value)) as T;
}

/// The last message the renderer posted, ready to compare.
function last(posted: Record<string, unknown>[]): Record<string, unknown> {
  return shape(posted[posted.length - 1]);
}

describe("webview footer", () => {
  it("paints one chip per footer chip, splitting the key from the value", () => {
    const { byId, send } = loadRenderer();
    send(stateMessage());
    const chips = byId.get("meta")!.children;
    assert.equal(chips.length, 5);
    assert.deepEqual(
      chips.map((chip) => chip.className),
      ["meta-chip", "meta-chip", "meta-chip", "meta-chip", "meta-chip"],
    );
    // `model: glm-5 · 128.0k` reads as a dim key and a value, not one string.
    assert.equal(chips[0].children[0].textContent, "model");
    assert.equal(chips[0].children[0].className, "chip-key");
    assert.equal(chips[0].children[1].textContent, "glm-5 · 128.0k");
    assert.equal(chips[0].children[1].className, "chip-value");
    assert.equal(chips[0].dataset.control, "setModel");
    assert.equal(chips[0].title, "Switch the model");
    assert.equal(chips[1].children[0].textContent, "thinking");
    assert.equal(chips[1].children[1].textContent, "auto");
  });

  it("keeps a label with no key whole", () => {
    const { byId, send } = loadRenderer();
    send(stateMessage({ footer: { ...footer, chips: [{ id: "setModel", label: "glm-5" }] } }));
    const chip = byId.get("meta")!.children[0];
    assert.equal(chip.children.length, 0);
    assert.equal(chip.textContent, "glm-5");
    assert.equal(chip.title, "glm-5 — click to change");
  });

  it("posts the chip's control id when it is clicked", () => {
    const { byId, posted, send } = loadRenderer();
    send(stateMessage());
    // A click lands on the chip's own span, as it does in the real DOM: the
    // event bubbles to the strip, and the handler reads the control id off the
    // chip around the target.
    const key = byId.get("meta")!.children[1].children[0];
    byId.get("meta")!.fire("click", { target: key });
    const message = last(posted);
    assert.equal(message.k, "control");
    assert.equal(message.control, "cycleReasoning");
  });

  it("paints the branch, the usage line and the gauge under the composer", () => {
    const { byId, send } = loadRenderer();
    send(stateMessage());
    assert.equal(byId.get("branch")!.textContent, "main");
    assert.equal(byId.get("branch")!.title, "Current branch: main");
    assert.equal(byId.get("usage-text")!.textContent, footer.usage);
    assert.equal(byId.get("usage")!.hidden, false);
    assert.equal(byId.get("gauge")!.hidden, false);
    assert.equal(byId.get("gauge")!.dataset.level, "ok");
    assert.equal(byId.get("gauge-fill")!.style.width, "12%");
  });

  it("hides the gauge and the usage line when there is nothing to show", () => {
    const { byId, send } = loadRenderer();
    send(stateMessage({ footer: { chips: [], info: "", percent: null } }));
    assert.equal(byId.get("gauge")!.hidden, true);
    assert.equal(byId.get("usage")!.hidden, true);
    assert.equal(byId.get("branch")!.textContent, "");
    assert.equal(byId.get("meta")!.children.length, 0);
  });

  it("escalates the gauge color at the terminal's thresholds", () => {
    const { byId, send } = loadRenderer();
    send(stateMessage({ footer: { ...footer, percent: 75, level: "warn" } }));
    assert.equal(byId.get("gauge")!.dataset.level, "warn");
    send(stateMessage({ footer: { ...footer, percent: 95, level: "high" } }));
    assert.equal(byId.get("gauge")!.dataset.level, "high");
  });

  it("never paints a gauge past its track", () => {
    const { byId, send } = loadRenderer();
    send(stateMessage({ footer: { ...footer, percent: 140 } }));
    assert.equal(byId.get("gauge-fill")!.style.width, "100%");
  });
});

describe("webview composer", () => {
  it("starts two rows tall", () => {
    assert.match(shell, /id="input" rows="2"/);
  });

  it("renders the header and composer actions as icon buttons", () => {
    for (const id of ["new-session", "resume-session", "attach", "send", "stop"]) {
      assert.match(shell, new RegExp(`id="${id}" class="icon`), id);
    }
  });

  it("enables Send only when there is something to send", () => {
    const { byId, send } = loadRenderer();
    send(stateMessage());
    const sendButton = byId.get("send")!;
    assert.equal(sendButton.disabled, true, "an empty composer cannot send");

    byId.get("input")!.value = "hello";
    byId.get("input")!.fire("input");
    assert.equal(sendButton.disabled, false);

    byId.get("input")!.value = "   ";
    byId.get("input")!.fire("input");
    assert.equal(sendButton.disabled, true, "whitespace is not a message");

    // A pending attachment is enough on its own, like the terminal's chips.
    send(
      stateMessage({
        attachments: [{ id: 7, label: "shot.png", kind: "image", preview: null, detail: "2 KB" }],
      }),
    );
    assert.equal(sendButton.disabled, false);
  });

  it("shows the thread title in the header", () => {
    const { byId, send } = loadRenderer();
    send(stateMessage({ title: "Fix the flaky test" }));
    assert.equal(byId.get("title")!.textContent, "Fix the flaky test");
    // A thread with no message yet falls back to a neutral placeholder.
    send(stateMessage({ title: "" }));
    assert.equal(byId.get("title")!.textContent, "New chat");
  });

  it("reads Queue and shows Stop while a turn runs", () => {
    const { byId, send } = loadRenderer();
    send(stateMessage({ busy: true, status: "Thinking…", queued: 2 }));
    assert.equal(byId.get("status")!.textContent, "Thinking… · 2 queued");
    assert.equal(byId.get("status")!.classList.contains("busy"), true);
    assert.equal(byId.get("status")!.hidden, false);
    assert.equal(byId.get("stop")!.hidden, false);
    assert.equal(byId.get("send")!.getAttribute("aria-label"), "Queue");
    // A busy composer can still queue, so Send stays live with an empty box.
    assert.equal(byId.get("send")!.disabled, false);

    // An idle turn has no phase to report, so the label (and its dot) goes
    // rather than sitting there reading like a control.
    send(stateMessage());
    assert.equal(byId.get("status")!.hidden, true);
    assert.equal(byId.get("stop")!.hidden, true);
    assert.equal(byId.get("send")!.getAttribute("aria-label"), "Send");
    assert.equal(byId.get("send")!.disabled, true);
  });

  it("renders a thumbnail for an image and a glyph for a PDF", () => {
    const { byId, send } = loadRenderer();
    send(
      stateMessage({
        context: [{ id: 1, label: "src/main.rs" }],
        attachments: [
          {
            id: 2,
            label: "shot.png",
            kind: "image",
            preview: "data:image/png;base64,QUJD",
            detail: "12 KB · pasted",
          },
          { id: 3, label: "spec.pdf", kind: "pdf", preview: null, detail: "1.4 MB" },
        ],
      }),
    );
    const chips = byId.get("chips")!;
    assert.equal(chips.hidden, false);
    // The attachments come first: they are what the message carries.
    assert.deepEqual(
      chips.children.map((chip) => chip.className),
      ["chip attachment", "chip attachment", "chip context", "chip-clear"],
    );
    // An image's thumbnail sits in a button: a chip has room for a 96px copy,
    // so a click opens the full-size image.
    const open = chips.children[0].children[0];
    assert.equal(open.tagName, "button");
    assert.equal(open.className, "chip-open");
    assert.equal(open.children[0].tagName, "img");
    assert.equal(open.children[0].src, "data:image/png;base64,QUJD");
    assert.equal(chips.children[0].children[1].children[0].textContent, "shot.png");
    assert.equal(chips.children[0].children[1].children[1].textContent, "12 KB · pasted");
    // A PDF has no thumbnail, so it falls back to its glyph and name.
    assert.equal(chips.children[1].children[0].className, "chip-glyph");
    assert.equal(chips.children[1].children[0].textContent, "▤");
    assert.equal(chips.children[2].children[1].textContent, "src/main.rs");
  });

  it("opens the full-size image from a thumbnail, and closes it again", () => {
    const { byId, fireDocument, send } = loadRenderer();
    const preview = "data:image/png;base64,QUJDRA==";
    send(
      stateMessage({
        attachments: [{ id: 2, label: "shot.png", kind: "image", preview, detail: "12 KB" }],
      }),
    );
    assert.equal(byId.get("image-view")!.hidden, true);
    byId.get("chips")!.children[0].children[0].fire("click");
    assert.equal(byId.get("image-view")!.hidden, false);
    assert.equal(byId.get("image-view-img")!.src, preview);
    assert.equal(byId.get("image-view-img")!.alt, "shot.png");

    // Clicking the backdrop, then Escape, closes it; the thumbnail itself only
    // opens it, so a click there while it is open leaves it open.
    byId.get("image-view")!.fire("click", { target: byId.get("image-view") });
    assert.equal(byId.get("image-view")!.hidden, true);
    byId.get("chips")!.children[0].children[0].fire("click");
    fireDocument("keydown", { key: "Escape" });
    assert.equal(byId.get("image-view")!.hidden, true);
  });

  it("keeps the chip-sized copy of a thumbnail for the repaints", async () => {
    const { byId, send } = loadRenderer();
    const attachment = {
      id: 2,
      label: "photo.jpg",
      kind: "image",
      preview: "data:image/png;base64,U0hPVC1ZT1VH",
      detail: "4.2 MB · pasted",
    };
    send(stateMessage({ attachments: [attachment] }));
    // The first paint shows what the host sent, so the chip is never empty...
    const first = byId.get("chips")!.children[0].children[0].children[0];
    assert.equal(first.src, attachment.preview);
    // ...and the canvas copy it drew in the background replaces it.
    await new Promise((resolve) => setTimeout(resolve, 10));
    assert.equal(first.src, "data:image/png;base64,QUJD");
    // Every later repaint starts from the small copy instead of decoding the
    // photo-sized one again.
    send(stateMessage({ attachments: [attachment] }));
    assert.equal(
      byId.get("chips")!.children[0].children[0].children[0].src,
      "data:image/png;base64,QUJD",
    );
  });

  it("starts one decode when a repaint arrives before the thumbnail is ready", async () => {
    const { send, readable } = loadRenderer();
    const attachment = {
      id: 3,
      label: "big.png",
      kind: "image",
      preview: "data:image/png;base64,QUJDRA==",
      detail: "9.1 MB · pasted",
    };
    // Both paints land in the same tick, before the first decode can finish.
    send(stateMessage({ attachments: [attachment] }));
    send(stateMessage({ attachments: [attachment] }));
    await new Promise((resolve) => setTimeout(resolve, 10));
    assert.equal(readable().image.loads, 1, "the repaint reused the decode in flight");
  });

  it("refuses a paste past the attachment limit before reading it", async () => {
    const { byId, posted } = loadRenderer();
    const file = { name: "huge.png", type: "image/png", size: 30 * 1024 * 1024 };
    byId.get("input")!.fire("paste", {
      clipboardData: { items: [{ kind: "file", getAsFile: () => file }] },
      preventDefault: () => {},
    });
    await new Promise((resolve) => setTimeout(resolve, 10));
    assert.deepEqual(last(posted), {
      k: "notice",
      text: "huge.png is past the 20 MB attachment limit.",
    });
    assert.ok(!posted.some((message) => message.k === "attach"), "the blob was never read");
  });

  it("hides the strip when nothing is pending", () => {
    const { byId, send } = loadRenderer();
    send(stateMessage());
    assert.equal(byId.get("chips")!.hidden, true);
  });

  it("removes one chip by id, and clears them all", () => {
    const { byId, posted, send } = loadRenderer();
    send(
      stateMessage({
        context: [{ id: 1, label: "src/main.rs" }],
        attachments: [{ id: 2, label: "shot.png", kind: "image", preview: null, detail: "2 KB" }],
      }),
    );
    const chips = byId.get("chips")!;
    chips.children[0].children[2].fire("click");
    assert.deepEqual(last(posted), { k: "removeChip", id: 2 });
    // The context chip is removed the same way: one id space, one message.
    chips.children[1].children[2].fire("click");
    assert.deepEqual(last(posted), { k: "removeChip", id: 1 });
    chips.children[2].fire("click");
    assert.deepEqual(last(posted), { k: "clearChips" });
  });

  it("offers Clear only once there is more than one chip", () => {
    const { byId, send } = loadRenderer();
    send(stateMessage({ context: [{ id: 1, label: "src/main.rs" }] }));
    assert.deepEqual(
      byId.get("chips")!.children.map((chip) => chip.className),
      ["chip context"],
    );
  });

  it("sends the message and empties the box", () => {
    const { byId, posted, send } = loadRenderer();
    send(stateMessage());
    const input = byId.get("input")!;
    input.value = "explain this repo";
    input.fire("input");
    byId.get("send")!.fire("click");
    assert.deepEqual(last(posted), { k: "send", text: "explain this repo" });
    assert.equal(input.value, "");
    assert.equal(byId.get("send")!.disabled, true);
  });

  it("sends with Enter and keeps the newline with Shift+Enter", () => {
    const { byId, posted, send } = loadRenderer();
    send(stateMessage());
    const input = byId.get("input")!;
    input.value = "first\nsecond";
    input.fire("input");

    let prevented = false;
    const preventDefault = () => {
      prevented = true;
    };
    input.fire("keydown", { key: "Enter", shiftKey: true, preventDefault });
    assert.equal(prevented, false, "Shift+Enter is a newline");
    assert.equal(
      posted.some((message) => message.k === "send"),
      false,
    );

    input.fire("keydown", { key: "Enter", shiftKey: false, preventDefault });
    assert.equal(prevented, true);
    assert.equal(last(posted).k, "send");
    assert.equal(last(posted).text, "first\nsecond");
  });

  it("stops the run on Escape while busy", () => {
    const { byId, posted, send } = loadRenderer();
    send(stateMessage({ busy: true }));
    byId.get("input")!.fire("keydown", { key: "Escape", preventDefault: () => {} });
    assert.equal(last(posted).k, "stop");
  });

  it("asks for the file picker from the Attach button", () => {
    const { byId, posted, send } = loadRenderer();
    send(stateMessage());
    byId.get("attach")!.fire("click");
    assert.deepEqual(last(posted), { k: "pickFiles" });
  });

  it("turns a pasted image into an attachment and leaves a text paste alone", async () => {
    const { byId, posted, send } = loadRenderer();
    send(stateMessage());
    const input = byId.get("input")!;

    // A text paste is the message itself, so the renderer must not touch it.
    input.fire("paste", { clipboardData: { items: [{ kind: "string" }] }, preventDefault: () => {} });
    assert.equal(posted.some((message) => message.k === "attach"), false);

    input.fire("paste", {
      clipboardData: { items: [{ kind: "file", getAsFile: () => ({ type: "image/png", name: "shot.png" }) }] },
      preventDefault: () => {},
    });
    await nextTick();
    const attach = posted.find((message) => message.k === "attach");
    assert.ok(attach, "the pasted image is sent to the host");
    assert.equal(attach.name, "shot.png");
    assert.equal(attach.data, "data:image/png;base64,QUJD");
  });

  it("downscales a pasted screenshot before sending it on", async () => {
    const image = new StubImage();
    image.width = 2400;
    image.height = 1200;
    const { byId, posted, readable, send } = loadRenderer({ image });
    send(stateMessage());
    byId.get("input")!.fire("paste", {
      clipboardData: {
        items: [{ kind: "file", getAsFile: () => ({ type: "image/png", name: "retina.png" }) }],
      },
      preventDefault: () => {},
    });
    await nextTick();
    const canvas = readable().canvas;
    assert.ok(canvas, "the image was drawn on a canvas");
    assert.equal(canvas.width, 1568);
    assert.equal(canvas.height, 784);
    assert.equal(canvas.drawn, 1);
    const attach = posted.find((message) => message.k === "attach");
    assert.equal(attach?.data, "data:image/png;base64,QUJD");
  });

  it("sends an image it cannot decode as it is, rather than dropping it", async () => {
    // A `src` the webview cannot load leaves the image's size unknown.
    const unreadable = new StubImage();
    unreadable.width = 0;
    unreadable.height = 0;
    const { byId, posted, readable, send } = loadRenderer({ image: unreadable });
    send(stateMessage());
    byId.get("input")!.fire("paste", {
      clipboardData: {
        items: [{ kind: "file", getAsFile: () => ({ type: "image/png", name: "big.png" }) }],
      },
      preventDefault: () => {},
    });
    await nextTick();
    assert.equal(readable().canvas, null);
    assert.equal(posted.find((message) => message.k === "attach")?.data, "data:image/png;base64,QUJD");
  });

  it("refuses a paste it cannot attach, and says so", () => {
    const { byId, posted, send } = loadRenderer();
    send(stateMessage());
    byId.get("input")!.fire("paste", {
      clipboardData: { items: [{ kind: "file", getAsFile: () => ({ type: "text/plain", name: "a.txt" }) }] },
      preventDefault: () => {},
    });
    assert.equal(posted.some((message) => message.k === "attach"), false);
    assert.deepEqual(last(posted), {
      k: "notice",
      text: "Only images and PDFs can be attached from a paste or a drop.",
    });
  });

  it("shows the drop overlay while files are dragged over it", () => {
    const { byId, fireDocument, send } = loadRenderer();
    send(stateMessage());
    const composer = byId.get("composer")!;
    assert.equal(byId.get("dropzone")!.hidden, true);

    fireDocument("dragenter", { dataTransfer: { types: ["Files"] } });
    assert.equal(composer.classList.contains("dragging"), true);
    assert.equal(byId.get("dropzone")!.hidden, false);

    // Crossing a child element does not drop the overlay...
    fireDocument("dragleave", { relatedTarget: byId.get("input") });
    assert.equal(composer.classList.contains("dragging"), true);
    // ...leaving the view does.
    fireDocument("dragleave", { relatedTarget: null });
    assert.equal(composer.classList.contains("dragging"), false);
    assert.equal(byId.get("dropzone")!.hidden, true);
  });

  it("ignores a drag that carries no files", () => {
    const { byId, fireDocument, send } = loadRenderer();
    send(stateMessage());
    fireDocument("dragenter", { dataTransfer: { types: ["text/plain"] } });
    assert.equal(byId.get("composer")!.classList.contains("dragging"), false);
  });

  it("attaches dropped paths as paths, and a pathless blob as a data URL", async () => {
    const { byId, fireDocument, posted, send } = loadRenderer();
    send(stateMessage());
    fireDocument("dragenter", { dataTransfer: { types: ["Files"] } });
    fireDocument("drop", {
      dataTransfer: {
        files: [
          { type: "text/plain", name: "notes.txt", path: "/tmp/notes.txt" },
          { type: "image/png", name: "shot.png" },
        ],
      },
      preventDefault: () => {},
    });
    await nextTick();
    assert.deepEqual(shape(posted.find((message) => message.k === "attachFiles")), {
      k: "attachFiles",
      paths: ["/tmp/notes.txt"],
    });
    const attach = posted.find((message) => message.k === "attach");
    assert.equal(attach?.name, "shot.png");
    assert.equal(attach?.data, "data:image/png;base64,QUJD");
    // The drop ends the drag, so the overlay does not stick.
    assert.equal(byId.get("dropzone")!.hidden, true);
    assert.equal(byId.get("composer")!.classList.contains("dragging"), false);
  });
});

describe("webview dialogs", () => {
  /// The two listings the host composes (`src/core/dialogs.ts`): the MCP servers
  /// with the button that turns each one over, and the session history with the
  /// rows that resume one. A row arrives with the action it posts.
  const mcp = {
    k: "dialog",
    dialog: {
      pin: "footer",
      title: "MCP servers",
      subtitle: "The servers this project loads, and whether Oxide can reach them.",
      note: "",
      rows: [
        {
          value: "context7",
          label: "context7",
          detail: "http · source: claude (global)",
          status: "Connected",
          tone: "ok",
          action: "",
          button: "Disable context7",
          buttonAction: "mcpToggle",
          icon: "power",
        },
        {
          value: "sentry",
          label: "sentry",
          detail: "stdio · source: project",
          status: "Disabled",
          tone: "muted",
          action: "",
          button: "Enable sentry",
          buttonAction: "mcpToggle",
          icon: "power",
        },
      ],
      refreshLabel: "Recheck",
      refreshAction: "mcpRefresh",
    },
  };
  const sessions = {
    k: "dialog",
    dialog: {
      pin: "header",
      title: "Sessions",
      subtitle: "Threads stored for this project.",
      note: "",
      rows: [
        {
          value: "new",
          label: "New chat",
          detail: "Close this thread and start a fresh one",
          status: "",
          tone: "",
          action: "openSession",
          button: "",
          buttonAction: "",
          icon: "",
        },
        {
          value: "fe0031b1",
          label: "Fix the flaky test",
          detail: "195 messages",
          status: "Current",
          tone: "muted",
          action: "openSession",
          button: "Delete Fix the flaky test",
          buttonAction: "sessionDelete",
          icon: "trash",
        },
      ],
      refreshLabel: "",
      refreshAction: "",
    },
  };

  const confirmDelete = {
    k: "dialog",
    dialog: {
      pin: "header",
      title: "Delete thread",
      subtitle: "“Fix the flaky test” and its stored conversation are removed.",
      note: "This cannot be undone.",
      rows: [
        {
          value: "fe0031b1",
          label: "Delete thread",
          detail: "just now · 195 messages · fe0031b1",
          status: "",
          tone: "error",
          action: "sessionDeleteConfirm",
          button: "",
          buttonAction: "",
          icon: "",
        },
        {
          value: "",
          label: "Cancel",
          detail: "Keep the thread",
          status: "",
          tone: "",
          action: "dialogClose",
          button: "",
          buttonAction: "",
          icon: "",
        },
      ],
      refreshLabel: "",
      refreshAction: "",
    },
  };

  it("paints the rows the host composed", () => {
    const { byId, send } = loadRenderer();
    assert.equal(byId.get("dialog")!.hidden, true);
    send(mcp);
    assert.equal(byId.get("dialog")!.hidden, false);    assert.equal(byId.get("dialog-title")!.textContent, "MCP servers");
    assert.equal(
      byId.get("dialog-sub")!.textContent,
      "The servers this project loads, and whether Oxide can reach them.",
    );
    assert.equal(byId.get("dialog-note")!.hidden, true);

    const rows = byId.get("dialog-list")!.children;
    assert.equal(rows.length, 2);
    assert.equal(rows[0].className, "dialog-row");
    // The name, what it resolves to, and the state it is in.
    assert.equal(rows[0].children[0].children[0].textContent, "context7");
    assert.equal(rows[0].children[0].children[1].textContent, "http · source: claude (global)");
    assert.equal(rows[0].children[1].className, "dialog-status tone-ok");
    assert.equal(rows[0].children[1].textContent, "Connected");
    // The `Recheck` button is an icon the shell carries, so what the host sent
    // is its tooltip and its name for a screen reader.
    assert.equal(byId.get("dialog-refresh")!.textContent, "");
    assert.equal(byId.get("dialog-refresh")!.title, "Recheck");
    assert.equal(byId.get("dialog-refresh")!.dataset.action, "mcpRefresh");
    assert.equal(byId.get("dialog-refresh")!.hidden, false);
  });

  /// A row's own button carries a switch rather than a word, so it is one glyph
  /// wide and says what it does in its tooltip — the panel is a narrow side bar,
  /// and `Disable` on every row is a column of text where a switch will do.
  it("paints each listing on the edge its host pinned it to", () => {
    const { byId, send } = loadRenderer();
    // The host decides — `dialogs.ts` pins the server list to the composer and
    // the session history to the header — and the renderer only carries it: the
    // class is what the stylesheet moves and turns around. Read the name from
    // the stylesheet rather than spelling it a second time here, since a class
    // the two sides disagree about would leave the listing under the header.
    const pinned = /#dialog\.([\w-]+) \{/.exec(style)?.[1];
    assert.equal(pinned, "pin-footer");
    send(mcp);
    assert.equal(byId.get("dialog")!.classList.contains(pinned!), true);
    send(sessions);
    assert.equal(byId.get("dialog")!.classList.contains(pinned!), false);
    // And back, since both panes paint whichever listing the controller holds.
    send(mcp);
    assert.equal(byId.get("dialog")!.classList.contains(pinned!), true);
    // Closing one takes the pin with it: the element is hidden, but its siblings
    // are still ordered around it until the next listing says where it goes.
    send({ k: "dialog", dialog: null });
    assert.equal(byId.get("dialog")!.hidden, true);
    assert.equal(byId.get("dialog")!.classList.contains(pinned!), false);
  });

  it("paints a row's button as an icon with the host's label on it", () => {
    const { byId, send } = loadRenderer();
    send(mcp);

    const button = byId.get("dialog-list")!.children[0].children[2];
    assert.equal(button.className, "dialog-button icon-power tone-ok");
    // The stub keeps markup as text, so the SVG it was painted with is what its
    // content reads as; the words the host sent are on the button, not in it.
    assert.ok(button.innerHTML.includes("<svg"), button.innerHTML);
    assert.equal(button.title, "Disable context7");
    assert.equal(button.dataset.action, "mcpToggle");
    assert.equal(button.dataset.value, "context7");

    // A server that is off carries the same switch in the muted tone.
    const off = byId.get("dialog-list")!.children[1].children[2];
    assert.equal(off.className, "dialog-button icon-power tone-muted");
    assert.equal(off.title, "Enable sentry");
  });

  it("posts the toggle a row's button carries", () => {
    const { byId, posted, send } = loadRenderer();
    send(mcp);
    // A click on the button bubbles to the dialog the handler is on, which reads
    // the action off the element around the target.
    const button = byId.get("dialog-list")!.children[1].children[2];
    assert.equal(button.dataset.action, "mcpToggle");
    byId.get("dialog")!.fire("click", { target: button });
    assert.deepEqual(last(posted), {
      k: "dialogAction",
      action: "mcpToggle",
      value: "sentry",
    });
  });

  it("posts the action of the row a click lands in", () => {
    const { byId, posted, send } = loadRenderer();
    send(sessions);
    const rows = byId.get("dialog-list")!.children;
    // A click inside a row walks up to it, the way an event bubbles.
    assert.equal(rows[1].tagName, "button");
    byId.get("dialog")!.fire("click", { target: rows[1].children[0].children[0] });
    assert.deepEqual(last(posted), {
      k: "dialogAction",
      action: "openSession",
      value: "fe0031b1",
    });
    // The two rows the host adds itself post their own values.
    byId.get("dialog")!.fire("click", { target: rows[0] });
    assert.deepEqual(last(posted), { k: "dialogAction", action: "openSession", value: "new" });
    // A session listing has nothing to recheck, so the button is left off.
    assert.equal(byId.get("dialog-refresh")!.hidden, true);
  });

  it("paints the trash a session row carries, and posts what it means", () => {
    const { byId, posted, send } = loadRenderer();
    send(sessions);
    const row = byId.get("dialog-list")!.children[1];
    const button = row.children[2];
    assert.equal(button.className, "dialog-button icon-trash tone-muted");
    assert.ok(button.innerHTML.includes("<svg"), button.innerHTML);
    assert.equal(button.title, "Delete Fix the flaky test");
    assert.equal(button.dataset.action, "sessionDelete");
    assert.equal(button.dataset.value, "fe0031b1");
    // The row itself still resumes the thread; the trash is what deletes it.
    assert.equal(row.dataset.action, "openSession");

    byId.get("dialog")!.fire("click", { target: button });
    assert.deepEqual(last(posted), {
      k: "dialogAction",
      action: "sessionDelete",
      value: "fe0031b1",
    });
  });

  it("paints the confirmation and posts the row that deletes", () => {
    const { byId, posted, send } = loadRenderer();
    send(confirmDelete);
    assert.equal(byId.get("dialog-title")!.textContent, "Delete thread");
    const rows = byId.get("dialog-list")!.children;
    assert.deepEqual(
      rows.map((row) => row.dataset.action),
      ["sessionDeleteConfirm", "dialogClose"],
    );
    byId.get("dialog")!.fire("click", { target: rows[0] });
    assert.deepEqual(last(posted), {
      k: "dialogAction",
      action: "sessionDeleteConfirm",
      value: "fe0031b1",
    });
    byId.get("dialog")!.fire("click", { target: rows[1] });
    assert.deepEqual(last(posted), { k: "dialogAction", action: "dialogClose", value: "" });
  });

  it("closes on the Close button and on Escape", () => {
    const { byId, fireDocument, posted, send } = loadRenderer();
    send(sessions);
    byId.get("dialog-close")!.fire("click");
    assert.equal(byId.get("dialog")!.hidden, true);
    assert.deepEqual(last(posted), { k: "dialogAction", action: "dialogClose", value: "" });

    // The listing hangs from the header rather than covering the panel, so a
    // click inside it — or on the padding around its rows — keeps it up: closing
    // it is the Close button, Escape, or picking a row.
    send(sessions);
    byId.get("dialog")!.fire("click", { target: byId.get("dialog-list") });
    assert.equal(byId.get("dialog")!.hidden, false);
    byId.get("dialog")!.fire("click", { target: byId.get("dialog") });
    assert.equal(byId.get("dialog")!.hidden, false);
    assert.deepEqual(last(posted), { k: "dialogAction", action: "dialogClose", value: "" });

    send(sessions);
    fireDocument("keydown", { key: "Escape" });
    assert.equal(byId.get("dialog")!.hidden, true);
    // The host is told, so the pane that attaches next does not paint it again.
    assert.equal(last(posted).action, "dialogClose");
  });

  it("leaves Escape to the composer when no dialog is up", () => {
    const { byId, fireDocument, posted, send } = loadRenderer();
    send(stateMessage({ busy: true }));
    byId.get("input")!.fire("keydown", { key: "Escape", preventDefault: () => {} });
    fireDocument("keydown", { key: "Escape" });
    // The turn is stopped once, and nothing is posted about a dialog.
    assert.deepEqual(last(posted), { k: "stop" });
  });

  it("clears the dialog when the host closes it", () => {
    const { byId, send } = loadRenderer();
    send(mcp);
    send({ k: "dialog", dialog: null });
    assert.equal(byId.get("dialog")!.hidden, true);
    assert.equal(byId.get("dialog-list")!.children.length, 0);
  });

  it("shows a note where the list has nothing to show", () => {
    const { byId, send } = loadRenderer();
    send({ k: "dialog", dialog: { ...mcp.dialog, rows: [], note: "Could not list MCP servers: exit 1" } });
    assert.equal(byId.get("dialog-note")!.hidden, false);
    assert.equal(byId.get("dialog-note")!.textContent, "Could not list MCP servers: exit 1");
    assert.equal(byId.get("dialog-list")!.children.length, 0);
    assert.equal(byId.get("dialog")!.hidden, false, "the dialog still says what happened");
  });

  /// The listing is part of the panel's own column rather than a window over it:
  /// it sits between the header and the transcript, which is what makes it hang
  /// from the header and stay attached to it, with a row list that scrolls. It
  /// is also the one part of the column that never gives up height — a dialog
  /// squeezed down to a sliver of scrolling rows is what the flex there was for.
  it("anchors the dialog to the header and gives its buttons icons", () => {
    const at = (needle: string) => shell.indexOf(needle);
    assert.ok(at("</header>") < at('id="dialog"'), "the dialog comes after the header");
    assert.ok(at('id="dialog"') < at('id="transcript"'), "and before the transcript");
    assert.ok(
      shell.includes('<section id="dialog" class="popover"'),
      "it is a popover, not an overlay",
    );
    // The transcript below is a scroll container whose content is thousands of
    // pixels tall and whose base size is that content: a dialog that may shrink
    // loses that contest and opens a couple of rows tall, which is the bug this
    // pins. The cap carries a px floor so the footer stays on screen too.
    const rule = /#dialog \{[^}]*\}/.exec(style)?.[0] ?? "";
    assert.match(rule, /flex: 0 0 auto;/, "the dialog does not shrink");
    assert.match(rule, /max-height: min\([^)]*\d+px[^)]*\)/, "its cap leaves room for the footer");
    assert.match(rule, /border-top: none;/, "the header's border closes the sheet");
    // Icon-only, with the words as the tooltip and the accessible name: the
    // shell paints each one from its own `ICONS` map.
    for (const [id, label, icon] of [
      ["dialog-refresh", "Recheck", "refresh"],
      ["dialog-close", "Close", "close"],
    ]) {
      const start = at(`id="${id}"`);
      const button = shell.slice(shell.lastIndexOf("<button", start), shell.indexOf("</button>", start));
      assert.ok(button.includes(`title="${label}"`), button);
      assert.ok(button.includes(`aria-label="${label}"`), button);
      assert.ok(button.includes(`\${ICONS.${icon}}`), button);
      assert.match(shell, new RegExp(`${icon}: ` + "`<svg"), `${icon} is drawn in ICONS`);
    }
  });

  /// One element, two anchors: the markup keeps the position under the header
  /// that a header-pinned listing drops from, and `.pin-footer` moves the same
  /// node above the footer with `order`, turning the sheet's shape around with
  /// it. The transcript is ordered between the two so the listing takes its room
  /// from the transcript in either direction.
  it("turns the dialog around for the listing that belongs to the composer", () => {
    const pinned = /#dialog\.pin-footer \{[^}]*\}/.exec(style)?.[0] ?? "";
    assert.match(pinned, /order: 2;/, "it is placed after the transcript");
    assert.match(pinned, /border-bottom: none;/, "the footer's border closes the sheet");
    assert.match(pinned, /border-top: var\(--dialog-edge\);/, "and its own top edge comes back");
    assert.match(pinned, /border-radius: 8px 8px 0 0;/, "square where it meets the composer");
    assert.match(pinned, /box-shadow: 0 -10px/, "the shadow is cast upward");
    assert.match(pinned, /margin: 0 8px -1px;/, "its bottom edge overlaps the footer's border");

    const order = (selector: string) =>
      new RegExp(`#dialog\\.pin-footer ~ ${selector} \\{\\s*order: (\\d)`).exec(style)?.[1];
    assert.equal(order("#transcript"), "1", "the transcript keeps its place");
    assert.equal(order("footer"), "3", "so does the footer, after both");
  });
});

describe("webview approvals", () => {
  /// The card the CLI's `approval_request` becomes: the tool, what it would do,
  /// the command or path it would touch, and the three answers.
  const pending = {
    id: 9,
    kind: "approval",
    requestId: 12,
    tool: "bash",
    title: "Run a shell command",
    detail: "rm -rf build",
    state: "pending",
    label: "",
  };

  it("offers the three answers for a waiting request", () => {
    const { byId, send } = loadRenderer();
    send(stateMessage());
    const transcript = byId.get("transcript")!;
    send({ k: "push", item: pending });

    assert.equal(find(transcript, "atool")!.textContent, "bash");
    assert.equal(find(transcript, "asub")!.textContent, "Run a shell command");
    assert.equal(find(transcript, "adetail")!.textContent, "rm -rf build");
    const actions = find(transcript, "aactions")!;
    assert.deepEqual(
      actions.children.map((child) => child.textContent),
      ["Deny", "Allow once", "Always allow", "Waiting for your answer…"],
    );
  });

  it("hides an empty detail rather than leaving a blank block", () => {
    const { byId, send } = loadRenderer();
    send(stateMessage());
    const transcript = byId.get("transcript")!;
    send({ k: "push", item: { ...pending, detail: "" } });
    assert.equal(find(transcript, "adetail")!.hidden, true);
  });

  /// The answer itself is the host's to send: the webview only names the
  /// request and the decision it was answered with.
  it("posts the request id and the decision a button was clicked with", () => {
    const { byId, send, posted } = loadRenderer();
    send(stateMessage());
    const transcript = byId.get("transcript")!;
    send({ k: "push", item: pending });

    const buttons = find(transcript, "aactions")!.children;
    buttons[2].fire("click");
    assert.deepEqual(shape(posted[posted.length - 1]), {
      k: "approval",
      requestId: 12,
      decision: "always",
    });

    buttons[0].fire("click");
    assert.deepEqual(shape(posted[posted.length - 1]), {
      k: "approval",
      requestId: 12,
      decision: "deny",
    });
  });

  /// An answered card keeps only what it was answered with, so a stale button
  /// can never send a second answer for a request the CLI has moved past.
  it("replaces the buttons with the answer, and repaints it from a state replay", () => {
    const { byId, send } = loadRenderer();
    send(stateMessage());
    const transcript = byId.get("transcript")!;
    send({ k: "push", item: pending });
    send({ k: "approval", id: 9, state: "always", label: "Always allowed in this project" });

    const actions = find(transcript, "aactions")!;
    assert.deepEqual(
      actions.children.map((child) => child.textContent),
      ["Always allowed in this project"],
    );

    // A view that attaches later is painted from the transcript state.
    const second = loadRenderer();
    second.send(stateMessage({ items: [{ ...pending, state: "always", label: "Always allowed in this project" }] }));
    const replayed = find(second.byId.get("transcript")!, "aactions")!;
    assert.deepEqual(
      replayed.children.map((child) => child.textContent),
      ["Always allowed in this project"],
    );
  });

  it("settles a card whose run ended without an answer", () => {
    const { byId, send } = loadRenderer();
    send(stateMessage());
    const transcript = byId.get("transcript")!;
    send({ k: "push", item: pending });
    send({ k: "approval", id: 9, state: "closed", label: "Not answered" });
    assert.deepEqual(
      find(transcript, "aactions")!.children.map((child) => child.textContent),
      ["Not answered"],
    );
  });
});

describe("webview questions", () => {
  /// The card the CLI's `question_request` becomes: one block per question, each
  /// with its options and a field for an answer of the user's own wording.
  const pending = {
    id: 4,
    kind: "question",
    requestId: 21,
    title: "Database",
    questions: [
      {
        question: "Which database?",
        header: "Database",
        options: [
          { label: "Postgres", description: "Relational" },
          { label: "SQLite", description: "Embedded" },
        ],
        multiSelect: false,
      },
    ],
    state: "pending",
    label: "",
  };

  /// The card and every answer field in it, in the order they were painted.
  const fields = (card: StubElement) => card.querySelectorAll(".qchoice");

  it("paints a radio group and a free-text field for a single choice", () => {
    const { byId, send } = loadRenderer();
    send(stateMessage());
    const transcript = byId.get("transcript")!;
    send({ k: "push", item: pending });

    const card = find(transcript, "question")!;
    assert.equal(find(card, "qtitle")!.textContent, "Database");
    assert.equal(find(card, "qtext")!.textContent, "Which database?");
    const choices = fields(card);
    assert.deepEqual(
      choices.map((choice) => [choice.type, choice.value]),
      [
        ["radio", "Postgres"],
        ["radio", "SQLite"],
      ],
    );
    assert.deepEqual(
      card.querySelectorAll(".qdetail").map((detail) => detail.textContent),
      ["Relational", "Embedded"],
    );
    // A single-select question preselects its first option, so an answer can
    // never be blank by accident.
    assert.deepEqual(
      choices.map((choice) => choice.checked),
      [true, false],
    );
    const free = find(card, "qfree")!;
    assert.equal(free.placeholder, "Or type an answer…");
    assert.deepEqual(
      find(card, "qactions")!.children.map((child) => child.textContent),
      ["Answer", "Skip", "Waiting for your answer…"],
    );
  });

  it("paints boxes and a hint that there is more than one answer", () => {
    const { byId, send } = loadRenderer();
    send(stateMessage());
    const transcript = byId.get("transcript")!;
    send({
      k: "push",
      item: {
        ...pending,
        questions: [{ ...pending.questions[0], multiSelect: true }],
      },
    });
    const card = find(transcript, "question")!;
    assert.deepEqual(
      fields(card).map((choice) => choice.type),
      ["checkbox", "checkbox"],
    );
    // A multi-select question starts with nothing ticked.
    assert.deepEqual(
      fields(card).map((choice) => choice.checked),
      [false, false],
    );
  });

  it("asks for free text where no options are offered", () => {
    const { byId, send } = loadRenderer();
    send(stateMessage());
    const transcript = byId.get("transcript")!;
    send({
      k: "push",
      item: {
        ...pending,
        title: "What should it be called?",
        questions: [{ question: "What should it be called?", header: "", options: [], multiSelect: false }],
      },
    });
    const card = find(transcript, "question")!;
    assert.deepEqual(fields(card), []);
    assert.equal(find(card, "qfree")!.placeholder, "Type an answer…");
    assert.equal(find(card, "qtitle")!.textContent, "What should it be called?");
  });

  /// The answers are the host's to send: the view names the request and what
  /// was ticked and typed, and the question each answer belongs to.
  it("posts the ticks and the typed text as the answers", () => {
    const { byId, send, posted } = loadRenderer();
    send(stateMessage());
    const transcript = byId.get("transcript")!;
    send({
      k: "push",
      item: {
        ...pending,
        questions: [
          { ...pending.questions[0], multiSelect: true },
          { question: "Anything else?", header: "", options: [], multiSelect: false },
        ],
      },
    });
    const card = find(transcript, "question")!;
    const choices = fields(card);
    choices[1].checked = true;
    const free = card.querySelectorAll(".qfree");
    free[0].value = "  and fast  ";
    free[1].value = "ship it";

    find(card, "qactions")!.children[0].fire("click");
    assert.deepEqual(shape(posted[posted.length - 1]), {
      k: "question",
      requestId: 21,
      answers: [
        { question: "Which database?", values: ["SQLite", "and fast"] },
        { question: "Anything else?", values: ["ship it"] },
      ],
    });
  });

  it("sends an empty answer list for Skip, which is a dismissal", () => {
    const { byId, send, posted } = loadRenderer();
    send(stateMessage());
    const transcript = byId.get("transcript")!;
    send({ k: "push", item: pending });
    find(transcript, "qactions")!.children[1].fire("click");
    assert.deepEqual(shape(posted[posted.length - 1]), {
      k: "question",
      requestId: 21,
      answers: [],
    });
  });

  it("replaces the fields with the answers, and repaints it from a state replay", () => {
    const { byId, send } = loadRenderer();
    send(stateMessage());
    const transcript = byId.get("transcript")!;
    send({ k: "push", item: pending });
    send({ k: "question", id: 4, state: "answered", label: "Postgres" });

    const card = find(transcript, "question")!;
    assert.deepEqual(fields(card), []);
    assert.deepEqual(
      find(card, "qactions")!.children.map((child) => child.textContent),
      ["Postgres"],
    );

    const second = loadRenderer();
    second.send(stateMessage({ items: [{ ...pending, state: "closed", label: "Not answered" }] }));
    assert.deepEqual(
      find(second.byId.get("transcript")!, "qactions")!.children.map((child) => child.textContent),
      ["Not answered"],
    );
  });
});

describe("webview scrolling", () => {
  /// A reply arrives as a delta per model chunk and is painted on the next
  /// animation frame, so the view has to follow the height the paint adds rather
  /// than the one it had when the delta arrived.
  it("follows a streamed reply to its last line", async () => {
    const { byId, send } = loadRenderer();
    send(stateMessage());
    const transcript = byId.get("transcript")!;
    transcript.clientHeight = 100;

    send({ k: "push", item: { id: 1, kind: "assistant", text: "" } });
    for (let chunk = 0; chunk < 4; chunk += 1) {
      send({ k: "append", id: 1, field: "text", delta: "a line of the reply. ".repeat(20) });
      await nextTick();
    }
    assert.ok(transcript.scrollHeight > transcript.clientHeight, "the reply outgrows the pane");
    assert.equal(transcript.scrollTop, transcript.scrollHeight);
  });

  it("stays put for a reader who scrolled away, and follows again from the bottom", async () => {
    const { byId, send } = loadRenderer();
    send(stateMessage());
    const transcript = byId.get("transcript")!;
    transcript.clientHeight = 100;
    send({ k: "push", item: { id: 1, kind: "assistant", text: "" } });
    send({ k: "append", id: 1, field: "text", delta: "a line of the reply. ".repeat(20) });
    await nextTick();

    // Reading something further up: the output must not pull the view back.
    transcript.scrollTop = 0;
    send({ k: "append", id: 1, field: "text", delta: "another line. ".repeat(20) });
    await nextTick();
    assert.equal(transcript.scrollTop, 0);

    // Back at the bottom, the view follows once more.
    transcript.scrollTop = transcript.scrollHeight;
    send({ k: "append", id: 1, field: "text", delta: "one line more. ".repeat(20) });
    await nextTick();
    assert.equal(transcript.scrollTop, transcript.scrollHeight);
  });

  /// A running command's body is its own scroll container capped at 40vh, and
  /// writing its text back resets it to the top: a long build would sit showing
  /// its first line while the output it is producing goes past below.
  it("keeps a running command's output on its newest line", () => {
    const { byId, send } = loadRenderer();
    send(stateMessage());
    const transcript = byId.get("transcript")!;
    transcript.clientHeight = 100;

    send({
      k: "push",
      item: { id: 4, kind: "tool", name: "bash", args: { command: "cargo test" }, running: true, output: "" },
    });
    const body = find(transcript, "tbody")!;
    body.clientHeight = 60;
    send({ k: "append", id: 4, field: "output", delta: "test result: ok. ".repeat(40) });
    assert.ok(body.scrollHeight > body.clientHeight, "the output outgrows the body");
    assert.equal(body.scrollTop, body.scrollHeight);
  });

  it("leaves a finished card's expanded output at the top", () => {
    const { byId, send } = loadRenderer();
    send(stateMessage());
    const transcript = byId.get("transcript")!;
    transcript.clientHeight = 100;
    send({
      k: "push",
      item: {
        id: 4,
        kind: "tool",
        name: "edit",
        args: { filePath: "src/main.rs" },
        output: "line\n".repeat(200),
        diff: "--- a/src/main.rs\n+++ b/src/main.rs\n@@ -1 +1 @@\n-old\n+new",
        running: false,
      },
    });
    const body = find(transcript, "tbody")!;
    body.clientHeight = 60;
    // Expanding a card is a request to read it from the beginning.
    assert.equal(body.scrollTop, 0);
  });

  it("re-follows the bottom when the pane itself shrinks", async () => {
    const { byId, resize, send } = loadRenderer();
    send(stateMessage());
    const transcript = byId.get("transcript")!;
    transcript.clientHeight = 100;
    send({ k: "push", item: { id: 1, kind: "assistant", text: "a line of the reply. ".repeat(20) } });
    await nextTick();
    assert.equal(transcript.scrollTop, transcript.scrollHeight);

    // An attachment chip or a taller message box takes a bite out of the pane,
    // which would leave the newest line just under the fold.
    transcript.clientHeight = 60;
    resize();
    assert.equal(transcript.scrollTop, transcript.scrollHeight);
  });

  it("leaves a reader who scrolled away alone when the pane shrinks", async () => {
    const { byId, resize, send } = loadRenderer();
    send(stateMessage());
    const transcript = byId.get("transcript")!;
    transcript.clientHeight = 100;
    send({ k: "push", item: { id: 1, kind: "assistant", text: "a line of the reply. ".repeat(20) } });
    await nextTick();
    transcript.scrollTop = 0;

    transcript.clientHeight = 60;
    resize();
    assert.equal(transcript.scrollTop, 0);
  });
});

/// The block the panel starts on, which is the page a closed thread returns
/// you to: `#empty` is the new-chat page, and a notice is painted on it rather
/// than instead of it.
describe("webview welcome page", () => {
  it("stays up while the page only carries a line about it", () => {
    const { byId, send } = loadRenderer();
    send(stateMessage());
    assert.equal(byId.get("empty")!.hidden, false, "a fresh panel shows the welcome page");

    // What the host sends after New chat: the thread is closed, which is news,
    // and the page is still the page you can start typing on.
    send({
      k: "state",
      items: [
        {
          id: 1,
          kind: "notice",
          text: "New chat: the next message starts a thread of its own.",
          tone: "info",
        },
      ],
    });
    assert.equal(byId.get("empty")!.hidden, false, "a notice is not a message");
  });

  it("gives way to the first thing said in the thread", () => {
    const { byId, send } = loadRenderer();
    send(stateMessage());
    send({ k: "push", item: { id: 2, kind: "user", text: "say hi", context: [] } });
    assert.equal(byId.get("empty")!.hidden, true);
  });
});
