// The footer: the chips the composer shows under the input, plus the usage line
// and the context gauge. The labels are computed here rather than in the
// webview, so the footer reads the same in both panes and is unit tested.

import type { TrustSetting } from "./args";
import { formatTokens, type UsageTotals } from "./protocol";
import type { Access } from "./trust";

export type ContextLevel = "ok" | "warn" | "high";

export interface FooterChip {
  id: "model" | "reasoning" | "agent" | "access" | "session";
  label: string;
  title: string;
}

export interface FooterState {
  chips: FooterChip[];
  /// The branch, shown dim on the right of the chip row. Empty outside a repo.
  info: string;
  /// The cumulative usage line. Empty until the provider reports usage.
  usage: string;
  /// Context used by the last request, for the gauge.
  percent: number | null;
  level: ContextLevel;
}

export interface FooterInput {
  /// The model a turn would run with: the `oxide.model` setting, else the one
  /// in `config.json`.
  model: string;
  provider: string;
  contextWindow: number;
  reasoning: string;
  agent: string;
  /// How many agents were discovered, for the agent chip's tooltip.
  agentCount: number;
  access: Access;
  trustSetting: TrustSetting;
  /// `defaultProjectTrust`, which decides the access when no decision is saved.
  defaultTrust: "ask" | "always" | "never";
  savedTrust: boolean | undefined;
  sessionId: string | null;
  branch: string;
  autoCompact: boolean;
  usage: UsageTotals;
}

/// The cycle used by the TUI and the desktop app.
export const REASONING_LEVELS = ["auto", "off", "low", "medium", "high"];

export function nextReasoning(current: string): string {
  const index = REASONING_LEVELS.indexOf(current);
  return REASONING_LEVELS[(index + 1) % REASONING_LEVELS.length];
}

export function footerState(input: FooterInput): FooterState {
  const percent = contextPercent(input.usage.contextTokens, input.contextWindow);
  return {
    chips: [
      {
        id: "model",
        label: `model: ${input.model || "config.json"}${input.contextWindow ? ` · ${formatTokens(input.contextWindow)}` : ""}`,
        title: `Model a turn runs with (--model)${input.provider ? ` on ${input.provider}` : ""}. Click to change it.`,
      },
      {
        id: "reasoning",
        label: `thinking: ${input.reasoning}`,
        title: `Reasoning effort (--reasoning): ${REASONING_LEVELS.join(" → ")}. Click to cycle.`,
      },
      {
        id: "agent",
        label: `agent: ${input.agent || "default"}`,
        title:
          input.agentCount > 0
            ? `Subagent this chat runs with (--agent). ${input.agentCount} discovered. Click to pick one.`
            : "Subagent this chat runs with (--agent). No agents were found in this project or the global config. Click to type one.",
      },
      {
        id: "access",
        label: `access: ${input.access}`,
        title: accessTitle(input),
      },
      {
        id: "session",
        label: `session: ${input.sessionId ? shortSession(input.sessionId) : "new"}`,
        title: input.sessionId
          ? `Session ${input.sessionId}. Click to resume a stored session instead.`
          : "The next message starts a session. Click to resume a stored one.",
      },
    ],
    info: input.branch,
    usage: usageLine(input),
    percent,
    level: contextLevel(percent),
  };
}

/// The usage line, laid out like the terminal footer's: cumulative tokens, the
/// cache it read and wrote, the hit rate the provider reported, the spend, then
/// where the context stands.
export function usageLine(input: FooterInput): string {
  const usage = input.usage;
  const parts: string[] = [];
  if (usage.input) parts.push(`↑${formatTokens(usage.input)}`);
  if (usage.output) parts.push(`↓${formatTokens(usage.output)}`);
  if (usage.cacheRead) parts.push(`R${formatTokens(usage.cacheRead)}`);
  if (usage.cacheWrite) parts.push(`W${formatTokens(usage.cacheWrite)}`);
  if (usage.cacheHit !== null) parts.push(`CH${usage.cacheHit.toFixed(1)}%`);
  const cost = formatCost(usage.cost);
  if (cost) parts.push(cost);
  if (input.contextWindow > 0) {
    const auto = input.autoCompact ? " (auto)" : "";
    const limit = formatTokens(input.contextWindow);
    const used = contextPercent(usage.contextTokens, input.contextWindow);
    parts.push(used === null ? `ctx ?/${limit}${auto}` : `ctx ${used}%/${limit}${auto}`);
  }
  return parts.join(" · ");
}

/// Percent of the window the last request used, or `null` before anything ran.
export function contextPercent(used: number, limit: number): number | null {
  if (!used || !limit) return null;
  return Math.round((used / limit) * 100);
}

/// The terminal's thresholds, plus the error color at the very top: the gauge
/// escalates before the CLI would compact.
export function contextLevel(percent: number | null): ContextLevel {
  if (percent === null) return "ok";
  if (percent > 90) return "high";
  return percent > 70 ? "warn" : "ok";
}

/// A cost only worth showing when it rounds to something: a fraction of a cent
/// gets four decimals, anything above a cent two.
export function formatCost(cost: number): string {
  if (!cost) return "";
  return cost < 0.01 ? `$${cost.toFixed(4)}` : `$${cost.toFixed(2)}`;
}

/// Sessions are addressed by a full id; the footer only has room for enough of
/// it to tell two apart (and to paste back into `--session`).
export function shortSession(id: string): string {
  return id.length > 8 ? id.slice(0, 8) : id;
}

function accessTitle(input: FooterInput): string {
  const what =
    input.access === "trusted"
      ? "Project resources (.oxide agents, commands, skills and plugins) load for this run."
      : "Project resources are skipped for this run.";
  const override =
    input.trustSetting === "always"
      ? " oxide.projectTrust is always (--approve)."
      : input.trustSetting === "never"
        ? " oxide.projectTrust is never (--no-approve)."
        : "";
  const saved =
    input.trustSetting === "default"
      ? input.savedTrust === undefined
        ? ` No saved decision for this folder (defaultProjectTrust: ${input.defaultTrust}).`
        : ` Saved decision for this folder: ${input.savedTrust ? "trusted" : "declined"}.`
      : "";
  return `${what}${override}${saved} Click to change it.`;
}
