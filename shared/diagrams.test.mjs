import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";
import vm from "node:vm";

const renderer = readFileSync(new URL("diagrams.js", import.meta.url), "utf8");

function page(render = async (_id, source) => ({ svg: `<svg><text>${source}</text></svg>` })) {
  const nodes = [];
  class Element {
    constructor(tag) { this.tag = tag; this.children = []; this.attributes = {}; this.listeners = {}; this.hidden = false; nodes.push(this); }
    get isConnected() { return this === document.documentElement || Boolean(this.parent?.isConnected); }
    get textContent() { return this.ownText ?? this.children.map((node) => node.textContent).join(""); }
    set textContent(value) { this.ownText = value; }
    append(...children) { for (const node of children) { node.remove(); node.parent = this; this.children.push(node); } }
    appendChild(node) { this.append(node); if (this === document.head && node.tag === "script") queueMicrotask(() => node.onload()); }
    remove() { if (this.parent) this.parent.children = this.parent.children.filter((node) => node !== this); this.parent = undefined; }
    replaceWith(node) { const parent = this.parent; this.remove(); parent.append(node); }
    setAttribute(key, value) { this.attributes[key] = value; }
    removeAttribute(key) { delete this.attributes[key]; }
    cloneNode() { const node = new Element(this.tag); node.src = this.src; return node; }
    addEventListener(key, fn) { this.listeners[key] = fn; }
    focus() { document.activeElement = this; }
    showModal() { this.open = true; }
    close() { this.open = false; this.listeners.close?.(); }
  }
  const observers = [];
  const frames = [];
  const document = {
    createElement: (tag) => new Element(tag),
    querySelectorAll(selector) {
      assert.equal(selector, '.msg.assistant pre[data-lang="mermaid"][data-complete="true"]');
      return nodes.filter((node) => node.tag === "pre" && node.isConnected && node.attributes["data-complete"] === "true" && node.eligible);
    },
    currentScript: { src: "https://local.invalid/media/diagrams.js", nonce: "test-nonce" },
  };
  document.documentElement = new Element("html");
  document.documentElement.lang = "en";
  document.head = new Element("head");
  document.body = new Element("body");
  document.documentElement.append(document.head, document.body);
  let background = "rgb(28, 28, 28)";
  let config;
  const configurations = [];
  const calls = [];
  const context = vm.createContext({
    document, URL, queueMicrotask, encodeURIComponent,
    getComputedStyle: () => ({ backgroundColor: background }),
    requestAnimationFrame: (fn) => frames.push(fn),
    MutationObserver: class { constructor(fn) { observers.push(fn); } observe() {} },
    mermaid: {
      initialize(value) { config = value; configurations.push(value); },
      async render(id, source) { calls.push({ id, source, config }); return render(id, source); },
    },
  });
  vm.runInContext(renderer, context);
  function code(source, eligible = true, complete = true) {
    const pre = new Element("pre");
    pre.eligible = eligible;
    pre.textContent = source;
    if (complete) pre.setAttribute("data-complete", "true");
    document.body.append(pre);
    return pre;
  }
  async function flush() {
    for (const observer of observers) observer();
    for (const frame of frames.splice(0)) frame();
    await new Promise((resolve) => setImmediate(resolve));
  }
  return { document, nodes, code, flush, calls, configurations, light() { background = "rgb(255, 255, 255)"; } };
}
const figures = (p) => p.nodes.filter((node) => node.className === "mermaid-diagram" && node.isConnected);
const descendants = (node) => node.children.flatMap((child) => [child, ...descendants(child)]);
const find = (node, tag) => descendants(node).find((child) => child.tag === tag);

test("renders complete assistant diagrams locally and preserves source", async () => {
  const p = page();
  const source = "flowchart LR\n A --> B";
  const pre = p.code(source);
  p.code("user source", false);
  p.code("unfinished", true, false);
  await p.flush();
  assert.equal(figures(p).length, 1);
  const figure = figures(p)[0];
  assert.equal(pre.textContent, source);
  assert.equal(find(figure, "details").open, false);
  assert.match(find(figure, "img").src, /^data:image\/svg\+xml/);
  assert.equal(p.calls[0].config.securityLevel, "strict");
  assert.equal(p.calls[0].config.htmlLabels, false);
  const script = p.nodes.find((node) => node.tag === "script");
  assert.equal(script.src, "https://local.invalid/media/vendor/mermaid.min.js");
  assert.equal(script.nonce, "test-nonce");
});

test("serializes jobs and recovers after an invalid diagram", async () => {
  let active = 0;
  let maximum = 0;
  const p = page(async (_id, source) => {
    maximum = Math.max(maximum, ++active);
    await new Promise((resolve) => setImmediate(resolve));
    active--;
    if (source === "invalid") throw new Error("parse error");
    return { svg: "<svg />" };
  });
  p.code("invalid"); p.code("flowchart LR; A --> B");
  await p.flush(); await p.flush(); await p.flush();
  assert.equal(maximum, 1);
  assert.equal(p.calls.length, 2);
  const [bad, good] = figures(p);
  assert.equal(find(bad, "details").open, true);
  assert.match(find(bad, "p").textContent, /Could not render/);
  assert.equal(find(good, "img").hidden, false);
});

test("caches streamed repeats and rerenders when the theme changes", async () => {
  const p = page();
  p.code("flowchart LR; A --> B");
  await p.flush();
  p.code("flowchart LR; A --> B");
  await p.flush();
  assert.equal(p.calls.length, 1);
  assert.equal(p.calls[0].config.theme, "dark");
  p.light(); await p.flush();
  assert.equal(p.calls.length, 2);
  assert.equal(p.calls[1].config.theme, "default");
});

test("drops a late result after a chat switch and a stale theme result", async () => {
  let resolve;
  const p = page(() => new Promise((done) => { resolve = done; }));
  p.code("flowchart LR; A --> B");
  await p.flush();
  const old = figures(p)[0];
  old.remove();
  resolve({ svg: "<svg>old</svg>" });
  await p.flush();
  assert.equal(find(old, "img").src, undefined);

  p.code("flowchart LR; C --> D"); await p.flush();
  const current = figures(p)[0];
  p.light(); await p.flush();
  resolve({ svg: "<svg>dark</svg>" }); await p.flush();
  assert.equal(find(current, "img").src, undefined);
  resolve({ svg: "<svg>light</svg>" }); await p.flush();
  assert.match(decodeURIComponent(find(current, "img").src), /light/);
});

test("fullscreen closes on dismissal and restores keyboard focus", async () => {
  const p = page(); p.code("flowchart LR; A --> B"); await p.flush();
  const figure = figures(p)[0];
  const expand = find(figure, "button");
  expand.onclick();
  const dialog = p.nodes.find((node) => node.tag === "dialog");
  assert.equal(dialog.open, true);
  find(dialog, "button").onclick();
  assert.equal(dialog.isConnected, false);
  assert.equal(p.document.activeElement, expand);
  expand.onclick();
  figure.remove(); await p.flush();
  assert.equal(p.nodes.filter((node) => node.tag === "dialog" && node.isConnected).length, 0);
});
