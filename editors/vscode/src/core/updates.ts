// The update check, read from the CLI's own `oxide update --check --json
// --component extension`.
//
// Which release is newest, which file this platform installs and what its
// checksum is are the shared rules in `oxide_core::updates`, so the panel asks
// the installed binary for the answer — the same bargain the MCP listing and
// the session history make. What it asks about is the extension's own release
// train: the panel installs its own release and never the CLI's.

/// One artifact a release carries, as `oxide_core::updates::Artifact`
/// serializes it.
export interface UpdateAsset {
  /// The file's name on the release page (`oxide-vscode-0.34.0.vsix`).
  name: string;
  /// Where to fetch it.
  url: string;
  /// The `sha256:<hex>` the release records for it, or `""` when it records
  /// none — the release, not this panel, is what says how to know the download
  /// arrived whole.
  digest: string;
}

/// One check, as `crates/core/src/updates.rs::Check` serializes it.
export interface UpdateCheck {
  /// Which release train this is (`extension`), so a check answered about
  /// another component is not read as the panel's own.
  component: string;
  /// The version the panel reported running (`0.0.0` for a source build).
  current: string;
  /// The newest release's version, without the tag prefix.
  latest: string;
  /// The tag it is published under (`extension-v0.34.0`).
  tag: string;
  /// Whether the check was pinned to a version the user asked for.
  pinned: boolean;
  updateAvailable: boolean;
  /// How the running installation was found to have been made. Empty here:
  /// the panel's own version is the only installation there is.
  installation: string;
  /// Whether the thing that ran the check — the CLI — may replace itself. It
  /// says nothing about the panel, which installs its own `.vsix` (see
  /// `updateVsix`).
  installable: boolean;
  path: string;
  /// What to do instead when the caller cannot install the release itself.
  advice: string;
  /// The release page for `tag`.
  releaseUrl: string;
  /// The artifact this platform installs, when the release carries one.
  asset: UpdateAsset | null;
}

/// The component the panel asks about: the extension's own releases, published
/// under `extension-v*`.
export const UPDATE_COMPONENT = "extension";

/// Parses the answer. A CLI that predates `--json` prints its human report, and
/// a run that failed prints an error, so a body that is not the check yields
/// null rather than a check whose empty fields the dialog would paint as a
/// release with no version.
export function parseUpdateCheck(output: string): UpdateCheck | null {
  const text = output.trim();
  if (!text) return null;
  let value: unknown;
  try {
    value = JSON.parse(text);
  } catch {
    return null;
  }
  if (!value || typeof value !== "object" || Array.isArray(value)) return null;
  const record = value as Record<string, unknown>;
  const latest = stringOf(record.latest);
  // A check without a version is not a check: everything the dialog says hangs
  // off the release the CLI resolved.
  if (!latest) return null;
  return {
    component: stringOf(record.component),
    current: stringOf(record.current),
    latest,
    tag: stringOf(record.tag) || `v${latest}`,
    pinned: record.pinned === true,
    updateAvailable: record.updateAvailable === true,
    installation: stringOf(record.installation) || "unknown",
    installable: record.installable === true,
    path: stringOf(record.path),
    advice: stringOf(record.advice),
    releaseUrl: stringOf(record.releaseUrl),
    asset: assetOf(record.asset),
  };
}

/// The `.vsix` this panel installs, or null when the check resolved none: a
/// panel that is up to date, a release with no build for this platform, or an
/// artifact that is not a VSIX. `installable` is about the CLI that ran the
/// check and has no say here — the panel installs its own release, which is
/// what the check's advice says from the other side.
export function updateVsix(check: UpdateCheck): UpdateAsset | null {
  if (!check.updateAvailable || !check.asset) return null;
  if (!/\.vsix$/i.test(check.asset.name) || !check.asset.url) return null;
  return check.asset;
}

/// The check the panel runs: the shared resolver in the installed CLI, asked
/// about the extension's own releases and told which version this panel is, so
/// the answer is read rather than scraped. `current` is left out when the
/// panel could not read its own version, which leaves the newest release
/// offered rather than skipped.
export function updateCheckArgs(current: string): string[] {
  const args = ["update", "--check", "--json", "--component", UPDATE_COMPONENT];
  if (current) args.push("--current", current);
  return args;
}

/// The install a CLI too old to answer the check is offered. The panel's own
/// release is not what a binary that predates `--json` can install, and that
/// older binary is the thing standing between the user and a check, so the row
/// it gets is the CLI update that works on any version.
export function updateInstallArgs(): string[] {
  return ["update"];
}

/// Whether the CLI refused the check because it does not know `--json`.
///
/// An `oxide` released before this panel answers a flag it predates with clap's
/// own `unexpected argument '--json' found` on stderr and exit code 2. That is
/// not a check that failed: the installation is simply older than the report
/// this panel reads, and updating it is what makes the check — and with it the
/// extension's own updates — reachable from here.
export function rejectsJson(stderr: string): boolean {
  return stderr.includes("unexpected argument '--json'");
}

function assetOf(value: unknown): UpdateAsset | null {
  if (!value || typeof value !== "object" || Array.isArray(value)) return null;
  const record = value as Record<string, unknown>;
  const name = stringOf(record.name);
  const url = stringOf(record.url);
  if (!name || !url) return null;
  return { name, url, digest: stringOf(record.digest) };
}

function stringOf(value: unknown): string {
  return typeof value === "string" ? value : "";
}
