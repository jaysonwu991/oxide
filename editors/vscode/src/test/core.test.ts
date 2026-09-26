import assert from "node:assert/strict";
import * as path from "node:path";
import { describe, it } from "node:test";

import { buildTurnArgs, sessionsListArgs, splitList } from "../core/args";
import {
  configDir,
  contextWindow,
  contextWindowFromEnv,
  modelsForProvider,
  parseConfigSummary,
} from "../core/config";
import { canonicalTool, diffPreview, toolDiff } from "../core/preview";
import {
  buildPrompt,
  contextHeader,
  contextLabel,
  expandAtReferences,
  isAttachmentPath,
  relativePath,
  type AtReferenceSources,
} from "../core/prompt";
import { parseSessionList, parseVersion } from "../core/sessions";
import { resolveBinary } from "../cli";

/// One rendered diff row, laid out the way `oxide_core::diff` does it: a
/// marker, the line number on each side, then the text.
const diffRow = (marker: string, old: string, next: string, text: string) =>
  `${marker}${old.padStart(3)} ${next.padStart(3)}  ${text}`;

describe("buildTurnArgs", () => {
  it("always asks for the request channel, asking before gated tools", () => {
    assert.deepEqual(buildTurnArgs({}), ["--mode", "rpc", "--ask-approvals"]);
    assert.deepEqual(buildTurnArgs({ askApprovals: false }), ["--mode", "rpc", "--no-ask-approvals"]);
  });

  it("resumes a session by id, and only one of session/continue", () => {
    assert.deepEqual(buildTurnArgs({ session: "abc123", continueLast: true }), [
      "--mode",
      "rpc",
      "--ask-approvals",
      "--session",
      "abc123",
    ]);
    assert.deepEqual(buildTurnArgs({ session: null, continueLast: true }), [
      "--mode",
      "rpc",
      "--ask-approvals",
      "--continue",
    ]);
  });

  it("maps the per-turn options onto CLI flags", () => {
    assert.deepEqual(
      buildTurnArgs({
        model: "glm-5",
        agent: "rust-reviewer",
        reasoning: "high",
        ephemeral: true,
        tools: "read,grep",
        excludeTools: "bash",
      }),
      [
        "--mode",
        "rpc",
        "--ask-approvals",
        "--no-session",
        "--model",
        "glm-5",
        "--agent",
        "rust-reviewer",
        "--reasoning",
        "high",
        "--tools",
        "read,grep",
        "--exclude-tools",
        "bash",
      ],
    );
  });

  // Attachment paths are not flags any more: rpc mode reads them from the
  // `prompt` request (`core/rpc.ts`).
  it("carries no --image flags", () => {
    assert.equal(buildTurnArgs({}).includes("--image"), false);
    assert.equal(buildTurnArgs({}).includes("-p"), false);
  });

  it("omits the reasoning flag for the provider default", () => {
    assert.equal(buildTurnArgs({ reasoning: "auto" }).includes("--reasoning"), false);
    assert.equal(buildTurnArgs({ reasoning: "" }).includes("--reasoning"), false);
  });

  it("passes the project-trust setting as approve flags", () => {
    assert.deepEqual(buildTurnArgs({ trust: "always" }).slice(3), ["--approve"]);
    assert.deepEqual(buildTurnArgs({ trust: "never" }).slice(3), ["--no-approve"]);
    assert.deepEqual(buildTurnArgs({ trust: "default" }).slice(3), []);
  });

  it("appends extra arguments and drops blank ones", () => {
    assert.deepEqual(buildTurnArgs({ extra: ["--use-theme", "light", "", "  "] }), [
      "--mode",
      "rpc",
      "--ask-approvals",
      "--use-theme",
      "light",
    ]);
  });

  it("lists sessions for the project", () => {
    assert.deepEqual(sessionsListArgs(), ["sessions", "list"]);
  });

  it("normalizes a comma-separated tool list", () => {
    assert.equal(splitList(" read , grep ,, bash ,"), "read,grep,bash");
    assert.equal(splitList(""), "");
  });
});

