// The stored conversation of one session, read from `oxide sessions show
// <id> --json`.
//
// Resuming a thread in the panel sets the session the next turn continues, but
// the transcript the user is looking at is still the one they had — so the
// resumed thread looked like it had done nothing. The CLI's `show` prints what
// the thread holds, and this module turns that into the items the transcript
// paints plus the totals the footer shows, so reopening a thread reads like
// reopening it in the desktop app.
//
// Like the other readers here it is tolerant: a field that is missing or of the
// wrong type reads as empty rather than throwing mid-turn.

import { emptyUsage, type UsageTotals } from "./protocol";

/// The roles the panel paints. A stored thread also holds tool results and the
/// occasional system message; replaying those would bury the conversation
/// someone reopened the thread to read, so only what was said is shown.
export interface HistoryMessage {
  role: "user" | "assistant";
  text: string;
}

export interface SessionHistory {
  id: string;
  /// The thread's own name as the CLI stores it, when it has one.
  name: string;
  messages: HistoryMessage[];
  /// How many messages are left in the thread's context, and how many of them
  /// the CLI returned in `messages` — the two differ when only the tail was
  /// asked for, which is what lets the panel say what it is showing.
  total: number;
  shown: number;
  usage: UsageTotals;
}

const obj = (value: unknown): Record<string, unknown> =>
  value && typeof value === "object" && !Array.isArray(value)
    ? (value as Record<string, unknown>)
    : {};

const str = (value: unknown): string => (typeof value === "string" ? value : "");

const num = (value: unknown): number =>
  typeof value === "number" && Number.isFinite(value) ? value : 0;

/// Parses one session's stored conversation, or `null` when the output is not
/// the object `show` prints (a CLI error, a future format, an empty file).
export function parseSessionHistory(json: string): SessionHistory | null {
  let value: unknown;
  try {
    value = JSON.parse(json);
  } catch {
    return null;
  }
  const root = obj(value);
  if (!root.id && !Array.isArray(root.messages)) return null;

  const messages: HistoryMessage[] = [];
  for (const entry of Array.isArray(root.messages) ? root.messages : []) {
    const message = obj(entry);
    const role = str(message.role);
    if (role !== "user" && role !== "assistant") continue;
    const text = str(message.content);
    // An assistant step that only called tools carries no text, and an empty
    // bubble says nothing about the thread.
    if (!text.trim()) continue;
    messages.push({ role, text });
  }

  const usage = obj(root.usage);
  const cacheHit = usage.cacheHitRate;
  return {
    id: str(root.id),
    name: str(root.name),
    messages,
    total: num(root.messageCount) || messages.length,
    // Read from the answer rather than counted here: the messages the panel
    // paints are only the ones that were said, while the CLI's `shown` counts
    // the tool steps and tool results it sent along with them.
    shown: num(root.shown) || messages.length,
    usage: {
      ...emptyUsage(),
      input: num(usage.input),
      output: num(usage.output),
      cacheRead: num(usage.cacheRead),
      cacheWrite: num(usage.cacheWrite),
      cost: num(usage.cost),
      contextTokens: num(usage.contextTokens),
      cacheHit: typeof cacheHit === "number" && Number.isFinite(cacheHit) ? cacheHit : null,
    },
  };
}

/// How many of a thread's messages the panel replays. A long thread would
/// otherwise paint thousands of items into the side bar, and the part worth
/// reading on reopening a thread is its tail; the width of the tail travels in
/// the CLI's own answer, so the panel can say what it left out.
export const HISTORY_MESSAGES = 60;
