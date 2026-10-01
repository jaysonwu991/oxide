// The model catalog printed by `oxide models --json`. The CLI owns provider
// authentication, endpoints, bundled fallbacks and catalog normalization; the
// extension only validates the small view it needs to paint.

import { parseObject } from "./json";

export interface ModelCatalog {
  active: string;
  current: string;
  models: { provider: string; model: string }[];
  error: string;
}

export function modelsListArgs(): string[] {
  return ["models", "--json"];
}

export function parseModelCatalog(raw: string): ModelCatalog | null {
  const root = parseObject(raw);
  if (!root || !Array.isArray(root.providers)) return null;
  const active = typeof root.active === "string" ? root.active : "";
  const current = typeof root.current === "string" ? root.current : "";
  const provider = root.providers.find(
    (entry): entry is Record<string, unknown> =>
      Boolean(entry) &&
      typeof entry === "object" &&
      !Array.isArray(entry) &&
      ((entry as Record<string, unknown>).active === true ||
        (typeof (entry as Record<string, unknown>).provider === "string" &&
          String((entry as Record<string, unknown>).provider).toLowerCase() === active.toLowerCase())),
  );
  if (!provider) return { active, current, models: [], error: "" };
  const name = typeof provider.provider === "string" ? provider.provider : active;
  const seen = new Set<string>();
  const models: { provider: string; model: string }[] = [];
  for (const value of Array.isArray(provider.models) ? provider.models : []) {
    if (typeof value !== "string") continue;
    const model = value.trim();
    if (!model || seen.has(model)) continue;
    seen.add(model);
    models.push({ provider: name, model });
  }
  return {
    active,
    current,
    models,
    error: typeof provider.error === "string" ? provider.error : "",
  };
}
