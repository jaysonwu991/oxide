// The shared `settings.json` keys the footer reads. Resolution mirrors the CLI:
// `defaultProjectTrust` comes from the global file (`config.rs`), while
// `compaction.enabled` is read from the global file and the project's
// `.oxide/settings.json` with the project winning per key (`compact.rs`).

import { parseObject } from "./json";

export type DefaultTrust = "ask" | "always" | "never";

export interface SharedSettings {
  /// The fallback used when `trust.json` has no decision for the folder.
  defaultTrust: DefaultTrust;
  /// Whether the CLI summarizes the context once it approaches the window.
  autoCompact: boolean;
}

export interface SettingsKeys {
  defaultTrust?: DefaultTrust;
  autoCompact?: boolean;
}

/// The keys one `settings.json` sets. `null` (missing, unreadable, malformed)
/// sets none of them, so the caller's default applies.
export function parseSettings(raw: string | null): SettingsKeys {
  const value = parseObject(raw);
  const keys: SettingsKeys = {};
  if (!value) return keys;

  const trust = typeof value.defaultProjectTrust === "string" ? value.defaultProjectTrust.trim().toLowerCase() : "";
  const defaultTrust = parseDefaultTrust(trust);
  if (defaultTrust) keys.defaultTrust = defaultTrust;

  const compaction = value.compaction;
  if (compaction && typeof compaction === "object" && !Array.isArray(compaction)) {
    const enabled = (compaction as Record<string, unknown>).enabled;
    // The CLI deserializes `compaction`, so a wrong type resets the whole block
    // to its default rather than being coerced.
    if (typeof enabled === "boolean") keys.autoCompact = enabled;
  }
  return keys;
}

/// The same aliases `oxide_core::trust::DefaultTrust::parse` accepts.
function parseDefaultTrust(value: string): DefaultTrust | null {
  switch (value) {
    case "ask":
      return "ask";
    case "always":
    case "trust":
      return "always";
    case "never":
    case "deny":
      return "never";
    default:
      return null;
  }
}

/// The effective settings for a folder, merged the way `compact::load_config`
/// merges the two files and `OXIDE_COMPACTION_ENABLED` overrides.
export function sharedSettings(
  globalRaw: string | null,
  projectRaw: string | null,
  env: Record<string, string | undefined> = {},
): SharedSettings {
  const global = parseSettings(globalRaw);
  const project = parseSettings(projectRaw);
  const enabled = env.OXIDE_COMPACTION_ENABLED?.trim();
  const autoCompact =
    enabled === "true" || enabled === "false"
      ? enabled === "true"
      : (project.autoCompact ?? global.autoCompact ?? true);
  return {
    defaultTrust: global.defaultTrust ?? "ask",
    autoCompact,
  };
}
