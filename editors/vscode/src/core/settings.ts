// The shared `settings.json` keys the footer reads. Resolution mirrors the CLI:
// `defaultProjectTrust` comes from the global file (`config.rs`), while
// `compaction.enabled` is read from the global file and the project's
// `.oxide/settings.json` with the project winning per key (`compact.rs`),
// and so are the notification keys (`notify.rs`) and the update-check flag
// (`update_notice.rs`).

import { parseObject } from "./json";

export type DefaultTrust = "ask" | "always" | "never";

export interface SharedSettings {
  /// The fallback used when `trust.json` has no decision for the folder.
  defaultTrust: DefaultTrust;
  /// Whether the CLI summarizes the context once it approaches the window.
  autoCompact: boolean;
  /// Whether a finished turn raises a notification. The terminal's `/notify`
  /// and the desktop app's toast read the same key, so turning it off in one
  /// silences the other; `notifySound` is not read here, since a VS Code
  /// notification carries no sound of its own — the editor's own setting
  /// decides that (`notify.rs`).
  notifyOnComplete: boolean;
  /// Whether a launch looks for a newer release at all, which is what decides
  /// whether the panel makes its own check on activation. The terminal's
  /// `/updates off` and the desktop app's check write the same key, so turning
  /// it off in one stops the check in the others (`update_notice.rs`).
  checkForUpdates: boolean;
}

export interface SettingsKeys {
  defaultTrust?: DefaultTrust;
  autoCompact?: boolean;
  notifyOnComplete?: boolean;
  checkForUpdates?: boolean;
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

  // `notify.rs` reads both keys with `as_bool`, so anything else sets neither.
  if (typeof value.notifyOnComplete === "boolean") keys.notifyOnComplete = value.notifyOnComplete;
  // `update_notice.rs::enabled_within` does the same with `checkForUpdates`.
  if (typeof value.checkForUpdates === "boolean") keys.checkForUpdates = value.checkForUpdates;
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

/// The effective settings for a folder, merged the way `compact::load_config`,
/// `notify::load_config` and `update_notice::enabled` merge the two files
/// (project wins per key) and the `OXIDE_*` overrides win over both.
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
    notifyOnComplete: envBool(env.OXIDE_NOTIFY_ON_COMPLETE) ?? project.notifyOnComplete ?? global.notifyOnComplete ?? true,
    checkForUpdates: envBool(env.OXIDE_CHECK_FOR_UPDATES) ?? project.checkForUpdates ?? global.checkForUpdates ?? true,
  };
}

/// `notify.rs::env_bool`: only a trimmed `true`/`false` counts.
function envBool(value: string | undefined): boolean | null {
  const trimmed = value?.trim();
  return trimmed === "true" ? true : trimmed === "false" ? false : null;
}