describe("prompt assembly", () => {
  it("labels a whole-file block by path and a selection by line range", () => {
    assert.equal(contextHeader({ path: "src/a.rs", text: "x" }), "--- src/a.rs ---");
    assert.equal(
      contextHeader({ path: "src/a.rs", startLine: 10, endLine: 12, text: "x" }),
      "--- src/a.rs:10-12 ---",
    );
    assert.equal(
      contextHeader({ path: "src/a.rs", startLine: 7, endLine: 7, text: "x" }),
      "--- src/a.rs:7 ---",
    );
  });

  it("inlines context blocks above the message, in the CLI's @file shape", () => {
    const prompt = buildPrompt("What is wrong here?", [
      { path: "src/a.rs", startLine: 1, endLine: 2, text: "fn main() {\n}" },
      { path: "AGENTS.md", text: "# rules" },
    ]);
    assert.equal(
      prompt,
      [
        "--- src/a.rs:1-2 ---",
        "fn main() {",
        "}",
        "",
        "--- AGENTS.md ---",
        "# rules",
        "",
        "What is wrong here?",
      ].join("\n"),
    );
  });

  it("sends context without a message when the user attached only context", () => {
    assert.equal(buildPrompt("", [{ path: "a.rs", text: "x" }]), "--- a.rs ---\nx");
    assert.equal(buildPrompt("  hello  ", []), "hello");
  });

  it("chip labels name the range of a selection", () => {
    assert.equal(contextLabel({ path: "a.rs", text: "x" }), "a.rs");
    assert.equal(contextLabel({ path: "a.rs", startLine: 3, endLine: 4, text: "x" }), "a.rs:3-4");
  });

  it("treats images and PDFs as attachments, not text", () => {
    assert.equal(isAttachmentPath("/tmp/shot.PNG"), true);
    assert.equal(isAttachmentPath("C:\\Users\\me\\a.jpeg"), true);
    assert.equal(isAttachmentPath("docs/spec.pdf"), true);
    assert.equal(isAttachmentPath("src/main.rs"), false);
    assert.equal(isAttachmentPath("Makefile"), false);
    assert.equal(isAttachmentPath(".gitignore"), false);
  });

  it("shortens a path inside the workspace and keeps one outside it", () => {
    assert.equal(relativePath("/w/oxide", "/w/oxide/src/a.rs"), "src/a.rs");
    assert.equal(relativePath("/w/oxide/", "/w/oxide/src/a.rs"), "src/a.rs");
    assert.equal(relativePath("/w/oxide", "/other/a.rs"), "/other/a.rs");
    assert.equal(relativePath("/w/oxide", "/w/oxide"), "/w/oxide");
    assert.equal(relativePath("/w/oxide", "/w/oxidex/a.rs"), "/w/oxidex/a.rs");
  });
});

