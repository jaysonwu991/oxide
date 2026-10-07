// The update check the panel offers is the CLI's own answer, read out of
// `oxide update --check --json --component extension`: which release of this
// extension is newest, which file carries it and what its checksum is. Reading
// it here is what keeps the panel from re-deciding any of that — and the
// component it asks about is what keeps it from offering the oxide command line
// in place of its own release.
//
// What a launch does with that answer on its own is decided here too: whether
// there is anything to ask at all, and whether the release the check resolved is
// installed by the panel or only offered.

import assert from "node:assert/strict";
import { describe, it } from "node:test";

import {
  BACKGROUND_CHECK_MS,
  backgroundAction,
  cliCheckArgs,
  parseUpdateCheck,
  rejectsCheck,
  shouldBackgroundCheck,
  UPDATE_COMPONENT,
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

  /// The other train the panel's dialog reports: the CLI itself, which is the
  /// binary every turn runs through and the one this panel reaches GitHub with.
  /// It is asked without `--current` — the CLI answers with the version of the
  /// binary that is running — and the component the answer names is what keeps
  /// one train's release from being read as the other's.
  it("asks about the CLI's own train, and reads only that answer", () => {
    assert.deepEqual(cliCheckArgs(), ["update", "--check", "--json", "--component", "cli"]);
    const cliAnswer = JSON.stringify({
      component: "cli",
      current: "0.32.0",
      latest: "0.34.0",
      tag: "v0.34.0",
      updateAvailable: true,
      installable: true,
      path: "/home/me/.local/bin/oxide",
      advice: "Run oxide update.",
      asset: { name: "oxide-x86_64.tar.gz", url: "https://example/x", digest: "" },
    });
    assert.equal(parseUpdateCheck(cliAnswer, "cli")?.latest, "0.34.0");
    // The extension's own answer read as the CLI's — an older CLI that ignored
    // `--component` — is no answer, rather than the panel reporting the CLI's
    // release as its own.
    for (const expect of [UPDATE_COMPONENT, "desktop"]) {
      assert.equal(parseUpdateCheck(cliAnswer, expect), null, expect);
    }
    // A check that does not say which train it is stays readable: the field
    // arrives with the CLI that learned `--component`, and the panel asks for
    // one train at a time anyway.
    assert.equal(parseUpdateCheck(JSON.stringify({ latest: "0.34.0" }), "cli")?.latest, "0.34.0");
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

describe("the check a launch makes on its own", () => {
  /// A fixed clock, so the interval is a number rather than a test that runs
  /// long enough to cross it.
  const now = 1_800_000_000_000;

  it("asks when nothing is remembered, and not again inside the interval", () => {
    assert.equal(shouldBackgroundCheck(null, "0.32.0", now), true, "a window that never asked");
    assert.equal(
      shouldBackgroundCheck({ checkedAt: 0, installedVersion: "" }, "0.32.0", now),
      true,
      "one that remembers no check at all",
    );
    assert.equal(
      shouldBackgroundCheck({ checkedAt: now - 60_000, installedVersion: "" }, "0.32.0", now),
      false,
      "a window opened again a minute later asks nothing",
    );
    // The interval is `oxide_core::update_notice::REFRESH_AFTER_SECS`, and the
    // boundary belongs to the check rather than to the wait.
    assert.equal(BACKGROUND_CHECK_MS, 6 * 60 * 60 * 1000);
    assert.equal(
      shouldBackgroundCheck(
        { checkedAt: now - BACKGROUND_CHECK_MS + 1, installedVersion: "" },
        "0.32.0",
        now,
      ),
      false,
    );
    assert.equal(
      shouldBackgroundCheck(
        { checkedAt: now - BACKGROUND_CHECK_MS, installedVersion: "" },
        "0.32.0",
        now,
      ),
      true,
      "a check six hours old is asked again",
    );
  });

  it("waits for the reload an install is already waiting on", () => {
    // VS Code holds the release an earlier window installed, and the code
    // running here is the one it replaced: every answer a check could give
    // would be that same release, so the launch asks nothing and the user is
    // told nothing — the restart happened, or it did not, and neither is the
    // panel's to nag about.
    const pending = { checkedAt: now - BACKGROUND_CHECK_MS - 1, installedVersion: "0.34.0" };
    assert.equal(shouldBackgroundCheck(pending, "0.32.0", now), false);
    // Once the window runs what was installed there is nothing pending, and the
    // interval decides again — a version newer than the one running is the only
    // thing that skips a check.
    assert.equal(shouldBackgroundCheck(pending, "0.34.0", now), true);
    assert.equal(shouldBackgroundCheck(pending, "0.35.0", now), true);
    // A remembered version this panel cannot read as a release (an older
    // extension wrote the key) skips nothing.
    assert.equal(
      shouldBackgroundCheck({ checkedAt: 0, installedVersion: "dev" }, "0.32.0", now),
      true,
    );
  });

  it("offers the release that carries the file this editor installs", () => {
    const check = parseUpdateCheck(answer);
    assert.ok(check);
    const action = backgroundAction(check);
    // A `.vsix` that did not come from the Marketplace is never updated by VS
    // Code, so the row the notification raises is the one that fetches it: the
    // panel installs from that row rather than pointing at the release page.
    assert.deepEqual(action, { k: "install", vsix: check.asset });
    assert.equal(action.k === "install" ? action.vsix.name : "", "oxide-vscode-0.34.0.vsix");
  });

  it("points at a release it has no file for, and says nothing when there is none", () => {
    const check = parseUpdateCheck(answer);
    assert.ok(check);
    // No build for this platform, an artifact that is not a VSIX, and a CLI old
    // enough to answer without the file: there is nothing here to hand to VS
    // Code, so the notification opens the release the file is on rather than
    // offering an install that could not run.
    assert.deepEqual(backgroundAction({ ...check, asset: null }), { k: "offer" });
    assert.deepEqual(
      backgroundAction({ ...check, asset: { ...check.asset!, name: "oxide-vscode-0.34.0.zip" } }),
      { k: "offer" },
    );
    // A release already current — which is also what a check pinned to a
    // version older than this one resolves to — is nothing to do at all.
    assert.deepEqual(backgroundAction({ ...check, updateAvailable: false }), { k: "none" });
  });
});
