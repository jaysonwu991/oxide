// The active model's reasoning levels, printed by `oxide reasoning --json`.
// The CLI owns which levels a model advertises (its provider listing) and the
// clamp that keeps a request on them; the extension only validates the small
// view it paints.

import { parseObject } from "./json";

export interface ReasoningInfo {
  /// The level a turn would run at (the stored `reasoning`).
  current: string;
  /// Whether the active model is known to reason at all.
  supportsReasoning: boolean;
  /// The explicit levels the model accepts, cheapest first, with `off`. Empty
  /// when the model advertised none (the caller hides effort selection).
  levels: string[];
}

export function reasoningArgs(refresh = false, model = ""): string[] {
  const args = ["reasoning", "--json"];
  // `--refresh` warms missing reasoning metadata when the cache is cold.
  if (refresh) args.push("--refresh");
  // The panel's own `oxide.model` setting overrides the active model for a run,
  // so the levels read are that model's rather than `config.json`'s.
  const trimmed = model.trim();
  if (trimmed) args.push("--model", trimmed);
  return args;
}

export function parseReasoning(raw: string): ReasoningInfo | null {
  const root = parseObject(raw);
  if (!root) return null;
  const current = typeof root.current === "string" ? root.current : "auto";
  const levels: string[] = [];
  for (const value of Array.isArray(root.reasoningLevels) ? root.reasoningLevels : []) {
    if (typeof value !== "string") continue;
    const level = value.trim();
    if (!level || levels.includes(level)) continue;
    levels.push(level);
  }
  const supportsReasoning = root.supportsReasoning === true && levels.length > 0;
  return { current: supportsReasoning ? current : "auto", supportsReasoning, levels };
}
