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
        spans: Vec<Inline>,
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
        if let Some(fence) = trimmed.strip_prefix("```") {
            flush(&mut paragraph, &mut blocks);
            let language = fence.trim().to_string();
            let mut code = String::new();
            for line in lines.by_ref() {
                if line.trim_start().starts_with("```") {
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
            continue;
        }

        if let Some(rest) = trimmed.strip_prefix("### ") {
            flush(&mut paragraph, &mut blocks);
            blocks.push(Block::Heading {
                level: 3,
                spans: parse_inline(rest),
            });
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("## ") {
            flush(&mut paragraph, &mut blocks);
            blocks.push(Block::Heading {
                level: 2,
                spans: parse_inline(rest),
            });
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("# ") {
            flush(&mut paragraph, &mut blocks);
            blocks.push(Block::Heading {
                level: 1,
                spans: parse_inline(rest),
            });
            continue;
        }
        if trimmed == "---" || trimmed == "***" || trimmed == "___" {
            flush(&mut paragraph, &mut blocks);
            blocks.push(Block::Rule);
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("> ") {
            flush(&mut paragraph, &mut blocks);
            blocks.push(Block::Quote(parse_inline(rest)));
            continue;
        }
        let indent = line.len() - trimmed.len();
        let depth = (indent / 2).min(3);
        if let Some(rest) = trimmed
            .strip_prefix("- ")
            .or_else(|| trimmed.strip_prefix("* "))
        {
            flush(&mut paragraph, &mut blocks);
            blocks.push(Block::Bullet {
                depth,
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
    fn emit(current: &mut Inline, text: &str) {
        current.text.push_str(text);
    }

    fn flush(spans: &mut Vec<Inline>, current: &mut Inline) {
        if !current.text.is_empty() {
            spans.push(current.clone());
            *current = Inline::new();
        }
    }

    let mut spans: Vec<Inline> = Vec::new();
    let mut current = Inline::new();
    let mut index = 0;

    while index < text.len() {
        let ch = text[index..].chars().next().unwrap();
        match ch {
            '`' => match text[index + 1..].find('`') {
                Some(offset) => {
                    flush(&mut spans, &mut current);
                    spans.push(Inline {
                        text: text[index + 1..index + 1 + offset].to_string(),
                        code: true,
                        ..Inline::new()
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
                        spans.push(Inline {
                            text: text[search_start..search_start + offset].to_string(),
                            bold: doubled,
                            italic: !doubled,
                            ..Inline::new()
                        });
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
                        spans.push(Inline {
                            text: text[index + 2..index + 2 + offset].to_string(),
                            strike: true,
                            ..Inline::new()
                        });
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
                    let url_end = url.find(')')?;
                    Some((label_end, url_end, &tail[..label_end], &url[..url_end]))
                });
                match target {
                    Some((label_end, url_end, label, url)) => {
                        flush(&mut spans, &mut current);
                        spans.push(Inline {
                            text: label.to_string(),
                            link: Some(url.to_string()),
                            ..Inline::new()
                        });
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
}
