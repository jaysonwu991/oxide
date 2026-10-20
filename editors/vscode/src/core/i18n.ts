// The language the panel is drawn in, read from the CLI.
//
// The catalog is the core's (`oxide_core::i18n`), and this package cannot link
// it, so the panel asks the installed CLI for the answer — the language the
// shared settings resolve and the strings to draw in it. A window that has
// never answered paints the English it was authored in, which is what a CLI too
// old to know `oxide language` gives.

import { parseObject } from "./json";

export interface LanguageView {
  tag: string;
  label: string;
  nativeLabel: string;
}

export interface LanguageInfo {
  /// The language in use, as its tag (`en-GB`, `zh-CN`).
  language: string;
  /// The ones a picker offers.
  languages: LanguageView[];
  /// The strings to draw, keyed the way the core names them.
  strings: Record<string, string>;
}

/// The language the app is authored in, which every fallback reads as.
export const DEFAULT_LANGUAGE = "en-GB";

/// The two languages, for the moment before the CLI has answered.
export const FALLBACK_LANGUAGES: LanguageView[] = [
  { tag: "en-GB", label: "English (United Kingdom)", nativeLabel: "English (United Kingdom)" },
  { tag: "zh-CN", label: "Simplified Chinese", nativeLabel: "简体中文" },
];

/// What the panel runs to read the language and the strings in it.
export function languageArgs(): string[] {
  return ["language", "--json"];
}

/// What the panel runs to switch it, which writes the shared `settings.json`
/// key the terminal and the desktop app read. The CLI answers with the catalog
/// in the new language, so the panel repaints from its answer.
export function languageSetArgs(tag: string): string[] {
  return ["language", "set", tag, "--json"];
}

/// Reads the CLI's answer. Anything that is not a language tag reads as no
/// answer, so the panel keeps the language it was drawing rather than blanking.
export function parseLanguageInfo(raw: string): LanguageInfo | null {
  const root = parseObject(raw);
  if (!root) return null;
  const language = typeof root.language === "string" ? root.language : "";
  if (!language) return null;
  const languages: LanguageView[] = [];
  if (Array.isArray(root.languages)) {
    for (const entry of root.languages) {
      if (!entry || typeof entry !== "object") continue;
      const view = entry as Record<string, unknown>;
      if (typeof view.tag !== "string") continue;
      languages.push({
        tag: view.tag,
        label: typeof view.label === "string" ? view.label : view.tag,
        nativeLabel: typeof view.nativeLabel === "string" ? view.nativeLabel : view.tag,
      });
    }
  }
  const strings: Record<string, string> = {};
  if (root.strings && typeof root.strings === "object") {
    for (const [key, value] of Object.entries(root.strings as Record<string, unknown>)) {
      if (typeof value === "string") strings[key] = value;
    }
  }
  return {
    language,
    languages: languages.length ? languages : FALLBACK_LANGUAGES,
    strings,
  };
}

/// One word of the interface: the catalog's, else the English the panel was
/// written in. `{name}` placeholders are filled from `vars`.
export function translate(
  info: LanguageInfo | null,
  key: string,
  fallback: string,
  vars?: Record<string, unknown>,
): string {
  let text = info?.strings?.[key] || fallback;
  if (vars) {
    for (const [name, value] of Object.entries(vars)) {
      text = text.split(`{${name}}`).join(String(value));
    }
  }
  return text;
}
