// The footer: the chips the composer shows under the input, the checkout the
// chat runs in and the branch beside it on the bar above the box, plus the usage
// line and the context gauge. The labels are computed here rather than in the
// webview, so the footer reads the same in both panes and is unit tested.

import { translate, type LanguageInfo } from "./i18n";

import type { TrustSetting } from "./args";
import { formatTokens, type UsageTotals } from "./protocol";
import type { Access } from "./trust";

export type ContextLevel = "ok" | "warn" | "high";

export interface FooterChip {
  id: "model" | "reasoning" | "agent" | "access";
  label: string;
  title: string;
  value?: string;
  active?: boolean;
}

/// The checkout the chat runs in: a worktree is a copy of the project on its own
/// branch, and the bar above the box names it at the row's other end, opposite
/// that branch. An empty `name` is the folder the reader has open, which the
/// branch already names.
export interface FooterWorktree {
  name: string;
  title: string;
}

export interface FooterState {
  chips: FooterChip[];
  /// The branch, shown dim on the right of the chip row. Empty outside a repo.
  info: string;
  /// The checkout the chat runs in, read at the bar's other end: the one control
  /// there, since a worktree is picked rather than read.
  worktree: FooterWorktree;
  /// The cumulative usage line. Empty until the provider reports usage.
  usage: string;
  usageTitle?: string;
  /// Context used by the last request, for the gauge.
  percent: number | null;
  level: ContextLevel;
}

export interface FooterInput {
  language?: LanguageInfo | null;
  /// The model a turn would run with: the `oxide.model` setting, else the one
  /// in `config.json`.
  model: string;
  provider: string;
  contextWindow: number;
  reasoning: string;
  /// The active model's own advertised levels, when `oxide reasoning` answered
  /// them. The chip title names these rather than the built-in set; the dialog
  /// and the cycle use them too.
  reasoningLevels?: readonly string[];
  supportsReasoning?: boolean;
  agent: string;
  /// How many agents were discovered, for the agent chip's tooltip.
  agentCount: number;
  /// The worktree this chat runs in, when it is one: another checkout of the
  /// project on its own branch. `null` is the folder the reader has open.
  worktree?: string | null;
  access: Access;
  trustSetting: TrustSetting;
  /// `defaultProjectTrust`, which decides the access when no decision is saved.
  defaultTrust: "ask" | "always" | "never";
  savedTrust: boolean | undefined;
  branch: string;
  autoCompact: boolean;
  /// Whether the credential is a plan rather than a metered key: the price
  /// table's number is what the plan would have billed, which the terminal
  /// marks ` (sub)` and so does this.
  subscription: boolean;
  usage: UsageTotals;
}

/// The cycle used by the TUI and the desktop app. A model whose listing
/// advertised its own levels narrows this set; the CLI clamps a level the model
/// does not accept onto the nearest one it does.
export const REASONING_LEVELS = [
  "auto",
  "off",
  "minimal",
  "low",
  "medium",
  "high",
  "xhigh",
  "max",
];

export function nextReasoning(current: string, levels: readonly string[] = REASONING_LEVELS): string {
  const index = levels.indexOf(current);
  return levels[(index + 1) % levels.length];
}

/// The levels a chip's tooltip, the picker and the cycle offer: `auto` plus the
/// model's own advertised levels. Missing metadata offers no selectable effort.
export function reasoningChoices(input: Pick<FooterInput, "reasoningLevels" | "supportsReasoning">): readonly string[] {
  if (input.supportsReasoning === false) return ["auto"];
  const advertised = input.reasoningLevels;
  if (!advertised || advertised.length === 0) return ["auto"];
  return ["auto", ...advertised.filter((level) => level !== "auto")];
}

