// `settings.json` is shared with the CLI, so the two keys the footer reports
// have to resolve the way `config.rs` and `compact.rs` resolve them.

import assert from "node:assert/strict";
import { describe, it } from "node:test";

import { parseSettings, sharedSettings } from "../core/settings";

describe("parseSettings", () => {
  it("accepts the trust aliases the CLI accepts", () => {
    assert.equal(parseSettings('{"defaultProjectTrust":"ask"}').defaultTrust, "ask");
    assert.equal(parseSettings('{"defaultProjectTrust":"always"}').defaultTrust, "always");
    assert.equal(parseSettings('{"defaultProjectTrust":"trust"}').defaultTrust, "always");
    assert.equal(parseSettings('{"defaultProjectTrust":"never"}').defaultTrust, "never");
    assert.equal(parseSettings('{"defaultProjectTrust":"deny"}').defaultTrust, "never");
  });

  it("ignores an unknown trust value instead of guessing", () => {
    assert.equal(parseSettings('{"defaultProjectTrust":"maybe"}').defaultTrust, undefined);
    assert.equal(parseSettings('{"defaultProjectTrust":true}').defaultTrust, undefined);
  });

  it("reads compaction.enabled, and only when it is a boolean", () => {
    assert.equal(parseSettings('{"compaction":{"enabled":false}}').autoCompact, false);
    assert.equal(parseSettings('{"compaction":{"enabled":true}}').autoCompact, true);
    // The CLI deserializes the block, so a wrong type falls back to the default.
    assert.equal(parseSettings('{"compaction":{"enabled":"false"}}').autoCompact, undefined);
    assert.equal(parseSettings('{"compaction":{}}').autoCompact, undefined);
  });

  it("reads the notification switch the CLI writes, and only as a boolean", () => {
    assert.equal(parseSettings('{"notifyOnComplete":false}').notifyOnComplete, false);
    assert.equal(parseSettings('{"notifyOnComplete":true}').notifyOnComplete, true);
    assert.equal(parseSettings('{"notifyOnComplete":"false"}').notifyOnComplete, undefined);
    assert.equal(parseSettings("{}").notifyOnComplete, undefined);
  });

  it("reads nothing out of a missing or malformed file", () => {
    assert.deepEqual(parseSettings(null), {});
    assert.deepEqual(parseSettings("{"), {});
    assert.deepEqual(parseSettings("[]"), {});
  });
});

describe("sharedSettings", () => {
  it("defaults to ask and auto-compaction on", () => {
    assert.deepEqual(sharedSettings(null, null), {
      defaultTrust: "ask",
      autoCompact: true,
      notifyOnComplete: true,
    });
  });

  it("resolves the notification flag like notify.rs", () => {
    assert.equal(sharedSettings(null, '{"notifyOnComplete":false}').notifyOnComplete, false);
    assert.equal(
      sharedSettings('{"notifyOnComplete":false}', '{"notifyOnComplete":true}').notifyOnComplete,
      true,
      "the project's .oxide/settings.json wins per key",
    );
    assert.equal(
      sharedSettings(
        '{"notifyOnComplete":true}',
        null,
        { OXIDE_NOTIFY_ON_COMPLETE: "false" },
      ).notifyOnComplete,
      false,
      "the env override wins over both files",
    );
    // `notify.rs::env_bool` accepts a bare true/false and nothing else.
    assert.equal(
      sharedSettings(null, '{"notifyOnComplete":false}', { OXIDE_NOTIFY_ON_COMPLETE: "0" })
        .notifyOnComplete,
      false,
    );
  });

  it("takes defaultProjectTrust from the global file only", () => {
    const settings = sharedSettings('{"defaultProjectTrust":"always"}', '{"defaultProjectTrust":"never"}');
    assert.equal(settings.defaultTrust, "always");
  });

  it("lets the project's .oxide/settings.json win per key", () => {
    assert.equal(
      sharedSettings('{"compaction":{"enabled":true}}', '{"compaction":{"enabled":false}}').autoCompact,
      false,
    );
    assert.equal(
      sharedSettings('{"defaultProjectTrust":"always"}', '{"compaction":{"enabled":false}}').defaultTrust,
      "always",
    );
  });

  it("honors the OXIDE_COMPACTION_ENABLED override", () => {
    assert.equal(sharedSettings(null, null, { OXIDE_COMPACTION_ENABLED: "false" }).autoCompact, false);
    assert.equal(
      sharedSettings('{"compaction":{"enabled":false}}', null, { OXIDE_COMPACTION_ENABLED: "true" })
        .autoCompact,
      true,
    );
    assert.equal(sharedSettings(null, null, { OXIDE_COMPACTION_ENABLED: "junk" }).autoCompact, true);
  });
});
