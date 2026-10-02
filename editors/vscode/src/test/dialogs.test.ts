// The dialogs the panel paints are composed here as data, so what a row says —
// a setting's current choice, a server's state and scope, a session's age and
// size, what a deletion would remove — is assertable without a webview or CLI.

import assert from "node:assert/strict";
import { describe, it } from "node:test";

import {
  agentDialog,
  APPLY_AGENT,
  APPLY_MODEL,
  APPLY_REASONING,
  APPLY_TRUST,
  CHANGES_UNDO_CONFIRM,
  CLOSE_DIALOG,
  CONTINUE_SESSION,
  deleteSessionDialog,
  MCP_TOGGLE,
  modelDialog,
  NEW_SESSION,
  reasoningDialog,
  mcpDialog,
  OPEN_SESSION,
  SESSION_DELETE,
  SESSION_DELETE_CONFIRM,
  sessionDialog,
  trustDialog,
  undoChangesDialog,
  UPDATE_INSTALL,
  UPDATE_NOTES,
  updateDialog,
} from "../core/dialogs";
import { parseMcpList } from "../core/mcps";
import { parseSessionList } from "../core/sessions";
import type { UpdateCheck } from "../core/updates";

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

describe("settings dialogs", () => {
  it("keeps model selection in the composer and accepts a custom model ID", () => {
    const dialog = modelDialog(
      "claude-opus-5",
      "portkey",
      "claude-opus-5",
      [
        { model: "claude-opus-5", provider: "portkey" },
        { model: "glm-5", provider: "zai" },
      ],
      "gpt-6",
    );
    assert.equal(dialog.kind, "model");
    assert.equal(dialog.pin, "footer");
    assert.equal(dialog.search, true);
    assert.equal(dialog.searchPlaceholder, "Filter or enter a model ID…");
    assert.deepEqual(dialog.rows.map((entry) => entry.label), ["Use “gpt-6”"]);
    assert.equal(dialog.rows[0].value, "gpt-6");
    assert.equal(dialog.rows[0].action, APPLY_MODEL);
  });

  it("marks the active remembered model and the config fallback", () => {
    const active = modelDialog(
      "claude-opus-5",
      "portkey",
      "glm-5",
      [
        { model: "claude-opus-5", provider: "portkey" },
        { model: "glm-5", provider: "zai" },
      ],
    );
    assert.equal(active.rows[0].label, "Oxide config default");
    assert.match(active.rows[0].detail, /claude-opus-5/);
    assert.equal(active.rows[0].status, "");
    assert.equal(active.rows[1].status, "portkey");
    assert.equal(active.rows[2].status, "Current");
    assert.equal(active.count, 2);
    assert.ok(active.rows.every((entry) => entry.action === APPLY_MODEL));

    const configured = modelDialog("claude-opus-5", "portkey", "", []);
    assert.equal(configured.rows[0].status, "Current");
  });

  it("searches the config fallback by its complete visible label", () => {
    for (const query of ["oxide", "default", "config"]) {
      const dialog = modelDialog("claude-opus-5", "portkey", "", [], query);
      assert.equal(dialog.rows[0].label, "Oxide config default");
      assert.equal(dialog.rows[0].value, "");
    }
  });

  it("filters agents and turns an unknown name into a selectable row", () => {
    const dialog = agentDialog(
      [
        { name: "reviewer", description: "Reviews a change" },
        { name: "builder", description: "Implements a change" },
      ],
      "reviewer",
      "security",
    );
    assert.equal(dialog.kind, "agent");
    assert.equal(dialog.pin, "footer");
    assert.equal(dialog.searchPlaceholder, "Filter or enter an agent name…");
    assert.deepEqual(dialog.rows.map((entry) => entry.label), ["Use “security”"]);
    assert.equal(dialog.rows[0].action, APPLY_AGENT);
  });

  it("offers reasoning and project access without opening VS Code chrome", () => {
    const reasoning = reasoningDialog("medium", ["auto", "off", "medium", "high"]);
    assert.equal(reasoning.pin, "footer");
    assert.equal(reasoning.rows.find((entry) => entry.value === "medium")?.status, "Current");
    assert.ok(reasoning.rows.every((entry) => entry.action === APPLY_REASONING));

    const trust = trustDialog("always");
    assert.equal(trust.pin, "footer");
    assert.deepEqual(trust.rows.map((entry) => entry.value), ["default", "always", "never"]);
    assert.equal(trust.rows[1].status, "Current");
    assert.ok(trust.rows.every((entry) => entry.action === APPLY_TRUST));
  });
});

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

  it("counts what it lists, and is not a listing to search", () => {
    const dialog = mcpDialog(parseMcpList(listing));
    assert.equal(dialog.count, 3, "the head says how many servers are listed");
    assert.equal(dialog.search, false, "a handful of servers needs no search box");
    assert.equal(dialog.query, "");
    assert.ok(
      dialog.rows.every((row) => row.kind === ""),
      "a server row is an ordinary one",
    );
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

  /// The listing is the one with a box to filter it: a project can hold
  /// hundreds of threads, and the rows that are the things being listed are
  /// told apart from the rows that are ways out of the list, so the panel can
  /// paint the two shapes without reading their words.
  it("is a listing to search, and counts what it lists", () => {
    const dialog = sessionDialog(sessions, "fe0031b1");
    assert.equal(dialog.search, true);
    assert.equal(dialog.count, 2, "the threads, not the two ways out of them");
    assert.equal(dialog.query, "", "nothing is filtered until it is asked for");
    assert.deepEqual(
      dialog.rows.map((row) => row.kind),
      ["action", "action", "thread", "thread"],
    );
    // The thread on screen stands in for itself, and is a thread row too.
    const live = sessionDialog([], "9f00c0de", "", { id: "9f00c0de", label: "Fix it" });
    assert.equal(live.count, 1);
    assert.deepEqual(
      live.rows.map((row) => row.kind),
      ["action", "action", "thread"],
    );
  });

  /// The search box is the host's filter over the rows it already has: the
  /// listing is the store's own answer, so typing narrows it without a second
  /// read, and what is left is counted the same way an unfiltered listing is.
  it("narrows its rows to the query, and says so when nothing is left", () => {
    const filtered = sessionDialog(sessions, null, "", null, "say");
    assert.deepEqual(
      filtered.rows.map((row) => row.value),
      [NEW_SESSION, CONTINUE_SESSION, "7c8031b1"],
    );
    assert.equal(filtered.count, 1, "the count is what the filter left");
    assert.equal(filtered.query, "say", "echoed, so a redraw keeps the box");
    assert.equal(filtered.note, "");
    // The ways out of the listing survive every search: they are what you can do
    // with the list, not something in it.
    assert.equal(filtered.rows[0].kind, "action");

    const nothing = sessionDialog(sessions, null, "", null, "nope");
    assert.deepEqual(
      nothing.rows.map((row) => row.value),
      [NEW_SESSION, CONTINUE_SESSION],
    );
    assert.equal(nothing.count, 0);
    assert.equal(nothing.note, "No thread matches “nope”.");

    // A blank query is not a query, and an empty listing keeps saying what it
    // says rather than blaming a search nobody made.
    assert.equal(sessionDialog(sessions, null, "", null, "  ").count, 2);
    assert.equal(sessionDialog([], null, "", null, "nope").count, 0);
    assert.match(sessionDialog([], null, "", null, "nope").note, /No sessions for this project yet/);
  });

  /// A search matches what the listing paints rather than what the store happened
  /// to hold: the thread on screen is named by the panel's own header while the
  /// store has no name for it, so a search for that name finds the row showing it
  /// — a row that vanishes from a search for its own title reads as a thread that
  /// is not there.
  it("filters the thread on screen under the title it is painted with", () => {
    const stored = parseSessionList("fe0031b1  just now  2 msg  \n");
    const live = { id: "fe0031b1", label: "Fix the flaky test" };
    const found = sessionDialog(stored, "fe0031b1", "", live, "flaky");
    assert.equal(found.count, 1);
    assert.equal(found.rows[2].label, "Fix the flaky test");
    assert.equal(found.rows[2].current, true, "and it is still the thread on screen");
    assert.equal(
      sessionDialog(stored, "fe0031b1", "", live, "nope").count,
      0,
      "a query the title does not match still leaves nothing",
    );
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

describe("undo-turn confirmation", () => {
  const card = { id: 4, detail: "Edited 3 files · +12 −4" };

  it("asks before it puts a turn back, with the restore in a row of its own", () => {
    const dialog = undoChangesDialog(card);
    assert.equal(dialog.title, "Undo turn");
    // It answers the card it was opened from, so it opens on that card's end of
    // the panel rather than the composer's.
    assert.equal(dialog.pin, "header");
    assert.match(dialog.subtitle, /back to how the run found them/);
    // What it would refuse, rather than a promise the CLI may not keep.
    assert.match(dialog.note, /refused rather than undone/);
    assert.deepEqual(
      dialog.rows.map((row) => row.label),
      ["Undo changes", "Cancel"],
    );
    assert.deepEqual(
      dialog.rows.map((row) => row.action),
      [CHANGES_UNDO_CONFIRM, CLOSE_DIALOG],
    );
    // The row carries the card the restore acts on, and is the second to it
    // rather than a second Undo: the card's own button only opens this.
    assert.equal(dialog.rows[0].value, "4");
    assert.equal(dialog.rows[0].tone, "warn");
    assert.equal(dialog.rows[0].detail, card.detail);
    assert.equal(dialog.refreshLabel, "");

    // Cancel keeps the turn, and is the one row that is not an action on it.
    const cancel = dialog.rows[1];
    assert.match(cancel.detail, /Keep/);
    assert.equal(cancel.tone, "");
    assert.equal(cancel.value, "");
  });
});

describe("update dialog", () => {
  const check: UpdateCheck = {
    current: "0.32.0",
    latest: "0.33.0",
    tag: "v0.33.0",
    pinned: false,
    updateAvailable: true,
    installation: "prebuilt binary",
    installable: true,
    path: "/home/me/.local/bin/oxide",
    advice: "Update available: run `oxide update` to install it.",
    releaseUrl: "https://github.com/jaysonwu991/oxide/releases/tag/v0.33.0",
  };

  it("offers the install the CLI's own check resolved", () => {
    const dialog = updateDialog({ k: "ready", check });
    assert.equal(dialog.kind, "update");
    // It hangs near the composer, where the panel's own commands open, rather
    // than covering the transcript it is not about.
    assert.equal(dialog.pin, "footer");
    assert.equal(dialog.title, "Oxide 0.33.0 is available");
    // Installing is the `oxide` command line — the binary the terminal, this
    // panel and the desktop app run — rather than the IDE's own bundle.
    assert.match(dialog.subtitle, /oxide command line/);
    assert.match(dialog.note, /Current 0\.32\.0/);
    assert.match(dialog.note, /Latest v0\.33\.0/);
    assert.match(dialog.note, /prebuilt binary/);
    assert.deepEqual(
      dialog.rows.map((row) => row.label),
      ["Install 0.33.0", "Release notes", "Close"],
    );
    assert.deepEqual(
      dialog.rows.map((row) => row.action),
      [UPDATE_INSTALL, UPDATE_NOTES, CLOSE_DIALOG],
    );
    // The row installs the release that was checked, and the release page its
    // own URL; neither is looked up again when the click arrives.
    assert.equal(dialog.rows[0].value, "0.33.0");
    assert.equal(dialog.rows[0].detail, "Replaces /home/me/.local/bin/oxide");
    assert.equal(dialog.rows[1].value, check.releaseUrl);
    assert.equal(dialog.refreshLabel, "", "rechecking is the command, not a row");
  });

  it("reports an installation the CLI leaves to something else", () => {
    // Homebrew's oxide is not the CLI's to replace, so there is no install row:
    // the CLI's own sentence says which command is.
    const brew: UpdateCheck = {
      ...check,
      installation: "homebrew",
      installable: false,
      advice: "Update available; Homebrew manages this install: run `brew upgrade oxide`.",
    };
    const dialog = updateDialog({ k: "ready", check: brew });
    assert.equal(dialog.title, "Oxide 0.33.0 is available");
    assert.deepEqual(
      dialog.rows.map((row) => row.action),
      [UPDATE_NOTES, CLOSE_DIALOG],
    );
    assert.match(dialog.note, /brew upgrade oxide/);
  });

  it("says a release is here when there is nothing newer", () => {
    const dialog = updateDialog({
      k: "ready",
      check: { ...check, current: "0.33.0", updateAvailable: false, advice: "" },
    });
    assert.equal(dialog.title, "Oxide is up to date");
    assert.match(dialog.subtitle, /0\.33\.0 is the newest released version/);
    assert.deepEqual(
      dialog.rows.map((row) => row.action),
      [UPDATE_NOTES, CLOSE_DIALOG],
    );
    // A check with no advice adds none to the line of facts.
    assert.equal(/Update available/.test(dialog.note), false);
  });

  it("says what a finished install put on disk", () => {
    // The re-check an install runs reports the same release the row offered, and
    // the headline is what says something happened rather than only what is
    // newest.
    const dialog = updateDialog({
      k: "ready",
      check: { ...check, current: "0.33.0", updateAvailable: false, advice: "" },
      headline: "Installed Oxide 0.33.0",
    });
    assert.equal(dialog.title, "Installed Oxide 0.33.0");
  });

  it("has something to say while the check runs and while the install does", () => {
    const checking = updateDialog({ k: "checking" });
    assert.equal(checking.title, "Checking for updates");
    assert.match(checking.note, /Asking GitHub/);
    assert.deepEqual(checking.rows, [], "there is nothing to press until there is an answer");

    const installing = updateDialog({ k: "installing", what: "Oxide 0.33.0" });
    assert.equal(installing.title, "Installing Oxide 0.33.0");
    assert.deepEqual(installing.rows, [], "a second install cannot be asked for mid-flight");
    assert.equal(installing.pin, "footer");
    // A CLI too old to be checked has no version to name, and the state that
    // says so is the same one the check's own install runs through.
    assert.equal(
      updateDialog({ k: "installing", what: "the newest CLI" }).title,
      "Installing the newest CLI",
    );
  });

  /// A CLI released before this check existed answers `unexpected argument
  /// '--json'`. There is no release to report then — but the plain `oxide update`
  /// the dialog offers is exactly what replaces that older binary, so the one
  /// thing that would make this a dead end (no install row) is the thing it has.
  it("offers the update that works on a CLI too old to be checked", () => {
    const dialog = updateDialog({
      k: "legacy",
      text: "error: unexpected argument '--json' found\n\nUsage: oxide update --check",
      path: "/Users/me/.local/bin/oxide",
    });
    assert.equal(dialog.title, "The oxide CLI is older than this panel");
    assert.match(dialog.subtitle, /works on any version/);
    // The CLI's own refusal, verbatim, under an offer that acts on it.
    assert.match(dialog.note, /unexpected argument '--json'/);
    assert.deepEqual(
      dialog.rows.map((row) => row.action),
      [UPDATE_INSTALL, CLOSE_DIALOG],
    );
    assert.equal(dialog.rows[0].label, "Install the newest CLI");
    assert.equal(dialog.rows[0].value, "", "there is no version to install by name");
    assert.equal(dialog.rows[0].detail, "Replaces /Users/me/.local/bin/oxide");
    // Nothing here is about a release, so there is nothing to open.
    assert.equal(
      dialog.rows.some((row) => row.action === UPDATE_NOTES),
      false,
    );
    // The same state read after an install ran holds that install's report
    // rather than a refusal, and says so instead of naming the older CLI again.
    const ran = updateDialog({
      k: "legacy",
      text: "Already up to date; rerun with --force to reinstall v0.33.0.",
      path: "/Users/me/.local/bin/oxide",
      headline: "Ran oxide update",
    });
    assert.equal(ran.title, "Ran oxide update");
    assert.match(ran.note, /Already up to date/);
    assert.deepEqual(
      ran.rows.map((row) => row.action),
      [UPDATE_INSTALL, CLOSE_DIALOG],
      "the row is still there, since a CLI that is not newest is what it is for",
    );
  });

  it("reports a failure with the CLI's own words, and a way out", () => {
    const failed = updateDialog({
      k: "failed",
      stage: "check",
      message: "error: could not reach GitHub: operation timed out",
    });
    assert.equal(failed.title, "Could not check for updates");
    assert.match(failed.subtitle, /no network/);
    // Verbatim, since the CLI's message is the one that says what went wrong.
    assert.equal(failed.note, "error: could not reach GitHub: operation timed out");
    assert.deepEqual(
      failed.rows.map((row) => row.action),
      [CLOSE_DIALOG],
    );

    // The same failure at the other stage reads as what it is: an install that
    // was asked for and did not land.
    const install = updateDialog({ k: "failed", stage: "install", message: "exit 1" });
    assert.equal(install.title, "Could not install the update");
    assert.match(install.subtitle, /release archive/);
    assert.equal(install.note, "exit 1");
  });
});
