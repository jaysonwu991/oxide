// Tool approvals: the request the CLI emits when a permission rule holds a tool
// for the user's decision, and what the transcript shows for it. The answer
// travels back over the `--mode rpc` channel (`core/rpc.ts`), so nothing here
// touches a process. Webview-free and unit tested.

import type { WireEvent } from "./protocol";

/// The answers the CLI accepts (`Decision::parse` in `oxide_core::approval`):
/// run it once, run it and remember the tool for this project, or refuse it.
export type ApprovalDecision = "once" | "always" | "deny";

export function isApprovalDecision(value: unknown): value is ApprovalDecision {
  return value === "once" || value === "always" || value === "deny";
}

/// A parsed `approval_request`. `id` is the broker's request id, which the
/// answer frame has to echo back.
export interface ApprovalRequest {
  id: number;
  tool: string;
  detail: string;
}

/// Reads an `approval_request` wire event. A request without a usable id cannot
/// be answered, so it is dropped rather than rendered as a card whose buttons
/// would never reach the broker.
export function approvalRequest(event: WireEvent): ApprovalRequest | null {
  const id = event.id;
  if (typeof id !== "number" || !Number.isFinite(id) || id <= 0) return null;
  const tool = typeof event.toolName === "string" ? event.toolName.trim() : "";
  return {
    id,
    tool: tool || "tool",
    detail: typeof event.detail === "string" ? event.detail : "",
  };
}

/// What the tool would do, in a few words, so the card is readable without
/// knowing the tool names. Legacy aliases are listed too, because a saved
/// permission rule may still name them.
const TOOL_TITLES: Record<string, string> = {
  bash: "Run a shell command",
  write: "Write a file",
  write_file: "Write a file",
  edit: "Edit a file",
  patch: "Apply a patch",
  read: "Read a file",
  read_file: "Read a file",
  ls: "List a directory",
  list_dir: "List a directory",
  find: "Find files",
  glob: "Find files",
  grep: "Search the workspace",
  webfetch: "Fetch a URL",
  task: "Run a subagent",
};

export function approvalTitle(tool: string): string {
  const name = tool.trim();
  if (!name) return "Run a tool";
  const known = TOOL_TITLES[name];
  if (known) return known;
  // A connected MCP tool is `<server>__<tool>`: name the remote tool and the
  // server it belongs to, since the compound name alone reads as noise.
  const [server, remote] = name.split("__");
  if (server && remote) return `Call ${remote} on ${server}`;
  return `Run ${name}`;
}

/// What an answered card reads. `always` is remembered by the CLI's own broker
/// in the shared `approvals.json`, so it holds for the next run too.
export function approvalLabel(decision: ApprovalDecision): string {
  switch (decision) {
    case "once":
      return "Allowed once";
    case "always":
      return "Always allowed in this project";
    default:
      return "Denied";
  }
}
