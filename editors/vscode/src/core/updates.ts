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

/// What a window does with the release a check resolved, on its own.
///
/// VS Code updates what it installed from the Marketplace, and this extension is
/// a `.vsix` from the release page: nothing else will ever notice its own newer
/// release. A release that carries the file this editor installs is therefore
/// offered with the row that fetches it — the marketplace's own bargain, a check
/// that reports and a click that installs; one that carries none — no build for
/// this platform, or a CLI old enough to answer without the artifact — can only
/// be pointed at, since there is no file here to hand to VS Code. A check with
/// nothing newer is nothing at all.
export type BackgroundAction =
  | { k: "install"; vsix: UpdateAsset }
  | { k: "offer" }
  | { k: "none" };

export function backgroundAction(check: UpdateCheck): BackgroundAction {
  const vsix = updateVsix(check);
  if (vsix) return { k: "install", vsix };
  return check.updateAvailable ? { k: "offer" } : { k: "none" };
}

/// What a window remembers between launches about its own updates: when it last
/// asked the CLI, and the release an install has put in VS Code.
export interface UpdateMemory {
  /// When the last check ran, in Unix milliseconds — 0 for one that never did,
  /// which is every window's first launch.
  checkedAt: number;
  /// The version an install left in VS Code, or `""` for none. It outlives the
  /// window that installed it, which is what makes the reload still pending in
  /// the next one.
  installedVersion: string;
}

/// How long a remembered check is trusted. It is the interval the terminal's
/// launch notice refreshes on (`oxide_core::update_notice::REFRESH_AFTER_SECS`),
/// so a window opened twice in a morning asks GitHub once — long enough that a
/// daily launch costs one request, short enough that a release cut today is
/// noticed tomorrow.
export const BACKGROUND_CHECK_MS = 6 * 60 * 60 * 1000;

/// Whether a launch should ask the CLI which release of this extension is
/// newest.
///
/// A release an install put in VS Code is the one thing that still has to happen
/// somewhere else: every answer a check could give here would be the version
/// already on disk, since the code running is the one that was there when it was
/// replaced. Nothing is left to do but the reload, so a launch that has one
/// pending asks nothing and says nothing.
export function shouldBackgroundCheck(
  memory: UpdateMemory | null,
  current: string,
  now: number,
): boolean {
  if (memory && isNewer(memory.installedVersion, current)) return false;
  if (!memory?.checkedAt) return true;
  return now - memory.checkedAt >= BACKGROUND_CHECK_MS;
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

/// The flags the check asks the CLI with, in the order `updateCheckArgs` adds
/// them.
const CHECK_FLAGS = ["--json", "--component", "--current"];

/// Whether the CLI refused the check because it does not know one of the flags
/// it was asked with.
///
/// An `oxide` released before the check answers an argument it predates with
/// clap's own `unexpected argument '--json' found` on stderr and exit code 2 —
/// and a binary new enough to know `--json` but older than `--component` and
/// `--current` answers with the same words about those, which is why every flag
/// the check needs is read rather than only the first one. Either one is not a
/// check that failed: the installation is simply older than the report this
/// panel reads, and updating it is what makes the check — and with it the
/// extension's own updates — reachable from here.
export function rejectsCheck(stderr: string): boolean {
  return CHECK_FLAGS.some((flag) => stderr.includes(`unexpected argument '${flag}'`));
}

/// Whether `candidate` is a later release than `running`, by the numbers of the
/// two versions. A version this panel cannot read as numbers decides nothing,
/// which leaves the check that follows it asking rather than skipped.
function isNewer(candidate: string, running: string): boolean {
  const left = versionParts(candidate);
  const right = versionParts(running);
  if (!left || !right) return false;
  for (let index = 0; index < Math.max(left.length, right.length); index += 1) {
    const a = left[index] ?? 0;
    const b = right[index] ?? 0;
    if (a !== b) return a > b;
  }
  return false;
}

/// The numeric core of a version: `v0.34.0` and `0.34.0-rc.1` are `0.34.0`,
/// since the tags this panel compares are releases. Anything else — a version
/// from somewhere other than a release — is no answer rather than a guess.
function versionParts(version: string): number[] | null {
  const core = version.trim().replace(/^v/i, "").split(/[-+]/)[0];
  if (!/^\d+(\.\d+)*$/.test(core)) return null;
  return core.split(".").map(Number);
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
