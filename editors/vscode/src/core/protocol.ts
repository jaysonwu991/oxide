// Wire events from `oxide --mode rpc` and the transcript state machine the
// chat view renders. This module deliberately imports nothing from `vscode` so
// it can be unit tested under plain node (`node --test out/test/`).
//
// The extension owns the transcript; the webview is a dumb renderer that
// applies the `ViewMessage`s produced here. Every event-to-DOM decision is
// therefore testable without a webview.

import {
  approvalLabel,
  approvalRequest,
  approvalTitle,
  type ApprovalDecision,
} from "./approvals";
import {
  questionLabel,
  questionRequest,
  questionTitle,
  type Question,
  type QuestionAnswer,
} from "./questions";
import type { AtSuggestion } from "./at";
import { changesTitle, changesTotals, changeRows, changesMore, turnChanges, type ChangeRow, type ChangesMore } from "./changes";
import { argumentDiff } from "./preview";
import type { DialogState } from "./dialogs";
import type { FooterState } from "./footer";
import type { CommandRow } from "./palette";

/// One JSONL line from the agent's stdout. Only `type` is guaranteed.
export interface WireEvent {
  type?: string;
  [key: string]: unknown;
}

export interface UsageTotals {
  input: number;
  output: number;
  cacheRead: number;
  cacheWrite: number;
  cost: number;
  /// Prompt tokens of the most recent step (uncached prompt plus cache reads
  /// and writes), the number the context gauge tracks.
  contextTokens: number;
  /// The latest request's cache hit rate in percent, which is what the terminal
  /// footer's `CH` shows: a step that read or wrote no cache leaves the previous
  /// rate in place, so a cold turn does not erase a warm one.
  cacheHit: number | null;
}

export function emptyUsage(): UsageTotals {
  return {
    input: 0,
    output: 0,
    cacheRead: 0,
    cacheWrite: 0,
    cost: 0,
    contextTokens: 0,
    cacheHit: null,
  };
}

/// What a sent message carried, as the chips under its bubble name it: the file
/// or the attachment's name, and — where the message carried an image the host
/// could read a picture of — the thumbnail that goes with it, so the transcript
/// shows what was sent rather than only what it was called.
export interface SentChip {
  label: string;
  /// A data URL for the copy the composer's own chip was painted from, `null`
  /// for anything there is no picture of — a context file, a PDF, a text file.
  /// It rides the `state` messages with the item it belongs to, the way the
  /// composer's chips do, so a thread that carried images repaints with them.
  preview?: string | null;
}

export interface UserItem {
  id: number;
  kind: "user";
  text: string;
  /// Context the turn was sent with, shown as a chip under the bubble.
  context: SentChip[];
}

export interface AssistantItem {
  id: number;
  kind: "assistant";
  text: string;
}

export interface ThinkingItem {
  id: number;
  kind: "thinking";
  text: string;
}

export interface ToolItem {
  id: number;
  kind: "tool";
  name: string;
  /// The raw `arguments` JSON the model produced.
  args: string;
  output: string;
  /// When the call began, in Unix milliseconds, while it is running; `null`
  /// once it has settled, when the number to paint is `elapsed`.
  ///
  /// Carried in the transcript rather than timed by the view, so a card rebuilt
  /// from a `state` message — a second pane attaching, a thread switch — still
  /// counts from the call rather than from the card, and both panes report the
  /// same time. Absent on a stored thread's cards: they took no time here.
  startedAt?: number | null;
  /// What the call took, in milliseconds, once it has settled.
  elapsed?: number | null;
  /// A preview of the file change (`write`/`edit`/`patch`), already rendered in
  /// the same compact line format `oxide_core::diff` uses.
  diff: string | null;
  running: boolean;
  isError: boolean;
  /// A card replayed out of a stored thread: the thread does not record how the
  /// call ended, so its state is claimed by neither `isError` nor a success.
  unknown?: boolean;
}

export interface NoticeItem {
  id: number;
  kind: "notice";
  text: string;
  tone: "info" | "warn" | "error";
}

/// How a card stands: waiting for the user, answered with one of the three
/// decisions, or settled without an answer because the run ended or was
/// stopped before one arrived.
export type ApprovalState = "pending" | "once" | "always" | "deny" | "closed";

export interface ApprovalItem {
  id: number;
  kind: "approval";
  /// The broker's request id, echoed back in the answer frame.
  requestId: number;
  tool: string;
  /// What the tool would do (`approvalTitle`), shown beside its name.
  title: string;
  detail: string;
  state: ApprovalState;
  /// What an answered card reads; empty while it waits.
  label: string;
}

/// How a question card stands: waiting for the user, answered, or settled
/// without an answer because the run ended before one arrived.
export type QuestionState = "pending" | "answered" | "dismissed" | "closed";

/// A question the model asked through the `ask` tool and the agent is holding
/// until it is answered. The card paints one block per question — its options,
/// and a field to type a free-text answer in — and the answers travel to the
/// host, which forwards them over the CLI's request channel.
export interface QuestionItem {
  id: number;
  kind: "question";
  /// The broker's request id, echoed back in the answer frame.
  requestId: number;
  /// The card's heading (`questionTitle`).
  title: string;
  questions: Question[];
  state: QuestionState;
  /// What an answered card reads; empty while it waits.
  label: string;
}

