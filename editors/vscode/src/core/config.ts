// Locating the shared Oxide configuration.
//
// The CLI, the desktop app and this extension all read the same
// `<platform config dir>/Oxide` (see `oxide_core::config::config_dir`), which
// is what lets a provider connected in the terminal show up here. Nothing is
// written to it from the extension: `oxide.model` and friends are VS Code
// settings, and `--model` is simply passed to the CLI.

export interface ConfigEnv {
  platform: NodeJS.Platform | string;
  env: Record<string, string | undefined>;
  home: string;
  exists: (candidate: string) => boolean;
}

/// The directory holding `config.json`, `auth.json`, `sessions/`, `plugins/`
/// and `trust.json`.
export function configDir(input: ConfigEnv): string {
  const { platform, env, home } = input;
  const join = (base: string[]) => [...base, "Oxide"].join("/").replace(/\/+/g, "/");
  let base: string;
  if (platform === "darwin") {
    base = join([home, "Library", "Application Support"]);
  } else if (platform === "win32") {
    const roaming = env.APPDATA || `${home}/AppData/Roaming`;
    base = join([roaming]);
  } else {
    base = join([env.XDG_CONFIG_HOME || `${home}/.config`]);
  }
  // A pre-migration install kept the lowercase directory; the CLI moves it on
  // first write, so reading the old one keeps the extension usable meanwhile.
  const legacy = base.replace(/\/Oxide$/, "/oxide");
  if (!input.exists(base) && legacy !== base && input.exists(legacy)) return legacy;
  return base;
}

export interface ConfigSummary {
  provider: string;
  model: string;
}

/// The parts of `config.json` the status bar shows. A malformed file is
/// reported as unconfigured rather than throwing.
export function parseConfigSummary(raw: string): ConfigSummary | null {
  try {
    const value: unknown = JSON.parse(raw);
    if (!value || typeof value !== "object" || Array.isArray(value)) return null;
    const record = value as Record<string, unknown>;
    return {
      provider: typeof record.provider === "string" ? record.provider : "",
      model: typeof record.model === "string" ? record.model : "",
    };
  } catch {
    return null;
  }
}

/// The context window from `OXIDE_CONTEXT_LIMIT`, when the user set one. The
/// model's own window lives in the CLI's config, which the extension cannot
/// read, so a percentage is only shown when this override is present.
export function contextWindowFromEnv(env: Record<string, string | undefined>): number {
  const raw = env.OXIDE_CONTEXT_LIMIT;
  if (!raw) return 0;
  const value = Number(raw);
  return Number.isFinite(value) && value > 0 ? value : 0;
}
