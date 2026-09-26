// The plugin packages the CLI loads into the ecosystem. Their `agents/`
// directories hold names `--agent` can resolve, so the footer's picker has to
// read them too — `ecosystem::load_enabled_plugins` loads every enabled plugin
// before project resources, which is why a plugin beats a global agent but
// loses to a project one.
//
// The state file belongs to the CLI (`plugin_registry::load_state`), so a
// malformed one reads as "no plugins": the CLI fails the whole parse and finds
// none either.

import * as path from "node:path";

import { parseObject } from "./json";

export interface PluginDeps {
  read: (file: string) => string | null;
  exists: (candidate: string) => boolean;
  configDir: string;
}

/// The package directories of the installed, enabled plugins, in the order the
/// CLI's own listing uses (sorted by name, since the state file's keys are not
/// meaningful). A plugin whose directory or manifest is gone is skipped, the way
/// `enabled_plugins` skips it before its resources are loaded.
export function enabledPluginDirs(deps: PluginDeps): string[] {
  const state = parseObject(deps.read(path.join(deps.configDir, "plugins", "config.json")));
  const plugins = state?.plugins;
  if (!plugins || typeof plugins !== "object" || Array.isArray(plugins)) return [];

  const found: { name: string; dir: string }[] = [];
  for (const [name, value] of Object.entries(plugins as Record<string, unknown>)) {
    if (!value || typeof value !== "object" || Array.isArray(value)) return [];
    const record = value as Record<string, unknown>;
    // `path` has no serde default, and a wrong-typed `enabled` fails the whole
    // state parse rather than defaulting to installed.
    if (typeof record.path !== "string" || !record.path) return [];
    if (record.enabled !== undefined && typeof record.enabled !== "boolean") return [];
    if (record.enabled === false) continue;
    if (!deps.exists(record.path) || !hasManifest(deps.exists, record.path)) continue;
    found.push({ name, dir: record.path });
  }
  return found.sort((left, right) => left.name.localeCompare(right.name)).map((plugin) => plugin.dir);
}

/// `.oxide/plugin.json` wins over the Claude Code location, matching
/// `manifest_candidates`.
function hasManifest(exists: (candidate: string) => boolean, dir: string): boolean {
  return (
    exists(path.join(dir, ".oxide", "plugin.json")) ||
    exists(path.join(dir, ".claude-plugin", "plugin.json"))
  );
}
