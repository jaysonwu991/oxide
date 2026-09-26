// Project trust as the footer shows it. `trust.json` keeps one decision per
// directory, and the closest ancestor wins — the model `oxide_core::trust`
// implements, so the chip reports the access the next run actually gets.

import * as path from "node:path";

import type { TrustSetting } from "./args";
import { parseObject } from "./json";

/// Canonical directory → `true` (trusted) / `false` (declined).
export type TrustStore = Record<string, boolean>;

export type Access = "trusted" | "untrusted";

/// Saved decisions from `trust.json`. Entries that are not booleans are dropped
/// rather than reported as a decision nobody made.
export function parseTrustStore(raw: string | null): TrustStore {
  const value = parseObject(raw);
  const store: TrustStore = {};
  if (!value) return store;
  for (const [dir, decision] of Object.entries(value)) {
    if (typeof decision === "boolean") store[dir] = decision;
  }
  return store;
}

/// The saved decision for `folder` or its closest ancestor with one. `realpath`
/// canonicalizes a path, because that is what the CLI stores; a path that does
/// not resolve keeps its spelling so an unreadable ancestor still matches.
export function trustDecision(
  store: TrustStore,
  folder: string,
  realpath: (candidate: string) => string,
): boolean | undefined {
  let current: string | null = folder;
  while (current) {
    const decision = store[realpath(current)];
    if (decision !== undefined) return decision;
    const parent = path.dirname(current);
    current = parent === current ? null : parent;
  }
  return undefined;
}

/// The access the next run gets, resolved the way `oxide_core::trust::resolve`
/// does: the `oxide.projectTrust` override, then a saved decision, then
/// `defaultProjectTrust`. `ask` cannot prompt in a non-interactive run, so it
/// resolves to untrusted — exactly like the CLI.
export function resolveAccess(input: {
  setting: TrustSetting;
  saved: boolean | undefined;
  defaultTrust: "ask" | "always" | "never";
}): Access {
  if (input.setting === "always") return "trusted";
  if (input.setting === "never") return "untrusted";
  if (input.saved !== undefined) return input.saved ? "trusted" : "untrusted";
  return input.defaultTrust === "always" ? "trusted" : "untrusted";
}
