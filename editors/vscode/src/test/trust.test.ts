// Project trust decides whether the next turn loads the workspace's own
// `.oxide` resources, so the footer's answer has to match the CLI's resolution.

import assert from "node:assert/strict";
import * as path from "node:path";
import { describe, it } from "node:test";

import { parseTrustStore, resolveAccess, trustDecision } from "../core/trust";

const identity = (candidate: string): string => candidate;

describe("parseTrustStore", () => {
  it("keeps the boolean decisions and drops anything else", () => {
    assert.deepEqual(parseTrustStore('{"/repo":true,"/other":false,"/junk":"yes"}'), {
      "/repo": true,
      "/other": false,
    });
  });

  it("reads a missing or malformed file as no decisions", () => {
    assert.deepEqual(parseTrustStore(null), {});
    assert.deepEqual(parseTrustStore("{"), {});
    assert.deepEqual(parseTrustStore("[true]"), {});
  });
});

describe("trustDecision", () => {
  const store = { "/repo": true, "/repo/private": false };

  it("uses the closest ancestor with a decision", () => {
    assert.equal(trustDecision(store, "/repo/pkg/src", identity), true);
    assert.equal(trustDecision(store, "/repo/private/deep", identity), false);
    assert.equal(trustDecision(store, "/repo/private", identity), false);
  });

  it("has no decision for a folder outside every saved path", () => {
    assert.equal(trustDecision(store, "/elsewhere", identity), undefined);
    assert.equal(trustDecision({}, "/repo", identity), undefined);
  });

  it("canonicalizes before matching, since trust.json stores resolved paths", () => {
    const realpath = (candidate: string): string =>
      candidate === "/link" ? "/repo" : candidate;
    assert.equal(trustDecision(store, "/link", realpath), true);
    assert.equal(trustDecision(store, "/link/pkg", realpath), true);
  });
});

describe("resolveAccess", () => {
  it("lets oxide.projectTrust override everything", () => {
    assert.equal(
      resolveAccess({ setting: "always", saved: false, defaultTrust: "never" }),
      "trusted",
    );
    assert.equal(
      resolveAccess({ setting: "never", saved: true, defaultTrust: "always" }),
      "untrusted",
    );
  });

  it("prefers a saved decision over defaultProjectTrust", () => {
    assert.equal(
      resolveAccess({ setting: "default", saved: true, defaultTrust: "never" }),
      "trusted",
    );
    assert.equal(
      resolveAccess({ setting: "default", saved: false, defaultTrust: "always" }),
      "untrusted",
    );
  });

  it("treats ask as untrusted, because a run cannot prompt", () => {
    assert.equal(
      resolveAccess({ setting: "default", saved: undefined, defaultTrust: "ask" }),
      "untrusted",
    );
    assert.equal(
      resolveAccess({ setting: "default", saved: undefined, defaultTrust: "always" }),
      "trusted",
    );
  });
});

describe("trust decisions above the folder", () => {
  it("walks the whole path to the root", () => {
    const decision = trustDecision({ [path.parse("/repo/pkg").root]: true }, "/repo/pkg", identity);
    assert.equal(decision, true);
  });
});
