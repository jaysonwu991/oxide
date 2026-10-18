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
    editor.value = "Test `Markdown`";
    end();
    check("single rendered surface", editor.textContent === "Test Markdown" && editor.querySelector("code"));
    type("!");
    check("typing after code", editor.value === "Test `Markdown`!");
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
    type("world");
    check("newlines", editor.value === "hello\nworld");
    editor.value = "```js\nconst x = 1;\n```";
    end();
    check("caret after a code fence", editor.querySelector("pre code")?.textContent === "const x = 1;");
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
    const script = `${escape}\n${helpers}\n${editorCode(source)}\n(${exerciseEditor})();`.replaceAll("</script", "<\\/script");
    const page = join(scratch, `${name}.html`);
    writeFileSync(page, `<style>[contenteditable] { white-space: pre-wrap; }</style><div contenteditable="true"></div><pre id="results"></pre><script>${script}</script>`);
    const run = spawnSync(browser, ["--headless", "--no-sandbox", "--dump-dom", pathToFileURL(page).href], { encoding: "utf8", timeout: 30000, maxBuffer: 4 * 1024 * 1024 });
    assert.equal(run.status, 0, run.stderr || String(run.error));
    const result = run.stdout.match(/id="results">([\s\S]*?)<\/pre>/)?.[1] || "No browser test results";
    console.log(`${name}\n${result}`);
    assert.ok(result.endsWith("ALL PASSED"), result);
  }
} finally {
  rmSync(scratch, { recursive: true, force: true });
}