describe("@ references", () => {
  const files: Record<string, string | null> = {
    "src/a.rs": "fn a() {}\n",
    "shot.png": "not read as text",
    "notes.md": "# notes\n",
    "big.log": null,
  };
  const sources: AtReferenceSources = {
    resolve: (reference) => (reference in files ? `/w/${reference}` : null),
    read: (absolute) => files[absolute.slice(3)] ?? null,
    label: (absolute) => absolute.slice(3),
  };

  it("turns a reference into a context block and drops it from the message", () => {
    const result = expandAtReferences("why does @src/a.rs do nothing?", sources);
    assert.equal(result.message, "why does do nothing?");
    assert.deepEqual(result.blocks, [{ path: "src/a.rs", text: "fn a() {}\n" }]);
    assert.deepEqual(result.attachments, []);
  });

  it("keeps a reference that does not resolve, rather than dropping the text", () => {
    const result = expandAtReferences("what is @src/missing.rs for", sources);
    assert.equal(result.message, "what is @src/missing.rs for");
    assert.deepEqual(result.blocks, []);
  });

  it("keeps a reference whose file cannot be read as text", () => {
    const result = expandAtReferences("look at @big.log", sources);
    assert.equal(result.message, "look at @big.log");
    assert.deepEqual(result.blocks, []);
  });

  it("attaches an image instead of inlining it", () => {
    const result = expandAtReferences("what is wrong with @shot.png", sources);
    assert.equal(result.message, "what is wrong with");
    assert.deepEqual(result.attachments, ["/w/shot.png"]);
    assert.deepEqual(result.blocks, []);
  });

  it("reads a reference twice but attaches it once", () => {
    const result = expandAtReferences("@src/a.rs and @src/a.rs again", sources);
    assert.equal(result.message, "and again");
    assert.equal(result.blocks.length, 1);
  });

  it("leaves punctuation next to a reference where it belongs", () => {
    const result = expandAtReferences("see @src/a.rs, then @notes.md.", sources);
    assert.equal(result.message, "see, then.");
    assert.deepEqual(
      result.blocks.map((block) => block.path),
      ["src/a.rs", "notes.md"],
    );
  });

  it("ignores a lone @ and a reference inside a word", () => {
    assert.deepEqual(expandAtReferences("@", sources), {
      message: "@",
      blocks: [],
      attachments: [],
    });
    assert.equal(expandAtReferences("mail me at a@src/a.rs", sources).message, "mail me at a@src/a.rs");
  });

  it("collapses the blank a removed reference leaves behind", () => {
    assert.equal(expandAtReferences("@notes.md what now", sources).message, "what now");
    assert.equal(expandAtReferences("@notes.md", sources).message, "");
    assert.equal(expandAtReferences("  @notes.md  ", sources).message, "");
  });
});

describe("diff preview", () => {
  it("returns null when nothing changed", () => {
    assert.equal(diffPreview("a\n", "a\n"), null);
    assert.equal(diffPreview("", ""), null);
  });

  it("renders a line-numbered diff with the core's layout", () => {
    assert.equal(
      diffPreview("a\nb\nc", "a\nx\nc"),
      ["   1   1  a", "-  2      b", "+      2  x", "   3   3  c"].join("\n"),
    );
  });

  it("shows a pure addition with an empty old column", () => {
    assert.equal(diffPreview("", "b"), diffRow("+", "", "1", "b"));
  });

  it("marks the gap between two distant changes", () => {
    const lines = Array.from({ length: 40 }, (_, index) => `line ${index + 1}`);
    const changed = [...lines];
    changed[2] = "first change";
    changed[30] = "second change";
    const diff = diffPreview(lines.join("\n"), changed.join("\n"));
    assert.ok(diff, "expected a diff");
    assert.match(diff, /⋯/);
    assert.match(diff, /first change/);
    assert.match(diff, /second change/);
    assert.equal(diff.includes("line 20"), false);
  });

  it("summarizes an oversized diff instead of rendering it", () => {
    const huge = Array.from({ length: 2_100 }, (_, index) => `line ${index}`).join("\n");
    assert.equal(diffPreview("", huge), "(diff omitted: 0 -> 2100 lines)");
  });
});

