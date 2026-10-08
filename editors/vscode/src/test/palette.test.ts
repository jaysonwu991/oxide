// The `/` palette's rules: what the CLI's catalog parses to, which rows a
// slash command offers, and which messages the panel answers itself.

import assert from "node:assert/strict";
import { describe, it } from "node:test";

import {
  MAX_COMMAND_ROWS,
  commandQuery,
  commandRows,
  paletteCommands,
  canonicalCommand,
  panelCommand,
  parseCommandList,
  routeCommand,
  type CommandEntry,
} from "../core/palette";

/// The catalog `oxide commands --json` prints, reduced to what the tests use.
const CATALOG = `[
  {
    "name": "model",
    "description": "Choose the model to run",
    "arguments": null,
    "kind": "client",
    "desktop_only": false,
    "source": "builtin",
    "front_ends": ["terminal", "desktop", "panel"]
  },
  {
    "name": "mcp",
    "description": "List MCP servers",
    "arguments": null,
    "kind": "client",
    "desktop_only": false,
    "source": "builtin",
    "front_ends": ["terminal", "desktop", "panel"]
  },
  {
    "name": "permissions",
    "description": "Review the tools allowed without prompting",
    "arguments": "on|off|list|clear",
    "kind": "client",
    "desktop_only": false,
    "source": "builtin",
    "front_ends": ["terminal", "desktop", "panel"]
  },
  {
    "name": "logout",
    "description": "Forget a provider's stored credentials",
    "arguments": "provider",
    "kind": "client",
    "desktop_only": false,
    "source": "builtin",
    "front_ends": ["terminal", "desktop", "panel"]
  },
  {
    "name": "theme",
    "description": "Choose the color theme",
    "arguments": null,
    "kind": "client",
    "desktop_only": true,
    "source": "builtin",
    "front_ends": ["terminal", "desktop"]
  },
  {
    "name": "connect",
    "description": "Sign in to a provider",
    "arguments": "provider",
    "kind": "client",
    "desktop_only": false,
    "source": "builtin",
    "front_ends": ["terminal", "desktop", "panel"]
  },
  {
    "name": "build",
    "description": "Build the project",
    "arguments": "arguments",
    "kind": "prompt",
    "desktop_only": false,
    "source": "project",
    "front_ends": ["terminal", "desktop", "panel"]
  },
  {
    "name": "oxide-architecture",
    "description": "Use when navigating the oxide internals",
    "arguments": "[arguments]",
    "kind": "skill",
    "desktop_only": false,
    "source": "project",
    "front_ends": ["terminal", "desktop", "panel"]
  },
  {
    "name": "session",
    "description": "Resume a saved conversation",
    "arguments": "id",
    "kind": "client",
    "desktop_only": false,
    "source": "builtin",
    "front_ends": ["terminal", "desktop", "panel"]
  }
]`;

function catalog(): CommandEntry[] {
  return parseCommandList(CATALOG);
}

function names(entries: readonly CommandEntry[]): string[] {
  return entries.map((entry) => entry.name);
}

describe("command catalog", () => {
  it("parses the CLI's listing, with an absent argument hint as none", () => {
    const entries = catalog();
    assert.deepEqual(names(entries), [
      "model",
      "mcp",
      "permissions",
      "logout",
      "theme",
      "connect",
      "build",
      "oxide-architecture",
      "session",
    ]);
    assert.equal(entries[0].arguments, "");
    assert.equal(entries[2].arguments, "on|off|list|clear");
    assert.equal(entries[4].desktopOnly, true);
    assert.equal(entries[5].desktopOnly, false, "a provider login is not the desktop's alone");
    // The front-ends a command is offered to: the theme picker is not this
    // panel's, and a configured command is sent as a prompt by every client.
    assert.deepEqual(entries[4].frontEnds, ["terminal", "desktop"]);
    assert.deepEqual(entries[2].frontEnds, ["terminal", "desktop", "panel"]);
    assert.deepEqual(entries[7].frontEnds, ["terminal", "desktop", "panel"]);
    assert.equal(entries[7].kind, "skill");
    assert.equal(entries[7].source, "project");
  });

  it("yields nothing for a CLI that prints something other than the listing", () => {
    // An older CLI answers with its human table, and a broken one with an error;
    // neither is worth throwing over mid-keystroke.
    assert.deepEqual(parseCommandList("Commands (5):\n  /help  ..."), []);
    assert.deepEqual(parseCommandList(""), []);
    assert.deepEqual(parseCommandList("{}"), []);
    // A partial entry without a name is dropped, not listed blank.
    assert.deepEqual(parseCommandList('[{"description":"no name"},{"name":"ok"}]'), [
      {
        name: "ok",
        description: "",
        arguments: "",
        kind: "prompt",
        source: "",
        desktopOnly: false,
        frontEnds: [],
      },
    ]);
  });
});

