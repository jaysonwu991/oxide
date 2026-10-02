// The update check the panel offers is the CLI's own answer, read out of
// `oxide update --check --json --component extension`: which release of this
// extension is newest, which file carries it and what its checksum is. Reading
// it here is what keeps the panel from re-deciding any of that — and the
// component it asks about is what keeps it from offering the oxide command line
// in place of its own release.

import assert from "node:assert/strict";
import { describe, it } from "node:test";

import {
  parseUpdateCheck,
  rejectsCheck,
  updateCheckArgs,
  updateInstallArgs,
  updateVsix,
} from "../core/updates";

/// What the CLI prints for `--check --json --component extension`, as it printed
/// it (the struct is serialized pretty, and the desktop app and this panel both
/// read the same bytes).
const answer = JSON.stringify(
  {
    component: "extension",
    current: "0.32.0",
    latest: "0.34.0",
    tag: "extension-v0.34.0",
    pinned: false,
    updateAvailable: true,
    installation: "",
    installable: false,
    path: "",
    advice:
      "Update available: the Oxide extension for VS Code installs this release itself (Check for Updates…), or oxide-vscode-0.34.0.vsix is on the release page.",
    releaseUrl: "https://github.com/jaysonwu991/oxide/releases/tag/extension-v0.34.0",
    asset: {
      name: "oxide-vscode-0.34.0.vsix",
      url: "https://github.com/jaysonwu991/oxide/releases/download/extension-v0.34.0/oxide-vscode-0.34.0.vsix",
      digest: "sha256:e02cf08397c21fc2b6f35e1d1220ac1e68bc056dbdef2ae7eb181308146fc101",
    },
  },
  null,
  2,
);

