import assert from "node:assert/strict";
import { describe, it } from "node:test";

import { modelsListArgs, parseModelCatalog } from "../core/models";

describe("model catalog", () => {
  it("asks the CLI for its normalized provider catalogs", () => {
    assert.deepEqual(modelsListArgs(), ["models", "--json"]);
  });

  it("returns the complete active-provider catalog", () => {
    const catalog = parseModelCatalog(
      JSON.stringify({
        active: "portkey",
        current: "claude-opus-5",
        providers: [
          {
            provider: "portkey",
            active: true,
            current: "claude-opus-5",
            models: ["claude-opus-5", "gpt-5.6-sol", "claude-opus-5", ""],
          },
          { provider: "openai", active: false, models: ["gpt-5.4"] },
        ],
      }),
    );
    assert.deepEqual(catalog, {
      active: "portkey",
      current: "claude-opus-5",
      models: [
        { provider: "portkey", model: "claude-opus-5" },
        { provider: "portkey", model: "gpt-5.6-sol" },
      ],
      error: "",
    });
  });

  it("keeps an active provider's catalog failure readable", () => {
    assert.deepEqual(
      parseModelCatalog(
        JSON.stringify({
          active: "portkey",
          current: "claude-opus-5",
          providers: [{ provider: "portkey", active: true, models: [], error: "forbidden" }],
        }),
      ),
      { active: "portkey", current: "claude-opus-5", models: [], error: "forbidden" },
    );
  });

  it("refuses output that is not a catalog", () => {
    assert.equal(parseModelCatalog("not json"), null);
    assert.equal(parseModelCatalog('{"providers":{}}'), null);
  });
});
