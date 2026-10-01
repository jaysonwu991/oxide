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
  /// The models remembered per provider (`provider_models`), used as the
  /// model picker's immediate/failure fallback while the CLI loads a catalog.
  models: { provider: string; model: string }[];
  /// The reply cap (`max_tokens`).
  maxTokens: number;
  /// The full input window (`context_window`), separate from the reply cap.
  contextWindow: number;
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
    contextWindow:
      typeof record.context_window === "number" && record.context_window > 0
        ? record.context_window
        : 0,
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

/// The input context window of a known model, mirroring
/// `Config::builtin_context_window` in `crates/core/src/config.rs`: matched by
/// its longest prefix so `gpt-4o` and `gpt-4.1` do not fall under `gpt-4`.
/// Returns 0 for a model the table does not know. The panel is TypeScript and
/// cannot link the crate, so the table is kept here in step with the Rust one.
export function modelContextWindow(model: string | undefined): number {
  const windows: [string, number][] = [
    ["gpt-3.5", 16_385],
    ["gpt-4.1", 1_000_000],
    ["gpt-4o", 128_000],
    ["gpt-4-turbo", 128_000],
    ["gpt-4", 8_192],
    ["o1", 200_000],
    ["o3", 200_000],
    ["o4-mini", 200_000],
    ["claude-3-5", 200_000],
    ["claude-3-7", 200_000],
    ["claude-3-opus", 200_000],
    ["claude-3-sonnet", 200_000],
    ["claude-3-haiku", 200_000],
    ["deepseek", 128_000],
    ["glm-4", 128_000],
    ["glm-5", 128_000],
    ["gemini", 1_048_576],
  ];
  const name = (model ?? "").trim().toLowerCase();
  let best = 0;
  let bestLength = -1;
  for (const [prefix, window] of windows) {
    if (name.startsWith(prefix) && prefix.length > bestLength) {
      best = window;
      bestLength = prefix.length;
    }
  }
  return best;
}

/// The window the CLI measures context against, mirroring
/// `Config::context_window`: the `OXIDE_CONTEXT_LIMIT` override, else an
/// explicit `context_window`, else the model's known window, falling back to
/// 1M, kept at least as large as the response cap for compatibility with older
/// configurations.
export function contextWindow(
  env: Record<string, string | undefined>,
  summary: ConfigSummary | null,
): number {
  const override = contextWindowFromEnv(env);
  if (override) return override;
  const configured =
    summary?.contextWindow || modelContextWindow(summary?.model) || 1_000_000;
  return Math.max(configured, summary?.maxTokens ?? 0);
}

/// The models remembered for one provider (`provider_models`), used before or
/// when the complete CLI catalog cannot load. A model id is sent to whichever
/// provider the CLI has active, so a remembered model from another one would
/// run against the wrong endpoint. Switching provider is the terminal's
/// `/login`, which updates `config.json` and therefore this list.
export function modelsForProvider(
  summary: { models: { provider: string; model: string }[] } | null,
  provider: string,
): { provider: string; model: string }[] {
  const wanted = provider.trim().toLowerCase();
  if (!wanted) return [];
  return (summary?.models ?? []).filter((entry) => entry.provider.trim().toLowerCase() === wanted);
}
