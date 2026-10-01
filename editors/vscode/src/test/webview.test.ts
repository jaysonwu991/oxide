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

/// The element the document reports as `document.activeElement`. The renderer
/// asks for it once — whether the caret is in the session listing's search box,
/// since a repaint that took the text away mid-word would be the listing typing
/// over the reader. A real webview's focus is whatever the browser moved it to;
/// here it is what a test focused last, and `blur` takes it away.
let activeElement: StubElement | null = null;

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
  checked = false;
  /// What the DOM reports for an element that is not focusable, which is what
  /// the renderer's own handles are before they are made buttons.
  tabIndex = -1;
  title = "";
  type = "";
  src = "";
  alt = "";
  placeholder = "";
  spellcheck = false;
  rows = 0;
  clientHeight = 0;
  /// A textarea reports where its caret is; the completion asks for it, so the
  /// stub tracks it with the value and the selection a row's insert sets.
  selectionStart = 0;
  selectionEnd = 0;
  private offset = 0;
  private text = "";
  private inputValue = "";

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

  /// What `removeAttribute` leaves behind: no attribute at all, so a card that
  /// turned its header back into plain text reports no role and no state.
  removeAttribute(name: string): void {
    delete this.attributes[name];
  }

  /// A textarea's value: writing it moves the caret to the end, as it does in
  /// the DOM, which is where the completion asks for it.
  get value(): string {
    return this.inputValue;
  }

  set value(next: string) {
    this.inputValue = next;
    this.selectionStart = next.length;
    this.selectionEnd = next.length;
  }

  /// The renderer puts the caret after the row it took; writing the value moves
  /// the caret to the end, as it does in the DOM.
  setSelectionRange(start: number, end: number): void {
    this.selectionStart = start;
    this.selectionEnd = end;
  }

  getAttribute(name: string): string | null {
    return this.attributes[name] ?? null;
  }

  /// Enough of `matches` for the renderer's own selectors: it walks up from a
  /// click target to find `[data-control]`, `[data-path]`, `a[href]`, `.thead`
  /// or the card a tool row belongs to, and the question card reads back the
  /// answers it painted with a compound of a class, a data attribute with a
  /// value and `:checked`. A selector list (`.change-row, .changes-open`) asks
  /// for whichever of its parts matches.
  matches(selector: string): boolean {
    if (selector.includes(",")) {
      return selector.split(",").some((part) => this.matches(part));
    }
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

  focus(): void {
    activeElement = this;
  }

  fire(kind: string, event: unknown = {}): void {
    // Losing the caret is what a blur is, and the document reports it: an
    // element that is blurred is no longer the one being typed into.
    if (kind === "blur" && activeElement === this) activeElement = null;
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
  fireDocument(kind: string, event?: unknown): void;
  /// The element the document reports as focused, which is what the host's own
  /// focus is told apart from by.
  active(): StubElement | null;
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
  activeElement = null;
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
    get activeElement(): StubElement | null {
      return activeElement;
    },
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
    active: () => activeElement,
    readable: () => ({ image, canvas }),
    resize: () => {
      for (const observer of StubResizeObserver.observers) observer.resize();
    },
  };
}