export type Item =
  | UserItem
  | AssistantItem
  | ThinkingItem
  | ToolItem
  | NoticeItem
  | ApprovalItem
  | QuestionItem
  | ChangesItem;

/// The files a finished turn changed, as the CLI's own `turn_changes` frame
/// reports them. The card is a listing rather than a diff: a row opens VS Code's
/// own diff editor (see `chatView`), against the baseline the frame names, so
/// the panel never renders a second diff format of its own.
export interface ChangesItem {
  id: number;
  kind: "changes";
  /// The header, e.g. `Edited 3 files`.
  title: string;
  /// The turn's own `+N −N`, or empty when nothing counted.
  totals: string;
  /// The revision every file's diff is drawn against.
  baseline: string;
  /// The state the turn left behind, which the CLI's undo checks the work tree
  /// still holds before it puts anything back — so an older card is refused
  /// rather than taking a newer turn's work with it. Empty for a CLI too old to
  /// report one, which is taken at its word.
  after: string;
  /// The folder the run started in, which `rows`' paths are relative to and
  /// `baseline` is read out of. Carried with the card rather than looked up when
  /// a row is clicked: a multi-root window can move the active editor to
  /// another root while the card stays in the transcript, and the card belongs
  /// to the run, not to whatever folder is active now. Empty when the frame did
  /// not name one, which falls back to the active folder.
  project: string;
  /// The row that folds the rest of a long listing away, or `null` when every
  /// file fits (`changesMore`).
  more: ChangesMore | null;
  /// Whether this card's Undo is offered: only the newest turn's is, since an
  /// older one's restore would take the newer turn's work with it.
  undoable: boolean;
  /// Set once the turn has been put back, which the card reports instead of
  /// offering the same Undo again.
  undone: boolean;
  rows: ChangeRow[];
}

/// What a tool card's own state changes to: the result, which way it ended, and
/// the time it took, which is measured here as the call ends. `startedAt` is in
/// the patch so a card that a `state` message painted from a stale item has its
/// start taken off it rather than left counting.
export type ToolPatch = Partial<
  Pick<ToolItem, "output" | "diff" | "running" | "isError" | "name" | "args" | "startedAt" | "elapsed">
>;

/// One turn of a stored thread, as `oxide sessions show --json` reports it and
/// as the transcript replays it (`core/history.ts` reads that answer into
/// these). A reopened thread is shown as the thread it is: the calls it made
/// are as much of it as what was said, and a thread whose tail is mostly tool
/// steps would otherwise replay as a bubble or two.
export type ReplayEntry =
  | { kind: "user"; text: string; attachments?: SentChip[] }
  | { kind: "assistant"; text: string }
  | { kind: "tool"; name: string; args: string; output: string; isError: boolean };

/// A file or selection that is inlined into the prompt.
export interface ContextChip {
  id: number;
  label: string;
  /// The file the editor has open. The composer tracks it rather than waiting
  /// to be given it, so the view marks its chip and does not count it as
  /// something to send: an empty box with only that chip submits nothing.
  auto?: boolean;
  /// What the chip is holding where the label cannot say it — the lines a
  /// selection covers, and that they are what goes with the next message. The
  /// renderer paints it as the chip's tooltip rather than deciding any of it.
  detail?: string;
}

/// An image, a PDF or a text file the message carries as an attachment
/// (`--image`). A text file is one the CLI attaches as its own text, which is
/// what a pasted blob of source or data becomes.
export interface AttachmentChip extends ContextChip {
  kind: "image" | "pdf" | "text";
  /// A data URL for the chip's thumbnail, or `null` when there is no picture
  /// to draw — a PDF, a text file, or an image this webview cannot paint, all
  /// of which show a glyph instead.
  preview: string | null;
  /// The size and origin, for the chip's tooltip.
  detail: string;
}

/// A follow-up waiting above the composer while the current response finishes.
export interface QueuedMessage {
  index: number;
  text: string;
  attachmentCount: number;
}

/// Everything the webview needs to repaint from scratch.
export interface TranscriptState {
  items: Item[];
  status: string;
  busy: boolean;
  queued: number;
  queue: QueuedMessage[];
  followUpBehavior: "queue" | "steer";
  usage: UsageTotals;
  context: ContextChip[];
  attachments: AttachmentChip[];
  sessionId: string | null;
  /// The project root a turn would run in, or `null` with no folder open — a
  /// turn is refused then, so the home state says which of the two it is
  /// instead of inviting a message that cannot be sent.
  folder: string | null;
  /// The thread's summarized title: the session name or a one-line summary of
  /// the first thing the user sent, shown in the header. Empty for a thread
  /// that has not been written to yet.
  title: string;
  binary: string;
  showThinking: boolean;
  footer: FooterState;
  /// The thread a turn belongs to when it is not the one the panel is showing,
  /// so the view can say which conversation is running and offer the way back
  /// to it. `null` when the panel's own thread is the run's — or when nothing
  /// is running and nothing is parked.
  run: RunThread | null;
}

