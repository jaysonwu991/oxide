// Questions the model asks through the `ask` tool: the request the CLI emits
// when it needs something only the user can decide — typically while following
// a skill — and the answers the transcript sends back. The answer travels over
// the `--mode rpc` channel (`core/rpc.ts`), so nothing here touches a process.
// Webview-free and unit tested.

import type { WireEvent } from "./protocol";

/// One offered answer. `label` is what the user picks and what the model reads
/// back; `description` says what choosing it means.
export interface QuestionChoice {
  label: string;
  description: string;
}

/// One question: free text, a single choice, or several when `multiSelect` is
/// set.
export interface Question {
  question: string;
  /// A short label for the question, used as the card's heading.
  header: string;
  options: QuestionChoice[];
  multiSelect: boolean;
}

/// A parsed `question_request`. `id` is the broker's request id, which the
/// answer frame has to echo back.
export interface QuestionRequest {
  id: number;
  questions: Question[];
}

/// An answer to one question: the labels that were picked and the text that was
/// typed. `question` echoes the question it answers, so the CLI matches each
/// answer to the question it belongs to without depending on their order.
export interface QuestionAnswer {
  question: string;
  values: string[];
}

function text(value: unknown): string {
  return typeof value === "string" ? value.trim() : "";
}

/// Reads a `question_request` wire event. A request without a usable id or
/// without a question cannot be answered, so it is dropped rather than rendered
/// as a card whose buttons would never reach the broker.
export function questionRequest(event: WireEvent): QuestionRequest | null {
  const id = event.id;
  if (typeof id !== "number" || !Number.isFinite(id) || id <= 0) return null;
  const raw = Array.isArray(event.questions) ? event.questions : [];
  const questions: Question[] = [];
  for (const entry of raw) {
    if (!entry || typeof entry !== "object") continue;
    const fields = entry as Record<string, unknown>;
    const question = text(fields.question);
    if (!question) continue;
    const options = (Array.isArray(fields.options) ? fields.options : [])
      .map((option) => {
        // The core's own parser accepts an option written as a plain string, so
        // a request from an older CLI still paints its choices.
        if (typeof option === "string") {
          return { label: option.trim(), description: "" };
        }
        if (!option || typeof option !== "object") return null;
        const choice = option as Record<string, unknown>;
        const label = text(choice.label);
        return label ? { label, description: text(choice.description) } : null;
      })
      .filter((choice): choice is QuestionChoice => choice !== null);
    questions.push({
      question,
      header: text(fields.header),
      options,
      multiSelect: fields.multiSelect === true,
    });
  }
  if (!questions.length) return null;
  return { id, questions };
}

/// Reads the answers a webview posted: one entry per question, each carrying the
/// labels that were ticked and the text that was typed. An entry with no values
/// still travels, since the question it belongs to is the one it answers, and
/// anything unusable is dropped rather than sent to the CLI as an answer to
/// nothing.
export function questionAnswers(
  raw: readonly { question?: unknown; values?: unknown }[],
): QuestionAnswer[] {
  const answers: QuestionAnswer[] = [];
  for (const entry of raw) {
    if (!entry || typeof entry !== "object") continue;
    const question = text((entry as Record<string, unknown>).question);
    if (!question) continue;
    const values = (Array.isArray(entry.values) ? entry.values : [])
      .map((value) => text(value))
      .filter(Boolean);
    answers.push({ question, values: [...new Set(values)] });
  }
  return answers;
}

/// The card's heading: the first question's own header, else the first question
/// itself, so a card always says what it is about.
export function questionTitle(questions: readonly Question[]): string {
  const first = questions[0];
  if (!first) return "Question";
  return first.header || first.question;
}

/// What an answered card reads: the labels that were picked and the text that
/// was typed, joined so a multi-select answer still fits the card's line.
/// Nothing answered at all reads as a dismissal, which is what the CLI was told.
export function questionLabel(answers: readonly QuestionAnswer[]): string {
  const answered = answers.filter((answer) => answer.values.length > 0);
  if (!answered.length) return "Dismissed without an answer";
  return answered
    .map((answer) => answer.values.join(", "))
    .join(" · ");
}
