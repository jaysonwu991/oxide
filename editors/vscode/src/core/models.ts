// The model catalogs printed by `oxide models --json`. The CLI owns provider
// authentication, endpoints, bundled fallbacks and catalog normalization; the
// extension only validates the small view it needs to paint.
//
// Every provider the machine holds a credential for is read, not only the one in
// use: the desktop app's model picker groups the same way, so a model behind
// another logged-in provider is a row here too rather than something the reader
// has to switch providers in the terminal to reach.

import { parseObject } from "./json";

export interface ModelCatalog {
  /// The provider `config.json` selects.
  active: string;
  /// The model the active provider resolves to.
  current: string;
  /// Every model the listing offered, each tagged with the provider that
  /// serves it — the tag is what a pick switches to.
  models: { provider: string; model: string }[];
  /// The providers whose catalog could not be listed, and why: a listing that
  /// failed for one of several is worth saying rather than hiding.
  errors: { provider: string; error: string }[];
}

export function modelsListArgs(): string[] {
  // Every stored provider is asked, since a row from one of them is a model the
  // reader can pick: the CLI fans the requests out at once, so the wait is the
  // slowest provider rather than the sum of them. `--active` narrows this to the
  // one in use, which is what a caller that only wants the current catalog's
  // name should ask for instead.
  return ["models", "--json"];
}

/// Parses the listing. A CLI too old to know `--json` prints its human table, so
/// a body that is not the listing yields null rather than a catalog with no
/// models in it.
export function parseModelCatalog(raw: string): ModelCatalog | null {
  const root = parseObject(raw);
  if (!root || !Array.isArray(root.providers)) return null;
  const active = typeof root.active === "string" ? root.active : "";
  const current = typeof root.current === "string" ? root.current : "";
  const models: { provider: string; model: string }[] = [];
  const errors: { provider: string; error: string }[] = [];
  // The active provider's catalog leads, the way the desktop app's picker lists
  // it first: the model in use is the one being changed.
  const listed = root.providers.filter((entry) => isObject(entry)) as Record<string, unknown>[];
  const ordered = [
    ...listed.filter((entry) => entry.active === true || sameName(entry.provider, active)),
    ...listed.filter((entry) => !(entry.active === true || sameName(entry.provider, active))),
  ];
  for (const provider of ordered) {
    const name = typeof provider.provider === "string" ? provider.provider : active;
    const error = typeof provider.error === "string" ? provider.error.trim() : "";
    if (error) errors.push({ provider: name, error });
    const seen = new Set<string>();
    for (const value of Array.isArray(provider.models) ? provider.models : []) {
      if (typeof value !== "string") continue;
      const model = value.trim();
      if (!model || seen.has(model)) continue;
      seen.add(model);
      models.push({ provider: name, model });
    }
  }
  return { active, current, models, errors };
}

function isObject(value: unknown): boolean {
  return Boolean(value) && typeof value === "object" && !Array.isArray(value);
}

function sameName(value: unknown, name: string): boolean {
  return typeof value === "string" && value.toLowerCase() === name.toLowerCase();
}
