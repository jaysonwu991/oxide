// The stored conversation of one session, read from `oxide sessions show
// <id> --json`.
//
// Resuming a thread in the panel sets the session the next turn continues, but
// the transcript the user is looking at is still the one they had — so the
// resumed thread looked like it had done nothing. The CLI's `show` prints what
// the thread holds, and this module turns that into the turns the transcript
// replays plus the totals the footer shows, so reopening a thread reads like
// reopening it in the desktop app.
//
// A stored thread is mostly tool steps: a step that only called tools carries no
// text, and its results are separate messages. Replaying only what was said left
// a long thread looking like it held a bubble or two, so a call is replayed too,
// paired with the result that answered it — which is why the tail the CLI is
// asked for is counted in stored messages rather than in turns.
//
// Like the other readers here it is tolerant: a field that is missing or of the
// wrong type reads as empty rather than throwing mid-turn.

import { emptyUsage, type ReplayEntry, type UsageTotals } from "./protocol";

export interface SessionHistory {
  id: string;
  /// The thread's own name as the CLI stores it, when it has one.
  name: string;
  /// The thread's turns, oldest first: what was said and what was called.
  entries: ReplayEntry[];
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

/// The one entry kind that holds a result of its own, which its `tool` message
/// fills in once the walk reaches it.
type ReplayedCall = Extract<ReplayEntry, { kind: "tool" }>;

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

  const entries: ReplayEntry[] = [];
  // The card a call's result belongs to, keyed by the call's own id: a result
  // travels as a message of its own (`toolCallId`) after the step that asked
  // for it, so the result is folded into the card rather than painted beside it.
  const calls = new Map<string, ReplayedCall>();
  for (const entry of Array.isArray(root.messages) ? root.messages : []) {
    const message = obj(entry);
    const role = str(message.role);
    const text = str(message.content);
    if (role === "tool") {
      // A result whose call the tail left out names no tool and carries no
      // arguments, so there is no card to put it under.
      const call = calls.get(str(message.toolCallId));
      if (call) {
        call.isError = text.startsWith("error:");
        call.output = call.isError ? text.slice("error:".length).trimStart() : text;
      }
      continue;
    }
    if (role === "assistant") {
      // A step that only called tools carries no text, and an empty bubble says
      // nothing about the thread.
      if (text.trim()) entries.push({ kind: "assistant", text });
      for (const raw of Array.isArray(message.toolCalls) ? message.toolCalls : []) {
        const call = obj(raw);
        const name = str(call.name);
        if (!name) continue;
        const card: ReplayedCall = {
          kind: "tool",
          name,
          args: str(call.arguments),
          output: "",
          isError: false,
        };
        entries.push(card);
        calls.set(str(call.id), card);
      }
      continue;
    }
    if (role === "user" && text.trim()) entries.push({ kind: "user", text });
  }

  const usage = obj(root.usage);
  const cacheHit = usage.cacheHitRate;
  return {
    id: str(root.id),
    name: str(root.name),
    entries,
    total: num(root.messageCount) || entries.length,
    // Read from the answer rather than counted here: the messages the panel
    // replays are the CLI's, with a call and the result that answered it folded
    // into one card.
    shown: num(root.shown) || entries.length,
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
/// the CLI's own answer, so the panel can say what it left out. It is counted
/// in stored messages, which is what a thread is mostly made of: a turn's tool
/// steps and their results are messages too, and asking for turns instead would
/// leave the end of a long thread showing its last exchange only.
export const HISTORY_MESSAGES = 60;