describe("palette queries", () => {
  it("offers rows only for a bare name being typed", () => {
    assert.equal(commandQuery("/"), "");
    assert.equal(commandQuery("/ox"), "ox");
    assert.equal(commandQuery("/Model"), "Model");
    assert.equal(commandQuery("/model claude"), null);
    assert.equal(commandQuery("plain text"), null);
    assert.equal(commandQuery("mail me at a/b"), null);
  });

  it("falls back to the field a CLI too old for `front_ends` prints", () => {
    // The extension is released on its own, so the CLI it drives may be one that
    // never heard of `front_ends`: `desktop_only` is what that CLI said, and a
    // built-in this panel has no action for is still no row.
    const older = parseCommandList(
      '[{"name":"theme","kind":"client","desktop_only":true},' +
        '{"name":"agent","kind":"client","desktop_only":false},' +
        '{"name":"model","kind":"client","desktop_only":false},' +
        '{"name":"permissions","kind":"client","desktop_only":true}]',
    );
    assert.deepEqual(older.map((entry) => entry.frontEnds), [[], [], [], []]);
    assert.deepEqual(names(paletteCommands(older, "")), ["agent", "model"]);
  });

  it("offers the configured commands and skills, and the built-ins it can run", () => {
    const query = paletteCommands(catalog(), "");
    // The catalog names the front-ends that perform a command, and this panel is
    // one of them for these; `/theme` is the desktop app's, so it is not a row
    // here — taking one would send text the CLI hands the model.
    assert.deepEqual(names(query), [
      "model",
      "mcp",
      "permissions",
      "logout",
      "connect",
      "build",
      "oxide-architecture",
      "session",
    ]);
  });

  it("matches the catalog's name and no retired spelling of it", () => {
    assert.deepEqual(names(paletteCommands(catalog(), "mc")), ["mcp"]);
    // A spelling the catalog does not declare is not a row: a retired one names
    // no command here, and the command it used to reach is not one this panel
    // could run anyway.
    assert.deepEqual(names(paletteCommands(catalog(), "mcps")), []);
    assert.deepEqual(names(paletteCommands(catalog(), "appr")), []);
  });

  it("ranks a name that starts with the query before one that mentions it", () => {
    const entries = parseCommandList(
      '[{"name":"rebuild","kind":"prompt"},{"name":"build","kind":"prompt"}]',
    );
    assert.deepEqual(names(paletteCommands(entries, "build")), ["build", "rebuild"]);
    assert.deepEqual(names(paletteCommands(entries, "x")), []);
  });

  it("caps how many rows a bare slash offers", () => {
    const many = Array.from({ length: MAX_COMMAND_ROWS + 25 }, (_, index) => ({
      name: `command-${index}`,
      description: "",
      arguments: "",
      kind: "prompt",
      source: "project",
      desktopOnly: false,
      frontEnds: [],
    }));
    assert.equal(paletteCommands(many, "").length, MAX_COMMAND_ROWS);
  });
});

describe("palette rows", () => {
  it("replaces the whole value with the command, ready for its arguments", () => {
    const answer = commandRows(catalog(), "/ox");
    assert.ok(answer);
    assert.deepEqual({ start: answer.start, end: answer.end }, { start: 0, end: 3 });
    assert.deepEqual(answer.rows, [
      {
        name: "oxide-architecture",
        insert: "/oxide-architecture ",
        arguments: "[arguments]",
        description: "Use when navigating the oxide internals",
        kind: "skill",
        source: "project",
      },
    ]);
  });

  it("leaves a command without arguments to be sent as it is", () => {
    // A client entry always takes its arguments elsewhere, so only a configured
    // command whose body uses none is sent bare.
    const entries = parseCommandList('[{"name":"ship","kind":"prompt","description":"Ship"}]');
    const answer = commandRows(entries, "/ship");
    assert.deepEqual(answer?.rows, [
      {
        name: "ship",
        insert: "/ship",
        arguments: "",
        description: "Ship",
        kind: "prompt",
        source: "",
      },
    ]);
  });

  it("offers nothing for a value that is not a slash command", () => {
    assert.equal(commandRows(catalog(), "read the file"), null);
    assert.equal(commandRows(catalog(), "/model claude"), null);
  });
});

