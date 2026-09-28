// The files a turn changed, read from the CLI's own `turn_changes` frame.
//
// The listing is the one the terminal's review and the desktop app's card draw,
// built in `oxide_core::changes` from the project's shadow snapshot — so a file
// a shell command, a formatter or an MCP server wrote is listed the same as an
// edited one. This module turns it into the rows the webview paints and the
// diff plan VS Code's own diff editor opens, which is how the panel shows a
// change: it never renders a second diff format of its own, not even in its own
// review, which walks these rows and opens each file in that editor.

import type { WireEvent } from "./protocol";

/// How the run touched a file, as `git status --name-status` spells it.
export type ChangeStatus = "added" | "modified" | "deleted";

/// What a row and a diff both need: the file in the project and how the run
/// touched it.
export interface ChangeLike {
  path: string;
  status: ChangeStatus;
}

/// One changed file as the frame carries it.
export interface ChangedFile extends ChangeLike {
  added: number;
  removed: number;
  binary: boolean;
}

/// One row of the card as the webview paints it: what it is badged with, what it
/// says, and the words its tooltip carries. Composed here rather than in the
/// webview, which only paints what it is handed.
export interface ChangeRow extends ChangeLike {
  letter: string;
  detail: string;
  title: string;
  index: number;
}

/// The turn's listing, with the baseline each file's diff is drawn against and
/// the folder the run was in — its paths are relative to that project, and its
/// baseline is read out of that project's shadow snapshot.
export interface TurnChanges {
  project: string;
  baseline: string;
  files: ChangedFile[];
  added: number;
  removed: number;
}

/// The `+N`/`−N` a row shows, or what a file with no lines to count says.
export function changeDetail(file: ChangedFile): string {
  if (file.binary) return "binary";
  const counts: string[] = [];
  if (file.added > 0) counts.push(`+${file.added}`);
  if (file.removed > 0) counts.push(`−${file.removed}`);
  return counts.join(" ") || "no line changes";
}

/// The letter the row is badged with, matching `git status` and the terminal.
export function changeLetter(status: ChangeStatus): string {
  return status === "added" ? "A" : status === "deleted" ? "D" : "M";
}

/// The card's heading, e.g. `Edited 3 files` or `Edited 1 file`. An empty
/// listing is named as one, so a caller that drew a header anyway still reads.
export function changesTitle(count: number): string {
  if (count === 1) return "Edited 1 file";
  return `Edited ${count} files`;
}

/// The turn's `+N −N`, which is the card's own total. Binary files contribute
/// nothing, so a turn that only rewrote one reads as blank rather than `+0 −0`.
export function changesTotals(added: number, removed: number): string {
  const counts: string[] = [];
  if (added > 0) counts.push(`+${added}`);
  if (removed > 0) counts.push(`−${removed}`);
  return counts.join(" ");
}

export function changeRows(files: readonly ChangedFile[]): ChangeRow[] {
  return files.map((file, index) => ({
    path: file.path,
    status: file.status,
    letter: changeLetter(file.status),
    detail: changeDetail(file),
    title: `Show ${file.path} in VS Code's diff editor`,
    index,
  }));
}

/// The title of the diff editor for one file, which is also the label VS Code
/// shows in the multi-file diff.
export function diffLabel(change: ChangeLike): string {
  return `${change.path} (${change.status})`;
}

/// VS Code's own diff editor draws a change; the panel only says which file and
/// against what. The left side — the file as the run found it — exists only in
/// the project's shadow snapshots, so the host registers a content provider for
/// this scheme and serves it from the CLI's own read (`changeArgs`).
export const CHANGE_SCHEME = "oxide-changes";

/// What one row opens: the file in the project, and the sides the diff editor
/// draws. `baseline` is `null` for a file the run added, which has nothing to
/// compare against, and a file the run removed is no longer on disk, so its
/// right side is empty.
export interface DiffPlan {
  title: string;
  path: string;
  baseline: string | null;
  /// Whether the file is still on disk, which is what the right side shows.
  present: boolean;
}

export function diffPlan(baseline: string, change: ChangeLike): DiffPlan {
  return {
    title: diffLabel(change),
    path: change.path,
    baseline: change.status === "added" ? null : baseline,
    present: change.status !== "deleted",
  };
}

/// The CLI read behind the provider above: one file as the run's baseline
/// recorded it, printed byte for byte.
export function changeArgs(path: string, baseline: string, project: string): string[] {
  return ["changes", "show", path, "--baseline", baseline, "--project", project];
}

/// The query a snapshot URI carries: the project the file is relative to and the
/// revision to read it at. Both travel in the URI because the content provider is
/// handed the URI alone, and the folder a card belongs to is not necessarily the
/// one the window has active when a row is clicked.
export function snapshotQuery(project: string, revision: string | null): string {
  return new URLSearchParams({ project, revision: revision ?? "" }).toString();
}

/// The two parts of that query as the provider reads them. Both empty for a query
/// that is not one of ours, which is a side with no content.
export function parseSnapshotQuery(query: string): { project: string; revision: string } {
  const params = new URLSearchParams(query);
  return { project: params.get("project") ?? "", revision: params.get("revision") ?? "" };
}

/// Parses the frame's payload. A frame without a listing, or one whose entries
/// are not objects, yields nothing rather than half a card.
export function turnChanges(event: WireEvent): TurnChanges | null {
  if (event.type !== "turn_changes") return null;
  return parseChanges(event.changes, text(event.baseline), text(event.project));
}

/// The listing inside a payload, with a baseline to diff against and the folder
/// it belongs to. `null` when the payload names no files: a turn that only read
/// files draws no card.
export function parseChanges(
  payload: unknown,
  baseline: string,
  project: string,
): TurnChanges | null {
  if (!payload || typeof payload !== "object") return null;
  const record = payload as Record<string, unknown>;
  const raw = Array.isArray(record.files) ? record.files : [];
  const files: ChangedFile[] = [];
  for (const entry of raw) {
    if (!entry || typeof entry !== "object") continue;
    const file = entry as Record<string, unknown>;
    const path = text(file.path);
    if (!path) continue;
    files.push({
      path,
      status: status(text(file.status)),
      added: count(file.added),
      removed: count(file.removed),
      binary: file.binary === true,
    });
  }
  if (!files.length) return null;
  return {
    project,
    baseline,
    files,
    added: files.reduce((total, file) => total + file.added, 0),
    removed: files.reduce((total, file) => total + file.removed, 0),
  };
}

function status(value: string): ChangeStatus {
  if (value === "added" || value === "deleted") return value;
  return "modified";
}

function count(value: unknown): number {
  return typeof value === "number" && Number.isFinite(value) && value > 0 ? Math.floor(value) : 0;
}

function text(value: unknown): string {
  return typeof value === "string" ? value : "";
}
