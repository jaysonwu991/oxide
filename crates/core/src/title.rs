//! One-line titles: the summarized name a front-end shows a thread, a session
//! or a finished turn under.
//!
//! The terminal, the desktop app and the VS Code panel all name the same
//! conversation, so they agree on these rules. The panel is TypeScript and
//! cannot link this crate, so `summarizeTitle` in `editors/vscode/src/core/
//! protocol.ts` mirrors them, and `editors/vscode/src/test/protocol.test.ts`
//! and the tests below hold the two in step with the same cases.

/// How long a title may be, in characters: short enough to read in a side bar,
/// a session picker row and a notification.
pub const TITLE_LIMIT: usize = 64;

/// Condenses `text` into a one-line title, or `""` when it carries nothing a
/// title could be made of.
pub fn summarize(text: &str, max: usize) -> String {
    match first_prose_line(text) {
        Some(line) => bounded(&line, max),
        None => String::new(),
    }
}

/// The first line of `text` that reads as prose, with its Markdown taken off: a
/// fence, a rule and a table row say nothing on their own, and a fenced block's
/// contents are a listing rather than a title.
fn first_prose_line(text: &str) -> Option<String> {
    let mut fenced = false;
    for raw in text.lines() {
        let line = raw.trim();
        if line.starts_with("```") || line.starts_with("~~~") {
            fenced = !fenced;
            continue;
        }
        if fenced || line.is_empty() || line.starts_with('|') {
            continue;
        }
        let plain = plain_text(line);
        // A line that is only punctuation (`---`) is a rule, not a title.
        if plain.chars().any(char::is_alphanumeric) {
            return Some(plain);
        }
    }
    None
}

/// One line with its Markdown markers removed: a link or an image keeps its
/// text, and the emphasis, code and quote markers go.
fn plain_text(line: &str) -> String {
    let line = strip_links(strip_block_marker(line));
    let line: String = line
        .chars()
        .filter(|c| !matches!(c, '`' | '*' | '_' | '~'))
        .collect();
    line.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// A leading heading, quote, bullet or ordered-item marker, including several
/// in a row (`> - ### x`), as the Markdown they are.
fn strip_block_marker(line: &str) -> &str {
    let mut rest = line;
    loop {
        let trimmed = rest.trim_start();
        let next = trimmed
            .strip_prefix('>')
            .or_else(|| heading_marker(trimmed))
            .or_else(|| bullet_marker(trimmed))
            .or_else(|| ordered_marker(trimmed));
        match next {
            // Each helper consumes its marker and the whitespace that ends it,
            // so a marker that is part of a word (`#hashtag`, `2.5`, `--flag`)
            // is left alone, and every round shortens the line.
            Some(after) => rest = after,
            None => return trimmed,
        }
    }
}

/// `# ` through `###### `.
fn heading_marker(line: &str) -> Option<&str> {
    let hashes = line.chars().take_while(|c| *c == '#').count();
    if hashes == 0 || hashes > 6 {
        return None;
    }
    line[hashes..].strip_prefix(char::is_whitespace)
}

/// `- `, `* ` or `+ `.
fn bullet_marker(line: &str) -> Option<&str> {
    if !matches!(line.chars().next(), Some('-' | '*' | '+')) {
        return None;
    }
    line[1..].strip_prefix(char::is_whitespace)
}

/// `1. ` or `1) `.
fn ordered_marker(line: &str) -> Option<&str> {
    let digits = line.chars().take_while(char::is_ascii_digit).count();
    if digits == 0 {
        return None;
    }
    let rest = &line[digits..];
    let after = rest.strip_prefix('.').or_else(|| rest.strip_prefix(')'))?;
    after.strip_prefix(char::is_whitespace)
}

/// `[text](url)` and `![alt](url)` as `text` and `alt`.
fn strip_links(line: &str) -> String {
    let chars: Vec<char> = line.chars().collect();
    let mut out = String::new();
    let mut index = 0;
    while index < chars.len() {
        if chars[index] == '[' {
            if let Some(close) = (index + 1..chars.len()).find(|i| chars[*i] == ']') {
                if chars.get(close + 1) == Some(&'(') {
                    if let Some(end) = (close + 2..chars.len()).find(|i| chars[*i] == ')') {
                        // The `!` of an image is part of the marker, not text.
                        if index > 0 && chars[index - 1] == '!' {
                            out.pop();
                        }
                        out.extend(&chars[index + 1..close]);
                        index = end + 1;
                        continue;
                    }
                }
            }
        }
        out.push(chars[index]);
        index += 1;
    }
    out
}

/// Truncates to `max` characters at a word boundary, so a title never ends
/// mid-word, and marks the cut with an ellipsis.
fn bounded(text: &str, max: usize) -> String {
    if max == 0 {
        return String::new();
    }
    if text.chars().count() <= max {
        return text.to_string();
    }
    let cut: String = text.chars().take(max - 1).collect();
    let kept = match cut.rfind(' ') {
        Some(index) if index > 0 => &cut[..index],
        _ => cut.as_str(),
    };
    format!("{}…", kept.trim_end())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The cases `summarizeTitle`'s own test asserts, so the two stay in step.
    #[test]
    fn summarizes_prose_rather_than_markdown() {
        assert_eq!(
            summarize("### Fix the **flaky** test", TITLE_LIMIT),
            "Fix the flaky test"
        );
        assert_eq!(
            summarize(
                "See [the docs](https://example.com/x) and `cargo test`",
                TITLE_LIMIT
            ),
            "See the docs and cargo test"
        );
        assert_eq!(summarize("> quoted request", TITLE_LIMIT), "quoted request");
        assert_eq!(summarize("- first item", TITLE_LIMIT), "first item");
        assert_eq!(
            summarize("```rust\nfn main() {}\n```\nRun this", TITLE_LIMIT),
            "Run this"
        );
        assert_eq!(
            summarize("---\n| a | b |\nWhitespace   collapsed", TITLE_LIMIT),
            "Whitespace collapsed"
        );
        assert_eq!(summarize("   ", TITLE_LIMIT), "");
        assert_eq!(summarize("", TITLE_LIMIT), "");
    }

    #[test]
    fn cuts_a_long_title_at_a_word_boundary() {
        let long = summarize(&"word ".repeat(30), TITLE_LIMIT);
        assert!(long.chars().count() <= TITLE_LIMIT, "{long}");
        assert!(long.ends_with("word…"), "{long}");

        // A single huge word has no boundary to cut at, and is cut anyway.
        let one_word = summarize(&"x".repeat(400), TITLE_LIMIT);
        assert_eq!(one_word.chars().count(), TITLE_LIMIT);
        assert!(one_word.ends_with('…'));
    }

    #[test]
    fn keeps_a_marker_that_is_part_of_a_word() {
        assert_eq!(
            summarize("#hashtag and 2.5 --flag", TITLE_LIMIT),
            "#hashtag and 2.5 --flag"
        );
        assert_eq!(summarize("1. first\n2. second", TITLE_LIMIT), "first");
        assert_eq!(summarize("> - ### deep", TITLE_LIMIT), "deep");
    }

    #[test]
    fn takes_the_first_line_that_says_something() {
        assert_eq!(summarize("\n\n## Heading\n\nbody", TITLE_LIMIT), "Heading");
        assert_eq!(summarize("![alt text](a.png)", TITLE_LIMIT), "alt text");
        assert_eq!(summarize("---\n***", TITLE_LIMIT), "");
    }
}