/// A turn's own thread, named for a pane that is showing another one. The
/// reader moved to a different thread while the turn ran (or a finished turn's
/// thread is still waiting to be opened), and the run keeps writing into its
/// own transcript, which the reader is not looking at.
///
/// The run belongs to the thread it started in: a delta, a tool card and a
/// change card are all applied to that transcript, so a reply is never painted
/// into the conversation the reader moved to. The strip is where it is named,
/// and `open` is the door back to it.
///
/// A message typed while it is up belongs to the running turn rather than to
/// the thread on screen — a Queue or a Steer with nowhere to land — so the
/// composer offers Stop alone until the reader is back in the run's thread,
/// which is what `run.running` says.
export interface RunThread {
  /// The run's thread id, or `null` while the CLI has not named it yet (the
  /// session header is what writes it, and it arrives after the first event).
  sessionId: string | null;
  /// What the strip names it by: the title the header showed when the reader
  /// left it, else the thread's own summarized title.
  title: string;
  /// Whether the turn is still running, so the strip says "running" rather
  /// than "finished" and the composer's corner keeps Stop.
  running: boolean;
}

/// One change card's state after it was undone, or after a newer card took the
/// Undo away from it. Not a `patch`: a card keeps its listing (what the turn
/// did is still what it did), so only the action under it changes.
export type ChangesState = { k: "changes"; id: number; undoable: boolean; undone: boolean };

export type ViewMessage =
  | ({ k: "state" } & TranscriptState)
  | { k: "push"; item: Item }
  | { k: "remove"; id: number }
  | { k: "append"; id: number; field: "text" | "output"; delta: string }
  | { k: "patch"; id: number; patch: ToolPatch }
  /// One approval card's state. It is not a `patch`: a tool card repaints from
  /// the item, while a card that is settled keeps its buttons removed.
  | { k: "approval"; id: number; state: ApprovalState; label: string }
  /// One question card's state, on the same terms: the answered card keeps only
  /// what it was answered with instead of the fields it was asked with.
  | { k: "question"; id: number; state: QuestionState; label: string }
  /// One change card's state: undone, or no longer the newest turn's.
  | ChangesState
  | {
      k: "status";
      status: string;
      busy: boolean;
      queued: number;
      queue: QueuedMessage[];
      followUpBehavior: "queue" | "steer";
      footer: FooterState;
      /// The thread's summarized title. It changes the moment the first message
      /// is sent, before a `state` message repaints the view, so the header's
      /// title follows the send without waiting for one.
      title?: string;
      /// The turn's own thread when it is not the one on screen, so the strip
      /// follows a turn's start and end without a full `state` repaint.
      run?: RunThread | null;
    }
  /// The footer is attached by the controller (the transcript only knows the
  /// totals), so a usage event repaints the whole footer row.
  | { k: "usage"; usage: UsageTotals; footer?: FooterState }
  /// The composer's pending context and attachments, which travel together:
  /// one removal message addresses either list by chip id.
  | { k: "context"; context: ContextChip[]; attachments: AttachmentChip[] }
  | { k: "composer"; text: string }
  /// The dialog the panel paints over the transcript — the MCP server list, the
  /// session history — or `null` to close it. `core/dialogs.ts` composes it, so
  /// the view only paints the rows and posts back the action one carries; the
  /// controller keeps the dialog, so opening it from either pane shows it in
  /// both.
  | { k: "dialog"; dialog: DialogState | null }
  /// The composer's `@path` completion: the rows to offer for the token at the
  /// caret, and the range of the value they replace — or no rows at all, to
  /// close the list. The host decides both (from the sequence number the view
  /// posted, so a list for a value that has moved on is dropped); the view only
  /// splices in the row that was taken.
  | {
      k: "atSuggestions";
      kind: "path";
      seq: number;
      start: number;
      end: number;
      rows: AtSuggestion[];
    }
  /// The composer's `/` palette: the commands, prompt templates and skills the
  /// CLI lists for this project, for the slash command being typed. The range
  /// is the whole value, since a palette row replaces it; `kind` is what tells
  /// the view which renderer and which splice the rows want, the two
  /// completions sharing one box.
  | {
      k: "paletteRows";
      kind: "command";
      seq: number;
      start: number;
      end: number;
      rows: CommandRow[];
    }
  /// A reference the editor's insert shortcut built (`@src/app.ts#5-10`), to be
  /// written into the composer at the caret. The host reads the file and the
  /// selection; the view only splices what it is handed.
  | { k: "insert"; text: string }
  /// The caret goes in the composer — what bringing the chat forward leaves
  /// behind, since focusing the view does not focus its message box.
  | { k: "focusComposer" };

