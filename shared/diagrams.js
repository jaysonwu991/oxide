/* Shared by the desktop window and the VS Code webview. */
(() => {
  "use strict";
  const script = document.currentScript;
  let library;
  let queue = Promise.resolve();
  let nextId = 0;
  const cache = new Map();
  const figures = new Map();
  const records = new WeakMap();
  let fullscreen;
  let scheduled = false;

  const labels = {
    en: { title: "Mermaid diagram", source: "Diagram source", loading: "Rendering diagram…", error: "Could not render diagram. The source is available below.", expand: "View fullscreen", close: "Close diagram" },
    zh: { title: "Mermaid 图表", source: "图表源码", loading: "正在渲染图表…", error: "无法渲染图表。可在下方查看源码。", expand: "全屏查看", close: "关闭图表" },
  };
  const text = (key) => labels[document.documentElement.lang.startsWith("zh") ? "zh" : "en"][key];

  function theme() {
    const color = getComputedStyle(document.body).backgroundColor;
    const rgb = color.match(/[\d.]+/g)?.slice(0, 3).map(Number);
    return rgb?.length === 3 && rgb[0] * 0.2126 + rgb[1] * 0.7152 + rgb[2] * 0.0722 < 128 ? "dark" : "default";
  }

  function load() {
    if (!library) library = new Promise((resolve, reject) => {
      const node = document.createElement("script");
      node.src = new URL("vendor/mermaid.min.js", script.src).href;
      if (script.nonce) node.nonce = script.nonce;
      node.onload = () => globalThis.mermaid ? resolve(globalThis.mermaid) : reject(new Error("Mermaid unavailable"));
      node.onerror = () => reject(new Error("Mermaid unavailable"));
      document.head.appendChild(node);
    });
    return library;
  }

  function render(source, appearance) {
    const key = `${appearance}\n${source}`;
    if (cache.has(key)) {
      const result = cache.get(key);
      cache.delete(key);
      cache.set(key, result);
      return result;
    }
    // Mermaid configuration is global, so initialization and rendering must be
    // serialized together. A rejection must not poison subsequent diagrams.
    const result = queue.then(async () => {
      const mermaid = await load();
      mermaid.initialize({
        startOnLoad: false,
        securityLevel: "strict",
        suppressErrorRendering: true,
        theme: appearance,
        fontFamily: "system-ui, sans-serif",
        htmlLabels: false,
        maxTextSize: 50000,
        maxEdges: 500,
        secure: ["securityLevel", "startOnLoad", "suppressErrorRendering", "maxTextSize", "maxEdges", "htmlLabels", "fontFamily", "themeCSS"],
      });
      const { svg } = await mermaid.render(`oxide-mermaid-${++nextId}`, source);
      // An SVG image isolates IDs between repeated diagrams and keeps diagram
      // links and markup out of the webview's actionable DOM.
      return `data:image/svg+xml;charset=utf-8,${encodeURIComponent(svg)}`;
    });
    queue = result.catch(() => undefined);
    cache.set(key, result);
    if (cache.size > 32) cache.delete(cache.keys().next().value);
    return result;
  }

  function button(label, path) {
    const node = document.createElement("button");
    node.type = "button";
    node.className = "diagram-button";
    node.title = node.ariaLabel = label;
    node.innerHTML = `<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" aria-hidden="true"><path d="${path}" /></svg>`;
    return node;
  }

  function open(record) {
    fullscreen?.dialog.close();
    const dialog = document.createElement("dialog");
    dialog.className = "diagram-fullscreen";
    dialog.ariaLabel = text("title");
    const header = document.createElement("div");
    header.className = "diagram-header";
    const title = document.createElement("span");
    title.textContent = text("title");
    const close = button(text("close"), "m6 6 12 12M6 18 18 6");
    close.onclick = () => dialog.close();
    header.append(title, close);
    const viewport = document.createElement("div");
    viewport.className = "diagram-viewport";
    viewport.tabIndex = 0;
    const image = record.image.cloneNode();
    viewport.append(image);
    dialog.append(header, viewport);
    dialog.onclick = (event) => { if (event.target === dialog) dialog.close(); };
    dialog.addEventListener("keydown", (event) => event.stopPropagation());
    dialog.addEventListener("close", () => {
      dialog.remove();
      if (fullscreen?.dialog === dialog) fullscreen = undefined;
      if (record.expand.isConnected) record.expand.focus();
    }, { once: true });
    document.body.append(dialog);
    fullscreen = { dialog, image, record };
    dialog.showModal();
    close.focus();
  }

  function update(record, appearance) {
    record.appearance = appearance;
    record.pending = true;
    const version = ++record.version;
    record.image.hidden = true;
    record.expand.disabled = true;
    record.status.hidden = false;
    record.status.textContent = text("loading");
    render(record.source, appearance).then((url) => {
      if (!record.figure.isConnected || record.version !== version) return;
      record.pending = false;
      record.image.src = url;
      record.image.hidden = false;
      record.expand.disabled = false;
      record.status.hidden = true;
      record.details.open = false;
      if (fullscreen?.record === record) fullscreen.image.src = url;
      notifyLayout(record.figure);
    }, () => {
      if (!record.figure.isConnected || record.version !== version) return;
      record.pending = false;
      record.status.textContent = text("error");
      record.details.open = true;
      notifyLayout(record.figure);
    });
  }

  function mount(pre, appearance) {
    const source = pre.textContent;
    pre.removeAttribute("data-complete");
    const figure = document.createElement("figure");
    figure.className = "mermaid-diagram";
    const header = document.createElement("figcaption");
    header.className = "diagram-header";
    const title = document.createElement("span");
    title.textContent = text("title");
    const expand = button(text("expand"), "M8 3H3v5m13-5h5v5M3 16v5h5m13-5v5h-5");
    expand.disabled = true;
    header.append(title, expand);
    const status = document.createElement("p");
    status.setAttribute("role", "status");
    const viewport = document.createElement("div");
    viewport.className = "diagram-viewport";
    viewport.tabIndex = 0;
    const image = document.createElement("img");
    image.alt = text("title");
    image.draggable = false;
    image.hidden = true;
    image.onload = () => notifyLayout(figure);
    viewport.append(image);
    const details = document.createElement("details");
    details.open = true;
    const summary = document.createElement("summary");
    summary.textContent = text("source");
    pre.replaceWith(figure);
    details.append(summary, pre);
    figure.append(header, status, viewport, details);
    const record = { figure, source, image, expand, status, details, version: 0 };
    expand.onclick = () => open(record);
    figures.set(figure, record);
    records.set(figure, record);
    details.addEventListener("toggle", () => notifyLayout(figure));
    update(record, appearance);
    notifyLayout(figure);
  }

  function notifyLayout(figure) {
    if (figure.isConnected) figure.dispatchEvent(new Event("oxide-diagram-layout", { bubbles: true }));
  }

  // Keep the same figures when a streaming message replaces its surrounding
  // prose, including two identical diagrams at distinct positions.
  globalThis.OxideDiagrams = {
    paint(body, html) {
      const previous = [...body.querySelectorAll(".mermaid-diagram")];
      body.innerHTML = html;
      for (const pre of body.querySelectorAll('pre[data-lang="mermaid"][data-complete="true"]')) {
        const index = previous.findIndex((figure) => records.get(figure)?.source === pre.textContent);
        if (index < 0) continue;
        pre.replaceWith(previous.splice(index, 1)[0]);
      }
    },
  };

  function scan() {
    scheduled = false;
    const appearance = theme();
    for (const [figure, record] of figures) {
      if (!figure.isConnected) {
        figures.delete(figure);
        if (record.pending) record.version++;
      } else if (record.appearance !== appearance) update(record, appearance);
    }
    // Tabs park actual DOM nodes. Weak state lets a restored figure recover
    // without keeping discarded transcripts alive through the active map.
    for (const figure of document.querySelectorAll(".msg.assistant .mermaid-diagram")) {
      const record = records.get(figure);
      if (!record || figures.has(figure)) continue;
      figures.set(figure, record);
      if (record.pending || record.appearance !== appearance) update(record, appearance);
    }
    for (const pre of document.querySelectorAll('.msg.assistant pre[data-lang="mermaid"][data-complete="true"]')) mount(pre, appearance);
    if (fullscreen && !fullscreen.record.figure.isConnected) fullscreen.dialog.close();
  }

  function schedule() {
    if (scheduled) return;
    scheduled = true;
    requestAnimationFrame(scan);
  }
  new MutationObserver(schedule).observe(document.body, { childList: true, subtree: true });
  const themes = new MutationObserver(schedule);
  themes.observe(document.documentElement, { attributes: true, attributeFilter: ["style", "class", "lang"] });
  themes.observe(document.body, { attributes: true, attributeFilter: ["style", "class"] });
  schedule();
})();