describe("update check", () => {
  it("reads the answer the CLI printed", () => {
    const check = parseUpdateCheck(answer);
    assert.ok(check);
    assert.equal(check.component, "extension");
    assert.equal(check.current, "0.32.0");
    assert.equal(check.latest, "0.34.0");
    assert.equal(check.tag, "extension-v0.34.0");
    assert.equal(check.updateAvailable, true);
    assert.equal(check.releaseUrl, "https://github.com/jaysonwu991/oxide/releases/tag/extension-v0.34.0");
    assert.equal(check.pinned, false);
    // Whether the CLI may replace itself says nothing about this panel, which
    // installs its own release: the check reports the artifact for that.
    assert.equal(check.installable, false);
    assert.equal(check.asset?.name, "oxide-vscode-0.34.0.vsix");
    assert.match(check.asset?.url ?? "", /extension-v0\.34\.0\/oxide-vscode-0\.34\.0\.vsix$/);
    assert.match(check.asset?.digest ?? "", /^sha256:[0-9a-f]{64}$/);
  });

  it("picks the VSIX out of the release", () => {
    const check = parseUpdateCheck(answer);
    assert.ok(check);
    assert.equal(updateVsix(check)?.name, "oxide-vscode-0.34.0.vsix");
    // Nothing to install means nothing to install: a release already current,
    // one whose artifact this platform cannot use, and one whose file is not a
    // VSIX at all are all refused here rather than handed to VS Code.
    assert.equal(updateVsix({ ...check, updateAvailable: false }), null);
    assert.equal(updateVsix({ ...check, asset: null }), null);
    assert.equal(
      updateVsix({ ...check, asset: { ...check.asset!, name: "oxide-vscode-0.34.0.zip" } }),
      null,
    );
  });

  it("takes an up-to-date answer as one, with nothing to install", () => {
    const check = parseUpdateCheck(
      JSON.stringify({
        component: "extension",
        current: "0.34.0",
        latest: "0.34.0",
        tag: "extension-v0.34.0",
        updateAvailable: false,
        asset: null,
        advice: null,
      }),
    );
    assert.ok(check);
    assert.equal(check.updateAvailable, false);
    // The CLI leaves `advice` out when there is nothing to advise, and the
    // dialog says nothing rather than a sentence about a release already here.
    assert.equal(check.advice, "");
    assert.equal(check.asset, null);
  });

  it("keeps the release pinned when the check was asked for one", () => {
    const check = parseUpdateCheck(
      JSON.stringify({ latest: "0.30.0", tag: "extension-v0.30.0", pinned: true, current: "0.34.0" }),
    );
    assert.ok(check);
    assert.equal(check.pinned, true);
    assert.equal(check.tag, "extension-v0.30.0");
    // An entry the CLI did not send is not invented: an installation it did not
    // name is the unknown one, and nothing is offered as installable.
    assert.equal(check.installation, "unknown");
    assert.equal(check.installable, false);
    assert.equal(check.updateAvailable, false);
    assert.equal(check.asset, null);
  });

  it("reads a tag for a release that came without one", () => {
    const check = parseUpdateCheck(JSON.stringify({ latest: "0.34.0" }));
    assert.ok(check);
    assert.equal(check.tag, "v0.34.0", "the version with the v the tags carry");
  });

  it("refuses a body that is not a check rather than painting an empty one", () => {
    // A CLI that predates `--json` prints its human report, which is what the
    // check looks like falling back to the prose: no version, no check.
    assert.equal(parseUpdateCheck("Oxide update\nInstallation: cargo\nCurrent: 0.32.0\n"), null);
    assert.equal(parseUpdateCheck(""), null);
    // An error on stdout, or a JSON body the check is not.
    assert.equal(parseUpdateCheck("error: could not reach GitHub"), null);
    assert.equal(parseUpdateCheck("[1,2,3]"), null);
    assert.equal(parseUpdateCheck("null"), null);
    // A check without the version the whole dialog hangs off.
    assert.equal(parseUpdateCheck(JSON.stringify({ current: "0.32.0" })), null);
    assert.equal(parseUpdateCheck(JSON.stringify({ latest: 34 })), null);
    // An artifact that is not one is no artifact rather than half of one.
    const partial = parseUpdateCheck(JSON.stringify({ latest: "0.34.0", asset: { name: "x.vsix" } }));
    assert.ok(partial);
    assert.equal(partial.asset, null);
  });

  it("asks about this extension's own release train, and its own version", () => {
    // The check is the shared resolution in the installed CLI, asked about the
    // extension; the component is what keeps the answer from being the oxide
    // command line's own newest release, and `--current` is what makes it a
    // comparison rather than a listing.
    assert.deepEqual(updateCheckArgs("0.32.0"), [
      "update",
      "--check",
      "--json",
      "--component",
      "extension",
      "--current",
      "0.32.0",
    ]);
    // A panel that cannot read its own version asks the same question without
    // claiming to run nothing, which leaves the newest release offered.
    assert.deepEqual(updateCheckArgs(""), [
      "update",
      "--check",
      "--json",
      "--component",
      "extension",
    ]);
    // The one install left to the CLI is the update that replaces a binary too
    // old to answer the check at all.
    assert.deepEqual(updateInstallArgs(), ["update"]);
  });

  it("tells a CLI too old to know the check's flags from a check that failed", () => {
    // Measured against the released 0.33.0 binary, which predates the flag: clap
    // writes this on stderr and exits 2. It is not a network failure and not an
    // answer, and the dialog offers the update that replaces it.
    assert.equal(
      rejectsCheck("error: unexpected argument '--json' found\n\nUsage: oxide update --check\n"),
      true,
    );
    // A binary new enough for `--json` but older than the flags the check asks
    // about the extension's own train with: it rejects the invocation the same
    // way, so it is the same state — a CLI older than this panel — rather than a
    // check that failed.
    assert.equal(
      rejectsCheck(
        "error: unexpected argument '--component' found\n\nUsage: oxide update --check [OPTIONS]\n",
      ),
      true,
    );
    assert.equal(rejectsCheck("error: unexpected argument '--current' found"), true);
    // A check that could not reach GitHub says something else, and so does a
    // flag mentioned in a message that is not a refusal.
    assert.equal(
      rejectsCheck("Error: requesting the manifest\n\nCaused by: Operation timed out (os error 60)"),
      false,
    );
    assert.equal(rejectsCheck(""), false);
    assert.equal(rejectsCheck("error: unexpected argument '--check' found"), false);
  });
});
