import assert from "node:assert/strict";
import { describe, it } from "node:test";

import { parseReasoning, reasoningArgs } from "../core/reasoning";

describe("reasoning levels", () => {
  it("asks the CLI for the active model's levels", () => {
    assert.deepEqual(reasoningArgs(), ["reasoning", "--json"]);
    assert.deepEqual(reasoningArgs(true), ["reasoning", "--json", "--refresh"]);
    assert.deepEqual(reasoningArgs(false, "deepseek-flash"), [
      "reasoning",
      "--json",
      "--model",
      "deepseek-flash",
    ]);
    assert.deepEqual(reasoningArgs(true, "  "), ["reasoning", "--json", "--refresh"]);
  });

  it("reads the levels a model advertised", () => {
    assert.deepEqual(
      parseReasoning(
        JSON.stringify({
          current: "auto",
          supportsReasoning: true,
          reasoningLevels: ["off", "low", "high", "max", "low", ""],
        }),
      ),
      {
        current: "auto",
        supportsReasoning: true,
        levels: ["off", "low", "high", "max"],
      },
    );
  });

  it("keeps a model that advertised none on the built-in set", () => {
    assert.deepEqual(
      parseReasoning(JSON.stringify({ current: "high", supportsReasoning: true })),
      { current: "high", supportsReasoning: true, levels: [] },
    );
  });

  it("refuses output that is not a reasoning listing", () => {
    assert.equal(parseReasoning("not json"), null);
    assert.deepEqual(parseReasoning("{}"), {
      current: "auto",
      supportsReasoning: false,
      levels: [],
    });
  });
});
