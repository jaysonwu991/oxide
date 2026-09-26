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
  shortSession,
  usageLine,
  type FooterInput,
} from "../core/footer";
import { emptyUsage, type UsageTotals } from "../core/protocol";

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
    sessionId: null,
    branch: "",
    autoCompact: true,
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
  it("labels the model, thinking level, agent, access and session", () => {
    const state = footerState(input({ sessionId: "abcdef1234567890" }));
    assert.equal(chip(state, "model"), "model: glm-5 · 128.0k");
    assert.equal(chip(state, "reasoning"), "thinking: auto");
    assert.equal(chip(state, "agent"), "agent: default");
    assert.equal(chip(state, "access"), "access: untrusted");
    assert.equal(chip(state, "session"), "session: abcdef12");
  });

  it("falls back to the config's model and a new session", () => {
    const state = footerState(input({ model: "", contextWindow: 0 }));
    assert.equal(chip(state, "model"), "model: config.json");
    assert.equal(chip(state, "session"), "session: new");
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
    assert.equal(line, "↑1.2k · ↓340 · R12.0k · W900 · CH12.5% · $0.01 · ctx 75%/128.0k (auto)");
  });

  it("marks the context line as manual when auto-compaction is off", () => {
    const line = usageLine(input({ autoCompact: false, usage: usage({ contextTokens: 1_000 }) }));
    assert.equal(line, "ctx 1%/128.0k");
  });

  it("asks for the window before anything has run", () => {
    assert.equal(usageLine(input({})), "ctx ?/128.0k (auto)");
    assert.equal(usageLine(input({ contextWindow: 0 })), "");
  });

  it("hides the cache hit rate until the provider reports one", () => {
    const line = usageLine(input({ usage: usage({ input: 10, cacheRead: 0 }) }));
    assert.equal(line, "↑10 · ctx ?/128.0k (auto)");
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

  it("shortens a session id to what tells two apart", () => {
    assert.equal(shortSession("abcdef1234567890"), "abcdef12");
    assert.equal(shortSession("abc"), "abc");
  });

  it("cycles reasoning the way the terminal's Shift+Tab does", () => {
    assert.equal(nextReasoning("auto"), "off");
    assert.equal(nextReasoning("high"), "auto");
    assert.equal(nextReasoning("junk"), "auto");
  });
});
