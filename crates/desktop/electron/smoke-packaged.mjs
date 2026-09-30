import assert from "node:assert/strict";

const port = Number(process.env.OXIDE_DEBUG_PORT || 9223);
const targets = await fetch(`http://127.0.0.1:${port}/json/list`).then((response) => response.json());
const page = targets.find((target) => target.type === "page" && target.title === "Oxide");
assert.ok(page?.webSocketDebuggerUrl, "Oxide renderer is not available over Chromium debugging");

const socket = new WebSocket(page.webSocketDebuggerUrl);
await new Promise((resolve, reject) => {
  socket.addEventListener("open", resolve, { once: true });
  socket.addEventListener("error", reject, { once: true });
});

let nextId = 1;
const pending = new Map();
socket.addEventListener("message", (event) => {
  const message = JSON.parse(event.data);
  const waiter = pending.get(message.id);
  if (!waiter) return;
  pending.delete(message.id);
  if (message.error) waiter.reject(new Error(message.error.message));
  else waiter.resolve(message.result);
});

function command(method, params = {}) {
  const id = nextId++;
  return new Promise((resolve, reject) => {
    pending.set(id, { resolve, reject });
    socket.send(JSON.stringify({ id, method, params }));
  });
}

async function evaluate(expression) {
  const answer = await command("Runtime.evaluate", {
    expression,
    awaitPromise: true,
    returnByValue: true,
  });
  if (answer.exceptionDetails) throw new Error(answer.exceptionDetails.text);
  return answer.result.value;
}

try {
  const rect = await evaluate(`(() => {
    const prompt = document.getElementById("prompt");
    const button = document.getElementById("help");
    document.getElementById("help-modal").hidden = true;
    window.__oxideSmokeClicks = 0;
    const onclick = button.onclick;
    button.onclick = (event) => {
      window.__oxideSmokeClicks += 1;
      return onclick.call(button, event);
    };
    prompt.value = "focused input";
    prompt.focus();
    const bounds = button.getBoundingClientRect();
    return { x: bounds.left + bounds.width / 2, y: bounds.top + bounds.height / 2 };
  })()`);

  await command("Input.dispatchMouseEvent", {
    type: "mousePressed",
    x: rect.x,
    y: rect.y,
    button: "left",
    clickCount: 1,
  });
  await command("Input.dispatchMouseEvent", {
    type: "mouseReleased",
    x: rect.x,
    y: rect.y,
    button: "left",
    clickCount: 1,
  });

  const result = await evaluate(`new Promise((resolve) => setTimeout(() => resolve({
    clicks: window.__oxideSmokeClicks,
    opened: !document.getElementById("help-modal").hidden,
  }), 100))`);
  assert.deepEqual(result, { clicks: 1, opened: true });
  console.log("Packaged Chromium focused-input click smoke test passed");
} finally {
  socket.close();
}
