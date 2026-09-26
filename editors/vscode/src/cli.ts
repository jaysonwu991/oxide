// The oxide CLI process layer: locating the binary, streaming one turn's JSON
// events, and running one-shot commands (`sessions list`, `--version`).
//
// A turn is one process: `oxide --mode rpc` with the prompt written to its
// stdin as a request frame. The process streams Pi-shaped JSONL events; a tool
// approval is answered on the same pipe (`core/rpc.ts`) rather than by starting
// another process, and the turn ends with a `quit` frame so the process exits
// on its own. Stopping a run is still a kill, and the next turn resumes the
// same thread with `--session <id>` (the id arrives in the `session` header
// event).

import { spawn } from "node:child_process";
import * as fs from "node:fs";
import * as os from "node:os";
import * as path from "node:path";

import type { ApprovalDecision } from "./core/approvals";
import { drainLines, parseEvent, type WireEvent } from "./core/protocol";
import { approvalFrame, promptFrame, quitFrame } from "./core/rpc";

export interface BinaryLookup {
  env: NodeJS.ProcessEnv;
  platform: NodeJS.Platform;
  exists: (candidate: string) => boolean;
}

/// Resolves the configured binary. An explicit path is used as given; a bare
/// name is looked up on PATH, then in the directories oxide's installer
/// (`~/.local/bin`) and `cargo install` (`~/.cargo/bin`) use, so the extension
/// works from a GUI session whose PATH is not the login shell's.
export function resolveBinary(configured: string, lookup: BinaryLookup): string {
  const value = (configured || "").trim() || "oxide";
  if (value.includes("/") || value.includes("\\")) return value;
  const suffix = lookup.platform === "win32" ? [".exe", ".cmd", ".bat", ""] : [""];
  const home = lookup.env.HOME || lookup.env.USERPROFILE || os.homedir();
  const dirs = [
    ...(lookup.env.PATH ?? "").split(path.delimiter),
    path.join(home, ".local", "bin"),
    path.join(home, ".cargo", "bin"),
  ];
  for (const dir of dirs) {
    if (!dir) continue;
    for (const ext of suffix) {
      const candidate = path.join(dir, value + ext);
      if (lookup.exists(candidate)) return candidate;
    }
  }
  return value;
}

export interface CommandResult {
  code: number | null;
  stdout: string;
  stderr: string;
  error?: string;
}

/// Runs a command to completion and collects its output. Used for the session
/// picker and the version probe, both of which are fast and one-shot.
export function runCapture(
  command: string,
  args: string[],
  cwd: string,
  timeoutMs = 20_000,
): Promise<CommandResult> {
  return new Promise((resolve) => {
    let stdout = "";
    let stderr = "";
    let settled = false;
    const finish = (result: CommandResult) => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      resolve(result);
    };
    let child: ReturnType<typeof spawn>;
    try {
      child = spawn(command, args, { cwd, env: process.env });
    } catch (error) {
      resolve({ code: null, stdout: "", stderr: "", error: String(error) });
      return;
    }
    const timer = setTimeout(() => {
      child.kill();
      finish({ code: null, stdout, stderr, error: `${command} timed out` });
    }, timeoutMs);
    child.stdout?.setEncoding("utf8");
    child.stderr?.setEncoding("utf8");
    child.stdout?.on("data", (chunk: string) => {
      stdout += chunk;
    });
    child.stderr?.on("data", (chunk: string) => {
      stderr += chunk;
    });
    child.on("error", (error: Error) => {
      finish({ code: null, stdout, stderr, error: error.message });
    });
    child.on("close", (code: number | null) => {
      finish({ code, stdout, stderr });
    });
  });
}

export interface TurnCallbacks {
  onEvent: (event: WireEvent) => void;
  onStderr: (line: string) => void;
  onExit: (result: { code: number | null; signal: string | null; error?: string }) => void;
}

export interface TurnInput {
  /// The assembled prompt (context blocks included) sent as a `prompt` request.
  prompt: string;
  /// Absolute paths of the images/PDFs the message carries, which the CLI reads
  /// as media parts of that prompt.
  images?: readonly string[];
}

export interface Turn {
  /// Answers a waiting tool approval. An unknown id is ignored by the CLI, so a
  /// double answer is harmless.
  approve(requestId: number, decision: ApprovalDecision): void;
  /// Stops the process. The session on disk stays intact, so the thread can be
  /// resumed with `--session <id>`.
  cancel(): void;
}

