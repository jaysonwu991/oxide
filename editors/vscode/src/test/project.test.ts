// `projectInfo` is what the footer reports about the shared on-disk state. The
// reads are injected, so the whole thing is exercised without a filesystem.

import assert from "node:assert/strict";
import * as path from "node:path";
import { describe, it } from "node:test";

import { projectInfo, projectRoot, type ProjectDeps } from "../core/project";

const repo = path.join("/repo");
const pkg = path.join(repo, "pkg");

/// A file map shaped like the paths the module reads. Any ancestor directory of
/// a known file counts as existing, so `.git/HEAD` implies `.git`.
function tree(files: Record<string, string>, env: Record<string, string | undefined> = {}): ProjectDeps {
  const dirs = new Set<string>();
  for (const file of Object.keys(files)) {
    let dir = path.dirname(file);
    while (dir && dir !== "/" && !dirs.has(dir)) {
      dirs.add(dir);
      dir = path.dirname(dir);
    }
  }
  return {
    read: (file) => files[file] ?? null,
    list: (dir) =>
      Object.entries(files)
        .filter(([file]) => path.dirname(file) === dir && file.endsWith(".md"))
        .map(([file, text]) => ({ stem: path.basename(file, ".md"), text })),
    exists: (candidate) => candidate in files || dirs.has(candidate),
    realpath: (candidate) => candidate,
    configDir: "/config",
    home: "/home/me",
    env,
  };
}

describe("projectRoot", () => {
  it("walks up to the closest project marker", () => {
    assert.equal(projectRoot(pkg, tree({ [path.join(repo, ".git/HEAD")]: "ref: refs/heads/main\n" })), repo);
  });

  it("accepts .oxide and .claude as markers too", () => {
    assert.equal(projectRoot(pkg, tree({ [path.join(pkg, ".oxide/settings.json")]: "{}" })), pkg);
    assert.equal(projectRoot(pkg, tree({ [path.join(repo, ".claude/agents/a.md")]: "" })), repo);
  });

  it("has no root when nothing marks one", () => {
    assert.equal(projectRoot(pkg, tree({})), null);
  });
});

describe("projectInfo", () => {
  it("reads the model, the remembered models and the window from config.json", () => {
    const deps = tree({
      "/config/config.json": '{"provider":"zai","model":"glm-5","provider_models":{"zai":"glm-5"}}',
    });
    const info = projectInfo(pkg, "default", deps);
    assert.equal(info.provider, "zai");
    assert.equal(info.model, "glm-5");
    assert.deepEqual(info.models, [{ provider: "zai", model: "glm-5" }]);
    // An untouched config has no `max_tokens`, so the window is the 128k floor.
    assert.equal(info.contextWindow, 128_000);
    assert.equal(info.configPath, path.join("/config", "config.json"));
  });

  it("lets OXIDE_CONTEXT_LIMIT override the window", () => {
    const deps = tree({ "/config/config.json": '{"max_tokens":1}' }, { OXIDE_CONTEXT_LIMIT: "300000" });
    assert.equal(projectInfo(pkg, "default", deps).contextWindow, 300_000);
  });

  it("resolves access from trust.json, defaultProjectTrust and the setting", () => {
    const deps = tree({
      "/config/trust.json": JSON.stringify({ [repo]: true }),
      "/config/settings.json": '{"defaultProjectTrust":"never"}',
    });
    assert.equal(projectInfo(pkg, "default", deps).access, "trusted");
    assert.equal(projectInfo(pkg, "default", deps).savedTrust, true);
    assert.equal(projectInfo(pkg, "never", deps).access, "untrusted");
    assert.equal(projectInfo(pkg, "always", tree({})).access, "trusted");

    const undecided = projectInfo(pkg, "default", tree({}));
    assert.equal(undecided.access, "untrusted");
    assert.equal(undecided.savedTrust, undefined);
    assert.equal(undecided.defaultTrust, "ask");
  });

  it("lets the project's .oxide/settings.json override the global compaction flag", () => {
    const deps = tree({
      [path.join(repo, ".git/HEAD")]: "ref: refs/heads/main\n",
      "/config/settings.json": '{"compaction":{"enabled":true}}',
      [path.join(repo, ".oxide/settings.json")]: '{"compaction":{"enabled":false}}',
    });
    assert.equal(projectInfo(pkg, "default", deps).autoCompact, false);
  });

  it("reports the branch and the discovered agents", () => {
    const deps = tree({
      [path.join(repo, ".git/HEAD")]: "ref: refs/heads/fix/footer\n",
      [path.join(repo, ".oxide/agents/reviewer.md")]: '---\nname: reviewer\ndescription: Reviews\n---\n',
      [path.join(repo, ".claude/agents/reviewer.md")]: "Loses to the .oxide one.\n",
      "/config/agents/reviewer.md": '---\nname: reviewer\ndescription: global\n---\n',
      "/config/agents/planner.md": '---\nname: planner\ndescription: config dir\n---\n',
      "/home/me/.oxide/agents/planner.md": "Loses to the config dir one.\n",
      "/home/me/.oxide/agents/dreamer.md": "\n",
      "/home/me/.claude/agents/legacy.md": "Legacy.\n",
    });
    const info = projectInfo(pkg, "always", deps);
    assert.equal(info.branch, "fix/footer");
    assert.deepEqual(
      info.agents.map((agent) => `${agent.name}:${agent.description}`),
      ["dreamer:", "legacy:", "planner:config dir", "reviewer:Reviews"],
    );
  });

  it("drops project agents when the project is not trusted", () => {
    const deps = tree({
      [path.join(repo, ".git/HEAD")]: "ref: refs/heads/main\n",
      [path.join(repo, ".oxide/agents/reviewer.md")]: "Project prompt and permissions.\n",
      "/config/agents/planner.md": "Global.\n",
    });
    // `default` with no saved decision resolves to untrusted, and the CLI
    // activates `--agent` before it drops project resources, so the project's
    // own agents must not be offered.
    const untrusted = projectInfo(pkg, "default", deps);
    assert.equal(untrusted.access, "untrusted");
    assert.deepEqual(
      untrusted.agents.map((agent) => agent.name),
      ["planner"],
    );
    assert.deepEqual(
      projectInfo(pkg, "always", deps).agents.map((agent) => agent.name),
      ["planner", "reviewer"],
    );
  });

  it("offers the agents installed plugins bundle", () => {
    const plugin = "/config/plugins/hello";
    const deps = tree({
      [path.join(repo, ".oxide/agents/worker.md")]: "Project.\n",
      "/config/plugins/config.json": JSON.stringify({
        plugins: { hello: { name: "hello", enabled: true, path: plugin } },
      }),
      [`${plugin}/.oxide/plugin.json`]: "{}",
      [`${plugin}/agents/helper.md`]: '---\nname: helper\ndescription: Bundled\n---\n',
      [`${plugin}/agents/worker.md`]: "Loses to the project one.\n",
    });
    const trusted = projectInfo(pkg, "always", deps);
    assert.deepEqual(
      trusted.agents.map((agent) => `${agent.name}:${agent.description}`),
      ["helper:Bundled", "worker:"],
    );
    // A plugin is a global resource: an untrusted project still gets it.
    assert.deepEqual(
      projectInfo(pkg, "never", deps).agents.map((agent) => agent.name),
      ["helper", "worker"],
    );
  });

  it("degrades to empty values when nothing is configured", () => {
    const info = projectInfo(pkg, "default", tree({}));
    assert.equal(info.model, "");
    assert.equal(info.branch, "");
    assert.deepEqual(info.agents, []);
    assert.equal(info.contextWindow, 128_000);
  });
});
