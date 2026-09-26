// The plugin packages whose agents `--agent` can resolve. The state file
// belongs to the CLI, so these cases follow `plugin_registry::enabled_plugins`
// and `load_state`: a broken entry reads as no plugins at all, a disabled one is
// skipped, and a package whose directory or manifest is gone is skipped too.

import assert from "node:assert/strict";
import * as path from "node:path";
import { describe, it } from "node:test";

import { enabledPluginDirs } from "../core/plugins";

const state = (plugins: Record<string, unknown>): string => JSON.stringify({ plugins });

function deps(files: Record<string, unknown>) {
  const texts = new Map<string, string>();
  for (const [file, value] of Object.entries(files)) {
    texts.set(file, typeof value === "string" ? value : JSON.stringify(value));
  }
  const dirs = new Set<string>();
  for (const file of texts.keys()) {
    let dir = path.dirname(file);
    while (dir && dir !== "/" && !dirs.has(dir)) {
      dirs.add(dir);
      dir = path.dirname(dir);
    }
  }
  return {
    read: (file: string) => texts.get(file) ?? null,
    exists: (candidate: string) => texts.has(candidate) || dirs.has(candidate),
    configDir: "/config",
  };
}

const statePath = "/config/plugins/config.json";
const hello = "/config/plugins/hello";
const alpha = "/config/plugins/alpha";
const broken = "/config/plugins/broken";

describe("enabledPluginDirs", () => {
  it("returns an enabled package's directory, in name order", () => {
    const dirs = enabledPluginDirs(
      deps({
        [statePath]: state({
          hello: { name: "hello", enabled: true, path: hello },
          alpha: { name: "alpha", enabled: true, path: alpha },
        }),
        [`${hello}/.claude-plugin/plugin.json`]: { name: "hello" },
        [`${alpha}/.oxide/plugin.json`]: { name: "alpha" },
      }),
    );
    assert.deepEqual(dirs, [alpha, hello]);
  });

  it("treats a missing enabled flag as installed, like the serde default", () => {
    const dirs = enabledPluginDirs(
      deps({
        [statePath]: state({ hello: { name: "hello", path: hello } }),
        [`${hello}/.oxide/plugin.json`]: "{}",
      }),
    );
    assert.deepEqual(dirs, [hello]);
  });

  it("skips a disabled package, a missing directory and a missing manifest", () => {
    const dirs = enabledPluginDirs(
      deps({
        [statePath]: state({
          off: { name: "off", enabled: false, path: hello },
          gone: { name: "gone", enabled: true, path: "/config/plugins/gone" },
          broken: { name: "broken", enabled: true, path: broken },
          ok: { name: "ok", enabled: true, path: alpha },
        }),
        [`${hello}/.oxide/plugin.json`]: "{}",
        [`${broken}/agents/x.md`]: "no manifest here",
        [`${alpha}/.claude-plugin/plugin.json`]: "{}",
      }),
    );
    assert.deepEqual(dirs, [alpha]);
  });

  it("reads a malformed state as no plugins", () => {
    assert.deepEqual(enabledPluginDirs(deps({ [statePath]: "{not json" })), []);
    assert.deepEqual(enabledPluginDirs(deps({})), []);
    // `path` has no serde default and `enabled` must be a boolean, so one bad
    // entry fails the whole state parse in the CLI as well.
    assert.deepEqual(enabledPluginDirs(deps({ [statePath]: state({ hello: { name: "hello" } }) })), []);
    assert.deepEqual(
      enabledPluginDirs(deps({ [statePath]: state({ hello: { path: hello, enabled: "yes" } }) })),
      [],
    );
    assert.deepEqual(enabledPluginDirs(deps({ [statePath]: state({ hello: "not an object" }) })), []);
    assert.deepEqual(enabledPluginDirs(deps({ [statePath]: '{"plugins":[]}' })), []);
  });
});
