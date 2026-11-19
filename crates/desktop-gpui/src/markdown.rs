//! The transcript's markdown.
//!
//! The page paints a model's answer with a markdown renderer; this is the
//! subset the transcript needs — headings, paragraphs, fenced code, lists,
//! quotes, rules and the inline emphasis, code and links inside them. Parsing
//! produces plain blocks the view paints with text runs, so a streaming answer
//! re-parses only its own last block set.

/// One styled run of inline text.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Inline {
    pub text: String,
    pub bold: bool,
    pub italic: bool,
    pub code: bool,
    pub strike: bool,
    pub link: Option<String>,
}

impl Inline {
    fn new() -> Self {
        Self::default()
    }
}

/// One block of a markdown document.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Block {
    Paragraph(Vec<Inline>),
    Heading {
        level: usize,
        spans: Vec<Inline>,
    },
    Code {
        language: String,
        text: String,
    },
    Quote(Vec<Inline>),
    Bullet {
        depth: usize,
        checked: Option<bool>,
        spans: Vec<Inline>,
    },
    Table {
        header: Vec<Vec<Inline>>,
        rows: Vec<Vec<Vec<Inline>>>,
    },
    Numbered {
        depth: usize,
        number: String,
        spans: Vec<Inline>,
    },
    Rule,
}

/// Parses a document into the blocks the transcript paints.
pub fn parse(text: &str) -> Vec<Block> {
    let mut blocks = Vec::new();
    let mut lines = text.lines().peekable();
    let mut paragraph: Vec<String> = Vec::new();
    let mut quote_open = false;

    fn flush(paragraph: &mut Vec<String>, blocks: &mut Vec<Block>) {
        if paragraph.is_empty() {
            return;
        }
        let text = paragraph.join("\n");
        paragraph.clear();
        blocks.push(Block::Paragraph(parse_inline(&text)));
    }

    while let Some(line) = lines.next() {
        let trimmed = line.trim_start();
        let fence_run = ["```", "~~~"]
            .into_iter()
            .find(|run| trimmed.starts_with(run));
        if let Some(run) = fence_run {
            let fence = &trimmed[run.len()..];
            flush(&mut paragraph, &mut blocks);
            let language = fence.trim().to_string();
            let mut code = String::new();
            for line in lines.by_ref() {
                if line.trim_start().starts_with(run) {
                    break;
                }
                if !code.is_empty() {
                    code.push('\n');
                }
                code.push_str(line);
            }
            blocks.push(Block::Code {
                language,
                text: code,
            });
            continue;
        }

        if trimmed.is_empty() {
            flush(&mut paragraph, &mut blocks);
            quote_open = false;
            continue;
        }

        let hashes = trimmed.chars().take_while(|c| *c == '#').count();
        if (1..=6).contains(&hashes) {
            if let Some(rest) = trimmed[hashes..].strip_prefix(' ') {
                flush(&mut paragraph, &mut blocks);
                blocks.push(Block::Heading {
                    level: hashes,
                    spans: parse_inline(rest.trim().trim_end_matches('#').trim_end()),
                });
                continue;
            }
        }
        if trimmed == "---" || trimmed == "***" || trimmed == "___" {
            flush(&mut paragraph, &mut blocks);
            blocks.push(Block::Rule);
            continue;
        }
        if let Some(rest) = trimmed
            .strip_prefix("> ")
            .or_else(|| trimmed.strip_prefix('>'))
        {
            flush(&mut paragraph, &mut blocks);
            match blocks.last_mut() {
                Some(Block::Quote(spans)) if quote_open => {
                    spans.push(Inline {
                        text: "\n".to_string(),
                        ..Inline::new()
                    });
                    spans.extend(parse_inline(rest));
                }
                _ => blocks.push(Block::Quote(parse_inline(rest))),
            }
            quote_open = true;
            continue;
        }
        quote_open = false;
        if trimmed.contains('|') && paragraph.is_empty() {
            if let Some(separator) = lines.peek() {
                if is_table_separator(separator) {
                    let header: Vec<Vec<Inline>> =
                        split_row(trimmed).iter().map(|c| parse_inline(c)).collect();
                    lines.next();
                    let mut rows = Vec::new();
                    while let Some(row) = lines.peek() {
                        if row.trim().is_empty() || !row.contains('|') {
                            break;
                        }
                        rows.push(split_row(row).iter().map(|c| parse_inline(c)).collect());
                        lines.next();
                    }
                    blocks.push(Block::Table { header, rows });
                    continue;
                }
            }
        }
        let indent = line.len() - trimmed.len();
        let depth = (indent / 2).min(3);
        if let Some(rest) = trimmed
            .strip_prefix("- ")
            .or_else(|| trimmed.strip_prefix("* "))
            .or_else(|| trimmed.strip_prefix("+ "))
        {
            flush(&mut paragraph, &mut blocks);
            let (checked, rest) = if let Some(rest) = rest.strip_prefix("[ ] ") {
                (Some(false), rest)
            } else if let Some(rest) = rest
                .strip_prefix("[x] ")
                .or_else(|| rest.strip_prefix("[X] "))
            {
                (Some(true), rest)
            } else {
                (None, rest)
            };
            blocks.push(Block::Bullet {
                depth,
                checked,
                spans: parse_inline(rest),
            });
            continue;
        }
        if let Some((number, rest)) = numbered(trimmed) {
            flush(&mut paragraph, &mut blocks);
            blocks.push(Block::Numbered {
                depth,
                number,
                spans: parse_inline(rest),
            });
            continue;
        }

        paragraph.push(line.trim_end().to_string());
    }
    flush(&mut paragraph, &mut blocks);
    blocks
}

