// CLI argv for one agent turn. Every flag maps onto an `oxide` option, so the
// extension never needs its own copy of the agent's configuration.

export type TrustSetting = "default" | "always" | "never";

export interface TurnOptions {
  model?: string;
  agent?: string;
  reasoning?: string;
  trust?: TrustSetting;
  tools?: string;
  excludeTools?: string;
  /// Resume this session id (`--session`).
  session?: string | null;
  /// Resume the most recent session for the project (`--continue`).
  continueLast?: boolean;
  /// Do not persist a session (`--no-session`).
  ephemeral?: boolean;
  /// Ask before running a permission-gated tool (`--ask-approvals`). Off passes
  /// `--no-ask-approvals`, and the tool runs without a prompt.
  askApprovals?: boolean;
  /// Extra arguments from `oxide.additionalArguments`.
  extra?: string[];
}

/// `--mode rpc` streams the same Pi-shaped JSON events as `--mode json`, but
/// keeps stdin open for requests, which is what lets the extension answer a
/// tool approval mid-turn (`core/rpc.ts`). The prompt therefore travels as a
/// request frame rather than through `-p`, and the images a message carries
/// travel with it (the CLI reads no `--image` flags in rpc mode).
/// The `@path` references a message may carry are still expanded by the
/// extension itself (`core/prompt.ts`), so the composer behaves like the CLI's
/// own positional `@file` handling.
export function buildTurnArgs(options: TurnOptions): string[] {
  const args = ["--mode", "rpc"];
  args.push(options.askApprovals === false ? "--no-ask-approvals" : "--ask-approvals");
  // Every rpc client can answer a question: the panel paints the `ask` tool's
  // `question_request` in the transcript and replies on the same pipe, so the
  // tool is offered rather than left out of the run.
  args.push("--ask-questions");
  if (options.session) args.push("--session", options.session);
  else if (options.continueLast) args.push("--continue");
  if (options.ephemeral) args.push("--no-session");
  if (options.model) args.push("--model", options.model);
  if (options.agent) args.push("--agent", options.agent);
  if (options.reasoning && options.reasoning !== "auto") {
    args.push("--reasoning", options.reasoning);
  }
  if (options.trust === "always") args.push("--approve");
  else if (options.trust === "never") args.push("--no-approve");
  if (options.tools) args.push("--tools", options.tools);
  if (options.excludeTools) args.push("--exclude-tools", options.excludeTools);
  for (const arg of options.extra ?? []) {
    // An empty or whitespace-only entry would be an argument clap rejects.
    if (arg.trim()) args.push(arg);
  }
  return args;
}

/// `oxide sessions list` for the current project.
export function sessionsListArgs(): string[] {
  return ["sessions", "list"];
}

/// `oxide sessions show <id> --json`: the stored conversation of one session,
/// which the panel paints when a thread is resumed. `tail` keeps the answer to
/// the newest `n` messages, since a long thread's history is bigger than a side
/// bar can show.
export function sessionShowArgs(id: string, tail: number): string[] {
  return ["sessions", "show", id, "--tail", String(tail), "--json"];
}

/// `oxide sessions delete <id> --force`: one thread, with the confirmation
/// already taken in the panel's own dialog — the CLI would otherwise ask on
/// stdin, which a front-end has no channel to answer on.
export function sessionDeleteArgs(id: string): string[] {
  return ["sessions", "delete", id, "--force"];
}

/// Splits a comma-separated tool list the way `--tools` expects, dropping the
/// empty entries a trailing comma leaves behind.
export function splitList(value: string): string {
  return value
    .split(",")
    .map((entry) => entry.trim())
    .filter(Boolean)
    .join(",");
}
