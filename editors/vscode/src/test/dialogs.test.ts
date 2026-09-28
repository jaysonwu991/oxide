// The two dialogs the panel paints are composed here as data, so what a row
// says — a server's state and the scope a toggle writes to, a session's age and
// size, what a deletion would remove — is assertable without a webview or a CLI.

import assert from "node:assert/strict";
import { describe, it } from "node:test";

import {
  CLOSE_DIALOG,
  CONTINUE_SESSION,
  deleteSessionDialog,
  MCP_TOGGLE,
  NEW_SESSION,
  mcpDialog,
  OPEN_SESSION,
  SESSION_DELETE,
  SESSION_DELETE_CONFIRM,
  sessionDialog,
} from "../core/dialogs";
import { parseMcpList } from "../core/mcps";
import { parseSessionList } from "../core/sessions";

const listing = JSON.stringify([
  {
    name: "sentry",
    transport: "stdio",
    detail: "npx -y @sentry/mcp",
    source: "project",
    scope: "project",
    enabled: false,
    state: "disabled",
    status: "Disabled",
  },
  {
    name: "context7",
    transport: "http",
    detail: "https://mcp.context7.com/mcp/oauth (oauth)",
    source: "claude (global)",
    scope: "global",
    enabled: true,
    state: "connected",
    status: "Connected",
  },
  {
    name: "broken",
    transport: "stdio",
    detail: "broken-server",
    source: "~/.oxide/mcp.json",
    scope: "global",
    enabled: true,
    state: "error",
    status: "Error",
  },
]);

describe("MCP dialog", () => {
  it("paints one row per server, in the name order the listing arrives in", () => {
    const dialog = mcpDialog(parseMcpList(listing));
    assert.equal(dialog.title, "MCP servers");
    // It grows up from the composer: that is where the `/mcps` that opens it was
    // typed, so the listing stays where the words that asked for it are.
    assert.equal(dialog.pin, "footer");
    assert.equal(dialog.refreshLabel, "Recheck");
    assert.deepEqual(
      dialog.rows.map((row) => row.label),
      ["broken", "context7", "sentry"],
    );
    assert.deepEqual(
      dialog.rows.map((row) => row.status),
      ["Error", "Connected", "Disabled"],
    );
    assert.deepEqual(
      dialog.rows.map((row) => row.tone),
      ["error", "ok", "muted"],
    );
  });

  it("carries what a row toggles, and where the change lands", () => {
    const rows = mcpDialog(parseMcpList(listing)).rows;
    const sentry = rows.find((row) => row.value === "sentry")!;
    assert.equal(sentry.button, "Enable sentry");
    assert.equal(sentry.buttonAction, MCP_TOGGLE);
    assert.equal(sentry.icon, "power", "the button is one glyph, not a word");
    assert.equal(sentry.detail, "stdio · npx -y @sentry/mcp · source: project");
    // A row opens nothing on its own: the button is the action.
    assert.equal(sentry.action, "");
  });

  it("offers Disable for a server that is on", () => {
    const rows = mcpDialog(parseMcpList(listing)).rows;
    assert.equal(rows.find((row) => row.value === "context7")!.button, "Disable context7");
  });

  it("names a state the core did not spell out, and a switched-off one either way", () => {
    const bare = mcpDialog(parseMcpList('[{"name":"bare","state":"needs-auth"}]')).rows[0];
    assert.equal(bare.status, "Needs auth", "the state is named when status is empty");
    const off = mcpDialog(
      parseMcpList('[{"name":"off","enabled":false,"state":"error","status":"Error"}]'),
    ).rows[0];
    assert.equal(off.status, "Disabled", "a server that is off reports that, not the probe");
    assert.equal(off.tone, "muted");
  });

  it("says what to do when the project has no servers", () => {
    const dialog = mcpDialog([]);
    assert.equal(dialog.rows.length, 0);
    assert.match(dialog.note, /No MCP servers configured/);
  });

  it("shows a failure in place of the reason it is empty", () => {
    const dialog = mcpDialog([], "Could not list MCP servers: exit 1");
    assert.equal(dialog.note, "Could not list MCP servers: exit 1");
  });
});

