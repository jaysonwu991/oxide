// Run both transcript scroll controllers against Chromium's actual layout:
// node crates/desktop/check-transcript-scroll.mjs /path/to/chrome-headless-shell
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
const declaration = (source, name, indent = "") => {
  const start = source.indexOf(`function ${name}(`);
  assert.ok(start >= 0, `Missing ${name}`);
  return source.slice(start, source.indexOf(`\n${indent}}`, start) + indent.length + 2);
};
assert.equal(declaration(desktop, "updateTranscriptFollow"), declaration(extension, "updateTranscriptFollow"));

async function exerciseScroll() {
  const results = document.querySelector("#results");
  const check = (name, pass) => {
    if (!pass) throw new Error(`${name}: top=${transcript.scrollTop}, previous=${transcript.followScrollTop}, height=${transcript.scrollHeight}, following=${transcript.followLatest}`);
    results.textContent += `PASS ${name}\n`;
  };
  const tick = () => new Promise((resolve) => setTimeout(resolve, 30));
  const grow = () => {
    updateTranscriptFollow(transcript);
    transcript.insertAdjacentHTML("beforeend", "<p>New streaming output</p>".repeat(10));
    scrollDown();
  };
  try {
    transcript.innerHTML = "<p>A line in the conversation</p>".repeat(40);
    scrollDown(true);
    await tick();
    const tool = document.createElement("pre");
    tool.textContent = "Running tool output\n".repeat(60);
    transcript.appendChild(tool);
    scrollDown();
    await tick();
    const runningTop = transcript.scrollTop;
    tool.textContent = "Finished tool preview";
    // Reading layout forces the browser to clamp the scroll position, before
    // the next model/tool update has had a chance to scroll again.
    check("finishing a tool shrinks the transcript", transcript.scrollTop < runningTop);
    grow();
    await tick();
    check("tool completion keeps following", transcript.followLatest && $("jump-latest").hidden
      && transcript.scrollHeight - transcript.clientHeight - transcript.scrollTop <= 2);
    transcript.style.height = "400px";
    grow();
    await tick();
    check("a taller viewport keeps following", transcript.followLatest && $("jump-latest").hidden);
    transcript.style.height = "200px";
    scrollDown();
    await tick();
    const bottom = transcript.scrollTop;
    transcript.dispatchEvent(new WheelEvent("wheel", { deltaY: -10, bubbles: true }));
    grow();
    check(`upward wheel pauses before the scroll event (was ${bottom})`, transcript.scrollTop === bottom && transcript.followLatest === false);
    transcript.scrollTop = bottom - 10;
    await tick();
    const reading = transcript.scrollTop;
    grow();
    await tick();
    check("streaming preserves reading position", transcript.scrollTop === reading && !$("jump-latest").hidden);
    $("jump-latest").click();
    await tick();
    check("jump resumes following", transcript.followLatest && $("jump-latest").hidden && getComputedStyle($("jump-latest")).display === "none");
    grow();
    await tick();
    check("later output follows again", transcript.scrollHeight - transcript.clientHeight - transcript.scrollTop <= 2);
    transcript.scrollTop -= 10;
    grow();
    const smallScroll = transcript.scrollTop;
    await tick();
    check("small scroll pauses without waiting for its event", transcript.followLatest === false && transcript.scrollTop === smallScroll);
    transcript.scrollTop = transcript.scrollHeight;
    await tick();
    grow();
    await tick();
    check("scrolling to bottom resumes following", transcript.followLatest && transcript.scrollHeight - transcript.clientHeight - transcript.scrollTop <= 2);
    transcript.scrollTop -= 100;
    await tick();
    const beforeResize = transcript.scrollTop;
    transcript.style.height = "150px";
    grow();
    await tick();
    check("resize preserves reading position", transcript.scrollTop === beforeResize && !transcript.followLatest);
    tool.textContent = "More live tool output\n".repeat(60);
    scrollDown(true);
    await tick();
    transcript.dispatchEvent(new WheelEvent("wheel", { deltaY: -100, bubbles: true }));
    transcript.scrollTop -= 100;
    await tick();
    tool.textContent = "Finished tool preview";
    grow();
    await tick();
    check("finishing a tool preserves a paused reader", transcript.followLatest === false && !$("jump-latest").hidden);
    results.textContent += "ALL PASSED";
  } catch (error) {
    results.textContent += `FAIL ${error.stack}`;
  }
}

const scratch = mkdtempSync(join(tmpdir(), "oxide-transcript-scroll-"));
try {
  for (const [name, source] of [["Desktop", desktop], ["Extension", extension]]) {
    let controller = declaration(source, "updateTranscriptFollow");
    if (name === "Desktop") {
      controller += "\n" + ["paintJumpLatest", "scrollDown", "initJumpLatest"].map((name) => declaration(source, name)).join("\n");
      controller += "\ninitJumpLatest();";
    } else {
      controller += "\n" + source.slice(source.indexOf("  transcript.followLatest = true;"), source.indexOf("  // ---------- inline markdown"));
      const start = source.indexOf('  $("jump-latest").addEventListener("click"');
      controller += "\n" + source.slice(start, source.indexOf('  window.addEventListener("message"', start));
    }
    const script = `const $ = (id) => document.getElementById(id); const el = $; const transcript = $("transcript");\n${controller}\n(${exerciseScroll})();`;
    const page = join(scratch, `${name}.html`);
    const css = readFileSync(new URL(name === "Desktop" ? "ui/style.css" : "../../editors/vscode/media/style.css", import.meta.url), "utf8");
    writeFileSync(page, `<style>${css}
body { display: block; overflow: auto; } #transcript { height: 200px; width: 400px; overflow: auto; flex: none; } p { margin: 0; line-height: 24px; }</style><div id="transcript"></div><button id="jump-latest" class="jump-latest" hidden>Latest</button><pre id="results"></pre><script>${script}</script>`);
    const run = spawnSync(browser, ["--headless", "--no-sandbox", "--virtual-time-budget=1500", "--dump-dom", pathToFileURL(page).href], { encoding: "utf8", timeout: 30000, maxBuffer: 4 * 1024 * 1024 });
    assert.equal(run.status, 0, run.stderr || String(run.error));
    const result = run.stdout.match(/id="results">([\s\S]*?)<\/pre>/)?.[1] || "No browser test results";
    console.log(`${name}\n${result}`);
    assert.ok(result.endsWith("ALL PASSED"), result);
  }
} finally {
  rmSync(scratch, { recursive: true, force: true });
}
