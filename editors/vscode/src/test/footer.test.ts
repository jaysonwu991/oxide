// The footer's labels are composed in the extension host, so the layout of the
// chips and the usage line is pinned down here instead of in the webview.

import assert from "node:assert/strict";
import { describe, it } from "node:test";

import {
  contextLevel,
  contextPercent,
  footerState,
  formatCost,
  nextReasoning,
  reasoningChoices,
  usageLine,
  type FooterInput,
} from "../core/footer";
import { emptyUsage, formatTokens, type UsageTotals } from "../core/protocol";

function usage(overrides: Partial<UsageTotals> = {}): UsageTotals {
  return { ...emptyUsage(), ...overrides };
}

function input(overrides: Partial<FooterInput> = {}): FooterInput {
  return {
    model: "glm-5",
    provider: "zai",
    contextWindow: 128_000,
    reasoning: "auto",
    agent: "",
    agentCount: 0,
    access: "untrusted",
    trustSetting: "default",
    defaultTrust: "ask",
    savedTrust: undefined,
    branch: "",
    autoCompact: true,
    subscription: false,
    usage: usage(),
    ...overrides,
  };
}

function chip(state: ReturnType<typeof footerState>, id: string): string {
  const found = state.chips.find((entry) => entry.id === id);
  assert.ok(found, `expected a ${id} chip`);
  return found.label;
}

describe("footerState", () => {
  it("labels the model, thinking level, agent and access", () => {
    const state = footerState(input());
    assert.equal(chip(state, "model"), "model: glm-5 · 128k");
    assert.equal(chip(state, "reasoning"), "thinking: auto");
    assert.equal(chip(state, "agent"), "agent: default");
    assert.equal(chip(state, "access"), "access: untrusted");
    assert.deepEqual(state.chips.map((entry) => entry.id), ["model", "reasoning", "agent", "access"]);
    assert.match(
      state.chips.find((entry) => entry.id === "reasoning")?.title ?? "",
      /Click to choose a level\./,
    );
  });

  it("falls back to the config's model", () => {
    const state = footerState(input({ model: "", contextWindow: 0 }));
    assert.equal(chip(state, "model"), "model: config.json");
  });

  it("carries the branch and the context gauge", () => {
    const state = footerState(
      input({ branch: "main", usage: usage({ contextTokens: 96_000, input: 96_000 }) }),
    );
    assert.equal(state.info, "main");
    assert.equal(state.percent, 75);
    assert.equal(state.level, "warn");
  });

  it("explains where the access came from", () => {
    const asked = footerState(input({})).chips.find((entry) => entry.id === "access");
    assert.match(asked?.title ?? "", /defaultProjectTrust: ask/);
    const saved = footerState(input({ access: "trusted", savedTrust: true })).chips.find(
      (entry) => entry.id === "access",
    );
    assert.match(saved?.title ?? "", /Saved decision for this folder: trusted/);
    const overridden = footerState(input({ trustSetting: "always", access: "trusted" })).chips.find(
      (entry) => entry.id === "access",
    );
    assert.match(overridden?.title ?? "", /oxide\.projectTrust is always/);
  });
});