/// Starts one agent turn. The prompt is written to stdin as a request frame and
/// the pipe stays open for approvals until the turn ends.
export function startTurn(
  command: string,
  args: string[],
  cwd: string,
  input: TurnInput,
  callbacks: TurnCallbacks,
): Turn {
  const child = spawn(command, args, { cwd, env: process.env });
  let stdoutBuffer = "";
  let stderrBuffer = "";
  let exited = false;

  child.stdout.setEncoding("utf8");
  child.stderr.setEncoding("utf8");

  // `agent_end` is the last event of the turn, but the CLI waits on its input
  // channel for another request instead of exiting, so the session is ended
  // and its stdin closed here.
  const quit = (): void => {
    if (exited) return;
    try {
      child.stdin.write(quitFrame());
      child.stdin.end();
    } catch {
      // The pipe is already gone; the exit handler reports why.
    }
  };

  child.stdout.on("data", (chunk: string) => {
    const { lines, rest } = drainLines(stdoutBuffer, chunk);
    stdoutBuffer = rest;
    for (const line of lines) {
      const event = parseEvent(line);
      if (!event) continue;
      if (event.type === "agent_end") quit();
      callbacks.onEvent(event);
    }
  });

  child.stderr.on("data", (chunk: string) => {
    const { lines, rest } = drainLines(stderrBuffer, chunk);
    stderrBuffer = rest;
    for (const line of lines) {
      if (line.trim()) callbacks.onStderr(line);
    }
  });

  // A failed spawn emits `error` and then `close`, and both are reported once.
  // The controller treats an exit as final (it clears the run and drains the
  // queue), so a second callback is ignored.
  const exit = (result: { code: number | null; signal: string | null; error?: string }): void => {
    if (exited) return;
    exited = true;
    callbacks.onExit(result);
  };

  child.on("error", (error: Error) => {
    exit({ code: null, signal: null, error: error.message });
  });

  child.on("close", (code: number | null, signal: NodeJS.Signals | null) => {
    if (stdoutBuffer.trim()) {
      const event = parseEvent(stdoutBuffer);
      if (event) callbacks.onEvent(event);
    }
    if (stderrBuffer.trim()) callbacks.onStderr(stderrBuffer);
    exit({ code, signal, error: undefined });
  });

  try {
    child.stdin.on("error", () => {
      // A process that exits before reading stdin (a startup failure) closes
      // the pipe; the exit handler reports the real problem.
    });
    child.stdin.write(promptFrame(input.prompt, input.images ?? []));
  } catch {
    // Nothing to do: the exit handler reports the failure.
  }

  return {
    approve(requestId: number, decision: ApprovalDecision) {
      if (exited) return;
      try {
        child.stdin.write(approvalFrame(requestId, decision));
      } catch {
        // The turn is gone; the approval simply stays unanswered.
      }
    },
    cancel() {
      child.kill("SIGTERM");
      setTimeout(() => {
        if (child.exitCode === null && child.signalCode === null) child.kill("SIGKILL");
      }, 3_000);
    },
  };
}

/// Reads a file for a diff preview, refusing anything unreadable or implausibly
/// large for a preview.
export function readTextFile(file: string, maxBytes = 2_000_000): string | null {
  try {
    const stat = fs.statSync(file);
    if (!stat.isFile() || stat.size > maxBytes) return null;
    return fs.readFileSync(file, "utf8");
  } catch {
    return null;
  }
}

/// True when the path is an existing file, used both to pick a binary and to
/// resolve an `@path` reference.
export function isFile(candidate: string): boolean {
  try {
    return fs.statSync(candidate).isFile();
  } catch {
    return false;
  }
}

/// True when the path exists at all. A `.git` marker is a directory in a normal
/// clone and a file in a worktree, so both have to count.
export function exists(candidate: string): boolean {
  try {
    return fs.existsSync(candidate);
  } catch {
    return false;
  }
}

/// The canonical path, or the input when it cannot be resolved. `trust.json`
/// stores resolved directories, so the lookup has to resolve them the same way.
export function realPath(candidate: string): string {
  try {
    return fs.realpathSync(candidate);
  } catch {
    return candidate;
  }
}

/// The markdown files of a directory with their contents, which is how the
/// footer lists the agents `--agent` can name. A missing or unreadable
/// directory is an empty list.
export function listMarkdown(dir: string): { stem: string; text: string }[] {
  let names: string[];
  try {
    names = fs.readdirSync(dir);
  } catch {
    return [];
  }
  const files: { stem: string; text: string }[] = [];
  for (const name of names.sort()) {
    if (!name.toLowerCase().endsWith(".md")) continue;
    const text = readTextFile(path.join(dir, name));
    if (text !== null) files.push({ stem: name.replace(/\.md$/i, ""), text });
  }
  return files;
}
