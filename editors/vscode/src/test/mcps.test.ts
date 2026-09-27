// The `/mcps` dialog reads the CLI's own JSON listing, so what it says about a
// server has to survive a CLI that answers with a human table, a partial entry
// or a state this build has not seen.

import assert from "node:assert/strict";
import * as fs from "node:fs";
import * as path from "node:path";
import { describe, it } from "node:test";

import {
  isMcpCommand,
  mcpListArgs,
  mcpStateLabel,
  mcpToggleArgs,
  parseMcpList,
} from "../core/mcps";

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
]);

describe("MCP server listing", () => {
  it("parses the JSON listing in name order", () => {
    const servers = parseMcpList(listing);
    assert.deepEqual(
      servers.map((server) => server.name),
      ["context7", "sentry"],
    );
    assert.equal(servers[0].transport, "http");
    assert.equal(servers[0].scope, "global");
    assert.equal(servers[1].enabled, false);
    assert.equal(servers[1].state, "disabled");
  });

  it("ignores output that is not the JSON listing", () => {
    assert.deepEqual(parseMcpList(""), []);
    assert.deepEqual(parseMcpList("no MCP servers configured"), []);
    assert.deepEqual(parseMcpList('{"name":"one"}'), []);
    assert.deepEqual(parseMcpList("[1,2,3]"), []);
  });

  it("keeps a partial entry readable", () => {
    const servers = parseMcpList('[{"name":"bare"}]');
    assert.equal(servers.length, 1);
    assert.equal(servers[0].enabled, true);
    assert.equal(servers[0].scope, "project");
    assert.deepEqual(parseMcpList('[{"transport":"stdio"}]'), []);
  });

  it("names each state the core can report", () => {
    assert.equal(mcpStateLabel("connected"), "Connected");
    assert.equal(mcpStateLabel("needs-auth"), "Needs auth");
    assert.equal(mcpStateLabel("needs-trust"), "Needs trust");
    assert.equal(mcpStateLabel("disabled"), "Disabled");
    assert.equal(mcpStateLabel("error"), "Error");
    assert.equal(mcpStateLabel("something-new"), "something-new");
  });

  it("pins a toggle to the scope that defines the server", () => {
    const [context7, sentry] = parseMcpList(listing);
    assert.deepEqual(mcpToggleArgs(sentry, true), [
      "mcp",
      "enable",
      "sentry",
      "--scope",
      "project",
    ]);
    assert.deepEqual(mcpToggleArgs(context7, false), [
      "mcp",
      "disable",
      "context7",
      "--scope",
      "global",
    ]);
    assert.deepEqual(mcpListArgs(), ["mcp", "list", "--json"]);
  });

  it("answers only the bare slash command", () => {
    assert.ok(isMcpCommand("/mcps"));
    assert.ok(isMcpCommand("  /MCP  "));
    assert.ok(!isMcpCommand("/mcp list"));
    assert.ok(!isMcpCommand("show me /mcps"));
    assert.ok(!isMcpCommand(""));
    // The slash is what makes it a command: plain text about MCP reaches the
    // model instead of opening the picker.
    assert.ok(!isMcpCommand("mcp"));
    assert.ok(!isMcpCommand("mcps"));
    assert.ok(!isMcpCommand("MCP: how do servers authenticate?"));
  });

  it("requires a workspace folder before listing servers", () => {
    const root = path.join(__dirname, "..", "..");
    const chat = fs.readFileSync(path.join(root, "src", "chat.ts"), "utf8");
    const start = chat.indexOf("async showMcps(");
    const end = chat.indexOf("\n  async ", start + 1);
    const showMcps = chat.slice(start, end > 0 ? end : undefined);
    // The listing and a toggle write into the files that define a project's
    // servers, so a window with no folder refuses instead of acting on the
    // directory the extension host happens to run in.
    assert.ok(showMcps.includes("const cwd = this.cwd();"), "resolves the workspace folder");
    assert.ok(showMcps.includes('showNotice("Open a folder first."'), "says so when there is none");
    assert.ok(!showMcps.includes("process.cwd()"), "uses no fallback directory");
  });

  // The controller imports `vscode`, so it cannot be loaded here; the decision
  // it consults is. A composer line that reaches the prompt path instead would
  // send `/mcps` to the model as text.
  it("is consulted before the message is prompted", () => {
    const root = path.join(__dirname, "..", "..");
    const chat = fs.readFileSync(path.join(root, "src", "chat.ts"), "utf8");
    const send = chat.slice(chat.indexOf("async send("), chat.indexOf("private ", chat.indexOf("async send(")));
    assert.ok(send.includes("isMcpCommand(message)"), "send consults the command");
    assert.ok(
      send.indexOf("isMcpCommand(message)") < send.indexOf("this.queue.push"),
      "the command is answered before a message is queued or prompted",
    );
  });
});
