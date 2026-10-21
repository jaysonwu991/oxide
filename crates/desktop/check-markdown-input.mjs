// Real browser coverage for both rendered composers. Run with a Chromium binary:
// node crates/desktop/check-markdown-input.mjs /path/to/chrome-headless-shell
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { pathToFileURL } from "node:url";

const browser = process.argv[2];
assert.ok(browser, "Pass a Chromium or chrome-headless-shell executable");
const desktop = readFileSync(new URL("ui/app.js", import.meta.url), "utf8");
const extension = readFileSync(new URL("../../editors/vscode/media/main.js", import.meta.url), "utf8");
const editorCode = (source) => {
  const start = source.indexOf("function installMarkdownInput");
  return source.slice(start, source.indexOf("\n}\n", start) + 3);
};
assert.equal(editorCode(desktop), editorCode(extension), "Both apps use the same editing behavior");
const referenceCode = (source) => {
  const start = source.indexOf("function referenceRanges");
  return source.slice(start, source.indexOf("\n}\n", start) + 3);
};

function exerciseEditor() {
  const editor = document.querySelector("[contenteditable]");
  const results = document.querySelector("#results");
  installMarkdownInput(editor, (text) => renderMarkdown(text, true));
  const check = (name, pass) => {
    if (!pass) throw new Error(`${name}: ${JSON.stringify({ source: editor.value, html: editor.innerHTML })}`);
    results.textContent += `PASS ${name}\n`;
  };
  const end = () => {
    editor.focus();
    editor.setSelectionRange(editor.value.length, editor.value.length);
  };
  const type = (text) => {
    const event = new InputEvent("beforeinput", { inputType: "insertText", data: text, bubbles: true, cancelable: true });
    if (editor.dispatchEvent(event)) document.execCommand("insertText", false, text);
  };
  try {
    const emptyHeight = editor.getBoundingClientRect().height;
    const computed = getComputedStyle(editor);
    const oneLine = parseFloat(computed.lineHeight) + parseFloat(computed.paddingTop) + parseFloat(computed.paddingBottom);
    check("empty composer uses its minimum height", Math.abs(emptyHeight - Math.max(oneLine, parseFloat(computed.minHeight) || 0)) <= 1);
    editor.value = "first\nsecond\nthird\nfourth";
    check("composer grows with multiline text", editor.getBoundingClientRect().height > emptyHeight);
    editor.style.height = `${editor.scrollHeight}px`;
    editor.value = "";
    check("clearing resets multiline height", Math.abs(editor.getBoundingClientRect().height - emptyHeight) <= 1);
    editor.value = Array(30).fill("line").join("\n");
    editor.style.height = "200px";
    editor.scrollTop = editor.scrollHeight;
    editor.value = "";
    check("clearing a scrolled composer resets height", Math.abs(editor.getBoundingClientRect().height - emptyHeight) <= 1 && editor.scrollTop === 0);
    editor.value = "Test `Markdown`";
    end();
    check("single rendered surface", editor.textContent === "Test Markdown" && editor.querySelector("code"));
    type("!");
    check("typing after code", editor.value === "Test `Markdown`!");
    const codeCaret = (offset) => {
      editor.focus();
      const text = editor.querySelector("code").firstChild;
      const range = document.createRange();
      range.setStart(text, offset);
      range.collapse(true);
      const selection = window.getSelection();
      selection.removeAllRanges();
      selection.addRange(range);
    };
    editor.value = "Use `code` here";
    codeCaret(4);
    const right = new KeyboardEvent("keydown", { key: "ArrowRight", cancelable: true });
    editor.dispatchEvent(right);
    type(" next");
    check("right arrow leaves inline code", right.defaultPrevented && editor.value === "Use `code` next here");
    editor.value = "Use `code` here";
    codeCaret(4);
    type(" ");
    type("next");
    check("space leaves existing inline code", editor.value === "Use `code` next here" && editor.querySelector("code").textContent === "code");
    editor.value = "Use `code`";
    codeCaret(4);
    type(" ");
    type("next");
    check("space leaves inline code at the end of the draft", editor.value === "Use `code` next" && editor.querySelector("code").textContent === "code");
    editor.value = "Use `code` here";
    codeCaret(0);
    const left = new KeyboardEvent("keydown", { key: "ArrowLeft", cancelable: true });
    editor.dispatchEvent(left);
    type("plain ");
    check("left arrow leaves inline code", left.defaultPrevented && editor.value === "Use plain `code` here");
    editor.value = "Use `code` here";
    codeCaret(2);
    const inside = new KeyboardEvent("keydown", { key: "ArrowRight", cancelable: true });
    editor.dispatchEvent(inside);
    check("arrows inside code stay native", !inside.defaultPrevented);
    codeCaret(4);
    const extend = new KeyboardEvent("keydown", { key: "ArrowRight", shiftKey: true, cancelable: true });
    editor.dispatchEvent(extend);
    check("shift arrow selection stays native", !extend.defaultPrevented);
    codeCaret(2);
    type(" ");
    check("spaces inside code stay inside code", editor.value === "Use `co de` here");
    editor.value = "Use `code` here";
    editor.setSelectionRange(6, 8);
    type(" ");
    check("replacing selected code with a space stays inside code", editor.value === "Use `c e` here");
    editor.value = "";
    end();
    for (const character of "**bold**") type(character);
    check("live formatting", editor.querySelector("strong")?.textContent === "bold");
    editor.setSelectionRange(4, 4);
    type("X");
    check("editing inside formatting", editor.value === "**boXld**");
    editor.value = "hello";
    end();
    editor.dispatchEvent(new InputEvent("beforeinput", { inputType: "insertLineBreak", bubbles: true, cancelable: true }));
    check("newline is visible before typing", editor.textContent === "hello\n" && editor.querySelector("[data-editor-padding]"));
    const selection = window.getSelection();
    check("caret stays on the new line", selection.focusNode.nodeType === Node.TEXT_NODE && selection.focusNode.nodeValue === "\n" && selection.focusOffset === 1);
    type("world");
    check("newlines", editor.value === "hello\nworld");
    editor.value = "";
    end();
    for (let i = 0; i < 2; i++) editor.dispatchEvent(new InputEvent("beforeinput", { inputType: "insertLineBreak", bubbles: true, cancelable: true }));
    type(" ");
    type("next");
    check("leading blank lines and spaces", editor.value === "\n\n next");
    editor.value = "hello";
    end();
    type(" ");
    type(" ");
    check("consecutive spaces stay visible", editor.value === "hello  " && editor.textContent === "hello  " && window.getSelection().focusNode.nodeValue === "  ");
    editor.dispatchEvent(new InputEvent("beforeinput", { inputType: "insertLineBreak", bubbles: true, cancelable: true }));
    document.execCommand("delete");
    check("deleting a trailing newline preserves spaces", editor.value === "hello  ");
    editor.value = "```js\nconst x = 1;\n```";
    end();
    check("caret after a code fence", editor.querySelector("pre code")?.textContent === "const x = 1;");
    editor.setSelectionRange(18, 18);
    type(" ");
    check("space inside a code fence stays in the fence", editor.value === "```js\nconst x = 1; \n```");
    editor.value = "```\nfirst\n\n\nlast\n```";
    const code = editor.querySelector("code").firstChild;
    const range = document.createRange();
    range.setStart(code, 5);
    range.collapse(true);
    window.getSelection().removeAllRanges();
    window.getSelection().addRange(range);
    document.execCommand("delete");
    check("deletion preserves code whitespace", editor.value === "```\nfirs\n\n\nlast\n```");
    editor.value = "go https://example.com";
    end();
    check("link URLs", editor.querySelector("a").getAttribute("href") === "https://example.com");
    const linkText = editor.querySelector("a").firstChild;
    const linkRange = document.createRange();
    linkRange.setStart(linkText, linkText.nodeValue.length);
    linkRange.collapse(true);
    window.getSelection().removeAllRanges();
    window.getSelection().addRange(linkRange);
    check("caret at a URL uses its source offset", editor.selectionStart === editor.value.length);
    type(" ");
    check("space leaves the link", editor.value === "go https://example.com " && editor.textContent === "go https://example.com " && !editor.querySelector("a").contains(window.getSelection().focusNode));
    type("and");
    check("typing after a URL space", editor.value === "go https://example.com and");
    editor.value = "hello @src/main.rs ";
    editor.setSelectionRange(19, 19);
    type("next");
    check("typing after completion", editor.value === "hello @src/main.rs next");
    editor.value = "hello world";
    editor.setSelectionRange(6, 11);
    type("there");
    check("replacing a selection", editor.value === "hello there");
    editor.value = "hello ";
    end();
    const paste = new Event("paste", { bubbles: true, cancelable: true });
    Object.defineProperty(paste, "clipboardData", { value: { items: [], getData: () => "**safe** <script>alert(1)</script>" } });
    editor.dispatchEvent(paste);
    check("safe Markdown paste", editor.querySelector("strong")?.textContent === "safe" && !editor.querySelector("script") && editor.value.includes("<script>"));
    editor.value = "IME ";
    end();
    editor.dispatchEvent(new CompositionEvent("compositionstart"));
    document.execCommand("insertText", false, "你好");
    check("composition waits", editor.value === "IME ");
    editor.dispatchEvent(new CompositionEvent("compositionend"));
    check("composition commits", editor.value === "IME 你好");
    editor.value = "hello";
    end();
    type("!");
    editor.dispatchEvent(new KeyboardEvent("keydown", { key: "z", ctrlKey: true, cancelable: true }));
    check("undo", editor.value === "hello");
    editor.dispatchEvent(new KeyboardEvent("keydown", { key: "z", ctrlKey: true, shiftKey: true, cancelable: true }));
    check("redo", editor.value === "hello!");
    for (const [source, code] of [
      ["Update @README using `cargo fmt` and Cargo.toml", ["cargo fmt"]],
      ['Update @"README" using `cargo fmt` and Cargo.toml', ["cargo fmt"]],
      ["Update @README using `cargo fmt`, then `cargo test`, and Cargo.toml", ["cargo fmt", "cargo test"]],
      ["Update `@README` using `cargo fmt` and Cargo.toml", ["@README", "cargo fmt"]],
      ["Update @README using `cargo fmt` and @src/main.rs", ["cargo fmt"]],
    ]) {
      editor.value = source;
      editor.focus();
      check("inline code survives reference decoration", JSON.stringify([...editor.querySelectorAll("code")].map((node) => node.textContent)) === JSON.stringify(code));
      check("no unresolved placeholders", !/[\u0000\uFFFD]/.test(editor.innerHTML));
      // Force a DOM-backed edit: reading only the saved source would hide a
      // damaged rendering until a paste, deletion or IME commit reads it back.
      const tail = editor.querySelector("[data-editor-tail]").firstChild;
      tail.appendData("!");
      const caret = document.createRange();
      caret.setStart(tail, tail.length);
      caret.collapse(true);
      window.getSelection().removeAllRanges();
      window.getSelection().addRange(caret);
      editor.dispatchEvent(new Event("input", { bubbles: true }));
      check("DOM-backed editing preserves the full Markdown prompt", editor.value === source + "!");
      check("caret follows the DOM edit", editor.selectionStart === source.length + 1);
      editor.dispatchEvent(new KeyboardEvent("keydown", { key: "z", ctrlKey: true, bubbles: true, cancelable: true }));
      check("undo restores the mixed reference/code prompt", editor.value === source);
    }
    editor.value = "";
    check("clearing", editor.innerHTML === "");
    results.textContent += "ALL PASSED";
  } catch (error) {
    results.textContent += `FAIL ${error.stack}`;
  }
}