describe("tool diff", () => {
  const reader = (content: string | null) => () => content;

  it("previews a write against the current file", () => {
    const change = toolDiff("write", { path: "a.txt", content: "b\n" }, reader("a\n"));
    assert.ok(change);
    assert.equal(change.path, "a.txt");
    assert.equal(
      change.diff,
      [diffRow("-", "1", "", "a"), diffRow("+", "", "1", "b")].join("\n"),
    );
  });

  it("previews a write to a file that does not exist yet", () => {
    const change = toolDiff("write_file", { path: "new.txt", content: "b\n" }, reader(null));
    assert.ok(change);
    assert.equal(change.diff, diffRow("+", "", "1", "b"));
  });

  it("ignores a write with unchanged content", () => {
    assert.equal(toolDiff("write", { path: "a.txt", content: "a\n" }, reader("a\n")), null);
    assert.equal(toolDiff("write", { path: "a.txt" }, reader("a\n")), null);
    assert.equal(toolDiff("write", {}, reader(null)), null);
  });

  it("applies an edit's old text to build the preview", () => {
    const change = toolDiff(
      "edit",
      { path: "a.rs", edits: [{ oldText: "b", newText: "x" }] },
      reader("a\nb\nc\n"),
    );
    assert.ok(change);
    assert.equal(
      change.diff,
      [
        diffRow(" ", "1", "1", "a"),
        diffRow("-", "2", "", "b"),
        diffRow("+", "", "2", "x"),
        diffRow(" ", "3", "3", "c"),
      ].join("\n"),
    );
  });

  it("previews an edit that differs only by trailing whitespace", () => {
    const change = toolDiff(
      "edit",
      { path: "a.rs", edits: [{ oldText: "b", newText: "x" }] },
      reader("a\nb   \nc\n"),
    );
    assert.ok(change);
    assert.match(change.diff, /\+ +2  x/);
  });

  it("previews an edit whose old text carries read line numbers", () => {
    const change = toolDiff(
      "edit",
      { path: "a.rs", edits: [{ oldText: "1|a\n2|b", newText: "1|A\n2|B" }] },
      reader("a\nb\nc\n"),
    );
    assert.ok(change);
    assert.match(change.diff, /\+ +1  A/);
    assert.match(change.diff, /\+ +2  B/);
  });

  it("repairs a stringified edits array closed with an extra brace", () => {
    const change = toolDiff(
      "edit",
      { path: "a.rs", edits: '[{"oldText":"b","newText":"x"}}]' },
      reader("a\nb\nc\n"),
    );
    assert.ok(change);
    assert.match(change.diff, /\+ +2  x/);
  });

  it("accepts a single edit object and the legacy old/new fields", () => {
    assert.match(
      toolDiff("edit", { path: "a.rs", edits: { oldText: "b", newText: "x" } }, reader("a\nb\n"))
        ?.diff ?? "",
      /x/,
    );
    assert.match(
      toolDiff("edit", { path: "a.rs", oldText: "b", newText: "x" }, reader("a\nb\n"))?.diff ?? "",
      /x/,
    );
  });

  it("shows no preview for an edit that cannot apply", () => {
    assert.equal(toolDiff("edit", { path: "a.rs", edits: [{ oldText: "zz", newText: "x" }] }, reader("a\nb\n")), null);
    assert.equal(
      toolDiff("edit", { path: "a.rs", edits: [{ oldText: "b", newText: "x" }] }, reader("b\nb\n")),
      null,
    );
    assert.equal(toolDiff("edit", { path: "a.rs", edits: [{ oldText: "", newText: "x" }] }, reader("b\n")), null);
    assert.equal(toolDiff("edit", { path: "a.rs", edits: [] }, reader("b\n")), null);
  });

  it("passes a unified patch through as its own preview", () => {
    const patch = "--- a/a.rs\n+++ b/a.rs\n@@ -1 +1 @@\n-old\n+new";
    const change = toolDiff("patch", { path: "a.rs", diff: patch }, reader("old\n"));
    assert.deepEqual(change, { path: "a.rs", diff: patch });
    assert.equal(toolDiff("patch", { diff: "   " }, reader(null)), null);
  });

  it("knows which tools change files", () => {
    assert.equal(toolDiff("read", { path: "a.rs" }, reader("x")), null);
    assert.equal(toolDiff("bash", { command: "sed -i s/a/b/ a.rs" }, reader("x")), null);
  });

  it("maps the Pi and legacy tool names onto one canonical name", () => {
    assert.equal(canonicalTool("read_file"), "read_file");
    assert.equal(canonicalTool("read"), "read_file");
    assert.equal(canonicalTool("write"), "write_file");
    assert.equal(canonicalTool("list_dir"), "list_dir");
    assert.equal(canonicalTool("glob"), "glob");
    assert.equal(canonicalTool("grep"), "grep");
  });
});

