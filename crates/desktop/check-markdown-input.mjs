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
// The composer paints with `renderMarkdown` and reads the page back with
// `readMarkdown`, so both halves have to ride along into the page.
const readerCode = (source) => source.slice(source.indexOf("const CARET"), source.indexOf("function installMarkdownInput"));

function exerciseEditor() {
  const editor = document.querySelector("[contenteditable]");
  // Force a DOM-backed edit: reading only the saved source would hide a damaged
  // rendering until a paste, deletion or IME commit reads it back.
  const editInPage = (text = "!") => {
    const tail = editor.querySelector("[data-editor-tail]").firstChild;
    tail.appendData(text);
    const caret = document.createRange();
    caret.setStart(tail, tail.length);
    caret.collapse(true);
    window.getSelection().removeAllRanges();
    window.getSelection().addRange(caret);
    editor.dispatchEvent(new Event("input", { bubbles: true }));
  };
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
  const lineBreak = () => {
    editor.dispatchEvent(new InputEvent("beforeinput", { inputType: "insertLineBreak", bubbles: true, cancelable: true }));
  };
  const erase = (type, start, finish = start) => {
    editor.setSelectionRange(start, finish);
    const event = new InputEvent("beforeinput", { inputType: type, bubbles: true, cancelable: true });
    editor.dispatchEvent(event);
    return event.defaultPrevented;
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
    const codeStyle = getComputedStyle(editor.querySelector("code"));
    check(
      "inline code uses Codex's highlighted pill",
      codeStyle.backgroundColor !== "rgba(0, 0, 0, 0)" &&
        parseFloat(codeStyle.paddingTop) === 2 &&
        parseFloat(codeStyle.paddingLeft) === 4 &&
        parseFloat(codeStyle.borderTopLeftRadius) === 4 &&
        codeStyle.fontSize === getComputedStyle(editor).fontSize,
      JSON.stringify({
        background: codeStyle.backgroundColor,
        padding: codeStyle.padding,
        radius: codeStyle.borderTopLeftRadius,
        size: codeStyle.fontSize,
      }),
    );
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
    editor.value = "hello";
    end();
    lineBreak();
    type("-");
    check(
      "a lone hyphen on the next line does not turn the previous line into a heading",
      editor.value === "hello\n-" && !editor.querySelector("h1, h2") && editor.querySelector("p")?.innerHTML.startsWith("hello<br>-"),
    );
    type(" ");
    check(
      "a hyphen and space start a list without restyling the line before it",
      editor.value === "hello\n- " && !editor.querySelector("h1, h2") && editor.querySelector("ul li")?.textContent === "",
    );
    editor.value = "hello";
    end();
    lineBreak();
    type("=");
    check(
      "a lone equals sign on the next line stays ordinary text",
      editor.value === "hello\n=" && !editor.querySelector("h1, h2") && editor.querySelector("p")?.innerHTML.startsWith("hello<br>="),
    );
    editor.value = "Title\n---";
    check("a rule-like line stays text until a blank line starts a new block", !editor.querySelector("h1, h2, hr") && editor.value === "Title\n---");
    for (const [marker, selector] of [
      ["# ", "h1"],
      ["1. ", "ol"],
      ["- [ ] ", "ul.tasks li.task"],
    ]) {
      editor.value = marker;
      check(`${JSON.stringify(marker)} renders its block as soon as it is typed`, Boolean(editor.querySelector(selector)));
      editInPage("item");
      check(`${JSON.stringify(marker)} keeps the text in that block`, editor.value === `${marker}item`);
    }
    editor.value = "[ ]";
    end();
    type(" ");
    check("a bare task marker becomes a checkbox", editor.value === "- [ ] " && editor.querySelector("input[data-editor-task]")?.checked === false);
    editor.querySelector("input[data-editor-task]").click();
    check("clicking a task checkbox checks it", editor.value === "- [x] " && editor.querySelector("input[data-editor-task]")?.checked === true);
    editor.querySelector("input[data-editor-task]").click();
    check("clicking a checked box unchecks it", editor.value === "- [ ] " && editor.querySelector("input[data-editor-task]")?.checked === false);
    editor.value = "```\n- [ ] code\n```\n- [ ] real";
    editor.querySelector("input[data-editor-task]").click();
    check("a task source is toggled by its rendered position, not a matching line in code", editor.value === "```\n- [ ] code\n```\n- [x] real");
    editor.value = "# ";
    end();
    check("Backspace removes an empty heading block", erase("deleteContentBackward", 2) && editor.value === "");
    editor.value = "## Title";
    end();
    const deeperHeading = new KeyboardEvent("keydown", { key: "Tab", cancelable: true });
    editor.dispatchEvent(deeperHeading);
    const shallowerHeading = new KeyboardEvent("keydown", { key: "Tab", shiftKey: true, cancelable: true });
    editor.dispatchEvent(shallowerHeading);
    const plainHeading = new KeyboardEvent("keydown", { key: "Tab", shiftKey: true, cancelable: true });
    editor.dispatchEvent(plainHeading);
    check(
      "Tab changes a heading's level and Shift+Tab returns it to a paragraph",
      deeperHeading.defaultPrevented && shallowerHeading.defaultPrevented && plainHeading.defaultPrevented && editor.value === "# Title",
    );
    const removeHeading = new KeyboardEvent("keydown", { key: "Tab", shiftKey: true, cancelable: true });
    editor.dispatchEvent(removeHeading);
    check("Shift+Tab removes the first heading level", removeHeading.defaultPrevented && editor.value === "Title");
    editor.value = "hello\n> ";
    end();
    check("Backspace removes an empty blockquote line", erase("deleteContentBackward", editor.value.length) && editor.value === "hello\n");
    editor.value = "```";
    end();
    check("Backspace removes an empty code block", erase("deleteContentBackward", 3) && editor.value === "");
    editor.value = "```\n```";
    const closedCode = editor.querySelector("pre code");
    const closedCaret = document.createRange();
    closedCaret.setStart(closedCode, 0);
    closedCaret.collapse(true);
    window.getSelection().removeAllRanges();
    window.getSelection().addRange(closedCaret);
    const closedDelete = new InputEvent("beforeinput", { inputType: "deleteContentBackward", bubbles: true, cancelable: true });
    editor.dispatchEvent(closedDelete);
    check("Backspace removes an empty fenced block at its start", closedDelete.defaultPrevented && editor.value === "");
    editor.value = "> first";
    end();
    lineBreak();
    type("second");
    check(
      "Shift+Enter stays inside a blockquote",
      editor.value === "> first\n> second" && editor.querySelectorAll("blockquote").length === 1 && editor.querySelector("blockquote")?.textContent === "firstsecond",
    );
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
    for (const list of ["- one", "* star", "1. ordered", "- [ ] task", "- outer\n  - inner"]) {
      editor.value = list;
      end();
      lineBreak();
      check(`one newline leaves ${JSON.stringify(list)}`, editor.value === `${list}\n\n`);
      type("plain");
      check(`typing after ${JSON.stringify(list)} is outside the list`, editor.value === `${list}\n\nplain` && editor.lastElementChild?.tagName === "P");
    }
    editor.value = "- one";
    editor.setSelectionRange(3, 3);
    lineBreak();
    check("a newline inside list text stays in the item", editor.value === "- o\nne");
    editor.value = "- outer\n  - inner";
    editor.setSelectionRange(9, 9);
    const tab = new KeyboardEvent("keydown", { key: "Tab", cancelable: true });
    editor.dispatchEvent(tab);
    check("Tab nests a list item", tab.defaultPrevented && editor.value === "- outer\n    - inner");
    const untab = new KeyboardEvent("keydown", { key: "Tab", shiftKey: true, cancelable: true });
    editor.dispatchEvent(untab);
    check("Shift+Tab outdents a list item", untab.defaultPrevented && editor.value === "- outer\n  - inner");
    editor.value = "- one";
    end();
    const rootTab = new KeyboardEvent("keydown", { key: "Tab", cancelable: true });
    editor.dispatchEvent(rootTab);
    const rootUntab = new KeyboardEvent("keydown", { key: "Tab", shiftKey: true, cancelable: true });
    editor.dispatchEvent(rootUntab);
    check("Tab navigation in a root list stays out of the window's shortcuts", rootTab.defaultPrevented && rootUntab.defaultPrevented && editor.value === "- one");
    editor.value = "- one";
    editor.setSelectionRange(2, 2);
    const unlist = new InputEvent("beforeinput", { inputType: "deleteContentBackward", bubbles: true, cancelable: true });
    editor.dispatchEvent(unlist);
    check("Backspace at the start of a list item removes its marker", unlist.defaultPrevented && editor.value === "one");
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
    editor.value = "Visit github.com/openai/codex, www.example.org and docs.example.dev/setup.";
    end();
    check("bare domains link to HTTPS and keep punctuation outside", JSON.stringify([...editor.querySelectorAll("a")].map((a) => [a.textContent, a.getAttribute("href")])) === JSON.stringify([
      ["github.com/openai/codex", "https://github.com/openai/codex"],
      ["www.example.org", "https://www.example.org"],
      ["docs.example.dev/setup", "https://docs.example.dev/setup"],
    ]));
    const domainSource = editor.value;
    const domainTail = editor.querySelector("[data-editor-tail]").firstChild;
    domainTail.appendData("!");
    editor.dispatchEvent(new Event("input", { bubbles: true }));
    check("DOM edits keep bare domain source unchanged", editor.value === domainSource + "!");
    editor.value = "github.com";
    end();
    type("/openai");
    check("editing a bare domain preserves its source and link", editor.value === "github.com/openai" && editor.querySelector("a").getAttribute("href") === "https://github.com/openai");
    editor.value = "mail@example.com src/main.rs config.json @github.com `github.com`";
    check("emails files references and inline code are not bare links", editor.querySelectorAll("a").length === 0 && editor.querySelector(".file-reference")?.textContent === "@github.com");
    for (const reference of ["@src/main.rs", '@"source files/main.rs"#12-14', "@folder/"]) {
      const original = `Use ${reference} please`;
      editor.value = original;
      check("backspace removes the entire reference", erase("deleteContentBackward", 4 + reference.length) && editor.value === "Use  please" && editor.selectionStart === 4);
      editor.dispatchEvent(new KeyboardEvent("keydown", { key: "z", ctrlKey: true, cancelable: true }));
      check("undo restores the whole reference", editor.value === original && editor.querySelector(".file-reference")?.textContent === reference);
      check("deleting selected path characters removes the reference", erase("deleteByCut", 6, 8) && editor.value === "Use  please");
      editor.value = original;
      check("forward deletion removes the whole reference", erase("deleteContentForward", 4) && editor.value === "Use  please");
    }
    editor.value = "Use @src/main.rs ";
    end();
    check("word deletion clears a path and selected trailing space", erase("deleteWordBackward", editor.value.length) && editor.value === "Use ");
    editor.value = "Use @src/a.rs and @src/b.rs now";
    const secondReference = editor.value.indexOf("@src/b.rs");
    check("selection across references removes complete references", erase("deleteContentBackward", 6, secondReference + 3) && editor.value === "Use  now");
    editor.value = "Use @src/a.rs and @src/b.rs now";
    check("deleting one path preserves the other", erase("deleteContentBackward", 12) && editor.value === "Use  and @src/b.rs now");
    editor.value = "Use @src/main.rs please";
    check("deleting adjacent prose stays native", !erase("deleteContentBackward", editor.value.length));
    editor.value = "Use `read @src/main.rs` please";
    check("deleting inside code stays native", !erase("deleteContentBackward", editor.value.indexOf("main") + 2));
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
      editInPage();
      check("DOM-backed editing preserves the full Markdown prompt", editor.value === source + "!");
      check("caret follows the DOM edit", editor.selectionStart === source.length + 1);
      editor.dispatchEvent(new KeyboardEvent("keydown", { key: "z", ctrlKey: true, bubbles: true, cancelable: true }));
      check("undo restores the mixed reference/code prompt", editor.value === source);
    }
    editor.value = "";
    check("clearing", editor.innerHTML === "");
    // Every construct the box can draw has to come back as the text that was
    // typed: what the page holds is re-read on the next keystroke, so a block
    // that writes a blank line around itself, a list that normalizes `*` to `-`
    // or a fence that invents a closing run would edit the reader's own text.
    for (const source of [
      "- one\n- two",
      "* star\n+ plus",
      "1) paren\n2) again",
      "3. three\n4. four",
      "1. one\n1. repeated",
      "- outer\n  - inner",
      "- an item\n  continued under itself",
      "- [ ] todo\n- [x] done",
      "# Heading",
      "> quoted",
      "***",
      "```\ncode\n```",
      "~~~\ncode\n~~~",
      "```\nstill being typed",
      "| a | b |\n| --- | --- |\n| 1 | 2 |",
      "| a | b |\n| :-- | --: |\n| 1 | 2 |",
      "2 * 3 * 4",
      "_italic_ and __bold__ and snake_case_name",
      "an \\*escaped\\* star",
      "``a ` b``",
      "see https://example.com/a_b",
      "[label](https://example.com \"a title\")",
      "Title\n=====",
    ]) {
      editor.value = source;
      editor.focus();
      editInPage();
      check(`kept ${JSON.stringify(source)} as it was typed`, editor.value === source + "!", JSON.stringify(editor.value));
    }
    // An entity is the one construct the box settles rather than keeps: it shows
    // the character the entity stands for, and reads back as that character.
    editor.value = "a &amp; b";
    editor.focus();
    editInPage();
    check("showed an entity as the character it stands for", editor.value === "a & b!" && editor.textContent === "a & b!", JSON.stringify(editor.value));
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
    const script = `${escape}\n${referenceCode(source)}\n${helpers}\n${readerCode(source)}\n${editorCode(source)}\n(${exerciseEditor})();`.replaceAll("</script", "<\\/script");
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