const footer = {
  chips: [
    { id: "model", label: "model: glm-5 · 128.0k", title: "Switch the model" },
    { id: "reasoning", label: "thinking: auto" },
    { id: "agent", label: "agent: review" },
    { id: "access", label: "access: trusted" },
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

/// The sequence number of the newest completion the renderer asked for, so a
/// test answers the question that is live rather than one it guessed the number
/// of.
function asked(posted: Record<string, unknown>[]): number {
  const questions = posted.filter((message) => message.k === "completeAt");
  return Number(questions[questions.length - 1]?.seq ?? 0);
}

/// The same, for the `/` palette the renderer asked the host to fill.
function paletteAsked(posted: Record<string, unknown>[]): number {
  const questions = posted.filter((message) => message.k === "completePalette");
  return Number(questions[questions.length - 1]?.seq ?? 0);
}

describe("webview footer", () => {
  it("paints one icon control per footer setting", () => {
    const { byId, send } = loadRenderer();
    send(stateMessage());
    const chips = byId.get("meta")!.children;
    assert.equal(chips.length, 4);
    assert.deepEqual(
      chips.map((chip) => chip.className),
      ["meta-chip", "meta-chip", "meta-chip", "meta-chip"],
    );
    assert.equal(chips[0].children.length, 1);
    assert.equal(chips[0].children[0].className, "control-icon");
    assert.equal(chips[0].dataset.control, "model");
    assert.equal(chips[0].getAttribute("aria-label"), "model: glm-5 · 128.0k");
    assert.equal(chips[0].title, "model: glm-5 · 128.0k\nSwitch the model");
    assert.equal(chips[1].getAttribute("aria-label"), "thinking: auto");
    assert.equal(
      new Set(chips.map((chip) => chip.children[0].innerHTML)).size,
      4,
      "model, reasoning, agent and project access each use their own glyph",
    );
  });

  it("keeps a label with no key as the icon's accessible name", () => {
    const { byId, send } = loadRenderer();
    send(stateMessage({ footer: { ...footer, chips: [{ id: "model", label: "glm-5" }] } }));
    const chip = byId.get("meta")!.children[0];
    assert.equal(chip.children.length, 1);
    assert.equal(chip.getAttribute("aria-label"), "glm-5");
    assert.equal(chip.title, "glm-5 — click to change");
  });

  it("posts the chip's control id when it is clicked", () => {
    const { byId, posted, send } = loadRenderer();
    send(stateMessage());
    // A click lands on the chip's own span, as it does in the real DOM: the
    // event bubbles to the strip, and the handler reads the control id off the
    // chip around the target.
    const icon = byId.get("meta")!.children[1].children[0];
    byId.get("meta")!.fire("click", { target: icon });
    const message = last(posted);
    assert.equal(message.k, "control");
    assert.equal(message.control, "reasoning");
  });

  it("paints the branch, the usage line and the gauge under the composer", () => {
    const { byId, send } = loadRenderer();
    send(stateMessage());
    assert.equal(byId.get("branch")!.textContent, "main");
    assert.equal(byId.get("branch")!.title, "Current branch: main");
    assert.equal(byId.get("usage-text")!.textContent, "12%");
    assert.equal(byId.get("usage")!.title, footer.usage);
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
    // Stop is the one icon that turns red: it ends the turn rather than
    // opening something.
    assert.match(shell, /id="stop" class="icon danger"/);
  });

  /// The full-size preview is the desktop app's: a ✕ in a head row above the
  /// picture rather than a text button under it, so the way out sits outside
  /// the image it closes and in the error color either way.
  it("puts the image preview's ✕ above the picture it closes", () => {
    assert.match(shell, /<div class="image-head">/);
    assert.match(shell, /id="image-view-close" class="icon danger"/);
    assert.match(shell, /id="image-view-close"[^>]*>✕</);
    const close = shell.slice(shell.indexOf('id="image-view-close"'));
    const image = shell.indexOf('id="image-view-img"');
    assert.ok(
      close.indexOf("</div>") < image,
      "the close button is in a head row of its own, not beside the image",
    );
    assert.match(style, /#image-view-close \{[^}]*--vscode-errorForeground/s);
  });

  /// The review is a panel of its own, over the transcript: the shell has to
  /// declare every part of it, since the renderer looks them up by id.
  it("declares the review's own overlay", () => {
    for (const id of ["review", "review-title", "review-total", "review-files", "review-close"]) {
      assert.match(shell, new RegExp(`id="${id}"`), id);
    }
    assert.match(shell, /id="review"[^>]*hidden/);
    // The panel lists the files and says so: each row opens the file in the
    // editor's own diff, which is where both sides are drawn.
    assert.match(shell, /↑↓ walk the files[^<]*VS Code's diff editor/);
    assert.doesNotMatch(shell, /review-diff/);
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

    // The file the editor has open is context rather than a message: on its own
    // it leaves the box empty and the button disabled.
    send(stateMessage({ context: [{ id: 8, label: "src/main.rs", auto: true }] }));
    assert.equal(sendButton.disabled, true, "a tracked file is not something to send");
  });

  it("shows the thread title in the header", () => {
    const { byId, send } = loadRenderer();
    send(stateMessage({ title: "Fix the flaky test" }));
    assert.equal(byId.get("title")!.textContent, "Fix the flaky test");
    // A thread with no message yet falls back to a neutral placeholder.
    send(stateMessage({ title: "" }));
    assert.equal(byId.get("title")!.textContent, "New chat");
  });

  it("swaps Send for Stop while a turn runs with nothing to say", () => {
    // The corner holds one action rather than two buttons beside each other: a
    // running turn with an empty box offers Stop, and the moment there is
    // something to say the same corner becomes Send, which the host queues for
    // after the turn. The desktop app swaps them the same way, so the button
    // does not move as the box is typed into.
    const { byId, send } = loadRenderer();
    send(stateMessage({ busy: true, status: "Thinking…", queued: 2 }));
    assert.equal(byId.get("status")!.textContent, "Thinking… · 2 queued");
    assert.equal(byId.get("status")!.classList.contains("busy"), true);
    assert.equal(byId.get("status")!.hidden, false);
    assert.equal(byId.get("stop")!.hidden, false, "a running turn offers Stop");
    assert.equal(byId.get("send")!.hidden, true, "and nothing to send hides Send");

    // An idle turn has no phase to report, so the label (and its dot) goes
    // rather than sitting there reading like a control.
    send(stateMessage());
    assert.equal(byId.get("status")!.hidden, true);
    assert.equal(byId.get("stop")!.hidden, true);
    assert.equal(byId.get("send")!.hidden, false);
    assert.equal(byId.get("send")!.getAttribute("aria-label"), "Send");
    assert.equal(byId.get("send")!.disabled, true);

    // Typing turns the corner into Send, which queues the message for after
    // the turn.
    send(stateMessage({ busy: true, status: "Thinking…", queued: 2 }));
    byId.get("input")!.value = "one more thing";
    byId.get("input")!.fire("input");
    assert.equal(byId.get("stop")!.hidden, true, "Stop gives the corner up");
    assert.equal(byId.get("send")!.hidden, false);
    assert.equal(byId.get("send")!.disabled, false);
    assert.equal(byId.get("send")!.getAttribute("aria-label"), "Queue");
    assert.equal(byId.get("busy-message-mode")!.hidden, false);
    assert.equal(byId.get("busy-message-mode")!.textContent, "Queue");
    byId.get("busy-message-mode")!.fire("click");
    assert.equal(byId.get("busy-message-mode")!.textContent, "Steer");
    assert.equal(byId.get("send")!.getAttribute("aria-label"), "Steer");
    byId.get("input")!.value = "";
    byId.get("input")!.fire("input");
    assert.equal(byId.get("send")!.hidden, true, "and the empty box offers Stop again");
    assert.equal(byId.get("stop")!.hidden, false);

    // A pending attachment is something to send too, so it swaps the corner
    // back even with an empty box.
    send(
      stateMessage({
        busy: true,
        attachments: [{ id: 7, label: "shot.png", kind: "image", preview: null, detail: "2 KB" }],
      }),
    );
    assert.equal(byId.get("send")!.hidden, false);
    assert.equal(byId.get("stop")!.hidden, true);

    // Hiding one of them is only half of the swap: the icon buttons are laid
    // out as inline-flex, which the UA's `[hidden]` rule cannot beat, so both
    // actions would sit in the corner at once.
    assert.match(style, /button\.icon\[hidden\],[\s\S]*?\{[^}]*display: none/s);
  });

  it("names the lines a tracked selection holds, in the label and in words", () => {
    // The chip is the only place the reader can see what the next message will
    // carry, so a selection is spelled out where it sits — and the tooltip says
    // the same thing in words, since `src/app.ts:12-15` is not what a screen
    // reader reads out.
    const { byId, send } = loadRenderer();
    send(
      stateMessage({
        context: [
          {
            id: 4,
            label: "src/app.ts:12-15",
            auto: true,
            detail: "4 lines selected — sent with the next message",
          },
          { id: 5, label: "src/other.rs", auto: true },
        ],
      }),
    );
    const chips = byId.get("chips")!;
    const selected = chips.children[0];
    assert.equal(selected.className, "chip context auto", "still the tracked chip");
    assert.equal(selected.children[1].textContent, "src/app.ts:12-15");
    assert.equal(
      selected.title,
      "src/app.ts:12-15 — 4 lines selected — sent with the next message",
    );
    assert.equal(
      selected.getAttribute("aria-label"),
      selected.title,
      "the tooltip and the name a screen reader reads agree word for word",
    );
    // A whole file says what it always did.
    const whole = chips.children[1];
    assert.equal(whole.title, "src/other.rs — the file you are editing, sent with the next message");
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
    const { active, byId, fireDocument, send } = loadRenderer();
    const preview = "data:image/png;base64,QUJDRA==";
    send(
      stateMessage({
        attachments: [{ id: 2, label: "shot.png", kind: "image", preview, detail: "12 KB" }],
      }),
    );
    assert.equal(byId.get("image-view")!.hidden, true);
    const open = byId.get("chips")!.children[0].children[0];
    assert.equal(open.title, "Open the full-size image");
    assert.equal(open.getAttribute("aria-label"), "Open shot.png");
    open.focus();
    open.fire("click");
    assert.equal(byId.get("image-view")!.hidden, false);
    assert.equal(byId.get("image-view-img")!.src, preview);
    assert.equal(byId.get("image-view-img")!.alt, "shot.png");
    assert.equal(active(), byId.get("image-view-close"));

    let trapped = false;
    byId.get("image-view")!.fire("keydown", {
      key: "Tab",
      preventDefault: () => { trapped = true; },
    });
    assert.equal(trapped, true);
    assert.equal(active(), byId.get("image-view-close"));

    // Clicking the backdrop, then Escape, closes it; the thumbnail itself only
    // opens it, so a click there while it is open leaves it open.
    byId.get("image-view")!.fire("click", { target: byId.get("image-view") });
    assert.equal(byId.get("image-view")!.hidden, true);
    assert.equal(active(), open);
    open.fire("click");
    fireDocument("keydown", { key: "Escape" });
    assert.equal(byId.get("image-view")!.hidden, true);
    // ...and the ✕ in the head row does too.
    open.fire("click");
    byId.get("image-view-close")!.fire("click");
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

  it("paints the file the editor has open as a tracked chip", () => {
    const { byId, posted, send } = loadRenderer();
    send(
      stateMessage({
        context: [
          { id: 1, label: "src/main.rs" },
          { id: 2, label: "docs/cli.md", auto: true },
        ],
      }),
    );
    const chips = byId.get("chips")!;
    // The tracked chip is painted after the ones the user attached, and reads
    // as tracked rather than attached: dashed, its own glyph, and a tooltip
    // that says where it comes from and what takes it away.
    assert.deepEqual(
      chips.children.map((chip) => chip.className),
      ["chip context", "chip context auto", "chip-clear"],
    );
    assert.equal(chips.children[0].children[0].textContent, "❮❯");
    assert.match(chips.children[0].title, /inlined into the next message/);
    assert.equal(chips.children[1].children[0].textContent, "✎");
    assert.equal(chips.children[1].children[1].textContent, "docs/cli.md");
    assert.match(chips.children[1].title, /the file you are editing/);
    assert.match(chips.children[1].children[2].title, /Take this file out/);

    // Its ✕ is the same message as any other chip's: one id space, one handler.
    chips.children[1].children[2].fire("click");
    assert.deepEqual(last(posted), { k: "removeChip", id: 2 });
  });

  it("keeps the tracked file out of an empty box's send", () => {
    const { byId, posted, send } = loadRenderer();
    send(stateMessage({ context: [{ id: 3, label: "src/main.rs", auto: true }] }));
    // The chip is on show in the strip...
    assert.equal(byId.get("chips")!.hidden, false);
    assert.equal(byId.get("chips")!.children.length, 1, "a lone chip brings no Clear");
    // ...but an empty box still submits nothing: Enter posts no message (the
    // renderer has already announced it is ready, which is the only thing on
    // the wire), and the host would have nothing to send either.
    byId.get("input")!.fire("keydown", { key: "Enter", shiftKey: false, preventDefault: () => {} });
    assert.deepEqual(last(posted), { k: "ready" });

    // A chip of the user's own is what turns the box live, and what Clear's
    // count includes.
    send(
      stateMessage({
        context: [
          { id: 4, label: "notes.md" },
          { id: 3, label: "src/main.rs", auto: true },
        ],
      }),
    );
    assert.equal(byId.get("send")!.disabled, false);
    assert.deepEqual(
      byId.get("chips")!.children.map((chip) => chip.className),
      ["chip context", "chip context auto", "chip-clear"],
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

  it("sends the selected busy behavior and returns to Queue", () => {
    const { byId, posted, send } = loadRenderer();
    send(stateMessage({ busy: true, status: "Thinking…" }));
    const input = byId.get("input")!;
    input.value = "use the new API";
    input.fire("input");
    byId.get("busy-message-mode")!.fire("click");
    byId.get("send")!.fire("click");
    assert.deepEqual(last(posted), { k: "send", text: "use the new API", mode: "steer" });

    input.value = "one more thing";
    input.fire("input");
    assert.equal(byId.get("busy-message-mode")!.textContent, "Queue");
  });

  it("asks the host what the caret is in once an @ is typed", () => {
    const { byId, posted, send } = loadRenderer();
    send(stateMessage());
    const input = byId.get("input")!;
    // Nothing is asked of a value with no reference in it, which is most of
    // them: `input` already fired once for `hello` and posted nothing.
    input.value = "hello";
    input.fire("input");
    assert.equal(posted.some((message) => message.k === "completeAt"), false);

    input.value = "look at @sr";
    input.fire("input");
    // Every settling of the list takes a number with it — closing it here for
    // the value with no reference in it — so this is the second question the
    // renderer has asked. What matters is that each answer says which one it is
    // answering.
    assert.deepEqual(last(posted), { k: "completeAt", text: "look at @sr", caret: 11, seq: 2 });
    assert.equal(byId.get("at")!.hidden, true, "nothing is up until the host answers");

    // A click moves the caret into another token, which is asked about again —
    // and the answer to the first question is numbered, so it is dropped rather
    // than painted under a caret that has moved.
    input.selectionStart = 3;
    input.fire("click");
    assert.equal(last(posted).seq, 3);
    send({ k: "atSuggestions", seq: asked(posted) - 1, start: 0, end: 7, rows: [
      { label: "src/main.rs", kind: "file", insert: "@src/main.rs " },
    ] });
    assert.equal(byId.get("at")!.hidden, true);
  });

  it("paints the rows the host offered, and takes the one that was clicked", () => {
    const { byId, posted, send } = loadRenderer();
    send(stateMessage());
    const input = byId.get("input")!;
    input.value = "see @src";
    input.fire("input");
    send({ k: "atSuggestions", seq: asked(posted), start: 4, end: 8, rows: [
      { label: "src/", kind: "folder", insert: "@src/" },
      { label: "src/main.rs", kind: "file", insert: "@src/main.rs " },
    ] });

    const at = byId.get("at")!;
    assert.equal(at.hidden, false);
    // The first row is the one a keystroke would take, and the glyph says which
    // rows are folders.
    assert.deepEqual(
      at.children.map((row) => row.className),
      ["at-row selected", "at-row"],
    );
    assert.deepEqual(
      at.children.map((row) => row.children[1].textContent),
      ["src/", "src/main.rs"],
    );
    assert.deepEqual(
      at.children.map((row) => row.children[0].textContent),
      ["▸", "·"],
    );
    assert.equal(at.children[0].title, "src/ — a folder in this project");
    assert.equal(at.children[1].title, "src/main.rs — inlined into the next message");
    assert.deepEqual(
      at.children.map((row) => row.getAttribute("role")),
      ["option", "option"],
    );

    // A click takes the row's own insert: the whole token goes, and a file is
    // left with a space so the next word can be typed.
    at.children[1].fire("click");
    assert.equal(input.value, "see @src/main.rs ");
    assert.equal(input.selectionStart, input.value.length);
    assert.equal(at.hidden, true);
  });

  it("walks the rows with the arrows and takes one with Tab or Enter", () => {
    const { byId, posted, send } = loadRenderer();
    send(stateMessage());
    const input = byId.get("input")!;
    input.value = "@src";
    input.fire("input");
    send({ k: "atSuggestions", seq: asked(posted), start: 0, end: 4, rows: [
      { label: "src/", kind: "folder", insert: "@src/" },
      { label: "src/main.rs", kind: "file", insert: "@src/main.rs " },
    ] });
    const at = byId.get("at")!;
    const press = (key: string, shiftKey = false) => {
      let prevented = false;
      input.fire("keydown", { key, shiftKey, preventDefault: () => { prevented = true; } });
      return prevented;
    };

    assert.equal(press("ArrowDown"), true, "the arrow walks the list, not the caret");
    assert.deepEqual(
      at.children.map((row) => row.className),
      ["at-row", "at-row selected"],
    );
    assert.equal(at.children[1].getAttribute("aria-selected"), "true");
    press("ArrowDown");
    assert.deepEqual(at.children.map((row) => row.className), ["at-row selected", "at-row"]);
    press("ArrowUp");
    assert.deepEqual(at.children.map((row) => row.className), ["at-row", "at-row selected"]);

    // Tab and Enter both take the highlighted row rather than sending: a
    // reference midway through a message is not the message.
    assert.equal(press("Enter"), true);
    assert.equal(input.value, "@src/main.rs ");
    assert.equal(at.hidden, true);

    input.value = "@docs";
    input.fire("input");
    send({ k: "atSuggestions", seq: asked(posted), start: 0, end: 5, rows: [
      { label: "docs/", kind: "folder", insert: "@docs/" },
    ] });
    assert.equal(press("Tab"), true);
    assert.equal(input.value, "@docs/");
  });

  it("closes the list on Escape, and leaves the next Escape to the turn", () => {
    const { byId, posted, send } = loadRenderer();
    send(stateMessage({ busy: true }));
    const input = byId.get("input")!;
    input.value = "@src";
    input.fire("input");
    send({ k: "atSuggestions", seq: asked(posted), start: 0, end: 4, rows: [
      { label: "src/", kind: "folder", insert: "@src/" },
    ] });
    const at = byId.get("at")!;

    input.fire("keydown", { key: "Escape", shiftKey: false, preventDefault: () => {} });
    assert.equal(at.hidden, true);
    assert.equal(posted.some((message) => message.k === "stop"), false, "a turn goes on running");

    // An answer that was still on its way — a walk of the project is not
    // instant — cannot pop the list back up after it was dismissed.
    send({ k: "atSuggestions", seq: asked(posted), start: 0, end: 4, rows: [
      { label: "src/", kind: "folder", insert: "@src/" },
    ] });
    assert.equal(at.hidden, true);

    input.fire("keydown", { key: "Escape", shiftKey: false, preventDefault: () => {} });
    assert.deepEqual(last(posted), { k: "stop" });
  });

  it("hides the list when nothing answers the token", () => {
    const { byId, posted, send } = loadRenderer();
    send(stateMessage());
    const input = byId.get("input")!;
    input.value = "@src";
    input.fire("input");
    const at = byId.get("at")!;
    send({ k: "atSuggestions", seq: asked(posted), start: 0, end: 4, rows: [
      { label: "src/", kind: "folder", insert: "@src/" },
    ] });
    assert.equal(at.hidden, false);
    // The answer for the request that is live: nothing matches the token, so the
    // list goes away rather than standing there offering nothing.
    send({ k: "atSuggestions", seq: asked(posted), start: 0, end: 9, rows: [] });
    assert.equal(at.hidden, true);
    assert.equal(at.children.length, 0);
  });

  it("sends what the box holds with the list out of the way", () => {
    const { byId, posted, send } = loadRenderer();
    send(stateMessage());
    const input = byId.get("input")!;
    input.value = "read @src/main.rs";
    input.fire("input");
    assert.deepEqual(
      posted.filter((message) => message.k === "send"),
      [],
      "a reference is not a send",
    );
    byId.get("send")!.fire("click");
    assert.deepEqual(last(posted), { k: "send", text: "read @src/main.rs" });
    assert.equal(byId.get("at")!.hidden, true);
  });

  it("attaches the list to the box it completes, above the chips", () => {
    // The rows belong to the message box, so the shell puts them inside the
    // composer card rather than floating them over the panel.
    const composer = shell.slice(shell.indexOf('<div id="composer">'));
    const at = composer.indexOf('id="at"');
    const chips = composer.indexOf('id="chips"');
    assert.ok(at > 0, "the list is in the composer");
    assert.match(composer.slice(at, chips), /class="at-list"[^>]*hidden/);
    assert.ok(chips > at, "the list sits above the attachment strip");
    assert.match(composer.slice(at, chips), /role="listbox"/);
    assert.match(style, /\.at-list\[hidden\]/);
  });

  it("offers the project's commands and skills for a slash command being typed", () => {
    const { byId, posted, send } = loadRenderer();
    send(stateMessage());
    const input = byId.get("input")!;
    input.value = "/ox";
    input.fire("input");
    const at = byId.get("at")!;
    // The catalog is the host's — the CLI's own — so the view asks for the rows
    // rather than listing anything of its own.
    assert.deepEqual(last(posted), {
      k: "completePalette",
      text: "/ox",
      seq: paletteAsked(posted),
    });

    send({ k: "paletteRows", kind: "command", seq: paletteAsked(posted), start: 0, end: 3, rows: [
      { name: "oxide-architecture", insert: "/oxide-architecture ", arguments: "[arguments]", description: "Use when navigating the internals", kind: "skill", source: "project" },
      { name: "build", insert: "/build ", arguments: "arguments", description: "Build the project", kind: "prompt", source: "project" },
    ] });

    assert.equal(at.hidden, false);
    assert.deepEqual(
      at.children.map((row) => row.className),
      ["cmd-row selected", "cmd-row"],
    );
    // A row says the name it inserts, the arguments it takes, what it does and
    // where it came from — a skill's own kind rather than the scope.
    assert.deepEqual(
      at.children.map((row) => row.children.map((child) => child.textContent)),
      [
        ["/oxide-architecture", "[arguments]", "Use when navigating the internals", "skill"],
        ["/build", "arguments", "Build the project", "project"],
      ],
    );
    assert.equal(at.children[0].title, "/oxide-architecture — Use when navigating the internals");
    assert.equal(at.getAttribute("aria-label"), "Commands and skills");

    // A row is completed rather than run: what goes in the box is the message
    // the CLI expands, which may take arguments after it.
    at.children[0].fire("click");
    assert.equal(input.value, "/oxide-architecture ");
    assert.equal(input.selectionStart, input.value.length);
    assert.equal(at.hidden, true);
    assert.equal(
      posted.filter((message) => message.k === "completePalette").length,
      1,
      "a command is left in the box rather than re-completing",
    );

    // Enter then sends it, so taking a skill's row is what activates it.
    input.fire("keydown", { key: "Enter", shiftKey: false, preventDefault: () => {} });
    assert.deepEqual(last(posted), { k: "send", text: "/oxide-architecture " });
  });

  it("walks the palette with the arrows and takes a row with Tab", () => {
    const { byId, posted, send } = loadRenderer();
    send(stateMessage());
    const input = byId.get("input")!;
    input.value = "/";
    input.fire("input");
    send({ k: "paletteRows", kind: "command", seq: paletteAsked(posted), start: 0, end: 1, rows: [
      { name: "help", insert: "/help", arguments: "", description: "List the commands", kind: "client", source: "builtin" },
      { name: "ship", insert: "/ship ", arguments: "arguments", description: "Open a pull request", kind: "prompt", source: "project" },
    ] });
    const at = byId.get("at")!;
    const press = (key: string) => {
      let prevented = false;
      input.fire("keydown", { key, shiftKey: false, preventDefault: () => { prevented = true; } });
      return prevented;
    };

    assert.equal(press("ArrowDown"), true, "the arrow walks the palette, not the caret");
    assert.deepEqual(at.children.map((row) => row.className), ["cmd-row", "cmd-row selected"]);
    assert.equal(at.children[1].getAttribute("aria-selected"), "true");

    // Tab takes the row and leaves the command in the box: a palette is one
    // word, so there is no caret inside it to keep completing.
    assert.equal(press("Tab"), true);
    assert.equal(input.value, "/ship ");
    assert.equal(at.hidden, true);
    assert.equal(posted.some((message) => message.k === "send"), false);

    // Arguments after the name are the message, not a name to complete, and the
    // host is the one that says so.
    input.value = "/ship main";
    input.fire("input");
    send({ k: "paletteRows", kind: "command", seq: paletteAsked(posted), start: 0, end: 10, rows: [] });
    assert.equal(at.hidden, true);
    assert.equal(at.children.length, 0);
  });

  it("closes the palette on Escape and drops an answer that arrived late", () => {
    const { byId, posted, send } = loadRenderer();
    send(stateMessage());
    const input = byId.get("input")!;
    input.value = "/si";
    input.fire("input");
    send({ k: "paletteRows", kind: "command", seq: paletteAsked(posted), start: 0, end: 3, rows: [
      { name: "ship", insert: "/ship ", arguments: "arguments", description: "Open a pull request", kind: "prompt", source: "project" },
    ] });
    const at = byId.get("at")!;
    assert.equal(at.hidden, false);

    input.fire("keydown", { key: "Escape", shiftKey: false, preventDefault: () => {} });
    assert.equal(at.hidden, true);
    // The catalog crossed the host boundary: an answer for the question that was
    // cancelled cannot pop the list back up over a box the user moved on from.
    send({ k: "paletteRows", kind: "command", seq: paletteAsked(posted), start: 0, end: 3, rows: [
      { name: "ship", insert: "/ship ", arguments: "arguments", description: "Open a pull request", kind: "prompt", source: "project" },
    ] });
    assert.equal(at.hidden, true);
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

  it("leaves a modified Escape to the host's own keybinding", () => {
    // The editor's focus shortcut is Cmd/Ctrl+Esc, which the browser delivers
    // to the box as an Escape: swallowing it would take the caret out of the
    // panel's hands and never bring it back.
    const { byId, posted, send } = loadRenderer();
    send(stateMessage({ busy: true }));
    const input = byId.get("input")!;
    let prevented = false;
    input.fire("keydown", {
      key: "Escape",
      metaKey: true,
      preventDefault: () => {
        prevented = true;
      },
    });
    assert.equal(prevented, false);
    assert.equal(posted.some((message) => message.k === "stop"), false);

    input.fire("keydown", { key: "Escape", preventDefault: () => {} });
    assert.equal(last(posted).k, "stop");
  });

  it("writes the host's reference into the box at the caret", () => {
    // The editor's insert shortcut: the host reads the file and the selection
    // and hands over the reference, and the view splices it where the caret is
    // — kept off the words around it the way a typed reference sits, with the
    // caret left after it so the question goes on.
    const { byId, send, active } = loadRenderer();
    send(stateMessage());
    const input = byId.get("input")!;
    input.value = "why does this loop spin";
    input.setSelectionRange(9, 9);

    send({ k: "insert", text: "@src/app.ts#5-10" });
    assert.equal(input.value, "why does @src/app.ts#5-10 this loop spin");
    assert.equal(input.selectionStart, 26);
    assert.equal(input.selectionEnd, 26);
    assert.equal(active(), input, "the caret is in the box to go on typing");
  });

  it("replaces what was selected with the host's reference, as a paste would", () => {
    const { byId, send } = loadRenderer();
    send(stateMessage());
    const input = byId.get("input")!;
    input.value = "check @old.rs please";
    input.setSelectionRange(6, 13);

    send({ k: "insert", text: "@src/new.rs" });
    assert.equal(input.value, "check @src/new.rs please");
  });

  it("puts the caret in the box when the host brings the chat forward", () => {
    const { byId, send, active } = loadRenderer();
    send(stateMessage());
    const input = byId.get("input")!;
    input.value = "half a question";
    input.setSelectionRange(0, 0);

    send({ k: "focusComposer" });
    assert.equal(active(), input);
    assert.equal(input.selectionStart, input.value.length);
  });

  it("tells the host when the pane takes the keyboard, and when it gives it up", () => {
    // The host toggles the caret between the editor and the composer, so it has
    // to know which side it is on.
    const { posted, fireDocument } = loadRenderer();
    fireDocument("focus");
    assert.deepEqual(last(posted), { k: "focus" });
    fireDocument("blur");
    assert.deepEqual(last(posted), { k: "blur" });
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
      kind: "mcp",
      pin: "footer",
      title: "MCP servers",
      subtitle: "The servers this project loads, and whether Oxide can reach them.",
      note: "",
      count: 2,
      search: false,
      query: "",
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
          kind: "",
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
          kind: "",
        },
      ],
      refreshLabel: "Recheck",
      refreshAction: "mcpRefresh",
    },
  };
  const sessions = {
    k: "dialog",
    dialog: {
      kind: "sessions",
      pin: "header",
      title: "Sessions",
      subtitle: "",
      note: "",
      count: 1,
      search: true,
      query: "",
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
          kind: "action",
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
          kind: "thread",
          current: true,
        },
      ],
      refreshLabel: "",
      refreshAction: "",
    },
  };

  const confirmDelete = {
    k: "dialog",
    dialog: {
      kind: "delete",
      pin: "header",
      title: "Delete thread",
      subtitle: "“Fix the flaky test” and its stored conversation are removed.",
      note: "This cannot be undone.",
      count: 0,
      search: false,
      query: "",
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
          kind: "",
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
          kind: "",
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

  /// The head carries how many rows the listing holds — a number a narrow pane
  /// cannot read off a scrollbar — and a listing long enough to need one carries
  /// a box to filter it. Both are the host's: only it knows how many threads the
  /// store gave it and which of them the query left.
  it("counts the listing, and offers the box the host asked for", () => {
    const { byId, posted, send } = loadRenderer();
    send(mcp);
    assert.equal(byId.get("dialog-count")!.hidden, false);
    assert.equal(byId.get("dialog-count")!.textContent, "2");
    assert.equal(byId.get("dialog-search")!.hidden, true, "a server list is not searched");

    send(sessions);
    assert.equal(byId.get("dialog-count")!.textContent, "1", "the threads, not the ways out of them");
    assert.equal(byId.get("dialog-search")!.hidden, false);
    assert.equal(byId.get("dialog-search-clear")!.hidden, true, "nothing to clear yet");

    // Typing asks the host rather than hiding rows here: the rows, the count and
    // the note a search with no matches carries are all composed where they are.
    const input = byId.get("dialog-search-input")!;
    input.value = "flaky";
    input.fire("input");
    assert.deepEqual(last(posted), { k: "dialogSearch", text: "flaky" });
    assert.equal(byId.get("dialog-search-clear")!.hidden, false);

    // The host echoes the filter back with the narrowed rows, so a repaint keeps
    // the reader's text in the box.
    send({ k: "dialog", dialog: { ...sessions.dialog, query: "flaky" } });
    assert.equal(input.value, "flaky");
    assert.equal(byId.get("dialog-search-clear")!.hidden, false);

    // The box's own ✕ clears it in one click, and says so to the host.
    byId.get("dialog-search-clear")!.fire("click");
    assert.equal(input.value, "");
    assert.equal(byId.get("dialog-search-clear")!.hidden, true);
    assert.deepEqual(last(posted), { k: "dialogSearch", text: "" });

    // A listing with no count hides it rather than showing a zero, and closing
    // the listing takes the box — and what was typed in it — with it.
    send(confirmDelete);
    assert.equal(byId.get("dialog-count")!.hidden, true);
    assert.equal(byId.get("dialog-search")!.hidden, true);
    send(sessions);
    input.value = "flaky";
    send({ k: "dialog", dialog: null });
    assert.equal(input.value, "", "the filter belongs to the listing, not the panel");
    assert.equal(byId.get("dialog-search-clear")!.hidden, true);
  });

  it("uses the setting dialog's own search prompt", () => {
    const { byId, send } = loadRenderer();
    send({
      k: "dialog",
      dialog: {
        ...sessions.dialog,
        kind: "model",
        pin: "footer",
        title: "Model",
        searchPlaceholder: "Filter or enter a model ID…",
      },
    });
    assert.equal(byId.get("dialog-search-input")!.placeholder, "Filter or enter a model ID…");

    send(sessions);
    assert.equal(byId.get("dialog-search-input")!.placeholder, "Search sessions…");
  });

  /// A repaint under a reader who is mid-word must not take the box away from
  /// them: the answer in flight was composed with the filter the host had when
  /// the keystroke arrived, so painting it back would undo the last one.
  it("leaves the search box alone while it is being typed in", () => {
    const { byId, posted, send } = loadRenderer();
    send(sessions);
    const input = byId.get("dialog-search-input")!;
    input.focus();
    input.value = "flak";
    input.fire("input");
    assert.deepEqual(last(posted), { k: "dialogSearch", text: "flak" });

    // The store catches up — the read a turn's own end asks for — and its answer
    // carries no filter, which is not the answer to paint into a box being typed
    // in.
    send({ k: "dialog", dialog: { ...sessions.dialog, query: "" } });
    assert.equal(input.value, "flak", "the half-typed filter is not thrown away");

    // With the caret elsewhere the host's listing is painted as it was composed.
    input.fire("blur");
    send({ k: "dialog", dialog: { ...sessions.dialog, query: "" } });
    assert.equal(input.value, "");
  });

  /// The listing is made of two kinds of row, and the host says which is which:
  /// a way out of the listing, or one of the threads it lists. The view paints
  /// the difference rather than reading the labels to guess it.
  it("paints a way out of the listing apart from the threads it lists", () => {
    const { byId, send } = loadRenderer();
    send(sessions);
    const rows = byId.get("dialog-list")!.children;
    assert.equal(rows[0].className, "dialog-row action");
    assert.equal(rows[1].className, "dialog-row thread current", "the open thread is marked");
    // The mark is the host's own flag rather than a word it painted, so a row
    // that is not the open thread carries none of it — and a row of a listing
    // that is not a list of threads (a confirmation's) is neither.
    send({
      k: "dialog",
      dialog: {
        ...sessions.dialog,
        rows: [{ ...sessions.dialog.rows[1], status: "7m ago", current: false }],
      },
    });
    assert.equal(byId.get("dialog-list")!.children[0].className, "dialog-row thread");
    send(confirmDelete);
    assert.equal(byId.get("dialog-list")!.children[0].className, "dialog-row");
  });

  /// The header's history button is the listing's own state: it says whether the
  /// history is on screen, so the click that opened it is the one that closes it
  /// and the button never offers an action it has no state for.
  it("says on the header's button whether the history is up", () => {
    const { byId, send } = loadRenderer();
    // The shell carries the resting state (`aria-expanded="false"` in the
    // markup, which the host's own test pins), and the renderer keeps it in step
    // with what it paints from here.
    send(mcp);
    assert.equal(byId.get("resume-session")!.getAttribute("aria-expanded"), "false");
    send(sessions);
    assert.equal(byId.get("resume-session")!.getAttribute("aria-expanded"), "true");
    // Whatever else the panel is showing, only the history is what that button
    // is about.
    send(mcp);
    assert.equal(byId.get("resume-session")!.getAttribute("aria-expanded"), "false");
    send(sessions);
    send({ k: "dialog", dialog: null });
    assert.equal(byId.get("resume-session")!.getAttribute("aria-expanded"), "false");
  });

  /// A row's own button carries a switch rather than a word, so it is one glyph
  /// wide and says what it does in its tooltip — the panel is a narrow side bar,
  /// and `Disable` on every row is a column of text where a switch will do.
  it("paints each listing near the end its host selected", () => {
    const { byId, send } = loadRenderer();
    // The host decides — `dialogs.ts` pins the server list to the composer and
    // the session history to the header — and the renderer only carries it: the
    // class is what the stylesheet moves. Read the name from
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
  /// it sits between the header and the transcript as a complete card with a
  /// row list that scrolls. It
  /// is also the one part of the column that never gives up height — a dialog
  /// squeezed down to a sliver of scrolling rows is what the flex there was for.
  it("positions the dialog near the header and gives its buttons icons", () => {
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
    assert.match(rule, /width: calc\(100% - 40px\);/, "it shares the composer's inset");
    assert.match(rule, /max-width: 760px;/, "it shares the composer's width cap");
    assert.match(rule, /margin: 10px auto 0;/, "the header dialog is centered with breathing room");
    assert.match(rule, /border: var\(--dialog-edge\);/, "the card has a complete outline");
    assert.match(rule, /border-radius: 8px;/, "all four corners use the same radius");
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

  /// One card, two positions: the markup keeps the position under the header,
  /// and `.pin-footer` moves the same complete card above the footer with
  /// `order`. The transcript is ordered between them so the listing takes its
  /// room from the transcript in either position.
  it("moves the complete dialog card to the composer when the host asks", () => {
    const pinned = /#dialog\.pin-footer \{[^}]*\}/.exec(style)?.[0] ?? "";
    assert.match(pinned, /order: 2;/, "it is placed after the transcript");
    assert.match(pinned, /box-shadow: 0 -10px/, "the shadow is cast upward");
    assert.match(pinned, /margin: 0 auto;/, "it centers on the composer without fusing to it");

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
  /// The card the CLI's `question_request` becomes: the questions are asked one
  /// at a time, each with its options and a field for an answer of the user's
  /// own wording.
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

  /// The card and every answer field in it, in the order they were painted. A
  /// single choice's last one is the row asking for the user's own wording,
  /// whose value is empty so its label is never an answer.
  const fields = (card: StubElement) => card.querySelectorAll(".qchoice");
  /// Every question's own block of fields, whether or not it is the one shown.
  const blocks = (card: StubElement) => card.querySelectorAll(".qblock");
  /// The buttons the card offers, in the order they were painted.
  const actions = (card: StubElement) =>
    find(card, "qactions")!.children.map((child) => child.textContent);
  /// The button the card names on this step, which is how a click is delivered.
  const button = (card: StubElement, label: string) =>
    find(card, "qactions")!.children.find((child) => child.textContent === label)!;
  /// What each dash says about where in the questions the reader is.
  const dashes = (card: StubElement) =>
    card.querySelectorAll(".qdash").map((dash) => dash.className);

  it("paints a radio group and a field for the user's own answer", () => {
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
        ["radio", ""],
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
      [true, false, false],
    );
    assert.equal(find(card, "qfree")!.placeholder, "Type your answer…");
    assert.equal(find(card, "qhint")!.textContent, "Select one answer");
    assert.deepEqual(actions(card), ["Dismiss", "Submit", "Waiting for your answer…"]);
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
    // A multi-select question starts with nothing ticked, and offers the user's
    // own wording beside the labels rather than as a row of its own.
    assert.deepEqual(
      fields(card).map((choice) => choice.checked),
      [false, false],
    );
    assert.equal(find(card, "qhint")!.textContent, "Select all that apply");
    assert.equal(find(card, "qfree")!.placeholder, "Or type your own answer…");
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
        questions: [
          { question: "What should it be called?", header: "", options: [], multiSelect: false },
        ],
      },
    });
    const card = find(transcript, "question")!;
    assert.deepEqual(fields(card), []);
    assert.equal(find(card, "qfree")!.placeholder, "Type your answer…");
    // Nothing to hint at, and the card's own title already asks the question.
    assert.equal(find(card, "qhint")!.hidden, true);
    assert.equal(find(card, "qtext")!.hidden, true);
    assert.equal(find(card, "qtitle")!.textContent, "What should it be called?");
  });

  /// A call that asks several things shows one of them at a time, counts them
  /// and marks the one being asked, so the card never reads as one long form.
  const twoQuestions = {
    ...pending,
    questions: [
      pending.questions[0],
      { question: "Anything else?", header: "Notes", options: [], multiSelect: false },
    ],
  };

  it("asks one question at a time, counting them", () => {
    const { byId, send } = loadRenderer();
    send(stateMessage());
    const transcript = byId.get("transcript")!;
    send({ k: "push", item: twoQuestions });

    const card = find(transcript, "question")!;
    assert.equal(find(card, "qsteplabel")!.textContent, "1 of 2 questions");
    assert.deepEqual(dashes(card), ["qdash current", "qdash todo"]);
    assert.deepEqual(
      blocks(card).map((block) => block.hidden),
      [false, true],
    );
    assert.equal(find(card, "qtext")!.textContent, "Which database?");
    assert.deepEqual(actions(card), ["Dismiss", "Next", "Waiting for your answer…"]);

    // Walking on marks the question answered and leaves it behind.
    const first = blocks(card)[0];
    first.querySelectorAll(".qchoice")[0].checked = false;
    first.querySelectorAll(".qchoice")[1].checked = true;
    button(card, "Next").fire("click");
    assert.equal(find(card, "qsteplabel")!.textContent, "2 of 2 questions");
    assert.deepEqual(dashes(card), ["qdash done", "qdash current"]);
    assert.deepEqual(
      blocks(card).map((block) => block.hidden),
      [true, false],
    );
    assert.equal(find(card, "qtext")!.textContent, "Anything else?");
    assert.equal(find(card, "qheader")!.textContent, "Notes");
    assert.deepEqual(actions(card), ["Dismiss", "Back", "Submit", "Waiting for your answer…"]);

    // Walking back finds the question that was left still answered.
    button(card, "Back").fire("click");
    assert.equal(find(card, "qsteplabel")!.textContent, "1 of 2 questions");
    assert.deepEqual(dashes(card), ["qdash current", "qdash todo"]);
    assert.deepEqual(
      blocks(card).map((block) => block.hidden),
      [false, true],
    );
    assert.deepEqual(
      fields(card).map((choice) => choice.checked),
      [false, true, false],
    );
  });

  /// The answers are the host's to send: the view names the request and what
  /// was ticked and typed, and the question each answer belongs to. The last
  /// question's Submit sends the whole set, including what earlier steps kept.
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
    const [first, second] = blocks(card);
    first.querySelectorAll(".qchoice")[1].checked = true;
    first.querySelector(".qfree")!.value = "  and fast  ";
    button(card, "Next").fire("click");
    second.querySelector(".qfree")!.value = "ship it";
    button(card, "Submit").fire("click");

    assert.deepEqual(shape(posted[posted.length - 1]), {
      k: "question",
      requestId: 21,
      answers: [
        { question: "Which database?", values: ["SQLite", "and fast"] },
        { question: "Anything else?", values: ["ship it"] },
      ],
    });
  });

  /// A single choice offers the user's own wording as one of its choices, so
  /// picking it is what makes the typed text answer — the row's empty value is
  /// never sent, and neither is the label it replaced.
  it("sends the user's own answer in place of the picked label", () => {
    const { byId, send, posted } = loadRenderer();
    send(stateMessage());
    const transcript = byId.get("transcript")!;
    send({ k: "push", item: pending });

    const card = find(transcript, "question")!;
    const choices = fields(card);
    choices[0].checked = false;
    choices[2].checked = true;
    find(card, "qfree")!.value = "  DuckDB  ";
    button(card, "Submit").fire("click");

    assert.deepEqual(shape(posted[posted.length - 1]), {
      k: "question",
      requestId: 21,
      answers: [{ question: "Which database?", values: ["DuckDB"] }],
    });
  });

  it("leaves text typed under a picked label out of the answer", () => {
    const { byId, send, posted } = loadRenderer();
    send(stateMessage());
    const transcript = byId.get("transcript")!;
    send({ k: "push", item: pending });

    const card = find(transcript, "question")!;
    fields(card)[0].checked = false;
    fields(card)[1].checked = true;
    find(card, "qfree")!.value = "a note the row never asked for";
    button(card, "Submit").fire("click");

    assert.deepEqual(shape(posted[posted.length - 1]), {
      k: "question",
      requestId: 21,
      answers: [{ question: "Which database?", values: ["SQLite"] }],
    });
  });

  it("sends an empty answer list for Dismiss, which is a dismissal", () => {
    const { byId, send, posted } = loadRenderer();
    send(stateMessage());
    const transcript = byId.get("transcript")!;
    send({ k: "push", item: pending });
    button(find(transcript, "question")!, "Dismiss").fire("click");
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
    assert.deepEqual(actions(card), ["Postgres"]);

    const second = loadRenderer();
    second.send(stateMessage({ items: [{ ...pending, state: "closed", label: "Not answered" }] }));
    assert.deepEqual(
      find(second.byId.get("transcript")!, "qactions")!.children.map((child) => child.textContent),
      ["Not answered"],
    );
  });
});

describe("webview change cards", () => {
  /// The card a turn's `turn_changes` frame becomes: what the run changed, the
  /// rows the host composed, and the baseline every diff is drawn against. The
  /// view paints it and nothing else — a click only names the card and the row.
  const card = {
    id: 4,
    kind: "changes",
    title: "Edited 2 files",
    totals: "+2 −9",
    baseline: "abc123",
    after: "def456",
    project: "/w",
    more: null,
    undoable: true,
    undone: false,
    rows: [
      {
        path: "src/main.rs",
        status: "modified",
        letter: "M",
        detail: "+2 −1",
        title: "Show src/main.rs in VS Code's diff editor",
        index: 0,
      },
      {
        path: "docs/old.md",
        status: "deleted",
        letter: "D",
        detail: "−8",
        title: "Show docs/old.md in VS Code's diff editor",
        index: 1,
      },
    ],
  };

  it("lists the turn's files with the badge, path and detail the host sent", () => {
    const { byId, send } = loadRenderer();
    send(stateMessage());
    const transcript = byId.get("transcript")!;
    send({ k: "push", item: card });

    const box = find(transcript, "changes")!;
    assert.equal(find(box, "changes-title")!.textContent, "Edited 2 files");
    assert.equal(find(box, "changes-total")!.textContent, "+2 −9");
    const rows = box.querySelectorAll(".change-row");
    assert.equal(rows.length, 2);
    assert.deepEqual(
      rows.map((row) => row.children.map((child) => child.textContent)),
      [
        ["M", "src/main.rs", "+2 −1"],
        ["D", "docs/old.md", "−8"],
      ],
    );
    // The badge is coloured by the status, and each row says what it opens.
    assert.equal(rows[1].children[0].className, "change-badge change-deleted");
    assert.equal(rows[0].title, "Show src/main.rs in VS Code's diff editor");
  });

  it("opens VS Code's diff editor for the row that was clicked", () => {
    const { byId, send, posted } = loadRenderer();
    send(stateMessage());
    const transcript = byId.get("transcript")!;
    send({ k: "push", item: card });

    const rows = find(transcript, "changes")!.querySelectorAll(".change-row");
    // A click that lands on the badge still belongs to the row it sits in.
    transcript.fire("click", { target: rows[1].children[0] });
    assert.deepEqual(last(posted), { k: "openChangeDiff", id: 4, index: 1 });
    transcript.fire("click", { target: rows[0] });
    assert.deepEqual(last(posted), { k: "openChangeDiff", id: 4, index: 0 });
  });

  it("opens the whole turn from the card's own icon", () => {
    const { byId, send, posted } = loadRenderer();
    send(stateMessage());
    const transcript = byId.get("transcript")!;
    send({ k: "push", item: card });

    const open = find(transcript, "changes-open")!;
    assert.equal(open.title, "Open every file in VS Code's diff editor");
    assert.equal(open.getAttribute("aria-label"), open.title);
    transcript.fire("click", { target: open });
    assert.deepEqual(last(posted), { k: "openAllChanges", id: 4 });

    // The header itself is words, so a click beside the icon is not a click on
    // it: nothing is opened by reading the listing.
    const before = posted.length;
    transcript.fire("click", { target: find(transcript, "changes-title")! });
    assert.equal(posted.length, before);
  });

  it("paints the card a pane that attaches later replays", () => {
    const { byId, send } = loadRenderer();
    send(stateMessage({ items: [card] }));
    const box = find(byId.get("transcript")!, "changes")!;
    assert.equal(box.querySelectorAll(".change-row").length, 2);
    // The card is a reply, so it takes the welcome page down with it.
    assert.equal(byId.get("empty")!.hidden, true);
  });

  it("leaves the total out when the turn changed no lines", () => {
    const { byId, send } = loadRenderer();
    send(stateMessage());
    const transcript = byId.get("transcript")!;
    send({ k: "push", item: { ...card, totals: "", rows: [{ ...card.rows[1], detail: "binary" }] } });
    assert.equal(find(transcript, "changes-total"), null);
    assert.equal(find(transcript, "change-detail")!.textContent, "binary");
  });

  it("offers Undo, which is the host's to confirm and the CLI's to do", () => {
    const { byId, send, posted } = loadRenderer();
    send(stateMessage());
    const transcript = byId.get("transcript")!;
    send({ k: "push", item: card });

    const undo = find(transcript, "changes-undo")!;
    assert.equal(undo.textContent, "Undo");
    assert.equal(undo.hidden, false, "the newest turn's card offers it");
    assert.equal(find(transcript, "changes-note")!.hidden, true);
    undo.fire("click");
    // Names the card rather than restoring: the host asks first, and the
    // restore itself is the confirmation's row.
    assert.deepEqual(last(posted), { k: "undoChanges", id: 4 });
  });

  it("takes the Undo away from a card a newer turn came after", () => {
    const { byId, send } = loadRenderer();
    send(stateMessage());
    const transcript = byId.get("transcript")!;
    send({ k: "push", item: card });
    send({ k: "changes", id: 4, undoable: false, undone: false });
    assert.equal(find(transcript, "changes-undo")!.hidden, true);
    assert.equal(find(transcript, "changes-note")!.hidden, true);

    // A pane that attaches later replays the same state, so the card it paints
    // offers nothing even though its listing is the same.
    const replayed = loadRenderer();
    replayed.send(stateMessage({ items: [{ ...card, undoable: false }] }));
    assert.equal(find(replayed.byId.get("transcript")!, "changes-undo")!.hidden, true);
  });

  it("says Undone once the turn has been put back", () => {
    const { byId, send } = loadRenderer();
    send(stateMessage());
    const transcript = byId.get("transcript")!;
    send({ k: "push", item: card });
    send({ k: "changes", id: 4, undoable: false, undone: true });

    const note = find(transcript, "changes-note")!;
    assert.equal(note.textContent, "Undone");
    assert.equal(note.hidden, false);
    assert.equal(find(transcript, "changes-undo")!.hidden, true);
    // The listing stays: what the turn did is still what it did.
    assert.equal(find(transcript, "changes")!.querySelectorAll(".change-row").length, 2);
  });

  it("folds a listing longer than the card shows behind one row", () => {
    const { byId, send } = loadRenderer();
    send(stateMessage());
    const transcript = byId.get("transcript")!;
    const rows = Array.from({ length: 7 }, (_, index) => ({
      path: `src/file${index}.rs`,
      status: "modified",
      letter: "M",
      detail: "+1 −1",
      title: `Show src/file${index}.rs in VS Code's diff editor`,
      index,
    }));
    send({
      k: "push",
      item: {
        ...card,
        title: "Edited 7 files",
        more: { visible: 5, closed: "+2 more files", open: "Show less" },
        rows,
      },
    });

    const listing = find(transcript, "changes")!.querySelectorAll(".change-row");
    assert.deepEqual(
      listing.map((row) => row.hidden),
      [false, false, false, false, false, true, true],
    );
    const more = find(transcript, "changes-more")!;
    assert.equal(more.textContent, "+2 more files");
    more.fire("click");
    assert.deepEqual(listing.map((row) => row.hidden).every((hidden) => !hidden), true);
    assert.equal(more.textContent, "Show less");
    more.fire("click");
    assert.equal(listing[6].hidden, true);
    assert.equal(more.textContent, "+2 more files");

    // A card whose listing fits has no such row at all.
    const small = loadRenderer();
    small.send(stateMessage({ items: [card] }));
    assert.equal(find(small.byId.get("transcript")!, "changes-more"), null);
  });
});

describe("webview review", () => {
  /// The turn's files read over the panel the way a changes view reads: the
  /// card's own rows, walked with the arrow keys, each one opening that file in
  /// VS Code's own diff editor — where both sides are drawn — rather than the
  /// panel painting a diff of its own.
  const card = {
    id: 4,
    kind: "changes",
    title: "Edited 2 files",
    totals: "+3 −9",
    baseline: "abc123",
    rows: [
      {
        path: "src/main.rs",
        status: "modified",
        letter: "M",
        detail: "+3 −1",
        title: "Show src/main.rs in VS Code's diff editor",
        index: 0,
      },
      {
        path: "docs/old.md",
        status: "deleted",
        letter: "D",
        detail: "−8",
        title: "Show docs/old.md in VS Code's diff editor",
        index: 1,
      },
    ],
  };

  function reviewed(rows = card.rows) {
    const harness = loadRenderer();
    harness.send(stateMessage());
    harness.send({ k: "push", item: { ...card, rows } });
    return {
      ...harness,
      /// The messages that name a file to open, which the card's own rows post
      /// the same way — the review adds only that it is a review, so the host
      /// opens the diff as a preview beside the panel.
      diffs: () =>
        harness.posted
          .filter((message) => message.k === "openChangeDiff")
          .map((message) => shape(message)),
      open: () => {
        const button = find(harness.byId.get("transcript")!, "changes-review")!;
        button.fire("click");
      },
    };
  }

  it("opens the turn's files from the card's Review button", () => {
    const { byId, open, diffs } = reviewed();
    assert.equal(byId.get("review")!.hidden, true, "nothing is open until it is asked for");
    open();
    assert.equal(byId.get("review")!.hidden, false);
    assert.equal(byId.get("review-title")!.textContent, "Edited 2 files");
    assert.equal(byId.get("review-total")!.textContent, "+3 −9");
    // The rows are the card's, which is what keeps the two listings from
    // disagreeing about the turn.
    const rows = byId.get("review-files")!.querySelectorAll(".change-row");
    assert.deepEqual(
      rows.map((row) => row.children.map((child) => child.textContent)),
      [
        ["M", "src/main.rs", "+3 −1"],
        ["D", "docs/old.md", "−8"],
      ],
    );
    assert.equal(rows[0].classList.contains("active"), true);
    assert.equal(rows[0].getAttribute("aria-selected"), "true");
    // It opens on the first file, and says it is the review — so the host draws
    // it in the editor's own diff view, as a preview beside the listing.
    assert.deepEqual(diffs(), [{ k: "openChangeDiff", id: 4, index: 0, review: true }]);
  });

  it("paints no diff of its own, since the editor beside it draws both sides", () => {
    const { byId, open } = reviewed([
      card.rows[0],
      { ...card.rows[1], detail: "binary" },
    ]);
    open();
    const sheet = byId.get("review")!;
    assert.equal(find(sheet, "diff"), null, "no diff block in the review");
    assert.equal(sheet.querySelectorAll(".dline").length, 0);
    const rows = byId.get("review-files")!.querySelectorAll(".change-row");
    assert.equal(rows.length, 2);
    // What kind of file it is still reads on the row itself, which is the only
    // place a file with nothing to line up says so.
    assert.equal(rows[1].children[2].textContent, "binary");
  });

  it("opens nothing for a card with no files to review", () => {
    const { byId, send } = loadRenderer();
    send(stateMessage());
    send({ k: "push", item: { ...card, rows: [], totals: "" } });
    assert.equal(find(byId.get("transcript")!, "changes-review"), null);
    assert.equal(byId.get("review")!.hidden, true);
  });

  it("walks the files with the arrow keys, wrapping at both ends", () => {
    const { byId, fireDocument, open, diffs } = reviewed();
    open();
    let prevented = false;
    const key = (name: string) => fireDocument("keydown", { key: name, preventDefault: () => { prevented = true; } });

    key("ArrowDown");
    assert.equal(prevented, true, "the arrows do not also move the caret");
    const rows = byId.get("review-files")!.querySelectorAll(".change-row");
    assert.equal(rows[1].classList.contains("active"), true);
    assert.equal(rows[0].classList.contains("active"), false);
    // Walking to a file is what opens it, so the arrows move the diff in the
    // editor beside the panel rather than only the highlight.
    assert.deepEqual(last(diffs()), { k: "openChangeDiff", id: 4, index: 1, review: true });
    assert.equal(diffs().length, 2, "the file it opened on, then the one it walked to");

    // Past the last file it comes back to the first, and before the first to
    // the last.
    key("ArrowDown");
    assert.equal(
      byId.get("review-files")!.querySelectorAll(".change-row")[0].classList.contains("active"),
      true,
    );
    assert.deepEqual(last(diffs()), { k: "openChangeDiff", id: 4, index: 0, review: true });
    key("ArrowUp");
    assert.equal(
      byId.get("review-files")!.querySelectorAll(".change-row")[1].classList.contains("active"),
      true,
    );
    assert.deepEqual(last(diffs()), { k: "openChangeDiff", id: 4, index: 1, review: true });

    // Any other key is left to the panel: the composer still stops a turn with
    // Escape while the review is up, since the review handles none.
    const opened = diffs().length;
    prevented = false;
    key("ArrowLeft");
    assert.equal(prevented, false);
    assert.equal(diffs().length, opened, "a key the review does not handle opens nothing");
    fireDocument("keydown", { key: "Escape" });
    assert.equal(byId.get("review")!.hidden, true);
    key("ArrowDown");
    assert.equal(byId.get("review")!.hidden, true, "a closed review walks nothing");
    assert.equal(diffs().length, opened, "and opens nothing");
  });

  it("opens the file a click on the listing names", () => {
    const { byId, open, diffs } = reviewed();
    open();
    byId.get("review-files")!.querySelectorAll(".change-row")[1].fire("click");
    assert.equal(
      byId.get("review-files")!.querySelectorAll(".change-row")[1].classList.contains("active"),
      true,
    );
    assert.deepEqual(last(diffs()), { k: "openChangeDiff", id: 4, index: 1, review: true });
  });

  it("closes its ✕, and closes with a rebuilt transcript", () => {
    const { byId, send, open } = reviewed();
    open();
    byId.get("review-close")!.fire("click");
    assert.equal(byId.get("review")!.hidden, true);
    // A state replay rebuilds the transcript the card belonged to, so a review
    // left open would be showing a card that is no longer there.
    open();
    send(stateMessage());
    assert.equal(byId.get("review")!.hidden, true);
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
    // An edit card is the one line it reads as until it is asked for, so it is
    // clicked before its output is on screen at all.
    transcript.fire("click", { target: find(transcript, "thead")! });
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

/// The call that changed a file reads as one line — the path in its header, the
/// counts beside it — because the turn's changes are listed together by the
/// change card at the end of the run. Its own diff is kept for the reader who
/// clicks the card, and never painted twice.
describe("webview tool card", () => {
  const editCard = (over: Record<string, unknown> = {}) => ({
    k: "push",
    item: {
      id: 4,
      kind: "tool",
      name: "edit",
      args: JSON.stringify({ path: "src/main.rs" }),
      output: "Successfully replaced 1 block(s) in src/main.rs.",
      diff: "@@ -1 +1 @@\n-old\n+new\n+another",
      running: false,
      ...over,
    },
  });

  it("shows an edited file as one line with its counts", () => {
    const { byId, send } = loadRenderer();
    send(stateMessage());
    const transcript = byId.get("transcript")!;
    send(editCard());

    const card = find(transcript, "tool")!;
    assert.equal(card.querySelector(".targ")!.textContent, "src/main.rs");
    assert.equal(card.querySelector(".tstate")!.textContent, "✔ +2 −1");
    assert.equal(card.querySelector(".tbody")!.hidden, true, "the body stays folded");
    assert.equal(card.querySelector(".thint")!.hidden, true, "there is nothing hidden but the diff");
    const diff = card.children[card.children.length - 1]!;
    assert.equal(diff.hidden, true, "the diff is held back");
    assert.match(diff.textContent, /src\/main\.rs/);
  });

  it("shows the call's own diff when the card is clicked, and folds it again", () => {
    const { byId, send } = loadRenderer();
    send(stateMessage());
    const transcript = byId.get("transcript")!;
    send(editCard());
    const card = find(transcript, "tool")!;
    const diff = card.children[card.children.length - 1]!;
    const head = card.querySelector(".thead")!;
    // A card with a diff has one line to unfold, so its header is the control
    // that opens it.
    assert.equal(head.getAttribute("role"), "button");
    assert.equal(head.tabIndex, 0);

    transcript.fire("click", { target: head });
    assert.equal(diff.hidden, false, "the diff is what the card was asked for");
    assert.equal(card.querySelector(".tbody")!.hidden, false, "and the result comes back with it");
    assert.equal(head.getAttribute("aria-expanded"), "true");

    transcript.fire("click", { target: head });
    assert.equal(diff.hidden, true, "clicking again folds it away");
    assert.equal(card.querySelector(".tbody")!.hidden, true);
  });

  it("opens a folded result from the row that offers it, and folds it back", () => {
    const { byId, send } = loadRenderer();
    send(stateMessage());
    const transcript = byId.get("transcript")!;
    send({
      k: "push",
      item: {
        id: 6,
        kind: "tool",
        name: "bash",
        args: JSON.stringify({ command: "cargo test" }),
        output: Array.from({ length: 40 }, (_, line) => `line ${line}`).join("\n"),
        running: false,
      },
    });

    const card = find(transcript, "tool")!;
    const body = card.querySelector(".tbody")!;
    const hint = card.querySelector(".thint")!;
    const preview = body.textContent;
    assert.equal(hint.textContent, "Show 37 earlier lines");
    assert.equal(hint.getAttribute("aria-expanded"), "false");

    // The words offering the rest of the command are the same handle as the
    // header's caret, so clicking them is what opens the card.
    transcript.fire("click", { target: hint });
    assert.equal(hint.textContent, "Show less", "the row offers the fold back where the output ends");
    assert.equal(hint.getAttribute("aria-expanded"), "true");
    assert.equal(card.classList.contains("expanded"), true);
    assert.match(body.textContent, /^line 0/, "the whole result, from its first line");
    assert.ok(body.textContent.length > preview.length);

    transcript.fire("click", { target: hint });
    assert.equal(hint.textContent, "Show 37 earlier lines", "the same row folds it away again");
    assert.equal(card.classList.contains("expanded"), false);
    assert.equal(body.textContent, preview);

    // The body is the output itself, selected and copied rather than clicked.
    transcript.fire("click", { target: body });
    assert.equal(body.textContent, preview, "a click in the output folds nothing");
  });

  it("toggles a folded card from its handle with the keyboard", () => {
    const { byId, send } = loadRenderer();
    send(stateMessage());
    const transcript = byId.get("transcript")!;
    send({
      k: "push",
      item: {
        id: 7,
        kind: "tool",
        name: "read",
        args: JSON.stringify({ path: "src/main.rs" }),
        output: Array.from({ length: 30 }, (_, line) => `line ${line}`).join("\n"),
        running: false,
      },
    });

    const card = find(transcript, "tool")!;
    const hint = card.querySelector(".thint")!;
    assert.equal(hint.textContent, "Show 26 more lines");
    assert.equal(hint.getAttribute("role"), "button", "the handle is reachable with Tab");
    assert.equal(hint.tabIndex, 0);

    let prevented = false;
    transcript.fire("keydown", { key: "Enter", target: hint, preventDefault: () => { prevented = true; } });
    assert.equal(prevented, true, "a handled key does not also act on the page");
    assert.equal(hint.textContent, "Show less");
    transcript.fire("keydown", { key: " ", target: hint, preventDefault: () => { prevented = true; } });
    assert.equal(hint.textContent, "Show 26 more lines");
    // Any other key is left alone, so the panel keeps its own shortcuts.
    transcript.fire("keydown", { key: "a", target: hint, preventDefault: () => { prevented = true; } });
    assert.equal(hint.textContent, "Show 26 more lines");
  });

  it("leaves a call that changed nothing reading as before", () => {
    const { byId, send } = loadRenderer();
    send(stateMessage());
    const transcript = byId.get("transcript")!;
    send({
      k: "push",
      item: {
        id: 5,
        kind: "tool",
        name: "read",
        args: JSON.stringify({ path: "src/main.rs" }),
        output: Array.from({ length: 30 }, (_, line) => `line ${line}`).join("\n"),
        running: false,
      },
    });

    const card = find(transcript, "tool")!;
    const body = card.querySelector(".tbody")!;
    assert.equal(body.hidden, false, "a plain result keeps its preview");
    assert.match(body.textContent, /line 0/);
    assert.equal(card.querySelector(".thint")!.hidden, false);
    assert.equal(card.querySelector(".tstate")!.textContent, "✔");
    assert.equal(card.classList.contains("foldable"), true, "there is a rest to open");
  });

  it("draws no fold on a card that already shows everything it has", () => {
    const { byId, send } = loadRenderer();
    send(stateMessage());
    const transcript = byId.get("transcript")!;
    send({
      k: "push",
      item: {
        id: 8,
        kind: "tool",
        name: "bash",
        args: JSON.stringify({ command: "git status --short" }),
        output: " M src/main.rs",
        running: false,
      },
    });

    const card = find(transcript, "tool")!;
    const head = card.querySelector(".thead")!;
    assert.equal(card.querySelector(".tbody")!.textContent, " M src/main.rs");
    assert.equal(card.querySelector(".thint")!.hidden, true);
    // Nothing behind the header, so it is not a control at all: no caret, no tab
    // stop, and no button for a screen reader to be told about.
    assert.equal(card.classList.contains("foldable"), false);
    assert.equal(head.getAttribute("role"), null);
    assert.equal(head.getAttribute("aria-expanded"), null);
    assert.equal(head.tabIndex, -1);
    transcript.fire("click", { target: head });
    assert.equal(card.classList.contains("foldable"), false);
    assert.equal(card.classList.contains("expanded"), false, "nothing to open");
    assert.equal(card.querySelector(".tbody")!.textContent, " M src/main.rs");
  });

  it("marks a card replayed from a stored thread as unrecorded, not successful", () => {
    const { byId, send } = loadRenderer();
    send(stateMessage());
    const transcript = byId.get("transcript")!;
    send(editCard({ unknown: true, output: "oldText not found in src/main.rs" }));

    const card = find(transcript, "tool")!;
    const state = card.querySelector(".tstate")!;
    // A stored thread records neither the file the call found nor whether it
    // applied, so the card claims no state: not the green of one that landed.
    assert.equal(state.textContent, "• +2 −1");
    assert.equal(card.classList.contains("done"), false);
    assert.equal(card.classList.contains("error"), false, "a failure reads the same as a success");
    assert.equal(card.classList.contains("unknown"), true);
    assert.match(state.title, /not recorded/);
    // The change it asked for is still what the card unfolds.
    assert.equal(card.classList.contains("foldable"), true);
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
