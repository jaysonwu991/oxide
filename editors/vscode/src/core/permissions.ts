// The saved "Always allow" rules, as `oxide approvals` reports them. Distinct
// from `approvals.ts`, which is the request the CLI emits while a turn runs and
// is waiting for an answer: this is the store a past answer left behind.
//
// The rules live in `approvals.json` beside `config.json`, keyed by project, and
// every front-end writes them through the same core (`oxide_core::approvals`).
// A front-end that cannot link the crate — this panel — reads and clears them
// through the CLI rather than keeping a copy of the store, so a rule the
// terminal saved is listed here and a rule cleared here is gone there.

import { parseObject } from "./json";

/// What a read answered: the folder the rules belong to, and the tools allowed
/// in it.
export interface ApprovalsListing {
  project: string;
  tools: string[];
}

/// The read. The folder is named rather than inherited from the process's own
/// working directory, so the listing always describes the project the panel is
/// showing.
export function approvalsArgs(project: string): string[] {
  return ["approvals", "list", "--json", "--project", project];
}

/// Forgetting every rule for this project, which is what the listing's own row
/// does.
export function approvalsClearArgs(project: string): string[] {
  return ["approvals", "clear", "--json", "--project", project];
}

/// Reads the listing. Anything that is not a list of names — a CLI too old to
/// know the command, a partial write — reads as no rules rather than throwing in
/// the middle of a click; the caller says what happened.
export function parseApprovals(raw: string): ApprovalsListing {
  const root = parseObject(raw);
  if (!root) return { project: "", tools: [] };
  return {
    project: typeof root.project === "string" ? root.project : "",
    tools: Array.isArray(root.tools)
      ? root.tools
          .filter((tool): tool is string => typeof tool === "string" && tool.trim() !== "")
          .map((tool) => tool.trim())
      : [],
  };
}
