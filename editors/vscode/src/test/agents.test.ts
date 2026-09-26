// The agents the footer offers are the ones `--agent` resolves a name against:
// markdown files whose name is their frontmatter `name`, else their stem.

import assert from "node:assert/strict";
import { describe, it } from "node:test";

import { agentChoices, frontmatterValue } from "../core/agents";

describe("frontmatterValue", () => {
  it("reads a quoted or bare value from the leading block", () => {
    const text = '---\nname: planner\ndescription: "Plans the work"\nmode: primary\n---\nPrompt.\n';
    assert.equal(frontmatterValue(text, "name"), "planner");
    assert.equal(frontmatterValue(text, "description"), "Plans the work");
    assert.equal(frontmatterValue(text, "mode"), "primary");
  });

  it("ignores a nested key and a missing block", () => {
    assert.equal(frontmatterValue("---\npermission:\n  name: nested\n---\n", "name"), "");
    assert.equal(frontmatterValue("No frontmatter.\nname: planner\n", "name"), "");
    assert.equal(frontmatterValue("---\nname: planner\n---\n", "missing"), "");
  });
});

describe("agentChoices", () => {
  it("falls back to the file stem when the frontmatter has no name", () => {
    assert.deepEqual(agentChoices([{ stem: "rust-reviewer", text: "Review Rust code.\n" }]), [
      { name: "rust-reviewer", description: "" },
    ]);
  });

  it("keeps the first file of a duplicated name, so project wins", () => {
    const choices = agentChoices([
      { stem: "planner", text: '---\nname: planner\ndescription: project\n---\n' },
      { stem: "planner", text: '---\nname: planner\ndescription: global\n---\n' },
    ]);
    assert.deepEqual(choices, [{ name: "planner", description: "project" }]);
  });

  it("sorts by name", () => {
    const choices = agentChoices([
      { stem: "zeta", text: "" },
      { stem: "alpha", text: "" },
      { stem: "beta", text: '---\nname: mid\n---\n' },
    ]);
    assert.deepEqual(
      choices.map((choice) => choice.name),
      ["alpha", "mid", "zeta"],
    );
  });
});