/// The `)` that closes a link target, past any parentheses the URL itself
/// carries, so `…/Foo_(bar)` keeps its tail.
fn closing_paren(url: &str) -> Option<usize> {
    let mut depth = 0usize;
    for (index, ch) in url.char_indices() {
        match ch {
            '(' => depth += 1,
            ')' if depth == 0 => return Some(index),
            ')' => depth -= 1,
            _ => {}
        }
    }
    None
}

fn split_row(line: &str) -> Vec<String> {
    let line = line.trim();
    let line = line.strip_prefix('|').unwrap_or(line);
    let line = line.strip_suffix('|').unwrap_or(line);
    line.split('|')
        .map(|cell| cell.trim().to_string())
        .collect()
}

fn is_table_separator(line: &str) -> bool {
    let line = line.trim();
    line.contains('-')
        && line.contains('|') | line.contains(':')
        && line
            .chars()
            .all(|c| matches!(c, '|' | '-' | ':' | ' ' | '\t'))
}

fn numbered(line: &str) -> Option<(String, &str)> {
    let digits: String = line.chars().take_while(|c| c.is_ascii_digit()).collect();
    if digits.is_empty() {
        return None;
    }
    let rest = &line[digits.len()..];
    let rest = rest
        .strip_prefix(". ")
        .or_else(|| rest.strip_prefix(") "))?;
    Some((format!("{digits}."), rest))
}

/// Parses emphasis, code, strikethrough and links out of one line of text.
pub fn parse_inline(text: &str) -> Vec<Inline> {
    parse_styled(text, Inline::new())
}

