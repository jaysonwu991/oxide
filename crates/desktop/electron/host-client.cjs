const { EventEmitter } = require("node:events");
const { spawn } = require("node:child_process");
const readline = require("node:readline");

class RustHost extends EventEmitter {
  constructor(executable) {
    super();
    this.nextId = 1;
    this.pending = new Map();
    this.closed = false;
    this.process = spawn(executable, [], {
      stdio: ["pipe", "pipe", "pipe"],
      windowsHide: true,
      env: { ...process.env, RUST_BACKTRACE: process.env.RUST_BACKTRACE || "1" },
    });

    readline.createInterface({ input: this.process.stdout }).on("line", (line) => {
      this.handleLine(line);
    });
    this.process.stderr.on("data", (chunk) => {
      const message = String(chunk).trimEnd();
      if (message) console.error(`[oxide-desktop-host] ${message}`);
    });
    this.process.once("error", (error) => this.fail(error));
    this.process.once("exit", (code, signal) => {
      const detail = signal ? `signal ${signal}` : `status ${code}`;
      this.fail(new Error(`Oxide desktop host exited with ${detail}`));
    });
  }

  invoke(command, args = {}) {
    if (this.closed || !this.process.stdin.writable) {
      return Promise.reject(new Error("Oxide desktop host is not running"));
    }
    const id = this.nextId++;
    return new Promise((resolve, reject) => {
      this.pending.set(id, { resolve, reject });
      this.process.stdin.write(`${JSON.stringify({ id, command, args })}\n`, (error) => {
        if (!error) return;
        this.pending.delete(id);
        reject(error);
      });
    });
  }

  handleLine(line) {
    let frame;
    try {
      frame = JSON.parse(line);
    } catch {
      // A line we cannot parse is one lost frame, not a dead host: the process
      // is still running and every pending call can still be answered.
      this.emit("host-error", { message: `Desktop host wrote invalid JSON: ${line}`, fatal: false });
      return;
    }
    if (frame.type === "event" && typeof frame.event === "string") {
      this.emit("event", { event: frame.event, payload: frame.payload });
      return;
    }
    if (frame.type !== "response" || !Number.isSafeInteger(frame.id)) return;
    const pending = this.pending.get(frame.id);
    if (!pending) return;
    this.pending.delete(frame.id);
    if (frame.ok) pending.resolve(frame.value);
    else pending.reject(new Error(String(frame.error || "Desktop command failed")));
  }

  fail(error) {
    if (this.closed) return;
    this.closed = true;
    for (const { reject } of this.pending.values()) reject(error);
    this.pending.clear();
    // The host is gone, so nothing can answer a run, an approval or a question
    // any more. The renderer is told so it can let go of that state instead of
    // waiting for an `agent-end` that can never arrive.
    this.emit("host-error", { message: error.message, fatal: true });
  }

  close() {
    if (this.closed) return;
    this.closed = true;
    this.process.stdin.end();
    this.process.kill();
    for (const { reject } of this.pending.values()) {
      reject(new Error("Oxide desktop host stopped"));
    }
    this.pending.clear();
  }
}

module.exports = { RustHost };
