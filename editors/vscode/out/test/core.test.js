"use strict";
var __createBinding = (this && this.__createBinding) || (Object.create ? (function(o, m, k, k2) {
    if (k2 === undefined) k2 = k;
    var desc = Object.getOwnPropertyDescriptor(m, k);
    if (!desc || ("get" in desc ? !m.__esModule : desc.writable || desc.configurable)) {
      desc = { enumerable: true, get: function() { return m[k]; } };
    }
    Object.defineProperty(o, k2, desc);
}) : (function(o, m, k, k2) {
    if (k2 === undefined) k2 = k;
    o[k2] = m[k];
}));
var __setModuleDefault = (this && this.__setModuleDefault) || (Object.create ? (function(o, v) {
    Object.defineProperty(o, "default", { enumerable: true, value: v });
}) : function(o, v) {
    o["default"] = v;
});
var __importStar = (this && this.__importStar) || (function () {
    var ownKeys = function(o) {
        ownKeys = Object.getOwnPropertyNames || function (o) {
            var ar = [];
            for (var k in o) if (Object.prototype.hasOwnProperty.call(o, k)) ar[ar.length] = k;
            return ar;
        };
        return ownKeys(o);
    };
    return function (mod) {
        if (mod && mod.__esModule) return mod;
        var result = {};
        if (mod != null) for (var k = ownKeys(mod), i = 0; i < k.length; i++) if (k[i] !== "default") __createBinding(result, mod, k[i]);
        __setModuleDefault(result, mod);
        return result;
    };
})();
var __importDefault = (this && this.__importDefault) || function (mod) {
    return (mod && mod.__esModule) ? mod : { "default": mod };
};
Object.defineProperty(exports, "__esModule", { value: true });
const strict_1 = __importDefault(require("node:assert/strict"));
const path = __importStar(require("node:path"));
const node_test_1 = require("node:test");
const args_1 = require("../core/args");
const config_1 = require("../core/config");
const preview_1 = require("../core/preview");
const prompt_1 = require("../core/prompt");
const sessions_1 = require("../core/sessions");
const cli_1 = require("../cli");
/// One rendered diff row, laid out the way `oxide_core::diff` does it: a
/// marker, the line number on each side, then the text.
const diffRow = (marker, old, next, text) => `${marker}${old.padStart(3)} ${next.padStart(3)}  ${text}`;
(0, node_test_1.describe)("buildTurnArgs", () => {
    (0, node_test_1.it)("always asks for the JSON stream with an explicit prompt", () => {
        strict_1.default.deepEqual((0, args_1.buildTurnArgs)({}), ["--mode", "json", "-p"]);
    });
    (0, node_test_1.it)("resumes a session by id, and only one of session/continue", () => {
        strict_1.default.deepEqual((0, args_1.buildTurnArgs)({ session: "abc123", continueLast: true }), [
            "--mode",
            "json",
            "-p",
            "--session",
            "abc123",
        ]);
        strict_1.default.deepEqual((0, args_1.buildTurnArgs)({ session: null, continueLast: true }), [
            "--mode",
            "json",
            "-p",
            "--continue",
        ]);
    });
    (0, node_test_1.it)("maps the per-turn options onto CLI flags", () => {
        strict_1.default.deepEqual((0, args_1.buildTurnArgs)({
            model: "glm-5",
            agent: "rust-reviewer",
            reasoning: "high",
            ephemeral: true,
            tools: "read,grep",
            excludeTools: "bash",
            attachments: ["/tmp/a.png", "/tmp/b.pdf"],
        }), [
            "--mode",
            "json",
            "-p",
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
            "--image",
            "/tmp/a.png",
            "--image",
            "/tmp/b.pdf",
        ]);
    });
    (0, node_test_1.it)("omits the reasoning flag for the provider default", () => {
        strict_1.default.equal((0, args_1.buildTurnArgs)({ reasoning: "auto" }).includes("--reasoning"), false);
        strict_1.default.equal((0, args_1.buildTurnArgs)({ reasoning: "" }).includes("--reasoning"), false);
    });
    (0, node_test_1.it)("passes the project-trust setting as approve flags", () => {
        strict_1.default.deepEqual((0, args_1.buildTurnArgs)({ trust: "always" }).slice(3), ["--approve"]);
        strict_1.default.deepEqual((0, args_1.buildTurnArgs)({ trust: "never" }).slice(3), ["--no-approve"]);
        strict_1.default.deepEqual((0, args_1.buildTurnArgs)({ trust: "default" }).slice(3), []);
    });
    (0, node_test_1.it)("appends extra arguments and drops blank ones", () => {
        strict_1.default.deepEqual((0, args_1.buildTurnArgs)({ extra: ["--use-theme", "light", "", "  "] }), [
            "--mode",
            "json",
            "-p",
            "--use-theme",
            "light",
        ]);
    });
    (0, node_test_1.it)("lists sessions for the project", () => {
        strict_1.default.deepEqual((0, args_1.sessionsListArgs)(), ["sessions", "list"]);
    });
    (0, node_test_1.it)("normalizes a comma-separated tool list", () => {
        strict_1.default.equal((0, args_1.splitList)(" read , grep ,, bash ,"), "read,grep,bash");
        strict_1.default.equal((0, args_1.splitList)(""), "");
    });
});
(0, node_test_1.describe)("prompt assembly", () => {
    (0, node_test_1.it)("labels a whole-file block by path and a selection by line range", () => {
        strict_1.default.equal((0, prompt_1.contextHeader)({ path: "src/a.rs", text: "x" }), "--- src/a.rs ---");
        strict_1.default.equal((0, prompt_1.contextHeader)({ path: "src/a.rs", startLine: 10, endLine: 12, text: "x" }), "--- src/a.rs:10-12 ---");
        strict_1.default.equal((0, prompt_1.contextHeader)({ path: "src/a.rs", startLine: 7, endLine: 7, text: "x" }), "--- src/a.rs:7 ---");
    });
    (0, node_test_1.it)("inlines context blocks above the message, in the CLI's @file shape", () => {
        const prompt = (0, prompt_1.buildPrompt)("What is wrong here?", [
            { path: "src/a.rs", startLine: 1, endLine: 2, text: "fn main() {\n}" },
            { path: "AGENTS.md", text: "# rules" },
        ]);
        strict_1.default.equal(prompt, [
            "--- src/a.rs:1-2 ---",
            "fn main() {",
            "}",
            "",
            "--- AGENTS.md ---",
            "# rules",
            "",
            "What is wrong here?",
        ].join("\n"));
    });
    (0, node_test_1.it)("sends context without a message when the user attached only context", () => {
        strict_1.default.equal((0, prompt_1.buildPrompt)("", [{ path: "a.rs", text: "x" }]), "--- a.rs ---\nx");
        strict_1.default.equal((0, prompt_1.buildPrompt)("  hello  ", []), "hello");
    });
    (0, node_test_1.it)("chip labels name the range of a selection", () => {
        strict_1.default.equal((0, prompt_1.contextLabel)({ path: "a.rs", text: "x" }), "a.rs");
        strict_1.default.equal((0, prompt_1.contextLabel)({ path: "a.rs", startLine: 3, endLine: 4, text: "x" }), "a.rs:3-4");
    });
    (0, node_test_1.it)("treats images and PDFs as attachments, not text", () => {
        strict_1.default.equal((0, prompt_1.isAttachmentPath)("/tmp/shot.PNG"), true);
        strict_1.default.equal((0, prompt_1.isAttachmentPath)("C:\\Users\\me\\a.jpeg"), true);
        strict_1.default.equal((0, prompt_1.isAttachmentPath)("docs/spec.pdf"), true);
        strict_1.default.equal((0, prompt_1.isAttachmentPath)("src/main.rs"), false);
        strict_1.default.equal((0, prompt_1.isAttachmentPath)("Makefile"), false);
        strict_1.default.equal((0, prompt_1.isAttachmentPath)(".gitignore"), false);
    });
    (0, node_test_1.it)("shortens a path inside the workspace and keeps one outside it", () => {
        strict_1.default.equal((0, prompt_1.relativePath)("/w/oxide", "/w/oxide/src/a.rs"), "src/a.rs");
        strict_1.default.equal((0, prompt_1.relativePath)("/w/oxide/", "/w/oxide/src/a.rs"), "src/a.rs");
        strict_1.default.equal((0, prompt_1.relativePath)("/w/oxide", "/other/a.rs"), "/other/a.rs");
        strict_1.default.equal((0, prompt_1.relativePath)("/w/oxide", "/w/oxide"), "/w/oxide");
        strict_1.default.equal((0, prompt_1.relativePath)("/w/oxide", "/w/oxidex/a.rs"), "/w/oxidex/a.rs");
    });
});
(0, node_test_1.describe)("@ references", () => {
    const files = {
        "src/a.rs": "fn a() {}\n",
        "shot.png": "not read as text",
        "notes.md": "# notes\n",
        "big.log": null,
    };
    const sources = {
        resolve: (reference) => (reference in files ? `/w/${reference}` : null),
        read: (absolute) => files[absolute.slice(3)] ?? null,
        label: (absolute) => absolute.slice(3),
    };
    (0, node_test_1.it)("turns a reference into a context block and drops it from the message", () => {
        const result = (0, prompt_1.expandAtReferences)("why does @src/a.rs do nothing?", sources);
        strict_1.default.equal(result.message, "why does do nothing?");
        strict_1.default.deepEqual(result.blocks, [{ path: "src/a.rs", text: "fn a() {}\n" }]);
        strict_1.default.deepEqual(result.attachments, []);
    });
    (0, node_test_1.it)("keeps a reference that does not resolve, rather than dropping the text", () => {
        const result = (0, prompt_1.expandAtReferences)("what is @src/missing.rs for", sources);
        strict_1.default.equal(result.message, "what is @src/missing.rs for");
        strict_1.default.deepEqual(result.blocks, []);
    });
    (0, node_test_1.it)("keeps a reference whose file cannot be read as text", () => {
        const result = (0, prompt_1.expandAtReferences)("look at @big.log", sources);
        strict_1.default.equal(result.message, "look at @big.log");
        strict_1.default.deepEqual(result.blocks, []);
    });
    (0, node_test_1.it)("attaches an image instead of inlining it", () => {
        const result = (0, prompt_1.expandAtReferences)("what is wrong with @shot.png", sources);
        strict_1.default.equal(result.message, "what is wrong with");
        strict_1.default.deepEqual(result.attachments, ["/w/shot.png"]);
        strict_1.default.deepEqual(result.blocks, []);
    });
    (0, node_test_1.it)("reads a reference twice but attaches it once", () => {
        const result = (0, prompt_1.expandAtReferences)("@src/a.rs and @src/a.rs again", sources);
        strict_1.default.equal(result.message, "and again");
        strict_1.default.equal(result.blocks.length, 1);
    });
    (0, node_test_1.it)("leaves punctuation next to a reference where it belongs", () => {
        const result = (0, prompt_1.expandAtReferences)("see @src/a.rs, then @notes.md.", sources);
        strict_1.default.equal(result.message, "see, then.");
        strict_1.default.deepEqual(result.blocks.map((block) => block.path), ["src/a.rs", "notes.md"]);
    });
    (0, node_test_1.it)("ignores a lone @ and a reference inside a word", () => {
        strict_1.default.deepEqual((0, prompt_1.expandAtReferences)("@", sources), {
            message: "@",
            blocks: [],
            attachments: [],
        });
        strict_1.default.equal((0, prompt_1.expandAtReferences)("mail me at a@src/a.rs", sources).message, "mail me at a@src/a.rs");
    });
    (0, node_test_1.it)("collapses the blank a removed reference leaves behind", () => {
        strict_1.default.equal((0, prompt_1.expandAtReferences)("@notes.md what now", sources).message, "what now");
        strict_1.default.equal((0, prompt_1.expandAtReferences)("@notes.md", sources).message, "");
        strict_1.default.equal((0, prompt_1.expandAtReferences)("  @notes.md  ", sources).message, "");
    });
});
(0, node_test_1.describe)("diff preview", () => {
    (0, node_test_1.it)("returns null when nothing changed", () => {
        strict_1.default.equal((0, preview_1.diffPreview)("a\n", "a\n"), null);
        strict_1.default.equal((0, preview_1.diffPreview)("", ""), null);
    });
    (0, node_test_1.it)("renders a line-numbered diff with the core's layout", () => {
        strict_1.default.equal((0, preview_1.diffPreview)("a\nb\nc", "a\nx\nc"), ["   1   1  a", "-  2      b", "+      2  x", "   3   3  c"].join("\n"));
    });
    (0, node_test_1.it)("shows a pure addition with an empty old column", () => {
        strict_1.default.equal((0, preview_1.diffPreview)("", "b"), diffRow("+", "", "1", "b"));
    });
    (0, node_test_1.it)("marks the gap between two distant changes", () => {
        const lines = Array.from({ length: 40 }, (_, index) => `line ${index + 1}`);
        const changed = [...lines];
        changed[2] = "first change";
        changed[30] = "second change";
        const diff = (0, preview_1.diffPreview)(lines.join("\n"), changed.join("\n"));
        strict_1.default.ok(diff, "expected a diff");
        strict_1.default.match(diff, /⋯/);
        strict_1.default.match(diff, /first change/);
        strict_1.default.match(diff, /second change/);
        strict_1.default.equal(diff.includes("line 20"), false);
    });
    (0, node_test_1.it)("summarizes an oversized diff instead of rendering it", () => {
        const huge = Array.from({ length: 2_100 }, (_, index) => `line ${index}`).join("\n");
        strict_1.default.equal((0, preview_1.diffPreview)("", huge), "(diff omitted: 0 -> 2100 lines)");
    });
});
(0, node_test_1.describe)("tool diff", () => {
    const reader = (content) => () => content;
    (0, node_test_1.it)("previews a write against the current file", () => {
        const change = (0, preview_1.toolDiff)("write", { path: "a.txt", content: "b\n" }, reader("a\n"));
        strict_1.default.ok(change);
        strict_1.default.equal(change.path, "a.txt");
        strict_1.default.equal(change.diff, [diffRow("-", "1", "", "a"), diffRow("+", "", "1", "b")].join("\n"));
    });
    (0, node_test_1.it)("previews a write to a file that does not exist yet", () => {
        const change = (0, preview_1.toolDiff)("write_file", { path: "new.txt", content: "b\n" }, reader(null));
        strict_1.default.ok(change);
        strict_1.default.equal(change.diff, diffRow("+", "", "1", "b"));
    });
    (0, node_test_1.it)("ignores a write with unchanged content", () => {
        strict_1.default.equal((0, preview_1.toolDiff)("write", { path: "a.txt", content: "a\n" }, reader("a\n")), null);
        strict_1.default.equal((0, preview_1.toolDiff)("write", { path: "a.txt" }, reader("a\n")), null);
        strict_1.default.equal((0, preview_1.toolDiff)("write", {}, reader(null)), null);
    });
    (0, node_test_1.it)("applies an edit's old text to build the preview", () => {
        const change = (0, preview_1.toolDiff)("edit", { path: "a.rs", edits: [{ oldText: "b", newText: "x" }] }, reader("a\nb\nc\n"));
        strict_1.default.ok(change);
        strict_1.default.equal(change.diff, [
            diffRow(" ", "1", "1", "a"),
            diffRow("-", "2", "", "b"),
            diffRow("+", "", "2", "x"),
            diffRow(" ", "3", "3", "c"),
        ].join("\n"));
    });
    (0, node_test_1.it)("previews an edit that differs only by trailing whitespace", () => {
        const change = (0, preview_1.toolDiff)("edit", { path: "a.rs", edits: [{ oldText: "b", newText: "x" }] }, reader("a\nb   \nc\n"));
        strict_1.default.ok(change);
        strict_1.default.match(change.diff, /\+ +2  x/);
    });
    (0, node_test_1.it)("previews an edit whose old text carries read line numbers", () => {
        const change = (0, preview_1.toolDiff)("edit", { path: "a.rs", edits: [{ oldText: "1|a\n2|b", newText: "1|A\n2|B" }] }, reader("a\nb\nc\n"));
        strict_1.default.ok(change);
        strict_1.default.match(change.diff, /\+ +1  A/);
        strict_1.default.match(change.diff, /\+ +2  B/);
    });
    (0, node_test_1.it)("repairs a stringified edits array closed with an extra brace", () => {
        const change = (0, preview_1.toolDiff)("edit", { path: "a.rs", edits: '[{"oldText":"b","newText":"x"}}]' }, reader("a\nb\nc\n"));
        strict_1.default.ok(change);
        strict_1.default.match(change.diff, /\+ +2  x/);
    });
    (0, node_test_1.it)("accepts a single edit object and the legacy old/new fields", () => {
        strict_1.default.match((0, preview_1.toolDiff)("edit", { path: "a.rs", edits: { oldText: "b", newText: "x" } }, reader("a\nb\n"))
            ?.diff ?? "", /x/);
        strict_1.default.match((0, preview_1.toolDiff)("edit", { path: "a.rs", oldText: "b", newText: "x" }, reader("a\nb\n"))?.diff ?? "", /x/);
    });
    (0, node_test_1.it)("shows no preview for an edit that cannot apply", () => {
        strict_1.default.equal((0, preview_1.toolDiff)("edit", { path: "a.rs", edits: [{ oldText: "zz", newText: "x" }] }, reader("a\nb\n")), null);
        strict_1.default.equal((0, preview_1.toolDiff)("edit", { path: "a.rs", edits: [{ oldText: "b", newText: "x" }] }, reader("b\nb\n")), null);
        strict_1.default.equal((0, preview_1.toolDiff)("edit", { path: "a.rs", edits: [{ oldText: "", newText: "x" }] }, reader("b\n")), null);
        strict_1.default.equal((0, preview_1.toolDiff)("edit", { path: "a.rs", edits: [] }, reader("b\n")), null);
    });
    (0, node_test_1.it)("passes a unified patch through as its own preview", () => {
        const patch = "--- a/a.rs\n+++ b/a.rs\n@@ -1 +1 @@\n-old\n+new";
        const change = (0, preview_1.toolDiff)("patch", { path: "a.rs", diff: patch }, reader("old\n"));
        strict_1.default.deepEqual(change, { path: "a.rs", diff: patch });
        strict_1.default.equal((0, preview_1.toolDiff)("patch", { diff: "   " }, reader(null)), null);
    });
    (0, node_test_1.it)("knows which tools change files", () => {
        strict_1.default.equal((0, preview_1.toolDiff)("read", { path: "a.rs" }, reader("x")), null);
        strict_1.default.equal((0, preview_1.toolDiff)("bash", { command: "sed -i s/a/b/ a.rs" }, reader("x")), null);
    });
    (0, node_test_1.it)("maps the Pi and legacy tool names onto one canonical name", () => {
        strict_1.default.equal((0, preview_1.canonicalTool)("read_file"), "read_file");
        strict_1.default.equal((0, preview_1.canonicalTool)("read"), "read_file");
        strict_1.default.equal((0, preview_1.canonicalTool)("write"), "write_file");
        strict_1.default.equal((0, preview_1.canonicalTool)("list_dir"), "list_dir");
        strict_1.default.equal((0, preview_1.canonicalTool)("glob"), "glob");
        strict_1.default.equal((0, preview_1.canonicalTool)("grep"), "grep");
    });
});
(0, node_test_1.describe)("session listing", () => {
    (0, node_test_1.it)("parses the CLI's listing", () => {
        const output = [
            "fe0031b1  just now     195 msg  Create VS Code Extension for Oxide",
            "7c8031b1  7m ago         2 msg  say hi",
            "a23031b1  9h ago       752 msg  Can we has two release-drafters?",
        ].join("\n");
        strict_1.default.deepEqual((0, sessions_1.parseSessionList)(output), [
            { id: "fe0031b1", age: "just now", messages: 195, label: "Create VS Code Extension for Oxide" },
            { id: "7c8031b1", age: "7m ago", messages: 2, label: "say hi" },
            { id: "a23031b1", age: "9h ago", messages: 752, label: "Can we has two release-drafters?" },
        ]);
    });
    (0, node_test_1.it)("keeps an unnamed session and drops lines it cannot parse", () => {
        strict_1.default.deepEqual((0, sessions_1.parseSessionList)("abc123  2d ago   4 msg  \nNo sessions found.\n\n"), [
            { id: "abc123", age: "2d ago", messages: 4, label: "" },
        ]);
        strict_1.default.deepEqual((0, sessions_1.parseSessionList)(""), []);
    });
    (0, node_test_1.it)("reads the version out of --version", () => {
        strict_1.default.equal((0, sessions_1.parseVersion)("oxide 0.0.0\n"), "0.0.0");
        strict_1.default.equal((0, sessions_1.parseVersion)("oxide 1.2.3-beta.1"), "1.2.3-beta.1");
        strict_1.default.equal((0, sessions_1.parseVersion)("not installed"), null);
    });
});
(0, node_test_1.describe)("shared configuration", () => {
    const none = () => false;
    (0, node_test_1.it)("reads the platform config directory", () => {
        strict_1.default.equal((0, config_1.configDir)({ platform: "darwin", env: {}, home: "/Users/me", exists: none }), "/Users/me/Library/Application Support/Oxide");
        strict_1.default.equal((0, config_1.configDir)({ platform: "linux", env: {}, home: "/home/me", exists: none }), "/home/me/.config/Oxide");
        strict_1.default.equal((0, config_1.configDir)({ platform: "linux", env: { XDG_CONFIG_HOME: "/xdg" }, home: "/home/me", exists: none }), "/xdg/Oxide");
        strict_1.default.equal((0, config_1.configDir)({ platform: "win32", env: { APPDATA: "C:\\Users\\me\\AppData\\Roaming" }, home: "C:\\Users\\me", exists: none }), "C:\\Users\\me\\AppData\\Roaming/Oxide");
    });
    (0, node_test_1.it)("falls back to a pre-migration lowercase directory", () => {
        strict_1.default.equal((0, config_1.configDir)({
            platform: "darwin",
            env: {},
            home: "/Users/me",
            exists: (candidate) => candidate.endsWith("/oxide"),
        }), "/Users/me/Library/Application Support/oxide");
    });
    (0, node_test_1.it)("reads the provider and model from config.json", () => {
        strict_1.default.deepEqual((0, config_1.parseConfigSummary)('{"provider":"zai","model":"glm-5"}'), {
            provider: "zai",
            model: "glm-5",
        });
        strict_1.default.deepEqual((0, config_1.parseConfigSummary)("{}"), { provider: "", model: "" });
        strict_1.default.equal((0, config_1.parseConfigSummary)("{"), null);
        strict_1.default.equal((0, config_1.parseConfigSummary)("[1]"), null);
    });
    (0, node_test_1.it)("only shows a context percentage when the limit is set", () => {
        strict_1.default.equal((0, config_1.contextWindowFromEnv)({ OXIDE_CONTEXT_LIMIT: "200000" }), 200000);
        strict_1.default.equal((0, config_1.contextWindowFromEnv)({ OXIDE_CONTEXT_LIMIT: "0" }), 0);
        strict_1.default.equal((0, config_1.contextWindowFromEnv)({ OXIDE_CONTEXT_LIMIT: "abc" }), 0);
        strict_1.default.equal((0, config_1.contextWindowFromEnv)({}), 0);
    });
});
(0, node_test_1.describe)("binary resolution", () => {
    const lookup = (platform, env, found) => ({
        platform,
        env,
        exists: (candidate) => found.includes(candidate),
    });
    (0, node_test_1.it)("uses an explicit path as given", () => {
        strict_1.default.equal((0, cli_1.resolveBinary)("/opt/oxide/bin/oxide", lookup("linux", {}, [])), "/opt/oxide/bin/oxide");
    });
    (0, node_test_1.it)("finds a bare name on PATH", () => {
        const bin = path.join("/usr/local/bin", "oxide");
        strict_1.default.equal((0, cli_1.resolveBinary)("oxide", lookup("linux", { PATH: "/usr/bin:/usr/local/bin" }, [bin])), bin);
    });
    (0, node_test_1.it)("falls back to the installer and cargo directories", () => {
        const installer = path.join("/home/me", ".local", "bin", "oxide");
        strict_1.default.equal((0, cli_1.resolveBinary)("oxide", lookup("linux", { PATH: "/usr/bin", HOME: "/home/me" }, [installer])), installer);
        const cargo = path.join("/home/me", ".cargo", "bin", "oxide");
        strict_1.default.equal((0, cli_1.resolveBinary)("oxide", lookup("linux", { PATH: "", HOME: "/home/me" }, [cargo])), cargo);
    });
    (0, node_test_1.it)("looks for a Windows executable suffix", () => {
        // The PATH entry avoids a drive-letter colon, which only separates entries
        // on Windows, where the extension would be running anyway.
        const dir = "/tools/bin";
        const exe = path.join(dir, "oxide.exe");
        strict_1.default.equal((0, cli_1.resolveBinary)("oxide", lookup("win32", { PATH: dir, APPDATA: "C:\\x" }, [exe])), exe);
    });
    (0, node_test_1.it)("falls back to the bare name so the spawn error names the missing binary", () => {
        strict_1.default.equal((0, cli_1.resolveBinary)("oxide", lookup("linux", { PATH: "/usr/bin", HOME: "/home/me" }, [])), "oxide");
        strict_1.default.equal((0, cli_1.resolveBinary)("", lookup("linux", { PATH: "", HOME: "/home/me" }, [])), "oxide");
    });
});
//# sourceMappingURL=core.test.js.map