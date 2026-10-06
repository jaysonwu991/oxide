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

/// The CLI's own last resort for a model the table does not know, mirroring
/// `Config::default_context_window` in `crates/core/src/config.rs`.
export const DEFAULT_CONTEXT_WINDOW = 128_000;

/// The vendor and region prefixes a model id may carry, mirroring `VENDORS` in
/// `crates/core/src/config.rs`.
export const MODEL_VENDORS = [
  "anthropic",
  "openai",
  "xai",
  "spacexai",
  "moonshotai",
  "mistral",
  "qwen",
  "meta",
  "cohere",
  "amazon",
  "ai21",
  "deepseek",
  "zai",
  "z-ai",
  "google",
  "us",
  "eu",
  "apac",
  "in",
  "jp",
  "au",
  "global",
  "bedrock",
  "vertex",
];

/// The part of a model id that names the model itself, with the vendor and
/// region prefixes off (`us.anthropic.claude-opus-4-6-v1` →
/// `claude-opus-4-6-v1`), so two spellings of one model can be compared.
function vendorless(model: string): string {
  let rest = model.split("/").pop() ?? model;
  for (;;) {
    const dot = rest.indexOf(".");
    if (dot < 0 || !MODEL_VENDORS.includes(rest.slice(0, dot))) break;
    rest = rest.slice(dot + 1);
  }
  return rest;
}

/// The spellings of a model id a table key may match, mirroring
/// `context_window_candidates`: the id itself, the part after the last `/`
/// (`anthropic/claude-opus-4.5`), and the vendor-stripped form.
function contextWindowCandidates(model: string): string[] {
  const normalized = model.trim().toLowerCase();
  const candidates = [normalized];
  const basename = normalized.split("/").pop() ?? normalized;
  if (basename !== normalized) candidates.push(basename);
  const stripped = vendorless(normalized);
  if (stripped !== basename && stripped !== normalized) candidates.push(stripped);
  return candidates;
}

/// The input context windows of known models, mirroring
/// `Config::builtin_context_window` in `crates/core/src/config.rs` — the same
/// rows, matched the same way. This is the fallback the footer paints before
/// `oxide context --json` answers and for a CLI too old to know the command;
/// `test/context.test.ts` reads the Rust table and holds the two together, so a
/// model added there cannot be forgotten here.
export const MODEL_CONTEXT_WINDOWS: readonly (readonly [string, number])[] = [
  // OpenAI
  ["gpt-3.5", 16_385],
  ["gpt-4.1", 1_047_576],
  ["gpt-4o", 128_000],
  ["gpt-4-turbo", 128_000],
  ["gpt-4", 8_192],
  ["o1", 200_000],
  ["o3", 200_000],
  ["o4-mini", 200_000],
  ["gpt-5.4-mini", 400_000],
  ["gpt-5.4-nano", 400_000],
  ["gpt-5.4", 1_050_000],
  ["gpt-5.5", 1_050_000],
  ["gpt-5.6", 1_050_000],
  ["gpt-5.2-chat", 128_000],
  ["gpt-5.3-chat", 128_000],
  ["gpt-5-chat", 128_000],
  ["gpt-5", 400_000],
  // Anthropic
  ["claude-fable-5", 1_000_000],
  ["claude-3.5", 200_000],
  ["claude-3-5", 200_000],
  ["claude-3-7", 200_000],
  ["claude-3-opus", 200_000],
  ["claude-3-sonnet", 200_000],
  ["claude-3-haiku", 200_000],
  ["claude-haiku-4.5", 200_000],
  ["claude-haiku-4-5", 200_000],
  ["claude-opus-4.1", 200_000],
  ["claude-opus-4-1", 200_000],
  ["claude-opus-4.5", 200_000],
  ["claude-opus-4-5", 200_000],
  ["claude-opus-4.6", 1_000_000],
  ["claude-opus-4-6", 1_000_000],
  ["claude-opus-4.7", 1_000_000],
  ["claude-opus-4-7", 1_000_000],
  ["claude-opus-4.8", 1_000_000],
  ["claude-opus-4-8", 1_000_000],
  ["claude-opus-4", 200_000],
  ["claude-opus-5", 1_000_000],
  ["claude-sonnet-4.5", 1_000_000],
  ["claude-sonnet-4-5", 1_000_000],
  ["claude-sonnet-4.6", 1_000_000],
  ["claude-sonnet-4-6", 1_000_000],
  ["claude-sonnet-4", 200_000],
  ["claude-sonnet-5", 1_000_000],
  // Google
  ["gemini-3.1-flash-lite-image", 65_536],
  ["gemini-3.1-flash-live", 131_072],
  ["gemini-3.5-flash-lite", 1_048_576],
  ["gemini-3.5-flash", 200_000],
  ["gemini-3-pro-image", 65_536],
  ["gemini", 1_048_576],
  // DeepSeek
  ["deepseek-v3.2", 131_072],
  ["deepseek-v3", 131_072],
  ["deepseek-flash", 1_000_000],
  ["deepseek-v4", 1_048_576],
  ["deepseek", 128_000],
  // Z.AI / GLM
  ["glm-5.3", 1_000_000],
  ["glm-5.2", 1_048_576],
  ["glm-5.1", 204_800],
  ["glm-5-turbo", 200_000],
  ["glm-5", 204_800],
  ["glm-4.7", 204_800],
  ["glm-4.6", 204_800],
  ["glm-4", 128_000],
  // xAI
  ["grok-4.20", 2_000_000],
  ["grok-4.3", 1_000_000],
  ["grok-4.5", 500_000],
  ["grok-4.6", 500_000],
  ["grok-4.7", 500_000],
  ["grok-build", 256_000],
  ["grok-3", 131_072],
  ["grok", 131_072],
  // Moonshot
  ["kimi-for-coding", 1_048_576],
  ["kimi-k3", 1_048_576],
  ["kimi-k2.7", 262_144],
  ["kimi-k2", 131_072],
  // Mistral, Meta, Cohere, Upstage
  ["mistral-large", 262_144],
  ["mistral", 131_072],
  ["llama-3.3", 131_072],
  ["llama-3.1", 131_072],
  ["command-r", 128_000],
  ["solar-pro", 524_288],
  // Alibaba Qwen
  ["qwen3.7", 1_000_000],
  ["qwen3.6", 1_000_000],
  ["qwen3.5", 1_000_000],
  ["qwen3-coder", 262_144],
  ["qwen3-max", 262_144],
  ["qwen3", 131_072],
  ["qwen-plus", 1_000_000],
  ["qwen-max", 32_768],
  ["qwen", 32_768],
];

