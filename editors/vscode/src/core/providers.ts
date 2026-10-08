// The provider table printed by `oxide providers --json`: the same rows the
// desktop app's Connect dialog searches, drawn from the one table the core's
// `known_providers()` holds and the terminal's `/connect` dialog uses. A front-end
// that only spawns the binary keeps no copy of it — the CLI owns which providers
// exist, which of them are stored, and which one `config.json` selects.
//
// Like the rest of `src/core/`, this imports nothing from `vscode`.

import { parseObject } from "./json";

/// Where a provider's credential comes from, which is what decides whether a
/// login asks for a key at all.
///
/// - `key`: one typed here and stored by the CLI (`auth.json`).
/// - `external`: one the machine already holds — an AWS signing identity, a
///   Google application-default file — read where it lives and never copied.
/// - `none`: nothing to present; a model server on this machine.
export type CredentialMode = "key" | "external" | "none";

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
  /// Where its credential comes from, so a provider the machine's own identity
  /// authorizes is not asked for a key nothing would read.
  credential: CredentialMode;
}

/// Whether a login for this provider asks for a key: an empty one is a login
/// for a server on this machine and for a credential the machine already holds,
/// and is refused for every provider that keeps a key in `auth.json`.
export function needsKey(provider: ProviderView): boolean {
  return !provider.local && provider.credential !== "external";
}

export function providersListArgs(): string[] {
  return ["providers", "--json"];
}

/// The arguments one login is run with. The key is never an argument — it would
/// be visible in the process listing and kept in a shell's history — so it goes
/// to the CLI on stdin with `--key-stdin`, which is what that flag is for.
///
/// `model` names the model the selection resolves to, which is how a row of the
/// model picker that belongs to another provider switches to it: the provider is
/// resolved from the credential already stored for it, and the model travels with
/// the same call.
export function providerLoginArgs(name: string, withKey: boolean, model = ""): string[] {
  const args = ["login", name, "--json", ...(withKey ? ["--key-stdin"] : [])];
  if (model.trim()) args.push("--model", model.trim());
  return args;
}

/// The arguments one logout is run with: `oxide logout <name> --json` forgets
/// the credential the CLI stored, wherever it was stored from, and switches to
/// another logged-in provider when the one being signed out is the one in use.
export function providerLogoutArgs(name: string): string[] {
  return ["logout", name, "--json"];
}

/// What `oxide logout <name> --json` answered.
export interface LogoutOutcome {
  provider: string;
  label: string;
  /// Whether a credential was actually removed.
  removed: boolean;
  /// The provider the CLI switched to, when the one signed out was in use.
  switchedTo: string;
}

export function parseLogoutOutcome(raw: string): LogoutOutcome | null {
  const root = parseObject(raw);
  if (!root || typeof root.provider !== "string") return null;
  return {
    provider: root.provider,
    label: stringOf(root.label) || root.provider,
    removed: root.removed === true,
    switchedTo: stringOf(root.switchedTo),
  };
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
      // A CLI too old to answer this keeps the key field, which is what every
      // provider it knows about takes.
      credential: credentialOf(record.credential),
    });
  }
  return providers;
}

function credentialOf(value: unknown): CredentialMode {
  return value === "external" || value === "none" ? value : "key";
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
  if (provider.credential === "external") return "Machine credential";
  return "";
}

function stringOf(value: unknown): string {
  return typeof value === "string" ? value : "";
}