/// Splits a chunk into complete lines, returning the unterminated remainder.
/// Mirrors `oxide_core::llm::drain_lines`: the buffer is compacted once per
/// chunk instead of once per line.
export function drainLines(
  buffer: string,
  chunk: string,
): { lines: string[]; rest: string } {
  const text = buffer + chunk;
  const lines: string[] = [];
  let start = 0;
  for (;;) {
    const end = text.indexOf("\n", start);
    if (end === -1) break;
    lines.push(text.slice(start, end).replace(/\r$/, ""));
    start = end + 1;
  }
  return { lines, rest: text.slice(start) };
}

/// Parses one line of the agent's JSONL stream, or `null` when the line is
/// blank or not a JSON object (a partial write, a stray log line).
export function parseEvent(line: string): WireEvent | null {
  const trimmed = line.trim();
  if (!trimmed) return null;
  try {
    const value: unknown = JSON.parse(trimmed);
    if (!value || typeof value !== "object" || Array.isArray(value)) return null;
    return value as WireEvent;
  } catch {
    return null;
  }
}

const str = (value: unknown): string => (typeof value === "string" ? value : "");

const num = (value: unknown): number =>
  typeof value === "number" && Number.isFinite(value) ? value : 0;

const obj = (value: unknown): WireEvent =>
  value && typeof value === "object" && !Array.isArray(value)
    ? (value as WireEvent)
    : {};

/// How long a summarized title may be, in characters. It is short enough to
/// read in a side bar and in a notification.
export const TITLE_LIMIT = 64;

