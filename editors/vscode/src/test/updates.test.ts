// The update check the panel offers is the CLI's own answer, read out of
// `oxide update --check --json`: the release, the version installed now, and
// whether this installation is one `oxide update` may replace. Reading it here
// is what keeps the panel from re-deciding any of that.

import assert from "node:assert/strict";
import { describe, it } from "node:test";

import { parseUpdateCheck, rejectsJson, updateCheckArgs, updateInstallArgs } from "../core/updates";

/// What the CLI prints for `--check --json`, as it printed it (the struct is
/// serialized pretty, and the desktop app and this panel both read the same
/// bytes).
const answer = JSON.stringify(
  {
    current: "0.32.0",
    latest: "0.33.0",
    tag: "v0.33.0",
    pinned: false,
    updateAvailable: true,
    installation: "prebuilt binary",
    installable: true,
    path: "/home/me/.local/bin/oxide",
    advice: "Update available: run `oxide update` to install it.",
    releaseUrl: "https://github.com/jaysonwu991/oxide/releases/tag/v0.33.0",
  },
  null,
  2,
);

describe("update check", () => {
  it("reads the answer the CLI printed", () => {
    const check = parseUpdateCheck(answer);
    assert.ok(check);
    assert.equal(check.current, "0.32.0");
    assert.equal(check.latest, "0.33.0");
    assert.equal(check.tag, "v0.33.0");
    assert.equal(check.updateAvailable, true);
    assert.equal(check.installation, "prebuilt binary");
    assert.equal(check.installable, true);
    assert.equal(check.path, "/home/me/.local/bin/oxide");
    assert.equal(check.releaseUrl, "https://github.com/jaysonwu991/oxide/releases/tag/v0.33.0");
    assert.equal(check.pinned, false);
  });

  it("takes an up-to-date answer as one, with nothing to install", () => {
    const check = parseUpdateCheck(
      JSON.stringify({
        current: "0.33.0",
        latest: "0.33.0",
        tag: "v0.33.0",
        pinned: false,
        updateAvailable: false,
        installation: "cargo",
        installable: true,
        path: "/home/me/.cargo/bin/oxide",
        advice: null,
        releaseUrl: "https://github.com/jaysonwu991/oxide/releases/tag/v0.33.0",
      }),
    );
    assert.ok(check);
    assert.equal(check.updateAvailable, false);
    // The CLI leaves `advice` out when there is nothing to advise, and the
    // dialog says nothing rather than a sentence about a release already here.
    assert.equal(check.advice, "");
  });

  it("keeps the release pinned when the check was asked for one", () => {
    const check = parseUpdateCheck(
      JSON.stringify({ latest: "0.30.0", tag: "v0.30.0", pinned: true, current: "0.33.0" }),
    );
    assert.ok(check);
    assert.equal(check.pinned, true);
    assert.equal(check.tag, "v0.30.0");
    // An entry the CLI did not send is not invented: an installation it did not
    // name is the unknown one, and nothing is offered as installable.
    assert.equal(check.installation, "unknown");
    assert.equal(check.installable, false);
    assert.equal(check.updateAvailable, false);
  });

  it("reads a tag for a release that came without one", () => {
    const check = parseUpdateCheck(JSON.stringify({ latest: "0.33.0" }));
    assert.ok(check);
    assert.equal(check.tag, "v0.33.0", "the version with the v the tags carry");
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
    assert.equal(parseUpdateCheck(JSON.stringify({ latest: 33 })), null);
  });

  it("runs the CLI's own check and install", () => {
    // The check is the one the terminal performs and the install is the update
    // it runs; the panel adds no flag of its own, so an installation that needs
    // `--force` is still refused here rather than replaced quietly.
    assert.deepEqual(updateCheckArgs(), ["update", "--check", "--json"]);
    assert.deepEqual(updateInstallArgs(), ["update"]);
  });

  it("tells a CLI too old to know --json from a check that failed", () => {
    // Measured against the released 0.33.0 binary, which predates the flag: clap
    // writes this on stderr and exits 2. It is not a network failure and not an
    // answer, and the dialog offers the update that replaces it.
    assert.equal(
      rejectsJson("error: unexpected argument '--json' found\n\nUsage: oxide update --check\n"),
      true,
    );
    // A check that could not reach GitHub says something else, and so does a
    // `--json` mentioned in a message that is not a refusal.
    assert.equal(
      rejectsJson("Error: requesting the manifest\n\nCaused by: Operation timed out (os error 60)"),
      false,
    );
    assert.equal(rejectsJson(""), false);
    assert.equal(rejectsJson("error: unexpected argument '--check' found"), false);
  });
});