const scratch = mkdtempSync(join(tmpdir(), "oxide-markdown-input-"));
try {
  for (const [name, source, lastHelper] of [
    ["Desktop", desktop, "/// One rendered diff"],
    ["Extension", extension, "// ---------- tool cards ----------"],
  ]) {
    const escape = source.match(/function escapeHtml\(text\) \{[\s\S]*?\n {0,2}\}(?=\r?\n)/)[0];
    const helpers = source.slice(source.indexOf("function anchor"), source.indexOf(lastHelper));
    const script = `${escape}\n${referenceCode(source)}\n${helpers}\n${editorCode(source)}\n(${exerciseEditor})();`.replaceAll("</script", "<\\/script");
    const page = join(scratch, `${name}.html`);
    const css = readFileSync(new URL(name === "Desktop" ? "ui/style.css" : "../../editors/vscode/media/style.css", import.meta.url), "utf8");
    const id = name === "Desktop" ? "prompt" : "input";
    writeFileSync(page, `<style>${css}</style><div class="composer" id="composer" style="width: 400px"><div id="${id}" class="markdown-input" contenteditable="true" data-placeholder="Ask Oxide… Enter to send · Shift+Enter for a newline · paste or drop an image"></div></div><pre id="results"></pre><script>${script}</script>`);
    const run = spawnSync(browser, ["--headless", "--no-sandbox", "--dump-dom", pathToFileURL(page).href], { encoding: "utf8", timeout: 30000, maxBuffer: 4 * 1024 * 1024 });
    assert.equal(run.status, 0, run.stderr || String(run.error));
    const result = run.stdout.match(/id="results">([\s\S]*?)<\/pre>/)?.[1] || "No browser test results";
    console.log(`${name}\n${result}`);
    assert.ok(result.endsWith("ALL PASSED"), result);
  }
} finally {
  rmSync(scratch, { recursive: true, force: true });
}