/// A leading block marker: a heading, a quote, a bullet or an ordered item.
const BLOCK_MARKER = /^(?:#{1,6}\s+|>\s*|[-*+]\s+|\d+[.)]\s+)+/;

/// Condenses a message into a one-line title: the first line that carries
/// prose (a fence, a rule or a table row says nothing on its own, and a
/// fenced block's contents are a listing rather than a title), with the
/// Markdown markers stripped and the whitespace collapsed. The result is cut
/// at a word boundary so it never ends mid-word.
export function summarizeTitle(text: string, max = TITLE_LIMIT): string {
  const line = titleLine(text);
  if (!line) return "";
  return bound(line, max);
}

/// The first line of `text` that reads as prose, with its Markdown taken off:
/// `""` when the message has none.
function titleLine(text: string): string {
  let fenced = false;
  for (const raw of text.split("\n")) {
    const line = raw.trim();
    if (/^(?:```|~~~)/.test(line)) {
      fenced = !fenced;
      continue;
    }
    if (fenced || !line || line.startsWith("|")) continue;
    const plain = plainText(line);
    // A line that is only punctuation (`---`, `| --- |`) is a rule or a table
    // border, not a title.
    if (plain && /[\p{L}\p{N}]/u.test(plain)) return plain;
  }
  return "";
}

/// One line with its Markdown markers removed: a link or an image keeps its
/// text, and the emphasis, code and quote markers go.
function plainText(line: string): string {
  return line
    .replace(BLOCK_MARKER, "")
    .replace(/!?\[([^\]]*)\]\([^)]*\)/g, "$1")
    .replace(/[`*_~]/g, "")
    .replace(/\s+/g, " ")
    .trim();
}

/// Truncates to `max` characters at a word boundary, marking the cut with an
/// ellipsis. A single word longer than the limit is cut where it has to be.
function bound(text: string, max: number): string {
  if (max === 0) return "";
  if (text.length <= max) return text;
  const cut = text.slice(0, max - 1);
  const space = cut.lastIndexOf(" ");
  const kept = space > 0 ? cut.slice(0, space) : cut;
  return `${kept.trimEnd()}…`;
}

/// Renders a byte count the way the terminal footer does (`1.2k`, `123k`,
/// `1.2M`, `66M`): the decimal is dropped once the number is large enough that
/// it says nothing, so a model window and a token count read the same in both
/// front-ends.
export function formatTokens(count: number): string {
  if (count < 1_000) return String(count);
  if (count < 10_000) return `${(count / 1_000).toFixed(1)}k`;
  if (count < 1_000_000) return `${Math.round(count / 1_000)}k`;
  if (count < 10_000_000) return `${(count / 1_000_000).toFixed(1)}M`;
  return `${Math.round(count / 1_000_000)}M`;
}

/// Turns one agent event into the messages the view applies.
export class Transcript {
  readonly items: Item[] = [];
  usage: UsageTotals = emptyUsage();
  status = "Idle";
  busy = false;
  sessionId: string | null = null;

  private currentAssistant: AssistantItem | null = null;
  private currentThinking: ThinkingItem | null = null;
  private nextId = 1;

  /// `lookupDiff` is how the transcript previews a file change. It is injected
  /// so this module stays free of filesystem access (and testable).
  constructor(
    private readonly lookupDiff: (name: string, args: unknown) => string | null = () => null,
  ) {}

  state(extra: {
    queued: number;
    queue?: QueuedMessage[];
    followUpBehavior?: "queue" | "steer";
    context: ContextChip[];
    attachments: AttachmentChip[];
    title: string;
    folder: string | null;
    binary: string;
    showThinking: boolean;
    footer: FooterState;
    run: RunThread | null;
  }): TranscriptState {
    return {
      items: this.items,
      status: this.status,
      busy: this.busy,
      usage: this.usage,
      sessionId: this.sessionId,
      ...extra,
      queue: extra.queue ?? [],
      followUpBehavior: extra.followUpBehavior ?? "queue",
    };
  }

  reset(): void {
    this.items.length = 0;
    this.usage = emptyUsage();
    this.status = "Idle";
    this.busy = false;
    this.sessionId = null;
    this.currentAssistant = null;
    this.currentThinking = null;
  }

  /// The thread's summarized title: a one-line summary of the first thing the
  /// user sent, so a pasted or multi-line message does not fill the header.
  /// A message that carried only media has no prose to summarize, so its first
  /// attachment label names the thread until a later message with text arrives.
  /// Empty for a thread that has not been written to yet, which the host leaves
  /// to the view's neutral placeholder.
  title(): string {
    let fallback = "";
    for (const item of this.items) {
      if (item.kind !== "user") continue;
      const text = summarizeTitle(item.text);
      if (text) return text;
      if (!fallback) fallback = item.context.find((chip) => chip.label.trim())?.label ?? "";
    }
    return fallback;
  }

  pushUser(text: string, labels: SentChip[]): ViewMessage[] {
    this.currentAssistant = null;
    this.currentThinking = null;
    const item: UserItem = {
      id: this.nextId++,
      kind: "user",
      text,
      context: labels.map((chip) => ({ label: chip.label, preview: chip.preview ?? null })),
    };
    this.items.push(item);
    return [{ k: "push", item }];
  }

  notice(text: string, tone: NoticeItem["tone"] = "info"): ViewMessage[] {
    const item: NoticeItem = { id: this.nextId++, kind: "notice", text, tone };
    this.items.push(item);
    return [{ k: "push", item }];
  }

  /// A reply pushed whole rather than streamed: the stored conversation of a
  /// thread the panel is reopening, which arrives already finished.
  pushAssistant(text: string): ViewMessage[] {
    this.closeAssistant();
    this.closeThinking();
    const item: AssistantItem = { id: this.nextId++, kind: "assistant", text };
    this.items.push(item);
    return [{ k: "push", item }];
  }

  /// A stored thread's turns, pushed whole rather than streamed. The caller
  /// sends one `state` message afterwards, so nothing is painted per entry.
  replay(entries: ReplayEntry[]): void {
    for (const entry of entries) {
      if (entry.kind === "user") this.pushUser(entry.text, entry.attachments ?? []);
      else if (entry.kind === "assistant") this.pushAssistant(entry.text);
      else this.pushTool(entry.name, entry.args, entry.output, entry.isError);
    }
  }

  /// A tool call pushed whole, as a stored thread records it: finished, with the
  /// result the thread stored beside it. It carries the diff its own arguments
  /// describe — the replacements an `edit` made, a `patch`'s own diff — rather
  /// than one built against the file as it stands.
  ///
  /// Successful stored calls remain unknown because the store does not record
  /// whether a file change applied. The compatibility error marker does record
  /// failures, so those cards retain their error state without showing the marker.
  private pushTool(name: string, args: string, output: string, isError: boolean): void {
    this.closeAssistant();
    this.closeThinking();
    const item: ToolItem = {
      id: this.nextId++,
      kind: "tool",
      name,
      args,
      output,
      diff: isError ? null : (argumentDiff(name, parseArgs(args))?.diff ?? null),
      running: false,
      isError,
      unknown: !isError,
    };
    this.items.push(item);
  }

  /// The current status/footer line. The controller sends this after applying
  /// a batch so the view never sees a stale busy flag or queue count.
  statusMessage(
    queueOrCount: QueuedMessage[] | number,
    behaviorOrFooter: "queue" | "steer" | FooterState,
    maybeFooter?: FooterState,
  ): ViewMessage {
    // While a turn runs the status names the activity; otherwise the agent is
    // idle and the outcome is in the transcript.
    const status = this.busy ? (this.status === "Idle" ? "Thinking…" : this.status) : "Idle";
    const queue = Array.isArray(queueOrCount) ? queueOrCount : [];
    const followUpBehavior = typeof behaviorOrFooter === "string" ? behaviorOrFooter : "queue";
    const footer = typeof behaviorOrFooter === "string" ? (maybeFooter as FooterState) : behaviorOrFooter;
    return {
      k: "status",
      status,
      busy: this.busy,
      queued: Array.isArray(queueOrCount) ? queue.length : queueOrCount,
      queue,
      followUpBehavior,
      footer,
    };
  }

  /// Applies one wire event, returning the view updates it implies.
  apply(event: WireEvent): ViewMessage[] {
    switch (str(event.type)) {
      case "session":
        this.sessionId = str(event.id) || this.sessionId;
        return [];
      case "thinking":
        // The block itself is created by the first reasoning delta it carries:
        // a turn can start a thinking block and never fill it, and an empty
        // one would sit in the transcript for the rest of the session.
        this.closeAssistant();
        this.closeThinking();
        return [];
      case "thinking_done":
        // The model step finished streaming. Closing the in-progress pointers
        // here means a retry in a later step drops only its own attempt, not a
        // previous step's committed reply.
        this.closeAssistant();
        this.closeThinking();
        return [];
      case "message_update":
        return this.applyDelta(obj(event.assistantMessageEvent));
      case "tool_call":
        return this.startTool(str(event.toolName) || "tool", event.arguments);
      case "tool_execution_update":
        return this.appendToolOutput(str(event.toolName), str(event.partialResult));
      case "tool_execution_end":
        return this.finishTool(str(event.toolName), str(event.result), event.isError === true);
      case "usage": {
        // Usage is emitted once a model step has completed, so it commits the
        // step's text and reasoning even when no tool call follows and the CLI
        // has not sent a `thinking_done` boundary (older CLI builds).
        this.closeAssistant();
        this.closeThinking();
        const usage = obj(event.usage);
        const cacheRead = num(usage.cacheRead);
        const cacheWrite = num(usage.cacheWrite);
        const prompt = num(usage.input) + cacheRead + cacheWrite;
        // Mirrors `UsageTotals::cache_hit_rate`: the rate belongs to one
        // request, so a step without cache traffic keeps the previous one.
        const cacheHit =
          cacheRead + cacheWrite > 0 && prompt > 0
            ? (cacheRead / prompt) * 100
            : this.usage.cacheHit;
        this.usage = {
          input: this.usage.input + num(usage.input),
          output: this.usage.output + num(usage.output),
          cacheRead: this.usage.cacheRead + cacheRead,
          cacheWrite: this.usage.cacheWrite + cacheWrite,
          cost: this.usage.cost + num(usage.cost),
          contextTokens: prompt,
          cacheHit,
        };
        return [{ k: "usage", usage: this.usage }];
      }
      case "auto_retry_start": {
        // A retry re-streams the response from the start, so the partial item a
        // failed attempt was streaming into is dropped rather than extended.
        const messages = this.discardAttempt();
        // The status line is also re-sent by the controller once per event
        // batch (it owns the queued count).
        const delay = num(event.delayMs);
        const wait = delay >= 1_000 ? ` in ${Math.round(delay / 1000)}s` : "";
        this.status = `Retrying (${num(event.attempt)}/${num(event.maxAttempts)})${wait}…`;
        return messages;
      }
      case "approval_request": {
        const request = approvalRequest(event);
        if (!request) return [];
        // The request arrives right after the `tool_call` it belongs to, so the
        // step's text and reasoning are already committed.
        this.closeAssistant();
        this.closeThinking();
        const item: ApprovalItem = {
          id: this.nextId++,
          kind: "approval",
          requestId: request.id,
          tool: request.tool,
          title: approvalTitle(request.tool),
          detail: request.detail,
          state: "pending",
          label: "",
        };
        this.items.push(item);
        this.status = "Waiting for approval…";
        return [{ k: "push", item }];
      }
      case "question_request": {
        const request = questionRequest(event);
        if (!request) return [];
        // Same ordering as an approval: the `ask` call it belongs to is already
        // in the transcript, so the step's own text is committed first.
        this.closeAssistant();
        this.closeThinking();
        const item: QuestionItem = {
          id: this.nextId++,
          kind: "question",
          requestId: request.id,
          title: questionTitle(request.questions),
          questions: request.questions,
          state: "pending",
          label: "",
        };
        this.items.push(item);
        this.status = "Waiting for your answer…";
        return [{ k: "push", item }];
      }
      // The CLI gave up on the request because nobody answered in time. The run
      // it belongs to may still be going, so the card stops taking an answer
      // the broker would no longer read, and the status goes back to the work
      // the turn resumed.
      case "question_closed": {
        const item = this.pendingQuestion(num(event.id));
        if (!item) return [];
        this.status = "Thinking…";
        return [this.settleQuestion(item, "closed", "Not answered")];
      }
      case "compaction":
        return this.notice(
          `Compacted ${num(event.summarized)} earlier messages (~${formatTokens(
            num(event.tokensBefore),
          )} tokens)`,
        );
      case "error":
        return this.notice(`Error: ${str(event.message)}`, "error");
      case "agent_end":
        this.closeAssistant();
        this.closeThinking();
        this.status = "Done";
        return [];
      // The files the turn changed, from the run's own shadow snapshot: the
      // controller does not have to know about them. The CLI writes it behind
      // that run's `agent_end` on the same channel, so the card lands at the end
      // of the turn it belongs to.
      case "turn_changes": {
        const changes = turnChanges(event);
        if (!changes) return [];
        this.closeAssistant();
        this.closeThinking();
        const item: ChangesItem = {
          id: this.nextId++,
          kind: "changes",
          title: changesTitle(changes.files.length),
          totals: changesTotals(changes.added, changes.removed),
          baseline: changes.baseline,
          project: changes.project,
          after: changes.after,
          more: changesMore(changes.files.length),
          // Only the newest turn can be put back: an earlier card's restore
          // would take this one's work with it, so this card takes the Undo
          // away from every card before it.
          undoable: true,
          undone: false,
          rows: changeRows(changes.files),
        };
        const messages: ViewMessage[] = [];
        for (const entry of this.items) {
          if (entry.kind === "changes" && entry.undoable) {
            entry.undoable = false;
            messages.push({ k: "changes", id: entry.id, undoable: false, undone: entry.undone });
          }
        }
        this.items.push(item);
        messages.push({ k: "push", item });
        return messages;
      }
      default:
        return [];
    }
  }

  /// Records the user's answer on a waiting card. `null` when the id is not a
  /// waiting request (already answered, or from a run that is gone), so the
  /// controller never sends a second answer for one request.
  answerApproval(requestId: number, decision: ApprovalDecision): ViewMessage[] | null {
    const item = this.items.find(
      (entry): entry is ApprovalItem =>
        entry.kind === "approval" && entry.requestId === requestId && entry.state === "pending",
    );
    if (!item) return null;
    return [this.settleApproval(item, decision, approvalLabel(decision))];
  }

  /// The change card a view is asking about, or `null` for one the transcript no
  /// longer holds (a card from a thread the panel has left).
  changes(id: number): ChangesItem | null {
    const item = this.items.find(
      (entry): entry is ChangesItem => entry.kind === "changes" && entry.id === id,
    );
    return item ?? null;
  }

  /// Records that a card's turn has been put back, which leaves the card as the
  /// listing of what that turn did with its Undo spent. `null` for a card the
  /// transcript no longer holds, so a stale click from the other pane does
  /// nothing; a card already undone is settled without a second restore.
  markUndone(id: number): ViewMessage[] | null {
    const item = this.changes(id);
    if (!item) return null;
    if (item.undone) return [{ k: "changes", id, undoable: false, undone: true }];
    item.undone = true;
    item.undoable = false;
    return [{ k: "changes", id, undoable: false, undone: true }];
  }

  /// Settles every card still waiting, which is what a run that ended (or was
  /// stopped, or whose process died) leaves behind: the request it was waiting
  /// on is gone with the process, so its buttons must stop offering an answer.
  closeApprovals(): ViewMessage[] {
    const messages: ViewMessage[] = [];
    for (const item of [...this.items]) {
      if (item.kind === "approval" && item.state === "pending") {
        messages.push(this.settleApproval(item, "closed", "Not answered"));
      }
    }
    return messages;
  }

  private settleApproval(
    item: ApprovalItem,
    state: ApprovalState,
    label: string,
  ): ViewMessage {
    item.state = state;
    item.label = label;
    return { k: "approval", id: item.id, state, label };
  }

  /// The waiting card for a request id, or `undefined` when none is waiting:
  /// the id was never painted here, was already answered, or belongs to a
  /// request the CLI has given up on.
  private pendingQuestion(requestId: number): QuestionItem | undefined {
    return this.items.find(
      (entry): entry is QuestionItem =>
        entry.kind === "question" && entry.requestId === requestId && entry.state === "pending",
    );
  }

  /// Records the user's answers on a waiting question card. `null` when the id
  /// is not a waiting request (already answered, or from a run that is gone), so
  /// the controller never sends a second answer for one request. An answer with
  /// nothing in it is how the card says the question was dismissed.
  answerQuestion(
    requestId: number,
    answers: readonly QuestionAnswer[],
  ): ViewMessage[] | null {
    const item = this.pendingQuestion(requestId);
    if (!item) return null;
    const answered = answers.some((answer) => answer.values.length > 0);
    return [
      this.settleQuestion(item, answered ? "answered" : "dismissed", questionLabel(answers)),
    ];
  }

  /// Settles every question still waiting, which is what a run that ended (or
  /// was stopped, or whose process died) leaves behind: the request it was
  /// waiting on is gone with the process, so its fields must stop taking an
  /// answer nobody will read.
  closeQuestions(): ViewMessage[] {
    const messages: ViewMessage[] = [];
    for (const item of [...this.items]) {
      if (item.kind === "question" && item.state === "pending") {
        messages.push(this.settleQuestion(item, "closed", "Not answered"));
      }
    }
    return messages;
  }

  private settleQuestion(
    item: QuestionItem,
    state: QuestionState,
    label: string,
  ): ViewMessage {
    item.state = state;
    item.label = label;
    return { k: "question", id: item.id, state, label };
  }

  /// Drops the assistant or thinking item a failed attempt was still streaming
  /// into, so the retry's fresh output does not extend it.
  private discardAttempt(): ViewMessage[] {
    const messages: ViewMessage[] = [];
    for (const item of [this.currentAssistant, this.currentThinking]) {
      if (!item) continue;
      const index = this.items.indexOf(item);
      if (index >= 0) this.items.splice(index, 1);
      messages.push({ k: "remove", id: item.id });
    }
    this.currentAssistant = null;
    this.currentThinking = null;
    return messages;
  }

  private applyDelta(delta: WireEvent): ViewMessage[] {
    const text = str(delta.delta);
    switch (str(delta.type)) {
      case "thinking_delta": {
        if (!text) return [];
        // A delta that opens a block has to push an empty block before
        // appending, or the view has no entry for the id and drops the text.
        // The pushed item must not carry the delta, because the `append` that
        // follows it already does: sending both would paint the first fragment
        // twice.
        const created = this.currentThinking === null;
        const item = this.ensureThinking();
        item.text += text;
        const messages: ViewMessage[] = created
          ? [{ k: "push", item: { ...item, text: "" } }]
          : [];
        messages.push({ k: "append", id: item.id, field: "text", delta: text });
        return messages;
      }
      case "text_delta": {
        if (!text) return [];
        // The same empty-push rule as the reasoning delta: the `push` only
        // creates the bubble, and the `append` carries the first fragment, so
        // the fragment is painted once rather than by both messages.
        const created = this.currentAssistant === null;
        const item = this.ensureAssistant();
        item.text += text;
        this.status = "Writing…";
        const messages: ViewMessage[] = created
          ? [{ k: "push", item: { ...item, text: "" } }]
          : [];
        messages.push({ k: "append", id: item.id, field: "text", delta: text });
        return messages;
      }
      default:
        return [];
    }
  }

  private ensureAssistant(): AssistantItem {
    if (this.currentAssistant) return this.currentAssistant;
    this.closeThinking();
    const item: AssistantItem = { id: this.nextId++, kind: "assistant", text: "" };
    this.items.push(item);
    this.currentAssistant = item;
    return item;
  }

  private ensureThinking(): ThinkingItem {
    if (this.currentThinking) return this.currentThinking;
    const item: ThinkingItem = { id: this.nextId++, kind: "thinking", text: "" };
    this.items.push(item);
    this.currentThinking = item;
    return item;
  }

  /// A tool call ends the current step: text after it starts a new bubble.
  private closeAssistant(): void {
    this.currentAssistant = null;
  }

  private closeThinking(): void {
    this.currentThinking = null;
  }

  private startTool(name: string, args: unknown): ViewMessage[] {
    this.closeAssistant();
    this.closeThinking();
    const raw = typeof args === "string" ? args : JSON.stringify(args ?? {});
    const item: ToolItem = {
      id: this.nextId++,
      kind: "tool",
      name,
      args: raw,
      output: "",
      diff: this.lookupDiff(name, parseArgs(raw)),
      running: true,
      isError: false,
      startedAt: Date.now(),
    };
    this.items.push(item);
    this.status = `Running ${name}…`;
    return [{ k: "push", item }];
  }

  /// Results are emitted in call order, so the oldest running card with a
  /// matching name owns the update.
  private runningTool(name: string): ToolItem | undefined {
    const tools = this.items.filter(
      (item): item is ToolItem => item.kind === "tool" && item.running,
    );
    return tools.find((item) => item.name === name) ?? tools[0];
  }

  /// Progress events carry only a tool name, so two parallel calls of the same
  /// tool are indistinguishable. Never guess between them: drop the update
  /// instead of appending it to the wrong card. The result pass uses `runningTool`
  /// because results arrive in call order.
  private progressTool(name: string): ToolItem | undefined {
    const running = this.items.filter(
      (item): item is ToolItem => item.kind === "tool" && item.running,
    );
    const named = running.filter((item) => item.name === name);
    if (named.length === 1) return named[0];
    if (named.length > 1) return undefined;
    return running.length === 1 ? running[0] : undefined;
  }

  private appendToolOutput(name: string, chunk: string): ViewMessage[] {
    if (!chunk) return [];
    const tool = this.progressTool(name);
    if (!tool) return [];
    tool.output += chunk;
    return [{ k: "append", id: tool.id, field: "output", delta: chunk }];
  }

  private finishTool(name: string, result: string, isError: boolean): ViewMessage[] {
    const tool = this.runningTool(name);
    if (!tool) return [];
    if (result) tool.output = result;
    tool.running = false;
    tool.isError = isError;
    // The call is over, so the card keeps the time it took rather than the
    // moment it began: nothing else reads the start once it has settled.
    const elapsed = tool.startedAt ? Math.max(0, Date.now() - tool.startedAt) : undefined;
    delete tool.startedAt;
    // The diff was built from the call's own arguments, before it ran, so a call
    // that failed never made that change: dropping it leaves the card showing
    // what went wrong instead of counting lines that are not on disk. A call
    // that changed a file reads as one line and keeps its diff for the reader
    // who unfolds it, since the turn's change card already lists it.
    const dropped = isError && tool.diff !== null;
    if (dropped) tool.diff = null;
    this.status = "Thinking…";
    return [
      {
        k: "patch",
        id: tool.id,
        patch: {
          output: tool.output,
          running: false,
          isError,
          ...(dropped ? { diff: null } : {}),
          // What the call took, measured where it began. `null` (rather than an
          // absent field) is what takes a start off a card a `state` message
          // painted from a stale item.
          startedAt: null,
          elapsed: elapsed ?? null,
        },
      },
    ];
  }
}

export function parseArgs(raw: string): unknown {
  try {
    return JSON.parse(raw || "{}");
  } catch {
    return {};
  }
}
