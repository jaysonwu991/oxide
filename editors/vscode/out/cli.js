"use strict";
// The oxide CLI process layer: locating the binary, streaming one turn's JSON
// events, and running one-shot commands (`sessions list`, `--version`).
//
// A turn is one process: `oxide --mode json -p` with the prompt on stdin. The
// process streams Pi-shaped JSONL events and exits, so stopping a run is a
// kill, and the next turn resumes the same thread with `--session <id>` (the
// id arrives in the `session` header event).
var __createBinding = (this && this.__createBinding) || (Object.create ? (function(o, m, k, k2) {
    if (k2 === undefined) k2 = k;
    var desc = Object.getOwnPropertyDescriptor(m, k);
    if (!desc || ("get" in desc ? !m.__esModule : desc.writable || desc.configurable)) {
      desc = { enumerable: true, get: function() { return m[k]; } };
    }
    Object.defineProperty(o, k2, desc);
}) : (function(o, m, k, k2) {
    if (k2 === undefined) k2 = k;
    o[k2] = m[k];
}));
var __setModuleDefault = (this && this.__setModuleDefault) || (Object.create ? (function(o, v) {
    Object.defineProperty(o, "default", { enumerable: true, value: v });
}) : function(o, v) {
    o["default"] = v;
});
var __importStar = (this && this.__importStar) || (function () {
    var ownKeys = function(o) {
        ownKeys = Object.getOwnPropertyNames || function (o) {
            var ar = [];
            for (var k in o) if (Object.prototype.hasOwnProperty.call(o, k)) ar[ar.length] = k;
            return ar;
        };
        return ownKeys(o);
    };
    return function (mod) {
        if (mod && mod.__esModule) return mod;
        var result = {};
        if (mod != null) for (var k = ownKeys(mod), i = 0; i < k.length; i++) if (k[i] !== "default") __createBinding(result, mod, k[i]);
        __setModuleDefault(result, mod);
        return result;
    };
})();
Object.defineProperty(exports, "__esModule", { value: true });
exports.resolveBinary = resolveBinary;
exports.runCapture = runCapture;
exports.startTurn = startTurn;
exports.readTextFile = readTextFile;
exports.isFile = isFile;
const node_child_process_1 = require("node:child_process");
const fs = __importStar(require("node:fs"));
const os = __importStar(require("node:os"));
const path = __importStar(require("node:path"));
const protocol_1 = require("./core/protocol");
/// Resolves the configured binary. An explicit path is used as given; a bare
/// name is looked up on PATH, then in the directories oxide's installer
/// (`~/.local/bin`) and `cargo install` (`~/.cargo/bin`) use, so the extension
/// works from a GUI session whose PATH is not the login shell's.
function resolveBinary(configured, lookup) {
    const value = (configured || "").trim() || "oxide";
    if (value.includes("/") || value.includes("\\"))
        return value;
    const suffix = lookup.platform === "win32" ? [".exe", ".cmd", ".bat", ""] : [""];
    const home = lookup.env.HOME || lookup.env.USERPROFILE || os.homedir();
    const dirs = [
        ...(lookup.env.PATH ?? "").split(path.delimiter),
        path.join(home, ".local", "bin"),
        path.join(home, ".cargo", "bin"),
    ];
    for (const dir of dirs) {
        if (!dir)
            continue;
        for (const ext of suffix) {
            const candidate = path.join(dir, value + ext);
            if (lookup.exists(candidate))
                return candidate;
        }
    }
    return value;
}
/// Runs a command to completion and collects its output. Used for the session
/// picker and the version probe, both of which are fast and one-shot.
function runCapture(command, args, cwd, timeoutMs = 20_000) {
    return new Promise((resolve) => {
        let stdout = "";
        let stderr = "";
        let settled = false;
        const finish = (result) => {
            if (settled)
                return;
            settled = true;
            clearTimeout(timer);
            resolve(result);
        };
        let child;
        try {
            child = (0, node_child_process_1.spawn)(command, args, { cwd, env: process.env });
        }
        catch (error) {
            resolve({ code: null, stdout: "", stderr: "", error: String(error) });
            return;
        }
        const timer = setTimeout(() => {
            child.kill();
            finish({ code: null, stdout, stderr, error: `${command} timed out` });
        }, timeoutMs);
        child.stdout?.setEncoding("utf8");
        child.stderr?.setEncoding("utf8");
        child.stdout?.on("data", (chunk) => {
            stdout += chunk;
        });
        child.stderr?.on("data", (chunk) => {
            stderr += chunk;
        });
        child.on("error", (error) => {
            finish({ code: null, stdout, stderr, error: error.message });
        });
        child.on("close", (code) => {
            finish({ code, stdout, stderr });
        });
    });
}
/// Starts one agent turn. The prompt is written to stdin and the stream is
/// closed, which is how `-p` receives a prompt without exposing it to `@file`
/// expansion.
function startTurn(command, args, cwd, prompt, callbacks) {
    const child = (0, node_child_process_1.spawn)(command, args, { cwd, env: process.env });
    let stdoutBuffer = "";
    let stderrBuffer = "";
    child.stdout.setEncoding("utf8");
    child.stderr.setEncoding("utf8");
    child.stdout.on("data", (chunk) => {
        const { lines, rest } = (0, protocol_1.drainLines)(stdoutBuffer, chunk);
        stdoutBuffer = rest;
        for (const line of lines) {
            const event = (0, protocol_1.parseEvent)(line);
            if (event)
                callbacks.onEvent(event);
        }
    });
    child.stderr.on("data", (chunk) => {
        const { lines, rest } = (0, protocol_1.drainLines)(stderrBuffer, chunk);
        stderrBuffer = rest;
        for (const line of lines) {
            if (line.trim())
                callbacks.onStderr(line);
        }
    });
    child.on("error", (error) => {
        callbacks.onExit({ code: null, signal: null, error: error.message });
    });
    child.on("close", (code, signal) => {
        if (stdoutBuffer.trim()) {
            const event = (0, protocol_1.parseEvent)(stdoutBuffer);
            if (event)
                callbacks.onEvent(event);
        }
        if (stderrBuffer.trim())
            callbacks.onStderr(stderrBuffer);
        callbacks.onExit({ code, signal, error: undefined });
    });
    try {
        child.stdin.on("error", () => {
            // A process that exits before reading stdin (a startup failure) closes
            // the pipe; the exit handler reports the real problem.
        });
        child.stdin.end(prompt);
    }
    catch {
        // Nothing to do: the exit handler reports the failure.
    }
    return {
        cancel() {
            child.kill("SIGTERM");
            setTimeout(() => {
                if (child.exitCode === null && child.signalCode === null)
                    child.kill("SIGKILL");
            }, 3_000);
        },
    };
}
/// Reads a file for a diff preview, refusing anything unreadable or implausibly
/// large for a preview.
function readTextFile(file, maxBytes = 2_000_000) {
    try {
        const stat = fs.statSync(file);
        if (!stat.isFile() || stat.size > maxBytes)
            return null;
        return fs.readFileSync(file, "utf8");
    }
    catch {
        return null;
    }
}
/// True when the path is an existing file, used both to pick a binary and to
/// resolve an `@path` reference.
function isFile(candidate) {
    try {
        return fs.statSync(candidate).isFile();
    }
    catch {
        return false;
    }
}
//# sourceMappingURL=cli.js.map