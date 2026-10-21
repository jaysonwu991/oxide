// The language the panel draws, read from the CLI.
//
// The catalog is the core's (`oxide_core::i18n`) and this package cannot link
// it, so the panel asks the installed CLI for the answer and paints what it
// gets. These hold the reading of that answer and the fallback that keeps a
// window whole when a CLI is too old to give one.

import assert from "node:assert/strict";
import { test } from "node:test";

import {
  DEFAULT_LANGUAGE,
  FALLBACK_LANGUAGES,
  languageArgs,
  languageSetArgs,
  parseLanguageInfo,
  translate,
} from "../core/i18n";

const answer = {
  language: "zh-CN",
  languages: [
    { tag: "en-GB", label: "English (United Kingdom)", nativeLabel: "English (United Kingdom)" },
    { tag: "zh-CN", label: "Simplified Chinese", nativeLabel: "简体中文" },
  ],
  strings: {
    "composer.send": "发送",
    "composer.queuedMessage": "已排队：{text}",
    "language.name": "语言",
  },
};

test("reads the answer the CLI prints", () => {
  const info = parseLanguageInfo(JSON.stringify(answer));
  assert.ok(info);
  assert.equal(info.language, "zh-CN");
  assert.deepEqual(
    info.languages.map((language) => language.tag),
    ["en-GB", "zh-CN"],
  );
  assert.equal(info.languages[1].nativeLabel, "简体中文");
  assert.equal(info.strings["composer.send"], "发送");
});

test("falls back to the two languages when the answer lists none", () => {
  const info = parseLanguageInfo(JSON.stringify({ language: "en-GB", strings: {} }));
  assert.ok(info);
  assert.deepEqual(info.languages, FALLBACK_LANGUAGES);
  assert.equal(info.language, "en-GB");
});

test("reads nothing from an answer that names no language", () => {
  assert.equal(parseLanguageInfo(""), null);
  assert.equal(parseLanguageInfo("not json"), null);
  assert.equal(parseLanguageInfo("{}"), null);
  assert.equal(parseLanguageInfo(JSON.stringify({ language: 3 })), null);
});

test("drops a string that is not text rather than painting it", () => {
  const info = parseLanguageInfo(
    JSON.stringify({ language: "zh-CN", strings: { "composer.send": "发送", "bad.key": 7 } }),
  );
  assert.ok(info);
  assert.equal(info.strings["composer.send"], "发送");
  assert.equal(info.strings["bad.key"], undefined);
});

test("asks the CLI for the language and for a switch", () => {
  assert.deepEqual(languageArgs(), ["language", "--json"]);
  assert.deepEqual(languageSetArgs("zh-CN"), ["language", "set", "zh-CN", "--json"]);
  assert.equal(DEFAULT_LANGUAGE, "en-GB");
});

test("translates by key and fills a sentence's own placeholders", () => {
  const info = parseLanguageInfo(JSON.stringify(answer));
  assert.equal(translate(info, "composer.send", "Send"), "发送");
  assert.equal(
    translate(info, "composer.queuedMessage", "Queued: {text}", { text: "hello" }),
    "已排队：hello",
  );
});

test("paints the English it was written in for a key the catalog has not", () => {
  const info = parseLanguageInfo(JSON.stringify(answer));
  assert.equal(translate(info, "composer.stop", "Stop"), "Stop");
  // ...and for a window that has no answer at all.
  assert.equal(translate(null, "composer.send", "Send"), "Send");
  assert.equal(translate(null, "language.name", "Language"), "Language");
});

import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { mcpDialog, providerDialog, reasoningDialog, sessionDialog, trustDialog, updateDialog } from "../core/dialogs";
import type { McpServerView } from "../core/mcps";
import { footerState } from "../core/footer";
import { emptyUsage, Transcript } from "../core/protocol";