describe("session dialog", () => {
  const sessions = parseSessionList(
    [
      "fe0031b1  just now     195 msg  Create VS Code Extension for Oxide",
      "7c8031b1  7m ago         2 msg  say hi",
    ].join("\n"),
  );

  it("offers the two ways out of the current thread first", () => {
    const dialog = sessionDialog(sessions, null);
    assert.equal(dialog.title, "Sessions");
    // It drops from the header: the header names the thread the listing is
    // about, and the "New chat" row is about leaving that one.
    assert.equal(dialog.pin, "header");
    assert.deepEqual(
      dialog.rows.map((row) => row.value),
      [NEW_SESSION, CONTINUE_SESSION, "fe0031b1", "7c8031b1"],
    );
    assert.deepEqual(
      dialog.rows.map((row) => row.action),
      [OPEN_SESSION, OPEN_SESSION, OPEN_SESSION, OPEN_SESSION],
    );
    // Both are answered by the panel, so neither carries a toggle.
    assert.equal(dialog.refreshLabel, "");
    // A session row ends in a trash: the two ways out of the current thread do
    // not, since neither deletes anything.
    assert.deepEqual(
      dialog.rows.map((row) => row.button),
      ["", "", "Delete Create VS Code Extension for Oxide", "Delete say hi"],
    );
    assert.deepEqual(
      dialog.rows.map((row) => row.buttonAction),
      ["", "", SESSION_DELETE, SESSION_DELETE],
    );
    assert.deepEqual(
      dialog.rows.map((row) => row.icon),
      ["", "", "trash", "trash"],
    );
    // With no thread open there is nothing to close, so the row offers a fresh
    // chat rather than claiming it closes one.
    assert.equal(dialog.rows[0].label, "New chat");
    assert.equal(dialog.rows[0].detail, "Start a fresh thread");
    assert.equal(dialog.rows[1].status, "", "and neither row carries the mark");
  });

  /// Closing the thread you are in and landing on the new-chat page is the row
  /// at the top, so the row says what it closes and the list says which thread
  /// that is: without the mark, "close this thread" names nothing.
  it("spells out closing the open thread, and marks it in the listing", () => {
    const dialog = sessionDialog(sessions, "fe0031b1");
    assert.equal(dialog.rows[0].label, "New chat");
    assert.equal(dialog.rows[0].detail, "Close this thread and start a fresh one");
    assert.equal(dialog.rows[2].status, "Current");
    // The age it reported still describes the thread, so the mark replaces the
    // row's age rather than its message count.
    assert.equal(dialog.rows[2].detail, "195 messages");
    assert.equal(dialog.rows[3].status, "7m ago", "every other row keeps its age");
    // A thread resumed from a listing that has since been renamed, or one the
    // CLI no longer lists, leaves no row marked rather than one marked wrongly.
    assert.ok(
      sessionDialog(sessions, "deadbeef").rows.every((row) => row.status !== "Current"),
    );
  });

  it("names a session the way the terminal's picker does", () => {
    const dialog = sessionDialog(sessions, null);
    assert.equal(dialog.rows[2].label, "Create VS Code Extension for Oxide");
    assert.equal(dialog.rows[2].status, "just now");
    assert.equal(dialog.rows[2].detail, "195 messages");
    assert.equal(dialog.rows[3].detail, "2 messages");
  });

  it("falls back to the id for a session that was never named", () => {
    const dialog = sessionDialog(parseSessionList("abc123  2d ago   1 msg  \n"), null);
    assert.equal(dialog.rows[2].label, "abc123");
    assert.equal(dialog.rows[2].detail, "1 message");
  });

  it("says so when the project has no sessions yet", () => {
    const dialog = sessionDialog([], null);
    assert.equal(dialog.rows.length, 2, "the ways to leave are always there");
    assert.match(dialog.note, /No sessions for this project yet/);
  });

  it("shows the listing in progress rather than an empty list", () => {
    const dialog = sessionDialog(sessions, null, "Loading sessions…");
    assert.equal(dialog.note, "Loading sessions…");
    assert.equal(dialog.rows.length, 4, "the rows of the last answer stay up");
  });

  /// Every redraw carries the open thread's id, not just the first one: a listing
  /// rebuilt to report a failure is still the listing of the thread you are in,
  /// so the note rides over a list that still marks it. That is why the argument
  /// is required here and supplied in one place (`chat.ts::showSessions`) — a
  /// rebuild that left it out silently turned the first row back into one that
  /// promises a fresh thread.
  it("keeps the mark on a rebuild that only adds a note", () => {
    const dialog = sessionDialog(sessions, "fe0031b1", "Could not delete say hi: exit 1");
    assert.equal(dialog.note, "Could not delete say hi: exit 1");
    assert.equal(dialog.rows[0].detail, "Close this thread and start a fresh one");
    assert.equal(dialog.rows[2].status, "Current");
  });

  /// A session file is written as its first turn runs, so the listing can be read
  /// before the row it belongs to exists — the store is what catches up to the
  /// thread, not the other way round. The thread the panel is in stands in for
  /// itself until then, so the list never shows a thread you are in as one you
  /// are not.
  it("lists the thread the panel is in before the store has a row for it", () => {
    const dialog = sessionDialog([], "9f00c0de", "", { id: "9f00c0de", label: "Fix the flaky test" });
    assert.deepEqual(
      dialog.rows.map((row) => row.value),
      [NEW_SESSION, CONTINUE_SESSION, "9f00c0de"],
    );
    const live = dialog.rows[2];
    assert.equal(live.label, "Fix the flaky test");
    assert.equal(live.status, "Current");
    assert.match(live.detail, /the store has no file for it yet/);
    assert.equal(live.button, "", "there is no file to delete yet");
    // A listing with a thread in it is not an empty one, so it says nothing
    // about the next message starting the project's first thread.
    assert.equal(dialog.note, "");
  });

  it("leaves a live thread to its own row once the store has one", () => {
    const stored = parseSessionList("fe0031b1  just now  2 msg  Fix the flaky test\n");
    const dialog = sessionDialog(stored, "fe0031b1", "", {
      id: "fe0031b1",
      label: "Fix the flaky test",
    });
    assert.deepEqual(
      dialog.rows.map((row) => row.value),
      [NEW_SESSION, CONTINUE_SESSION, "fe0031b1"],
      "the row is the store's, listed once",
    );
    assert.equal(dialog.rows[2].buttonAction, SESSION_DELETE, "and it can be deleted");
  });

  it("names an unnamed row after the thread the panel is showing", () => {
    // The store lists a thread whose first message it has no name for; the
    // panel's own header already has one, so the row reads like the header
    // rather than as a bare id.
    const stored = parseSessionList("fe0031b1  just now  2 msg  \n");
    const dialog = sessionDialog(stored, "fe0031b1", "", {
      id: "fe0031b1",
      label: "Fix the flaky test",
    });
    assert.equal(dialog.rows[2].label, "Fix the flaky test");
    // A live thread for another id is not this listing's business, so nothing
    // of it reaches the rows.
    const other = sessionDialog(stored, "7c8031b1", "", { id: "9f00c0de", label: "elsewhere" });
    assert.ok(other.rows.every((row) => row.label !== "elsewhere"));
  });
});

