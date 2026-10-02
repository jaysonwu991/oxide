// The update check, read from the CLI's own `oxide update --check --json`.
//
// Which release is newest, what the release's manifest says its checksum is and
// whether this installation can be replaced in place are the CLI's rules, so the
// panel asks the installed binary for its answer rather than keeping a second
// copy of them in step — the same bargain the MCP listing and the session
// history make.

/// One check, as `crates/cli/src/update.rs` serializes it.
export interface UpdateCheck {
  /// The version the installed binary reports (`0.0.0` for a source build).
  current: string;
  /// The newest release's version, without the leading `v`.
  latest: string;
  /// The tag it is published under (`v0.33.0`).
  tag: string;
  /// Whether the check was pinned to a version the user asked for.
  pinned: boolean;
  updateAvailable: boolean;
  /// How the running binary was installed: `cargo`, `homebrew`, `prebuilt
  /// binary`, or `unknown`.
  installation: string;
  /// Whether `oxide update` may replace this installation itself. A Homebrew or
  /// unknown installation is handed to the user instead.
  installable: boolean;
  /// The binary the update would replace.
  path: string;
  /// What the terminal would tell the user to run, in its own words.
  advice: string;
  /// The release page for `tag`.
  releaseUrl: string;
}

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
  };
}

/// The check the panel runs: the same one the terminal's `oxide update --check`
/// performs, reported as JSON so the answer is read rather than scraped.
export function updateCheckArgs(): string[] {
  return ["update", "--check", "--json"];
}

/// The install the panel runs once the check says it can: the update the
/// terminal runs, which replaces the installed binary in place.
export function updateInstallArgs(): string[] {
  return ["update"];
}

/// Whether the CLI refused the check because it does not know `--json`.
///
/// An `oxide` released before this panel is the CLI most machines have, and it
/// answers a flag it predates with clap's own `unexpected argument '--json'
/// found` on stderr and exit code 2. That is not a check that failed: the
/// installation is simply older than the report this panel reads, and the plain
/// `oxide update` the dialog offers in its place works on every version — which
/// matters most here, since a user asking to update is exactly the one running
/// the older binary.
export function rejectsJson(stderr: string): boolean {
  return stderr.includes("unexpected argument '--json'");
}

function stringOf(value: unknown): string {
  return typeof value === "string" ? value : "";
}
