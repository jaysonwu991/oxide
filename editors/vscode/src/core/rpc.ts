// The request channel of `oxide --mode rpc`: LF-delimited JSON written to the
// child's stdin. The extension runs rpc mode rather than one-shot json mode
// because a tool approval has to be answered mid-turn, which `--mode json`
// (whose stdin is closed after the prompt) cannot carry.
//
// The frames are plain data so this module stays free of process handling and
// is unit tested on its own; `cli.ts` owns the pipe.

import type { ApprovalDecision } from "./approvals";
import type { QuestionAnswer } from "./questions";

/// `{"type":"prompt","message":…,"images":[…]}`. In rpc mode the images travel
/// here instead of in `--image` flags, because the prompt itself does.
export function promptFrame(prompt: string, images: readonly string[] = []): string {
  return `${JSON.stringify({ type: "prompt", message: prompt, images: [...images] })}\n`;
}

/// Adds context to the process's in-flight turn. `followUp` uses the agent's
/// after-response queue; false steers before its next model step.
export function steerFrame(
  prompt: string,
  images: readonly string[] = [],
  followUp = false,
): string {
  return `${JSON.stringify({
    type: "steer",
    message: prompt,
    images: [...images],
    follow_up: followUp,
  })}\n`;
}

/// `{"type":"approval","id":…,"decision":…}` answers the request with that id.
/// An answer for an unknown id is ignored by the CLI.
export function approvalFrame(id: number, decision: ApprovalDecision): string {
  return `${JSON.stringify({ type: "approval", id, decision })}\n`;
}

/// `{"type":"question","id":…,"answers":[…]}` answers the question with that
/// id. An empty list is a dismissal, which the CLI reports to the model as a
/// question the user did not answer — and a submission with nothing filled in is
/// sent as that dismissal rather than as a set of blank answers, so answering an
/// empty form and pressing Dismiss reach the model the same way.
export function questionFrame(id: number, answers: readonly QuestionAnswer[]): string {
  const filled = answers.filter((answer) => answer.values.some((value) => value.trim()));
  return `${JSON.stringify({
    type: "question",
    id,
    answers: filled.map((answer) => ({
      question: answer.question,
      values: [...answer.values],
    })),
  })}\n`;
}

/// `{"type":"quit"}` ends the session. The CLI closes its event stream and
/// exits, which is what a one-shot turn does after the prompt.
export function quitFrame(): string {
  return '{"type":"quit"}\n';
}