const repository = resolve(__dirname, "../../../..");
const shared = readFileSync(resolve(repository, "crates/core/src/i18n.rs"), "utf8");
const sharedRows = [...shared.matchAll(/\(\s*("(?:[^"\\]|\\.)*")\s*,\s*("(?:[^"\\]|\\.)*")\s*,\s*("(?:[^"\\]|\\.)*")\s*,?\s*\)/g)]
  .map((row) => row.slice(1).map((value) => JSON.parse(value) as string));
const chinese = parseLanguageInfo(JSON.stringify({
  language: "zh-CN",
  strings: Object.fromEntries(sharedRows.map(([key, _en, zh]) => [key, zh])),
}))!;

test("uses the real Chinese catalog for dialogs while preserving action IDs and user content", () => {
  assert.equal(providerDialog([], "", "", chinese).title, "服务商");
  const reasoning = reasoningDialog("high", ["auto", "high"], chinese);
  assert.equal(reasoning.rows[0].label, "自动");
  assert.equal(reasoning.rows[1].label, "高");
  assert.equal(reasoning.rows[1].value, "high");
  assert.equal(reasoning.rows[1].current, true);
  assert.equal(reasoning.rows[0].current, false);
  assert.equal(reasoning.rows[1].status, "当前");
  const trust = trustDialog({ setting: "default", saved: undefined }, chinese);
  assert.equal(trust.rows[0].value, "default");
  assert.equal(trust.rows[0].label, "使用已保存的决定");
  assert.equal(trust.rows[0].current, true);
  assert.equal(trust.rows[1].current, false);
  const savedTrust = trustDialog({ setting: "default", saved: true }, chinese);
  assert.equal(savedTrust.rows[1].value, "trusted");
  assert.equal(savedTrust.rows[1].current, true);
  assert.equal(savedTrust.rows[0].current, false);
  const sessions = sessionDialog([{ id: "s1", label: "Original English title", age: "", messages: 2, pinned: true }], null, "", null, "", null, chinese);
  const row = sessions.rows.find((entry) => entry.value === "s1")!;
  assert.equal(row.label, "Original English title");
  assert.equal(row.detail, "2 条消息");
  assert.equal(row.buttons?.[0].label, "取消置顶对话");
  assert.equal(row.buttons?.[0].icon, "pin");
  assert.equal(updateDialog({ k: "checking" }, chinese).title, "正在检查更新");
});

test("localizes real MCP display statuses without discarding diagnostics or unknown states", () => {
  const server = (state: string, status: string, enabled = true): McpServerView => ({
    name: state, state, status, enabled, transport: "stdio", detail: "mcp-server", scope: "project", source: "project",
  });
  const servers = [server("connected", "Connected"), server("needs-auth", "Needs Auth"),
    server("needs-trust", "Needs Trust"), server("disabled", "Disabled", false),
    server("error", "Error: connection refused at localhost:3000"), server("future-state", "Waiting for setup")];
  assert.deepEqual(mcpDialog(servers, "", chinese).rows.map((row) => row.status),
    ["已连接", "需要授权", "需要信任", "已禁用", "错误：connection refused at localhost:3000", "Waiting for setup"]);
  assert.equal(mcpDialog([server("error", "Error")], "", chinese).rows[0].status, "错误");
  assert.equal(mcpDialog([server("future-state", "")], "", chinese).rows[0].status, "future-state");
});

test("keeps chip values and trust state independent of translated labels", () => {
  const footer = footerState({
    language: chinese, model: "claude-test", provider: "anthropic", contextWindow: 128000,
    reasoning: "auto", agent: "", agentCount: 0, access: "trusted", trustSetting: "default",
    defaultTrust: "ask", savedTrust: true, branch: "main", autoCompact: false,
    subscription: false, usage: emptyUsage(),
  });
  assert.equal(footer.chips.find((chip) => chip.id === "model")?.value, "claude-test");
  assert.equal(footer.chips.find((chip) => chip.id === "reasoning")?.value, "自动");
  assert.equal(footer.chips.find((chip) => chip.id === "access")?.active, true);
  assert.match(footer.chips[0].label, /^模型：/);
});

test("localizes manifest commands and settings from the same shared catalog", () => {
  const extension = resolve(repository, "editors/vscode");
  const english = JSON.parse(readFileSync(resolve(extension, "package.nls.json"), "utf8")) as Record<string, string>;
  const translated = JSON.parse(readFileSync(resolve(extension, "package.nls.zh-cn.json"), "utf8")) as Record<string, string>;
  const manifest = readFileSync(resolve(extension, "package.json"), "utf8");
  const originals = Object.fromEntries(sharedRows.map(([key, en]) => [key, en]));
  assert.deepEqual(Object.keys(english).sort(), Object.keys(translated).sort());
  for (const [, key] of manifest.matchAll(/%([\w.]+)%/g)) {
    assert.equal(english[key], originals[key], key);
    assert.equal(translated[key], chinese.strings[key], key);
    assert.ok(/[\u3400-\u9fff]/.test(translated[key]), key);
  }
});

test("every static panel label and accessible description is marked for translation", () => {
  const source = readFileSync(resolve(repository, "editors/vscode/src/chatView.ts"), "utf8");
  const html = source.slice(source.indexOf('<html lang='), source.indexOf('</html>'));
  const markers: Record<string, string> = {
    title: "title", "aria-label": "aria", placeholder: "placeholder", alt: "alt",
  };
  for (const [tag] of html.matchAll(/<[\w-]+\b[^<>]*>/g)) {
    for (const [attribute, marker] of Object.entries(markers)) {
      const value = new RegExp(`\\b${attribute}="([^"]+)"`).exec(tag)?.[1];
      if (!value || !/[a-z]/i.test(value) || value.includes("${")) continue;
      const key = new RegExp(`data-i18n-${marker}="([^"]+)"`).exec(tag)?.[1];
      assert.ok(key, `unmarked ${attribute}: ${tag}`);
      assert.ok(chinese.strings[key], `missing catalog key: ${key}`);
    }
  }
  for (const [, tag, attributes, text] of html.matchAll(/<([\w-]+)([^<>]*)>([^<>]+)<\/\1>/g)) {
    if (!/[a-z]/i.test(text) || text.includes("${") || text === "Oxide") continue;
    assert.match(attributes, /data-i18n(?:-code)?="/, `unmarked ${tag} text: ${text}`);
  }
  assert.match(chinese.strings["panel.sharedRuntime"], /共用配置、会话和项目信任设置/);
  assert.ok(chinese.strings["panel.sharedRuntime"].includes("`--mode rpc`"));
});

test("localized transcript notices preserve provider diagnostics and user text", () => {
  const transcript = new Transcript(undefined, (key, fallback, vars) => translate(chinese, key, fallback, vars));
  const compact = transcript.apply({ type: "compaction", summarized: 12, tokensBefore: 1200 })[0];
  assert.equal(compact.k, "push");
  if (compact.k === "push" && compact.item.kind === "notice") {
    assert.equal(compact.item.text, "已压缩前面的 12 条消息（约 1.2k 个 token）");
  } else assert.fail("compaction notice");
  const failure = transcript.apply({ type: "error", message: "Original provider diagnostic" })[0];
  if (failure.k === "push" && failure.item.kind === "notice") {
    assert.equal(failure.item.text, "错误：Original provider diagnostic");
  } else assert.fail("error notice");
  transcript.pushUser("Original English request", []);
  assert.equal(transcript.items.at(-1)?.kind, "user");
  const user = transcript.items.at(-1)!;
  if (user.kind === "user") assert.equal(user.text, "Original English request");
});
