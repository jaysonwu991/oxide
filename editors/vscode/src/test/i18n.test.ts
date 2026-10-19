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