describe("session listing", () => {
  it("parses the CLI's listing", () => {
    const output = [
      "fe0031b1  just now     195 msg  Create VS Code Extension for Oxide",
      "7c8031b1  7m ago         2 msg  say hi",
      "a23031b1  9h ago       752 msg  Can we has two release-drafters?",
    ].join("\n");
    assert.deepEqual(parseSessionList(output), [
      { id: "fe0031b1", age: "just now", messages: 195, label: "Create VS Code Extension for Oxide" },
      { id: "7c8031b1", age: "7m ago", messages: 2, label: "say hi" },
      { id: "a23031b1", age: "9h ago", messages: 752, label: "Can we has two release-drafters?" },
    ]);
  });

  it("keeps an unnamed session and drops lines it cannot parse", () => {
    assert.deepEqual(parseSessionList("abc123  2d ago   4 msg  \nNo sessions found.\n\n"), [
      { id: "abc123", age: "2d ago", messages: 4, label: "" },
    ]);
    assert.deepEqual(parseSessionList(""), []);
  });

  it("reads the version out of --version", () => {
    assert.equal(parseVersion("oxide 0.0.0\n"), "0.0.0");
    assert.equal(parseVersion("oxide 1.2.3-beta.1"), "1.2.3-beta.1");
    assert.equal(parseVersion("not installed"), null);
  });
});

describe("shared configuration", () => {
  const none = () => false;

  it("reads the platform config directory", () => {
    assert.equal(
      configDir({ platform: "darwin", env: {}, home: "/Users/me", exists: none }),
      "/Users/me/Library/Application Support/Oxide",
    );
    assert.equal(
      configDir({ platform: "linux", env: {}, home: "/home/me", exists: none }),
      "/home/me/.config/Oxide",
    );
    assert.equal(
      configDir({ platform: "linux", env: { XDG_CONFIG_HOME: "/xdg" }, home: "/home/me", exists: none }),
      "/xdg/Oxide",
    );
    assert.equal(
      configDir({ platform: "win32", env: { APPDATA: "C:\\Users\\me\\AppData\\Roaming" }, home: "C:\\Users\\me", exists: none }),
      "C:\\Users\\me\\AppData\\Roaming/Oxide",
    );
  });

  it("falls back to a pre-migration lowercase directory", () => {
    assert.equal(
      configDir({
        platform: "darwin",
        env: {},
        home: "/Users/me",
        exists: (candidate) => candidate.endsWith("/oxide"),
      }),
      "/Users/me/Library/Application Support/oxide",
    );
  });

  it("reads the provider, model, remembered models and reply cap from config.json", () => {
    assert.deepEqual(
      parseConfigSummary(
        '{"provider":"zai","model":"glm-5","max_tokens":16384,"provider_models":{"zai":"glm-5","openai":"gpt-5"}}',
      ),
      {
        provider: "zai",
        model: "glm-5",
        models: [
          { provider: "zai", model: "glm-5" },
          { provider: "openai", model: "gpt-5" },
        ],
        maxTokens: 16384,
      },
    );
    assert.deepEqual(parseConfigSummary("{}"), { provider: "", model: "", models: [], maxTokens: 0 });
    assert.deepEqual(parseConfigSummary(null), null);
    assert.equal(parseConfigSummary("{"), null);
    assert.equal(parseConfigSummary("[1]"), null);
  });

  it("only shows a context percentage when the limit is set", () => {
    assert.equal(contextWindowFromEnv({ OXIDE_CONTEXT_LIMIT: "200000" }), 200000);
    assert.equal(contextWindowFromEnv({ OXIDE_CONTEXT_LIMIT: " 200000 " }), 200000);
    assert.equal(contextWindowFromEnv({ OXIDE_CONTEXT_LIMIT: "+200000" }), 200000);
    assert.equal(contextWindowFromEnv({ OXIDE_CONTEXT_LIMIT: "0" }), 0);
    assert.equal(contextWindowFromEnv({ OXIDE_CONTEXT_LIMIT: "abc" }), 0);
    assert.equal(contextWindowFromEnv({}), 0);
    // `u64::from_str` rejects these, so the CLI falls back to its configured
    // window and the footer has to report the same one.
    assert.equal(contextWindowFromEnv({ OXIDE_CONTEXT_LIMIT: "1.5" }), 0);
    assert.equal(contextWindowFromEnv({ OXIDE_CONTEXT_LIMIT: "1e5" }), 0);
    assert.equal(contextWindowFromEnv({ OXIDE_CONTEXT_LIMIT: "12abc" }), 0);
    assert.equal(contextWindowFromEnv({ OXIDE_CONTEXT_LIMIT: "-5" }), 0);
    assert.equal(contextWindowFromEnv({ OXIDE_CONTEXT_LIMIT: "" }), 0);
    assert.equal(contextWindowFromEnv({ OXIDE_CONTEXT_LIMIT: "99999999999999999999" }), 0);
  });

  it("mirrors the CLI's context window, floored at 128k", () => {
    // `Config::context_window`: the override wins, else `max_tokens` floored.
    assert.equal(contextWindow({ OXIDE_CONTEXT_LIMIT: "200000" }, null), 200000);
    assert.equal(contextWindow({}, null), 128_000);
    assert.equal(contextWindow({ OXIDE_CONTEXT_LIMIT: "1.5" }, null), 128_000);
    assert.equal(
      contextWindow({}, { provider: "", model: "", models: [], maxTokens: 8192 }),
      128_000,
    );
    assert.equal(
      contextWindow({}, { provider: "", model: "", models: [], maxTokens: 1_000_000 }),
      1_000_000,
    );
  });

  it("offers only the models remembered for the active provider", () => {
    const summary = parseConfigSummary(
      '{"provider":"zai","provider_models":{"zai":"glm-5","openai":"gpt-5"}}',
    );
    // The id goes to whichever provider the CLI has active, so another
    // provider's model must not be offered as a choice.
    assert.deepEqual(modelsForProvider(summary, "zai"), [{ provider: "zai", model: "glm-5" }]);
    assert.deepEqual(modelsForProvider(summary, "ZAI"), [{ provider: "zai", model: "glm-5" }]);
    assert.deepEqual(modelsForProvider(summary, "openai"), [{ provider: "openai", model: "gpt-5" }]);
    assert.deepEqual(modelsForProvider(summary, ""), []);
    assert.deepEqual(modelsForProvider(summary, "anthropic"), []);
    assert.deepEqual(modelsForProvider(null, "zai"), []);
  });
});