export function footerState(input: FooterInput): FooterState {
  const t = (key: string, fallback: string, vars?: Record<string, unknown>) => translate(input.language ?? null, key, fallback, vars);
  const percent = contextPercent(input.usage.contextTokens, input.contextWindow);
  return {
    chips: [
      {
        id: "model",
        value: input.model || "config.json",
        label: t("ui.model", "model: {value0}{value1}", { value0: input.model || "config.json", value1: input.contextWindow ? ` · ${formatTokens(input.contextWindow)}` : "" }),
        title: t("ui.modelATurnRunsWithModelClickToChangeIt", "Model a turn runs with (--model){value0}. Click to change it.", { value0: input.provider ? ` on ${input.provider}` : "" }),
      },
      ...(reasoningChoices(input).length <= 1 ? [] : [{
        id: "reasoning" as const,
        value: translate(input.language ?? null, `reasoning.${input.reasoning}`, input.reasoning.charAt(0).toUpperCase() + input.reasoning.slice(1)),
        label: t("ui.thinking", "thinking: {value0}", { value0: input.reasoning }),
        title: t("ui.reasoningEffortReasoningClickToChooseALevel", "Reasoning effort (--reasoning): {value0}. Click to choose a level.", { value0: reasoningChoices(input).join(" → ") }),
      }]),
      {
        id: "access",
        active: input.access === "trusted",
        label: t("ui.access", "access: {value0}", { value0: input.access }),
        title: accessTitle(input),
      },
      {
        id: "agent",
        label: t("agent.label", "agent: {value0}", { value0: input.agent || "default" }),
        title:
          input.agentCount > 0
            ? t("ui.subagentThisChatRunsWithAgentDiscoveredClickToPickOne", "Subagent this chat runs with (--agent). {value0} discovered. Click to pick one.", { value0: input.agentCount })
            : t("ui.subagentThisChatRunsWithAgentNoAgentsWereFoundInThisProjectOrTheGlobalConfigClic", "Subagent this chat runs with (--agent). No agents were found in this project or the global config. Click to type one."),
      },
    ],
    // The checkout belongs to the folder rather than to the message, so it is
    // read on the bar above the box beside the branch it is on: a chat in a
    // worktree writes to a copy of the project, and the bar is where the panel
    // says which copy.
    worktree: {
      name: input.worktree || "",
      title: input.worktree
        ? `This chat runs in the worktree ${input.worktree}, not in the folder you have open. Click to switch checkouts.`
        : "This chat runs in the folder you have open. Click to run it in a worktree of the project instead.",
    },
    info: input.branch,
    usage: usageLine(input),
    ...(input.usage.attribution ? { usageTitle: usageDetails(input) } : {}),
    percent,
    level: contextLevel(percent),
  };
}

/// The usage line, laid out like the terminal footer's: cumulative tokens, the
/// cache it read and wrote, the hit rate the provider reported, the spend, then
/// where the context stands.
export function usageLine(input: FooterInput): string {
  const t = (key: string, fallback: string, vars?: Record<string, unknown>) => translate(input.language ?? null, key, fallback, vars);
  const usage = input.usage;
  const parts: string[] = [];
  if (usage.input) parts.push(`↑${formatTokens(usage.input)}`);
  if (usage.output) parts.push(`↓${formatTokens(usage.output)}`);
  if (usage.cacheRead) parts.push(`R${formatTokens(usage.cacheRead)}`);
  if (usage.cacheWrite) parts.push(`W${formatTokens(usage.cacheWrite)}`);
  if (usage.cacheHit !== null) parts.push(`CH${usage.cacheHit.toFixed(1)}%`);
  const cost = formatCost(usage.cost);
  // A plan's number is what it would have billed rather than money owed, so it
  // is marked the way the terminal's footer marks it.
  if (cost) parts.push(input.subscription ? `${cost} (sub)` : cost);
  if (input.contextWindow > 0) {
    const auto = input.autoCompact ? t("usage.auto", " (auto)") : "";
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

function accessTitle(input: FooterInput): string {
  const t = (key: string, fallback: string, vars?: Record<string, unknown>) => translate(input.language ?? null, key, fallback, vars);
  const what =
    input.access === "trusted"
      ? t("ui.projectResourcesOxideAgentsCommandsSkillsAndPluginsLoadForThisRun", "Project resources (.oxide agents, commands, skills and plugins) load for this run.")
      : t("ui.projectResourcesAreSkippedForThisRun", "Project resources are skipped for this run.");
  const override =
    input.trustSetting === "always"
      ? t("ui.oxideProjectTrustIsAlwaysApprove", " oxide.projectTrust is always (--approve).")
      : input.trustSetting === "never"
        ? t("ui.oxideProjectTrustIsNeverNoApprove", " oxide.projectTrust is never (--no-approve).")
        : "";
  const saved =
    input.trustSetting === "default"
      ? input.savedTrust === undefined
        ? t("ui.noSavedDecisionForThisFolderDefaultProjectTrust", " No saved decision for this folder (defaultProjectTrust: {value0}).", { value0: input.defaultTrust })
        : t("ui.savedDecisionForThisFolder", " Saved decision for this folder: {value0}.", { value0: input.savedTrust ? "trusted" : "declined" })
      : "";
  return t("ui.clickToChangeIt", "{value0}{value1}{value2} Click to change it.", { value0: what, value1: override, value2: saved });
}

function usageDetails(input: FooterInput): string {
  const usage = input.usage;
  const prompt = usage.input + usage.cacheRead + usage.cacheWrite;
  const lines = [usageLine(input), "CH: latest request with cache traffic"];
  if (prompt) lines.push(`Session cache hit: ${(100 * usage.cacheRead / prompt).toFixed(1)}%`);
  for (const [source, bucket] of Object.entries(usage.attribution || {})) {
    lines.push(`${source}: ${bucket.modelCalls} model calls · ↑${formatTokens(bucket.input)} ↓${formatTokens(bucket.output)} R${formatTokens(bucket.cacheRead)} W${formatTokens(bucket.cacheWrite)} · $${bucket.cost.toFixed(3)}`);
  }
  return lines.join("\n");
}
