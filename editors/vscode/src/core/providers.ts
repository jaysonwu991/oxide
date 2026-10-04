// The provider table printed by `oxide providers --json`: the same rows the
// desktop app's Connect dialog searches, drawn from the one table the core's
// `known_providers()` holds and the terminal's `/login` dialog uses. A front-end
// that only spawns the binary keeps no copy of it — the CLI owns which providers
// exist, which of them are stored, and which one `config.json` selects.
//
// Like the rest of `src/core/`, this imports nothing from `vscode`.

import { parseObject } from "./json";

/// One row of `oxide providers --json`.
export interface ProviderView {
  name: string;
  label: string;
  description: string;
  /// Where a key for this provider is issued, for the dialog's own words.
  keyUrl: string;
  /// Whether the provider needs no credential (a model server on this machine).
  local: boolean;
  /// Whether a credential is already stored for it.
  stored: boolean;
  /// Whether it is the provider `config.json` selects.
  active: boolean;
}

export function providersListArgs(): string[] {
  return ["providers", "--json"];
}

/// The arguments one login is run with. The key is never an argument — it would
/// be visible in the process listing and kept in a shell's history — so it goes
/// to the CLI on stdin with `--key-stdin`, which is what that flag is for.
export function providerLoginArgs(name: string, withKey: boolean): string[] {
  return ["login", name, "--json", ...(withKey ? ["--key-stdin"] : [])];
}

/// Parses the listing. A CLI older than this panel prints its human table, and a
/// roster that cannot be read is a dialog with nothing in it rather than a throw
/// in the middle of a click.
export function parseProviders(raw: string): ProviderView[] {
  const root = parseObject(raw);
  const list = root && Array.isArray(root.providers) ? root.providers : [];
  const providers: ProviderView[] = [];
  for (const value of list) {
    if (!value || typeof value !== "object" || Array.isArray(value)) continue;
    const record = value as Record<string, unknown>;
    const name = stringOf(record.name).trim();
    if (!name) continue;
    providers.push({
      name,
      label: stringOf(record.label).trim() || name,
      description: stringOf(record.description),
      keyUrl: stringOf(record.keyUrl),
      local: record.local === true,
      stored: record.stored === true,
      active: record.active === true,
    });
  }
  return providers;
}

/// The providers whose name, label or description mention the query, in the
/// table's own order. The table holds every provider a client can connect, which
/// is why it is searched rather than listed: the panel filters what the CLI
/// already answered, so a keystroke never spawns a process.
export function filterProviders(
  providers: readonly ProviderView[],
  query: string,
): ProviderView[] {
  const needle = query.trim().toLowerCase();
  if (!needle) return [...providers];
  return providers.filter((provider) =>
    `${provider.name} ${provider.label} ${provider.description}`.toLowerCase().includes(needle),
  );
}

/// What a login answered (`oxide login <name> --json`): the canonical provider,
/// the label to report, the model the selection resolved to, and whether the
/// provider needs no credential. `null` for output that is not that object, so a
/// caller falls back to what it already knew rather than throwing.
export interface LoginOutcome {
  provider: string;
  label: string;
  model: string;
  local: boolean;
}

export function parseLoginOutcome(raw: string): LoginOutcome | null {
  const root = parseObject(raw);
  if (!root || typeof root.provider !== "string") return null;
  return {
    provider: root.provider,
    label: stringOf(root.label) || root.provider,
    model: stringOf(root.model),
    local: root.local === true,
  };
}

/// What a row says beside it: the state a login would change, or nothing when
/// the provider is simply available.
export function providerState(provider: ProviderView): string {
  if (provider.active) return "In use";
  if (provider.stored) return "Stored";
  if (provider.local) return "No key needed";
  return "";
}

function stringOf(value: unknown): string {
  return typeof value === "string" ? value : "";
}
