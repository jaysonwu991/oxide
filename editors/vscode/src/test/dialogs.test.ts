// The two dialogs the panel paints are composed here as data, so what a row
// says — a server's state and the scope a toggle writes to, a session's age and
// size — is assertable without a webview or a CLI.

import assert from "node:assert/strict";
import { describe, it } from "node:test";

import {
  CONTINUE_SESSION,
  MCP_TOGGLE,
  NEW_SESSION,
  mcpDialog,
  OPEN_SESSION,
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
    assert.equal(sentry.button, "Enable");
    assert.equal(sentry.buttonAction, MCP_TOGGLE);
    assert.equal(sentry.detail, "stdio · npx -y @sentry/mcp · source: project");
    // A row opens nothing on its own: the button is the action.
    assert.equal(sentry.action, "");
  });

  it("offers Disable for a server that is on", () => {
    const rows = mcpDialog(parseMcpList(listing)).rows;
    assert.equal(rows.find((row) => row.value === "context7")!.button, "Disable");
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
    const dialog = sessionDialog(sessions);
    assert.equal(dialog.title, "Sessions");
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
    assert.deepEqual(
      dialog.rows.map((row) => row.button),
      ["", "", "", ""],
    );
  });

  it("names a session the way the terminal's picker does", () => {
    const dialog = sessionDialog(sessions);
    assert.equal(dialog.rows[2].label, "Create VS Code Extension for Oxide");
    assert.equal(dialog.rows[2].status, "just now");
    assert.equal(dialog.rows[2].detail, "195 messages");
    assert.equal(dialog.rows[3].detail, "2 messages");
  });

  it("falls back to the id for a session that was never named", () => {
    const dialog = sessionDialog(parseSessionList("abc123  2d ago   1 msg  \n"));
    assert.equal(dialog.rows[2].label, "abc123");
    assert.equal(dialog.rows[2].detail, "1 message");
  });

  it("says so when the project has no sessions yet", () => {
    const dialog = sessionDialog([]);
    assert.equal(dialog.rows.length, 2, "the ways to leave are always there");
    assert.match(dialog.note, /No sessions for this project yet/);
  });

  it("shows the listing in progress rather than an empty list", () => {
    assert.equal(sessionDialog(sessions, "Loading sessions…").note, "Loading sessions…");
  });
});
