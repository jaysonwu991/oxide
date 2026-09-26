// Locating the shared Oxide configuration.
//
// The CLI, the desktop app and this extension all read the same
// `<platform config dir>/Oxide` (see `oxide_core::config::config_dir`), which
// is what lets a provider connected in the terminal show up here. Nothing is
// written to it from the extension: `oxide.model` and friends are VS Code
// settings, and `--model` is simply passed to the CLI.

import { parseObject } from "./json";

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
  /// The models remembered per provider (`provider_models`), offered by the
  /// footer's model picker so switching providers keeps its own model.
  models: { provider: string; model: string }[];
  /// The reply cap (`max_tokens`), which the CLI also uses as its default
  /// context window.
  maxTokens: number;
}

/// The parts of `config.json` the status bar and the footer show. A malformed
/// file is reported as unconfigured rather than throwing.
export function parseConfigSummary(raw: string | null): ConfigSummary | null {
  const record = parseObject(raw);
  if (!record) return null;
  const models: { provider: string; model: string }[] = [];
  const remembered = record.provider_models;
  if (remembered && typeof remembered === "object" && !Array.isArray(remembered)) {
    for (const [provider, model] of Object.entries(remembered as Record<string, unknown>)) {
      if (typeof model === "string" && model.trim()) models.push({ provider, model: model.trim() });
    }
  }
  return {
    provider: typeof record.provider === "string" ? record.provider : "",
    model: typeof record.model === "string" ? record.model : "",
    models,
    maxTokens: typeof record.max_tokens === "number" && record.max_tokens > 0 ? record.max_tokens : 0,
  };
}

/// The context window from `OXIDE_CONTEXT_LIMIT`, when the user set one. The
/// CLI parses the value as a `u64` and falls back to its configured window when
/// that fails, so a decimal (`1.5`), an exponent (`1e5`) or anything else has to
/// be ignored here too — otherwise the footer would report a window the run is
/// not using.
export function contextWindowFromEnv(env: Record<string, string | undefined>): number {
  const raw = env.OXIDE_CONTEXT_LIMIT?.trim();
  // A leading `+` is the one form `u64::from_str` accepts beyond the digits.
  if (!raw || !/^\+?\d+$/.test(raw)) return 0;
  const value = Number(raw);
  return Number.isSafeInteger(value) && value > 0 ? value : 0;
}

/// The window the CLI measures context against, mirroring
/// `Config::context_window`: the `OXIDE_CONTEXT_LIMIT` override, else
/// `max_tokens` floored at 128k. The CLI's own default is 8192, so an
/// untouched config reads as 128k.
export function contextWindow(
  env: Record<string, string | undefined>,
  summary: ConfigSummary | null,
): number {
  return contextWindowFromEnv(env) || Math.max(summary?.maxTokens ?? 0, 128_000);
}

/// The models remembered for one provider (`provider_models`), which is what a
/// picker may offer: a model id is sent to whichever provider the CLI has
/// active, so a remembered model from another one would run against the wrong
/// endpoint. Switching provider is the terminal's `/login`, which updates
/// `config.json` and therefore this list.
export function modelsForProvider(
  summary: { models: { provider: string; model: string }[] } | null,
  provider: string,
): { provider: string; model: string }[] {
  const wanted = provider.trim().toLowerCase();
  if (!wanted) return [];
  return (summary?.models ?? []).filter((entry) => entry.provider.trim().toLowerCase() === wanted);
}