describe("usageLine", () => {
  it("reports tokens, cache, spend and context the way the terminal does", () => {
    const line = usageLine(
      input({
        usage: usage({
          input: 1_200,
          output: 340,
          cacheRead: 12_000,
          cacheWrite: 900,
          cost: 0.012,
          contextTokens: 96_000,
          cacheHit: 12.5,
        }),
      }),
    );
    assert.equal(line, "↑1.2k · ↓340 · R12k · W900 · CH12.5% · $0.01 · ctx 75%/128k (auto)");
  });

  /// A plan rather than a metered key: the price table's number is what the plan
  /// would have billed rather than money owed, which the terminal's footer marks
  /// ` (sub)` — the fact comes from the CLI with the window, so the panel says it
  /// for the credential the run actually resolves.
  it("marks the spend of a plan the way the terminal footer does", () => {
    const line = usageLine(
      input({
        contextWindow: 0,
        subscription: true,
        usage: usage({ input: 10, cost: 0.012 }),
      }),
    );
    assert.equal(line, "↑10 · $0.01 (sub)");
    // A metered key is not marked: the number is money owed.
    assert.equal(
      usageLine(input({ contextWindow: 0, usage: usage({ input: 10, cost: 0.012 }) })),
      "↑10 · $0.01",
    );
    // Nothing to mark when there is nothing to spend.
    assert.equal(usageLine(input({ contextWindow: 0, subscription: true })), "");
  });

  it("marks the context line as manual when auto-compaction is off", () => {
    const line = usageLine(input({ autoCompact: false, usage: usage({ contextTokens: 1_000 }) }));
    assert.equal(line, "ctx 1%/128k");
  });

  it("asks for the window before anything has run", () => {
    assert.equal(usageLine(input({})), "ctx ?/128k (auto)");
    assert.equal(usageLine(input({ contextWindow: 0 })), "");
  });

  it("shortens the numbers the way the terminal's own footer does", () => {
    // The same steps the CLI's `format_tokens` walks: 999 stays itself, then
    // `1.2k`, `123k`, `1.2M`, `66M` — so a window and a token count read the
    // same in the panel as in the terminal.
    assert.equal(
      usageLine(
        input({
          contextWindow: 0,
          usage: usage({ input: 999, output: 164_817, cacheRead: 66_249_728, cacheWrite: 1_234 }),
        }),
      ),
      "↑999 · ↓165k · R66M · W1.2k",
    );
    assert.equal(formatTokens(1_200), "1.2k");
    assert.equal(formatTokens(128_000), "128k");
    assert.equal(formatTokens(1_000_000), "1.0M");
    assert.equal(formatTokens(1_048_576), "1.0M");
    assert.equal(formatTokens(66_249_728), "66M");
  });

  it("hides the cache hit rate until the provider reports one", () => {
    const line = usageLine(input({ usage: usage({ input: 10, cacheRead: 0 }) }));
    assert.equal(line, "↑10 · ctx ?/128k (auto)");
  });
});

describe("context gauges", () => {
  it("rounds the percentage and escalates above the CLI's thresholds", () => {
    assert.equal(contextPercent(1, 3), 33);
    assert.equal(contextPercent(0, 128_000), null);
    assert.equal(contextPercent(1, 0), null);
    assert.equal(contextLevel(null), "ok");
    assert.equal(contextLevel(70), "ok");
    assert.equal(contextLevel(71), "warn");
    assert.equal(contextLevel(91), "high");
  });
});

describe("footer formatting", () => {
  it("shows a spend only when it would not round to nothing", () => {
    assert.equal(formatCost(0), "");
    assert.equal(formatCost(0.0004), "$0.0004");
    assert.equal(formatCost(0.5), "$0.50");
  });

  it("cycles reasoning the way the terminal's Shift+Tab does", () => {
    assert.equal(nextReasoning("auto"), "off");
    assert.equal(nextReasoning("high"), "xhigh");
    assert.equal(nextReasoning("max"), "auto");
    assert.equal(nextReasoning("junk"), "auto");
    // A narrowed cycle walks only the levels the model advertised.
    const deepseek = ["auto", "off", "low", "high", "max"];
    assert.equal(nextReasoning("high", deepseek), "max");
    assert.equal(nextReasoning("max", deepseek), "auto");
    assert.equal(nextReasoning("medium", deepseek), "auto");
  });

  it("narrows the offered levels to the model's own when it advertised them", () => {
    assert.deepEqual(reasoningChoices({}), [
      "auto",
      "off",
      "minimal",
      "low",
      "medium",
      "high",
      "xhigh",
      "max",
    ]);
    assert.deepEqual(reasoningChoices({ reasoningLevels: ["off", "low", "high", "max"] }), [
      "auto",
      "off",
      "low",
      "high",
      "max",
    ]);
    const title = footerState(
      input({ reasoningLevels: ["off", "low", "high", "max"] }),
    ).chips.find((entry) => entry.id === "reasoning")?.title;
    assert.match(title ?? "", /auto → off → low → high → max/);
  });
});
