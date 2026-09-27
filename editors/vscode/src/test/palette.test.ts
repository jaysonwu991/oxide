// The `/` palette's rules: what the CLI's catalog parses to, which rows a
// slash command offers, and which messages the panel answers itself.

import assert from "node:assert/strict";
import { describe, it } from "node:test";

import {
  MAX_COMMAND_ROWS,
  commandQuery,
  commandRows,
  paletteCommands,
  panelCommand,
  parseCommandList,
  routeCommand,
  type CommandEntry,
} from "../core/palette";

/// The catalog `oxide commands --json` prints, reduced to what the tests use.
const CATALOG = `[
  {
    "name": "model",
    "aliases": [],
    "description": "Choose the model to run",
    "arguments": null,
    "kind": "client",
    "desktop_only": false,
    "source": "builtin"
  },
  {
    "name": "mcp",
    "aliases": ["mcps"],
    "description": "List MCP servers",
    "arguments": null,
    "kind": "client",
    "desktop_only": false,
    "source": "builtin"
  },
  {
    "name": "permissions",
    "aliases": ["approvals"],
    "description": "Review the tools allowed without prompting",
    "arguments": "on|off|list|clear",
    "kind": "client",
    "desktop_only": false,
    "source": "builtin"
  },
  {
    "name": "theme",
    "aliases": [],
    "description": "Choose the color theme",
    "arguments": null,
    "kind": "client",
    "desktop_only": true,
    "source": "builtin"
  },
  {
    "name": "build",
    "aliases": [],
    "description": "Build the project",
    "arguments": "arguments",
    "kind": "prompt",
    "desktop_only": false,
    "source": "project"
  },
  {
    "name": "oxide-architecture",
    "aliases": [],
    "description": "Use when navigating the oxide internals",
    "arguments": "[arguments]",
    "kind": "skill",
    "desktop_only": false,
    "source": "project"
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
      "theme",
      "build",
      "oxide-architecture",
    ]);
    assert.deepEqual(entries[2].aliases, ["approvals"]);
    assert.equal(entries[0].arguments, "");
    assert.equal(entries[2].arguments, "on|off|list|clear");
    assert.equal(entries[3].desktopOnly, true);
    assert.equal(entries[5].kind, "skill");
    assert.equal(entries[5].source, "project");
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
        aliases: [],
        description: "",
        arguments: "",
        kind: "prompt",
        source: "",
        desktopOnly: false,
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

  it("offers the configured commands and skills, and the built-ins it can run", () => {
    const query = paletteCommands(catalog(), "");
    // A command the panel has an action for is offered; one it has none for is
    // not, since taking its row would send text the CLI hands the model.
    assert.deepEqual(names(query), ["model", "mcp", "build", "oxide-architecture"]);
  });

  it("matches an alias as well as a name", () => {
    assert.deepEqual(names(paletteCommands(catalog(), "mcps")), ["mcp"]);
    // An alias of a command the panel cannot run is no help: the row is still
    // one it has no action for.
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
      aliases: [],
      description: "",
      arguments: "",
      kind: "prompt",
      source: "project",
      desktopOnly: false,
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
  it("resolves a built-in by name and alias", () => {
    assert.equal(panelCommand("model"), "model");
    assert.equal(panelCommand("/MCPS"), "mcp");
    assert.equal(panelCommand("thinking"), "reasoning");
    assert.equal(panelCommand("access"), "trust");
    assert.equal(panelCommand("usage"), "usage");
    assert.equal(panelCommand("cost"), "usage");
    assert.equal(panelCommand("permissions"), null);
    assert.equal(panelCommand("theme"), null);
    assert.equal(panelCommand("ship"), null);
  });

  it("routes a built-in to the panel and refuses one it cannot run", () => {
    const entries = catalog();
    assert.deepEqual(routeCommand(entries, "/model"), { kind: "action", action: "model" });
    assert.deepEqual(routeCommand(entries, "/mcps"), { kind: "action", action: "mcp" });
    // A client command with no action here names the command it refused, so the
    // notice can say which one it was.
    assert.deepEqual(routeCommand(entries, "/approvals"), {
      kind: "refused",
      name: "permissions",
    });
    // The desktop's own commands are refused too: the panel cannot pick a theme
    // or sign in to a provider.
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
