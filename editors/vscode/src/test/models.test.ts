import assert from "node:assert/strict";
import { describe, it } from "node:test";

import { modelChoiceId, modelsListArgs, parseModelCatalog, parseModelChoice } from "../core/models";

describe("model catalog", () => {
  it("asks the CLI for every logged-in provider's catalog", () => {
    // Not `--active`: a model behind another provider is a row of the picker,
    // which is how the panel switches providers without the terminal. The CLI
    // fans the requests out at once, so the wait is the slowest one.
    assert.deepEqual(modelsListArgs(), ["models", "--json"]);
  });

  it("returns every provider's catalog, the one in use first", () => {
    const catalog = parseModelCatalog(
      JSON.stringify({
        active: "portkey",
        current: "claude-opus-5",
        providers: [
          {
            provider: "openai",
            active: false,
            models: ["gpt-5.4", "gpt-5.4", ""],
          },
          {
            provider: "portkey",
            active: true,
            current: "claude-opus-5",
            models: ["claude-opus-5", "gpt-5.6-sol"],
          },
        ],
      }),
    );
    assert.deepEqual(catalog, {
      active: "portkey",
      current: "claude-opus-5",
      models: [
        { provider: "portkey", model: "claude-opus-5" },
        { provider: "portkey", model: "gpt-5.6-sol" },
        { provider: "openai", model: "gpt-5.4" },
      ],
      errors: [],
    });
  });

  it("keeps a provider's catalog failure readable rather than dropping it", () => {
    // One provider that could not be listed is not a listing that failed: the
    // rows for it are what is missing, and the panel says which.
    assert.deepEqual(
      parseModelCatalog(
        JSON.stringify({
          active: "portkey",
          current: "claude-opus-5",
          providers: [
            { provider: "portkey", active: true, models: [], error: "forbidden" },
            { provider: "openai", active: false, models: ["gpt-5.4"] },
          ],
        }),
      ),
      {
        active: "portkey",
        current: "claude-opus-5",
        models: [{ provider: "openai", model: "gpt-5.4" }],
        errors: [{ provider: "portkey", error: "forbidden" }],
      },
    );
  });

  it("tags a model of a provider the listing named only by its entry", () => {
    // The tag is what a pick switches to, so it falls back to the active name
    // rather than to nothing.
    const catalog = parseModelCatalog(
      JSON.stringify({ active: "zai", providers: [{ active: true, models: ["glm-5"] }] }),
    );
    assert.deepEqual(catalog?.models, [{ provider: "zai", model: "glm-5" }]);
  });

  /// Two providers can serve the same model id, so a row carries the pair rather
  /// than the id: the action has to switch to the provider whose row was clicked,
  /// not to the first entry that happens to match the id.
  it("tells the same model id apart under two providers", () => {
    const models = [
      { provider: "openrouter", model: "claude-sonnet-5" },
      { provider: "anthropic", model: "claude-sonnet-5" },
    ];
    assert.equal(modelChoiceId("anthropic", "claude-sonnet-5"), "anthropic:claude-sonnet-5");
    assert.deepEqual(parseModelChoice(models, "anthropic:claude-sonnet-5"), {
      provider: "anthropic",
      model: "claude-sonnet-5",
    });
    assert.deepEqual(parseModelChoice(models, "openrouter:claude-sonnet-5"), {
      provider: "openrouter",
      model: "claude-sonnet-5",
    });
    // A row with no provider of its own, and a model typed into the search box,
    // name no provider: the caller falls back to the one in use.
    assert.equal(modelChoiceId("", "glm-5"), "glm-5");
    assert.equal(parseModelChoice(models, "glm-5"), null);
    assert.equal(parseModelChoice(models, "  "), null);
    // The pair is composed and compared rather than split, since a model id may
    // carry the separator itself.
    const dated = [{ provider: "openai", model: "gpt-4o:2024-08-06" }];
    assert.deepEqual(parseModelChoice(dated, "openai:gpt-4o:2024-08-06"), dated[0]);
  });

  it("refuses output that is not a catalog", () => {
    assert.equal(parseModelCatalog("not json"), null);
    assert.equal(parseModelCatalog('{"providers":{}}'), null);
    assert.equal(parseModelCatalog(""), null);
  });
});
