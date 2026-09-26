// The request channel of `oxide --mode rpc`: LF-delimited JSON written to the
// child's stdin. The extension runs rpc mode rather than one-shot json mode
// because a tool approval has to be answered mid-turn, which `--mode json`
// (whose stdin is closed after the prompt) cannot carry.
//
// The frames are plain data so this module stays free of process handling and
// is unit tested on its own; `cli.ts` owns the pipe.

import type { ApprovalDecision } from "./approvals";

/// `{"type":"prompt","message":…,"images":[…]}`. In rpc mode the images travel
/// here instead of in `--image` flags, because the prompt itself does.
export function promptFrame(prompt: string, images: readonly string[] = []): string {
  return `${JSON.stringify({ type: "prompt", message: prompt, images: [...images] })}\n`;
}

/// `{"type":"approval","id":…,"decision":…}` answers the request with that id.
/// An answer for an unknown id is ignored by the CLI.
export function approvalFrame(id: number, decision: ApprovalDecision): string {
  return `${JSON.stringify({ type: "approval", id, decision })}\n`;
}

/// `{"type":"quit"}` ends the session. The CLI closes its event stream and
/// exits, which is what a one-shot turn does after the prompt.
export function quitFrame(): string {
  return '{"type":"quit"}\n';
}
