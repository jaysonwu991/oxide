// The context window a run resolves, printed by `oxide context --json`.
//
// The window is not a property of a file: `Config::context_window` composes it
// from the `OXIDE_CONTEXT_LIMIT` override, an explicit `context_window`, a
// `modelContextWindows` entry in the shared settings, the window the provider's
// own catalog published and a built-in table, in that order. Only the CLI can
// see all of those — the catalog and the settings overrides are not read here —
// so the panel asks rather than keeping a table whose answer drifts.

import { parseObject } from "./json";

export interface ContextWindowInfo {
  /// The model the CLI resolved the window for, which is the one a turn would
  /// run with (the `oxide.model` setting, else `config.json`'s).
  model: string;
  /// The input window in tokens.
  window: number;
  /// The provider a run in this folder would go to.
  provider: string;
  /// Whether a turn would find a credential at all — the CLI's own question,
  /// the one it settles before a run starts, so the panel can say so before the
  /// reader sends rather than after a failed turn. Absent on a CLI too old to
  /// answer it, which reads as "unknown" rather than as "no".
  hasKey: boolean | null;
  /// Whether the credential is a plan rather than a metered key, which is what
  /// the usage line marks ` (sub)`.
  subscription: boolean;
  /// The variables a key for this provider could come from, for the words that
  /// say what to set.
  keyEnv: string[];
}

export function contextArgs(model = ""): string[] {
  const args = ["context", "--json"];
  // The panel's own `oxide.model` setting overrides the active model for a run,
  // so the window read is that model's rather than `config.json`'s.
  const trimmed = model.trim();
  if (trimmed) args.push("--model", trimmed);
  return args;
}

/// Reads the window out of the CLI's answer. Anything that is not a positive
/// number (a CLI too old to know the command, a partial write) reads as no
/// answer, so the caller keeps its own fallback rather than painting a zero.
///
/// The credential and plan facts ride the same answer: a front-end that has to
/// resolve a model's window has already asked about this project, and one read
/// that is right beats two that can disagree.
export function parseContextWindow(raw: string): ContextWindowInfo | null {
  const root = parseObject(raw);
  if (!root) return null;
  const window = root.window;
  if (typeof window !== "number" || !Number.isFinite(window) || window <= 0) return null;
  return {
    model: typeof root.model === "string" ? root.model : "",
    window: Math.floor(window),
    provider: typeof root.provider === "string" ? root.provider : "",
    hasKey: typeof root.hasKey === "boolean" ? root.hasKey : null,
    subscription: root.subscription === true,
    keyEnv: Array.isArray(root.keyEnv)
      ? root.keyEnv.filter((name): name is string => typeof name === "string" && name !== "")
      : [],
  };
}
