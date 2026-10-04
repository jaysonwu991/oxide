// The provider table the panel searches is the CLI's own listing, so what a row
// says — which provider is in use, which one has a credential — has to survive a
// CLI that answers with a human table, a partial entry or a roster this build has
// not seen.

import assert from "node:assert/strict";
import { describe, it } from "node:test";

import {
  filterProviders,
  parseLoginOutcome,
  parseProviders,
  providerLoginArgs,
  providersListArgs,
  providerState,
} from "../core/providers";

/// What `oxide providers --json` prints, reduced to what the tests use.
const LISTING = JSON.stringify({
  active: "openai",
  providers: [
    {
      name: "openai",
      label: "OpenAI",
      description: "GPT models",
      keyUrl: "https://platform.openai.com/api-keys",
      local: false,
      stored: true,
      active: true,
    },
    {
      name: "bedrock",
      label: "Amazon Bedrock",
      description: "Claude and Nova models on AWS",
      keyUrl: "https://docs.aws.amazon.com/bedrock/",
      local: false,
      stored: false,
      active: false,
    },
    {
      name: "ollama",
      label: "Ollama",
      description: "Local models served by Ollama",
      keyUrl: "",
      local: true,
      stored: false,
      active: false,
    },
  ],
});

describe("provider listing", () => {
  it("parses the JSON listing in the table's own order", () => {
    const providers = parseProviders(LISTING);
    assert.deepEqual(
      providers.map((provider) => provider.name),
      ["openai", "bedrock", "ollama"],
    );
    assert.equal(providers[0].stored, true);
    assert.equal(providers[0].active, true);
    assert.equal(providers[2].local, true);
    assert.equal(providers[2].keyUrl, "");
  });

  it("ignores output that is not the JSON listing", () => {
    // A CLI older than this panel prints its human table, which is not a roster
    // to paint: the dialog says it has nothing rather than showing a row per line
    // of prose.
    assert.deepEqual(parseProviders(""), []);
    assert.deepEqual(parseProviders("openai\tOpenAI — GPT models  [in use]"), []);
    assert.deepEqual(parseProviders('{"providers":"openai"}'), []);
  });

  it("keeps an entry with no label or description, named by its own name", () => {
    const providers = parseProviders('{"providers":[{"name":"zai"}]}');
    assert.deepEqual(providers, [
      {
        name: "zai",
        label: "zai",
        description: "",
        keyUrl: "",
        local: false,
        stored: false,
        active: false,
      },
    ]);
    // An entry with no name at all is dropped rather than painted blank.
    assert.deepEqual(parseProviders('{"providers":[{"label":"Nameless"},3]}'), []);
  });

  it("names the state a row carries beside it", () => {
    const providers = parseProviders(LISTING);
    assert.equal(providerState(providers[0]), "In use");
    assert.equal(providerState(providers[1]), "");
    assert.equal(providerState(providers[2]), "No key needed");
    assert.equal(providerState({ ...providers[2], stored: true }), "Stored");
  });
});

describe("provider search", () => {
  const providers = parseProviders(LISTING);

  it("matches a name, a label or a description", () => {
    assert.deepEqual(
      filterProviders(providers, "bed").map((provider) => provider.name),
      ["bedrock"],
    );
    assert.deepEqual(
      filterProviders(providers, "amazon").map((provider) => provider.name),
      ["bedrock"],
    );
    // A description the reader half-remembers is enough to find the row.
    assert.deepEqual(
      filterProviders(providers, "on aws").map((provider) => provider.name),
      ["bedrock"],
    );
    // Case does not matter, and the whole table comes back for an empty query.
    assert.deepEqual(
      filterProviders(providers, "OLLAMA").map((provider) => provider.name),
      ["ollama"],
    );
    assert.equal(filterProviders(providers, "  ").length, 3);
  });

  it("leaves nothing selected-and-invisible: no match is an empty list", () => {
    assert.deepEqual(filterProviders(providers, "watson"), []);
  });
});

describe("provider login", () => {
  it("asks for the table, and for one login", () => {
    assert.deepEqual(providersListArgs(), ["providers", "--json"]);
    assert.deepEqual(providerLoginArgs("openai", false), ["login", "openai", "--json"]);
    // A key is never an argument — it would be visible in the process listing —
    // so it goes on stdin and the flag is what says so.
    assert.deepEqual(providerLoginArgs("openai", true), [
      "login",
      "openai",
      "--json",
      "--key-stdin",
    ]);
  });

  it("reads what a login answered, and nothing else", () => {
    const outcome = parseLoginOutcome(
      '{"provider":"openai","label":"OpenAI","model":"gpt-5.1","local":false}',
    );
    assert.deepEqual(outcome, {
      provider: "openai",
      label: "OpenAI",
      model: "gpt-5.1",
      local: false,
    });
    // A partial answer still names the provider, so the notice says what was
    // connected; anything that is not one falls back to what the caller knew.
    assert.equal(parseLoginOutcome('{"provider":"zai"}')?.label, "zai");
    assert.equal(parseLoginOutcome(""), null);
    assert.equal(parseLoginOutcome("Connected OpenAI — model gpt-5.1"), null);
    assert.equal(parseLoginOutcome('{"label":"OpenAI"}'), null);
  });
});