describe("delete-thread confirmation", () => {
  const thread = {
    id: "fe0031b1",
    label: "Create VS Code Extension for Oxide",
    detail: "just now · 195 messages · fe0031b1",
  };

  it("names the thread, and puts the deletion in a row of its own", () => {
    const dialog = deleteSessionDialog(thread);
    assert.equal(dialog.title, "Delete thread");
    // It replaces the session listing it was opened from, so it stays where
    // that one was rather than jumping to the other end of the panel.
    assert.equal(dialog.pin, "header");
    assert.match(dialog.subtitle, /Create VS Code Extension for Oxide/);
    assert.match(dialog.note, /cannot be undone/);
    assert.deepEqual(
      dialog.rows.map((row) => row.label),
      ["Delete thread", "Cancel"],
    );
    assert.deepEqual(
      dialog.rows.map((row) => row.action),
      [SESSION_DELETE_CONFIRM, CLOSE_DIALOG],
    );
    // The row carries the id the deletion acts on, and reads as destructive.
    assert.equal(dialog.rows[0].value, "fe0031b1");
    assert.equal(dialog.rows[0].tone, "error");
    assert.equal(dialog.rows[0].detail, thread.detail);
    assert.equal(dialog.refreshLabel, "", "the listing is repainted, not rechecked");
  });

  it("falls back to the id for a thread that was never named", () => {
    const dialog = deleteSessionDialog({ id: "abc123", label: "", detail: "" });
    assert.match(dialog.subtitle, /abc123/);
  });
});
