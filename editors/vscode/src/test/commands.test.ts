// The manifest and the extension host have to agree: a contributed command with
// no handler, or a footer chip whose click the controller ignores, fails
// silently at the moment a user picks it.

import assert from "node:assert/strict";
import * as fs from "node:fs";
import * as path from "node:path";
import { describe, it } from "node:test";

import { footerState } from "../core/footer";
import { emptyUsage } from "../core/protocol";

/// The package root: this file compiles to `out/test/`.
const root = path.join(__dirname, "..", "..");

interface Manifest {
  contributes: {
    commands: { command: string; title: string; category?: string }[];
  };
}

const manifest = JSON.parse(fs.readFileSync(path.join(root, "package.json"), "utf8")) as Manifest;
const extension = fs.readFileSync(path.join(root, "src", "extension.ts"), "utf8");
const chat = fs.readFileSync(path.join(root, "src", "chat.ts"), "utf8");

describe("command contributions", () => {
  it("registers every command it contributes", () => {
    assert.ok(manifest.contributes.commands.length > 0);
    for (const { command, title, category } of manifest.contributes.commands) {
      assert.ok(title, `${command} has a title`);
      assert.equal(category, "Oxide", `${command} is grouped under Oxide`);
      // The registration may wrap onto its own line, so only the argument is
      // matched.
      const registered = new RegExp(`registerCommand\\(\\s*"${command.replace(/\./g, "\\.")}"`);
      assert.ok(registered.test(extension), `${command} has a registered handler`);
    }
  });

  it("handles every footer chip the controller paints", () => {
    const state = footerState({
      model: "",
      provider: "",
      contextWindow: 0,
      reasoning: "auto",
      agent: "",
      agentCount: 0,
      access: "untrusted",
      trustSetting: "default",
      defaultTrust: "ask",
      savedTrust: undefined,
      sessionId: null,
      branch: "",
      autoCompact: true,
      usage: emptyUsage(),
    });
    for (const chip of state.chips) {
      assert.ok(chat.includes(`case "${chip.id}":`), `the ${chip.id} chip is handled`);
    }
  });
});
