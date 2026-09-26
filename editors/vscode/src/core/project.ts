// The shared on-disk state the footer reports: the model and context window
// from `config.json`, the trust decision, the compaction setting, the git
// branch and the agents a turn can be sent to.
//
// Every read is injected, so the whole thing is testable without a filesystem,
// and every read is best-effort: a missing or malformed file degrades the chip
// it feeds instead of failing the panel.

import * as path from "node:path";

import { agentChoices, type AgentChoice, type AgentFile } from "./agents";
import type { TrustSetting } from "./args";
import { contextWindow, parseConfigSummary } from "./config";
import { gitBranch } from "./git";
import { enabledPluginDirs } from "./plugins";
import { sharedSettings } from "./settings";
import { resolveAccess, trustDecision, parseTrustStore, type Access } from "./trust";

export interface ProjectDeps {
  /// File contents, or `null` for anything unreadable (including a directory).
  read: (file: string) => string | null;
  /// The markdown files of a directory, with their contents.
  list: (dir: string) => AgentFile[];
  /// True when the path exists, file or directory — a `.git` marker is either.
  exists: (candidate: string) => boolean;
  /// The canonical spelling of a path, falling back to the input.
  realpath: (candidate: string) => string;
  /// `<platform config dir>/Oxide`, holding `config.json`, `settings.json` and
  /// `trust.json`.
  configDir: string;
  home: string;
  env: Record<string, string | undefined>;
}

export interface ProjectInfo {
  provider: string;
  /// The model from `config.json` (an `oxide.model` setting wins over it).
  model: string;
  models: { provider: string; model: string }[];
  contextWindow: number;
  access: Access;
  savedTrust: boolean | undefined;
  defaultTrust: "ask" | "always" | "never";
  autoCompact: boolean;
  branch: string;
  agents: AgentChoice[];
  configPath: string;
}

export function projectInfo(folder: string, trust: TrustSetting, deps: ProjectDeps): ProjectInfo {
  const configPath = path.join(deps.configDir, "config.json");
  const config = parseConfigSummary(deps.read(configPath));
  const root = projectRoot(folder, deps);
  const settings = sharedSettings(
    deps.read(path.join(deps.configDir, "settings.json")),
    deps.read(path.join(root ?? folder, ".oxide", "settings.json")),
    deps.env,
  );
  const savedTrust = trustDecision(
    parseTrustStore(deps.read(path.join(deps.configDir, "trust.json"))),
    folder,
    deps.realpath,
  );
  const access = resolveAccess({ setting: trust, saved: savedTrust, defaultTrust: settings.defaultTrust });
  return {
    provider: config?.provider ?? "",
    model: config?.model ?? "",
    models: config?.models ?? [],
    contextWindow: contextWindow(deps.env, config),
    access,
    savedTrust,
    defaultTrust: settings.defaultTrust,
    autoCompact: settings.autoCompact,
    branch: gitBranch(folder, { read: deps.read }),
    agents: agentChoices(agentFiles(root, access, deps)),
    configPath,
  };
}

/// The closest directory that looks like a project, mirroring
/// `oxide_core::ecosystem::project_root`. The session's own directory is the
/// workspace folder, but its resources and `.oxide/settings.json` live at the
/// repository root.
export function projectRoot(cwd: string, deps: Pick<ProjectDeps, "exists">): string | null {
  let current: string | null = cwd;
  while (current) {
    for (const marker of [".git", ".oxide", ".claude"]) {
      if (deps.exists(path.join(current, marker))) return current;
    }
    const parent = path.dirname(current);
    current = parent === current ? null : parent;
  }
  return null;
}

/// Every agent file the CLI could be asked for, in the CLI's own load order:
/// the project's layouts, then the installed plugins, then the global
/// directories. The first file of a name wins, which is what `upsert_agent`
/// does by overwriting, so a project agent shadows a plugin or global one.
///
/// The project's own directories are only read when it is trusted: an untrusted
/// run reloads the ecosystem without project resources, but `Config::load`
/// activates `--agent` before that reload, so offering a project agent would run
/// its prompt and permissions inside an untrusted project.
function agentFiles(root: string | null, access: Access, deps: ProjectDeps): AgentFile[] {
  const dirs = [
    ...(root && access === "trusted"
      ? [path.join(root, ".oxide", "agents"), path.join(root, ".claude", "agents")]
      : []),
    ...enabledPluginDirs(deps).map((dir) => path.join(dir, "agents")),
    path.join(deps.configDir, "agents"),
    path.join(deps.home, ".oxide", "agents"),
    path.join(deps.home, ".claude", "agents"),
  ];
  const files: AgentFile[] = [];
  for (const dir of dirs) files.push(...deps.list(dir));
  return files;
}