describe("panel commands", () => {
  it("resolves a built-in by its catalog name", () => {
    assert.equal(panelCommand("model"), "model");
    assert.equal(panelCommand("/MCP"), "mcp");
    assert.equal(panelCommand("reasoning"), "reasoning");
    assert.equal(panelCommand("trust"), "trust");
    assert.equal(panelCommand("connect"), "provider");
    assert.equal(panelCommand("/CONNECT"), "provider");
    assert.equal(panelCommand("usage"), "usage");
    assert.equal(panelCommand("permissions"), "permissions");
    assert.equal(panelCommand("/LOGOUT"), "logout");
    assert.equal(panelCommand("theme"), null);
    assert.equal(panelCommand("ship"), null);
    // One spelling per command: a retired spelling names nothing here, and a
    // name this switch does not hold is not the panel's to answer.
    assert.equal(panelCommand("mcps"), null);
    assert.equal(panelCommand("login"), null);
  });

  it("resolves a built-in to the name the catalog files it under", () => {
    const entries = catalog();
    assert.equal(canonicalCommand("mcp", entries), "mcp");
    assert.equal(canonicalCommand("/CONNECT", entries), "connect");
    assert.equal(canonicalCommand("/Session", entries), "session");
    // A configured command is not the catalog's to rename, and a name nobody
    // declares is left as it was typed — including a retired spelling of a
    // command, which is a message now rather than a second name for one.
    assert.equal(canonicalCommand("build", entries), "build");
    assert.equal(canonicalCommand("ship", entries), "ship");
    assert.equal(canonicalCommand("mcps", entries), "mcps");
    assert.equal(canonicalCommand("resume", entries), "resume");
    // The catalog holds one spelling per row: a second one it does not declare
    // is not invented here from the name it looks like.
    const reasoning = parseCommandList('[{"name":"reasoning","kind":"client"}]');
    assert.equal(canonicalCommand("/Reasoning", reasoning), "reasoning");
    assert.equal(canonicalCommand("thinking", reasoning), "thinking");
  });

  it("routes a built-in to the panel and refuses one it cannot run", () => {
    const entries = catalog();
    assert.deepEqual(routeCommand(entries, "/model"), { kind: "action", action: "model" });
    assert.deepEqual(routeCommand(entries, "/mcp"), { kind: "action", action: "mcp" });
    assert.deepEqual(routeCommand(entries, "/session"), { kind: "action", action: "session" });
    // The rules an `Always allow` saved, and the providers this machine holds a
    // credential for, are the panel's own listings now: the catalog names it as
    // one of the front-ends that runs them, so nothing has to leave the editor.
    assert.deepEqual(routeCommand(entries, "/permissions"), {
      kind: "action",
      action: "permissions",
    });
    assert.deepEqual(routeCommand(entries, "/logout"), {
      kind: "action",
      action: "logout",
    });
    assert.deepEqual(routeCommand(entries, "  /SESSION  "), { kind: "action", action: "session" });
    assert.equal(routeCommand(entries, "/mcp list"), null, "arguments are the agent's");
    assert.equal(routeCommand(entries, "/session fe0031b1"), null);
    assert.equal(routeCommand(entries, "mcp"), null, "the slash is what makes it a command");
    assert.equal(routeCommand(entries, "MCP: how do servers authenticate?"), null);
    // Signing in to a provider is the CLI's own `connect`, performed by the
    // panel's own dialog rather than sent on as a prompt.
    assert.deepEqual(routeCommand(entries, "/connect"), { kind: "action", action: "provider" });
    // Arguments are the agent's, so `/connect anthropic` stays a message: the
    // command itself is what the panel performs.
    assert.equal(routeCommand(entries, "/connect anthropic"), null);
    // A retired spelling is nobody's command: with nothing in the catalog to
    // refuse, it is a message the CLI expands like any other.
    assert.equal(routeCommand(entries, "/mcps"), null);
    assert.equal(routeCommand(entries, "/approvals"), null);
    // The desktop's own command is refused: the panel cannot pick a theme.
    assert.deepEqual(routeCommand(entries, "/theme"), { kind: "refused", name: "theme" });
  });

  it("leaves a message the CLI expands alone", () => {
    const entries = catalog();
    // A configured command and a skill are the CLI's: sending `/name` is what
    // expands a template and loads a skill's instructions.
    assert.equal(routeCommand(entries, "/build"), null);
    assert.equal(routeCommand(entries, "/oxide-architecture"), null);
    // Arguments are the agent's, as they are in the terminal.
    assert.equal(routeCommand(entries, "/session auth is broken"), null);
    assert.equal(routeCommand(entries, "plain text"), null);
    assert.equal(routeCommand(entries, "/"), null);
    // A skill the CLI has by another name is not a client command either.
    assert.equal(routeCommand(entries, "/skill:oxide-architecture"), null);
  });
});