describe("binary resolution", () => {
  const lookup = (platform: NodeJS.Platform, env: NodeJS.ProcessEnv, found: string[]) => ({
    platform,
    env,
    exists: (candidate: string) => found.includes(candidate),
  });

  it("uses an explicit path as given", () => {
    assert.equal(
      resolveBinary("/opt/oxide/bin/oxide", lookup("linux", {}, [])),
      "/opt/oxide/bin/oxide",
    );
  });

  it("finds a bare name on PATH", () => {
    const bin = path.join("/usr/local/bin", "oxide");
    assert.equal(
      resolveBinary("oxide", lookup("linux", { PATH: "/usr/bin:/usr/local/bin" }, [bin])),
      bin,
    );
  });

  it("falls back to the installer and cargo directories", () => {
    const installer = path.join("/home/me", ".local", "bin", "oxide");
    assert.equal(
      resolveBinary("oxide", lookup("linux", { PATH: "/usr/bin", HOME: "/home/me" }, [installer])),
      installer,
    );
    const cargo = path.join("/home/me", ".cargo", "bin", "oxide");
    assert.equal(
      resolveBinary("oxide", lookup("linux", { PATH: "", HOME: "/home/me" }, [cargo])),
      cargo,
    );
  });

  it("looks for a Windows executable suffix", () => {
    // The PATH entry avoids a drive-letter colon, which only separates entries
    // on Windows, where the extension would be running anyway.
    const dir = "/tools/bin";
    const exe = path.join(dir, "oxide.exe");
    assert.equal(
      resolveBinary("oxide", lookup("win32", { PATH: dir, APPDATA: "C:\\x" }, [exe])),
      exe,
    );
  });

  it("falls back to the bare name so the spawn error names the missing binary", () => {
    assert.equal(resolveBinary("oxide", lookup("linux", { PATH: "/usr/bin", HOME: "/home/me" }, [])), "oxide");
    assert.equal(resolveBinary("", lookup("linux", { PATH: "", HOME: "/home/me" }, [])), "oxide");
  });
});