/// The same parse with a style every span inherits, so a link or a code span
/// inside `**…**` keeps the bold it was written in instead of being emitted as
/// the literal text between the markers.
fn parse_styled(text: &str, style: Inline) -> Vec<Inline> {
    fn emit(current: &mut Inline, text: &str) {
        current.text.push_str(text);
    }

    let base = style;
    let flush = |spans: &mut Vec<Inline>, current: &mut Inline| {
        if !current.text.is_empty() {
            spans.push(current.clone());
            *current = base.clone();
        }
    };

    let mut spans: Vec<Inline> = Vec::new();
    let mut current = base.clone();
    let mut index = 0;

    while index < text.len() {
        let ch = text[index..].chars().next().unwrap();
        match ch {
            '\\' => {
                let escaped = text[index + 1..].chars().next();
                match escaped {
                    Some(next) if next.is_ascii_punctuation() => {
                        emit(&mut current, &next.to_string());
                        index += 1 + next.len_utf8();
                    }
                    _ => {
                        emit(&mut current, "\\");
                        index += 1;
                    }
                }
            }
            '`' => match text[index + 1..].find('`') {
                Some(offset) => {
                    flush(&mut spans, &mut current);
                    spans.push(Inline {
                        text: text[index + 1..index + 1 + offset].to_string(),
                        code: true,
                        ..base.clone()
                    });
                    index = index + 1 + offset + 1;
                }
                None => {
                    emit(&mut current, "`");
                    index += 1;
                }
            },
            '*' | '_' => {
                let doubled = text[index + 1..].starts_with(ch);
                let marker = if doubled { 2 } else { 1 };
                let closer = if doubled {
                    if ch == '*' {
                        "**"
                    } else {
                        "__"
                    }
                } else if ch == '*' {
                    "*"
                } else {
                    "_"
                };
                let search_start = index + marker;
                let found = text[search_start..]
                    .find(closer)
                    .filter(|offset| *offset > 0);
                match found {
                    Some(offset) => {
                        flush(&mut spans, &mut current);
                        let mut emphasis = base.clone();
                        if doubled {
                            emphasis.bold = true;
                        } else {
                            emphasis.italic = true;
                        }
                        spans.extend(parse_styled(
                            &text[search_start..search_start + offset],
                            emphasis,
                        ));
                        index = search_start + offset + marker;
                    }
                    None => {
                        emit(&mut current, &ch.to_string());
                        index += ch.len_utf8();
                    }
                }
            }
            '~' => match text[index + 1..].starts_with('~') {
                true => match text[index + 2..].find("~~") {
                    Some(offset) if offset > 0 => {
                        flush(&mut spans, &mut current);
                        spans.extend(parse_styled(
                            &text[index + 2..index + 2 + offset],
                            Inline {
                                strike: true,
                                ..base.clone()
                            },
                        ));
                        index = index + 2 + offset + 2;
                    }
                    _ => {
                        emit(&mut current, "~");
                        index += 1;
                    }
                },
                false => {
                    emit(&mut current, "~");
                    index += 1;
                }
            },
            '[' => {
                let tail = &text[index + 1..];
                let target = tail.find(']').and_then(|label_end| {
                    let after = &tail[label_end + 1..];
                    let url = after.strip_prefix('(')?;
                    let url_end = closing_paren(url)?;
                    Some((label_end, url_end, &tail[..label_end], &url[..url_end]))
                });
                match target {
                    Some((label_end, url_end, label, url)) => {
                        flush(&mut spans, &mut current);
                        spans.extend(parse_styled(
                            label,
                            Inline {
                                link: Some(url.to_string()),
                                ..base.clone()
                            },
                        ));
                        index = index + 1 + label_end + 1 + 1 + url_end + 1;
                    }
                    None => {
                        emit(&mut current, "[");
                        index += 1;
                    }
                }
            }
            other => {
                emit(&mut current, &other.to_string());
                index += other.len_utf8();
            }
        }
    }
    flush(&mut spans, &mut current);
    spans
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn emphasis_code_and_links_become_spans() {
        let spans = parse_inline("a **bold** and `code` and [link](https://example.com)");
        assert_eq!(spans.len(), 6);
        assert_eq!(spans[1].text, "bold");
        assert!(spans[1].bold);
        assert_eq!(spans[3].text, "code");
        assert!(spans[3].code);
        assert_eq!(spans[5].text, "link");
        assert_eq!(spans[5].link.as_deref(), Some("https://example.com"));
    }

    #[test]
    fn a_link_inside_emphasis_stays_a_link() {
        let spans =
            parse_inline("PR opened: **[#352 — Remove it](https://github.com/o/r/pull/352)**");
        assert_eq!(spans.last().unwrap().text, "#352 — Remove it");
        let link = spans.last().unwrap();
        assert!(link.bold);
        assert_eq!(
            link.link.as_deref(),
            Some("https://github.com/o/r/pull/352")
        );
        assert!(!spans.iter().any(|span| span.text.contains("](")));
    }

    #[test]
    fn emphasis_inside_a_link_label_keeps_both() {
        let spans = parse_inline("[**bold** and plain](https://example.com)");
        assert_eq!(spans.len(), 2);
        assert!(spans[0].bold);
        assert_eq!(spans[0].link.as_deref(), Some("https://example.com"));
        assert!(!spans[1].bold);
        assert_eq!(spans[1].link.as_deref(), Some("https://example.com"));
    }

    #[test]
    fn a_link_target_keeps_the_parentheses_it_carries() {
        let spans = parse_inline("[bar](https://en.wikipedia.org/wiki/Foo_(bar)) after");
        assert_eq!(
            spans[0].link.as_deref(),
            Some("https://en.wikipedia.org/wiki/Foo_(bar)")
        );
        assert_eq!(spans[1].text, " after");
    }

    #[test]
    fn code_and_strikethrough_keep_the_emphasis_around_them() {
        let spans = parse_inline("**run `cargo test` ~~now~~**");
        let code = spans.iter().find(|span| span.code).unwrap();
        assert_eq!(code.text, "cargo test");
        assert!(code.bold);
        let strike = spans.iter().find(|span| span.strike).unwrap();
        assert!(strike.bold);
    }

    #[test]
    fn an_escaped_marker_stays_text() {
        let spans = parse_inline("a \\*literal\\* and \\`code\\`");
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].text, "a *literal* and `code`");
        assert!(!spans[0].italic && !spans[0].code);
    }

    #[test]
    fn plus_markers_make_bullets() {
        let blocks = parse("+ one\n+ two");
        assert_eq!(blocks.len(), 2);
        assert!(matches!(blocks[0], Block::Bullet { .. }));
    }

    #[test]
    fn a_fence_is_one_code_block() {
        let blocks = parse("before\n```rust\nfn main() {}\n```\nafter");
        assert_eq!(blocks.len(), 3);
        match &blocks[1] {
            Block::Code { language, text } => {
                assert_eq!(language, "rust");
                assert_eq!(text, "fn main() {}");
            }
            other => panic!("expected code, got {other:?}"),
        }
    }

    #[test]
    fn lists_and_headings_keep_their_shape() {
        let blocks = parse("## Title\n- one\n1. first\n> quoted\n---");
        assert!(matches!(blocks[0], Block::Heading { level: 2, .. }));
        assert!(matches!(blocks[1], Block::Bullet { .. }));
        assert!(matches!(blocks[2], Block::Numbered { .. }));
        assert!(matches!(blocks[3], Block::Quote(_)));
        assert!(matches!(blocks[4], Block::Rule));
    }

    #[test]
    fn tables_tasks_and_deep_headings_parse() {
        let blocks = parse("| a | b |\n|---|---|\n| 1 | **2** |\n\n- [x] done\n\n#### Four\n\n~~~mermaid\ngraph TD\n~~~");
        match &blocks[0] {
            Block::Table { header, rows } => {
                assert_eq!(header.len(), 2);
                assert_eq!(rows.len(), 1);
                assert!(rows[0][1][0].bold);
            }
            other => panic!("expected table, got {other:?}"),
        }
        assert!(matches!(
            blocks[1],
            Block::Bullet {
                checked: Some(true),
                ..
            }
        ));
        assert!(matches!(blocks[2], Block::Heading { level: 4, .. }));
        assert!(matches!(&blocks[3], Block::Code { language, .. } if language == "mermaid"));
    }
}