/// The input context window of a known model, mirroring
/// `Config::builtin_context_window` in `crates/core/src/config.rs`: the entry
/// whose key is the longest prefix of the id, its basename
/// (`anthropic/claude-opus-4.5`) or its vendor-stripped form
/// (`us.anthropic.claude-opus-4-6-v1`), case-insensitively. Returns 0 for a
/// model the table does not know.
export function modelContextWindow(model: string | undefined): number {
  const candidates = contextWindowCandidates(model ?? "");
  let best = 0;
  let bestLength = -1;
  for (const [prefix, window] of MODEL_CONTEXT_WINDOWS) {
    if (window <= 0 || !candidates.some((candidate) => candidate.startsWith(prefix))) continue;
    if (prefix.length > bestLength) {
      best = window;
      bestLength = prefix.length;
    }
  }
  return best;
}

/// What `contextWindow` resolves from: the pieces the CLI composes the window
/// out of, plus its own answer when the panel has one.
export interface ContextWindowInput {
  env: Record<string, string | undefined>;
  summary: ConfigSummary | null;
  /// The model a turn would run with: the `oxide.model` setting, else the one
  /// in `config.json`. The window is the model's, so a named model decides it.
  model: string;
  /// What `oxide context --json` resolved, when the panel has an answer for
  /// this model. The CLI sees a `modelContextWindows` override in the shared
  /// settings and the window the provider's catalog published, neither of which
  /// is read here, so its answer outranks this file's own resolution.
  resolved?: number;
}

/// The window the CLI measures context against, mirroring
/// `Config::context_window`: the `OXIDE_CONTEXT_LIMIT` override, else the
/// window the CLI resolved, else an explicit `context_window`, else the
/// model's known window, falling back to the CLI's own 128k last resort, kept
/// at least as large as the response cap for compatibility with older
/// configurations.
export function contextWindow(input: ContextWindowInput): number {
  const override = contextWindowFromEnv(input.env);
  if (override) return override;
  if (input.resolved && input.resolved > 0) return input.resolved;
  const summary = input.summary;
  const configured =
    summary?.contextWindow || modelContextWindow(input.model) || DEFAULT_CONTEXT_WINDOW;
  return Math.max(configured, summary?.maxTokens ?? 0);
}

/// The models remembered for one provider (`provider_models`), used before or
/// when the complete CLI catalog cannot load. A model id is sent to whichever
/// provider the CLI has active, so a remembered model from another one would
/// run against the wrong endpoint. Switching provider is the terminal's
/// `/connect`, which updates `config.json` and therefore this list.
export function modelsForProvider(
  summary: { models: { provider: string; model: string }[] } | null,
  provider: string,
): { provider: string; model: string }[] {
  const wanted = provider.trim().toLowerCase();
  if (!wanted) return [];
  return (summary?.models ?? []).filter((entry) => entry.provider.trim().toLowerCase() === wanted);
}
