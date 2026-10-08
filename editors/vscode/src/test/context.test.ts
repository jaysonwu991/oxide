import assert from "node:assert/strict";
import * as fs from "node:fs";
import * as path from "node:path";
import { describe, it } from "node:test";

import {
  DEFAULT_CONTEXT_WINDOW,
  MODEL_CONTEXT_WINDOWS,
  MODEL_VENDORS,
} from "../core/config";
import { contextArgs, parseContextWindow } from "../core/context";

/// The extension package (this file compiles to `out/test/`) and the repository
/// root above it. The rows below mirror a table in `oxide-core`, which the
/// package cannot link, so the test reaches out of it to read the Rust one.
const root = path.join(__dirname, "..", "..");
const repo = path.join(root, "..", "..");
const configRs = path.join(repo, "crates", "core", "src", "config.rs");

const source = fs.readFileSync(configRs, "utf8");

/// The rows of `const WINDOWS: &[(&str, u64)] = &[…]` in `builtin_context_window`,
/// in the order they are written.
function rustContextWindows(): [string, number][] {
  const body = source
    .split("const WINDOWS: &[(&str, u64)] = &[", 2)[1]
    ?.split("\n    ];", 1)[0];
  assert.ok(body, "builtin_context_window still declares its rows as WINDOWS");
  const rows: [string, number][] = [];
  for (const line of body.split("\n")) {
    const match = /^\s*\("([^"]+)",\s*([0-9_]+)\),\s*$/.exec(line);
    if (match) rows.push([match[1], Number(match[2].replace(/_/g, ""))]);
  }
  return rows;
}

/// The entries of `const VENDORS: [&str; N] = &[…]`.
function rustVendors(): string[] {
  const body = source
    .split("const VENDORS: [&str; 24] = [", 2)[1]
    ?.split("\n];", 1)[0];
  assert.ok(body, "VENDORS still declares the prefixes as a list");
  return [...body.matchAll(/"([^"]+)"/g)].map((match) => match[1]);
}

function rustDefaultContextWindow(): number {
  const match = /fn default_context_window\(\) -> u64 \{\s*([0-9_]+)\s*\}/.exec(source);
  assert.ok(match, "default_context_window still returns a literal");
  return Number(match[1].replace(/_/g, ""));
}

describe("the context window table", () => {
  it("carries every row of the CLI's own table, unchanged", () => {
    const rust = rustContextWindows();
    assert.ok(rust.length > 50, `${rust.length} rows were read out of config.rs`);
    assert.deepEqual(
      MODEL_CONTEXT_WINDOWS.map(([key, window]) => [key, window]),
      rust,
      "MODEL_CONTEXT_WINDOWS is crates/core/src/config.rs's WINDOWS, row for row",
    );
  });

  it("takes the CLI's last resort when the table knows no model", () => {
    assert.equal(DEFAULT_CONTEXT_WINDOW, rustDefaultContextWindow());
  });

  it("strips the same vendor and region prefixes the CLI strips", () => {
    assert.deepEqual(MODEL_VENDORS, rustVendors());
  });
});

describe("contextArgs", () => {
  it("asks for the active model's window", () => {
    assert.deepEqual(contextArgs(), ["context", "--json"]);
    assert.deepEqual(contextArgs("  "), ["context", "--json"]);
    // The panel's own `oxide.model` setting decides what a turn runs with, so it
    // decides whose window is reported.
    assert.deepEqual(contextArgs(" glm-5 "), ["context", "--json", "--model", "glm-5"]);
  });
});

describe("parseContextWindow", () => {
  it("reads the window the CLI resolved", () => {
    assert.deepEqual(
      parseContextWindow('{"model":"deepseek-flash","window":1000000}'),
      {
        model: "deepseek-flash",
        window: 1_000_000,
        provider: "",
        hasKey: null,
        subscription: false,
        keyEnv: [],
      },
    );
    // A model the CLI did not name is still an answer about the window.
    assert.deepEqual(parseContextWindow('{"window":200000}'), {
      model: "",
      window: 200_000,
      provider: "",
      hasKey: null,
      subscription: false,
      keyEnv: [],
    });
  });

  /// The credential and plan facts ride the same answer: the panel says what
  /// the desktop app says — `no API key` before a send, ` (sub)` on the spend —
  /// from the one read it already makes for the window.
  it("reads the credential and plan facts the CLI resolves", () => {
    assert.deepEqual(
      parseContextWindow(
        '{"model":"m","window":200000,"provider":"anthropic","hasKey":false,' +
          '"subscription":true,"keyEnv":["ANTHROPIC_API_KEY","OXIDE_API_KEY"]}',
      ),
      {
        model: "m",
        window: 200_000,
        provider: "anthropic",
        hasKey: false,
        subscription: true,
        keyEnv: ["ANTHROPIC_API_KEY", "OXIDE_API_KEY"],
      },
    );
    // A CLI too old to answer either leaves the question open (`null`) rather
    // than reporting a key nobody checked for.
    const old = parseContextWindow('{"window":200000}');
    assert.equal(old?.hasKey, null);
    assert.equal(old?.subscription, false);
    assert.deepEqual(old?.keyEnv, []);
  });

  it("reads anything else as no answer, so the caller's fallback stands", () => {
    // A CLI too old to know the command, a partial write, and a window that is
    // not a positive number.
    assert.equal(parseContextWindow(""), null);
    assert.equal(parseContextWindow("error: unrecognized subcommand 'context'"), null);
    assert.equal(parseContextWindow("{}"), null);
    assert.equal(parseContextWindow('{"window":0}'), null);
    assert.equal(parseContextWindow('{"window":-5}'), null);
    assert.equal(parseContextWindow('{"window":"1000000"}'), null);
    assert.equal(parseContextWindow('{"window":null}'), null);
  });
});
