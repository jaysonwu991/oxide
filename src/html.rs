//! Dependency-free HTML to Markdown (and plain text) conversion, used by the
//! `webfetch` tool. The parser is intentionally small: it walks tags into a
//! lightweight tree, keeps the common structural and inline elements, decodes
//! entities, and normalizes whitespace. That is enough to turn a web page into
//! readable Markdown for the model without pulling in an HTML crate.

/// Output flavor for [`to_markdown`] / [`to_text`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Markdown,
    Text,
}

/// Converts an HTML document to Markdown.
pub fn to_markdown(html: &str) -> String {
    convert(html, Format::Markdown)
}

/// Converts an HTML document to readable plain text (structure kept, inline
/// Markdown syntax omitted).
pub fn to_text(html: &str) -> String {
    convert(html, Format::Text)
}

fn convert(html: &str, mode: Format) -> String {
    let nodes = parse(html);
    let mut out = String::with_capacity(html.len() / 2);
    render_nodes(&nodes, &mut out, mode, false);
    normalize(&out)
}

// ---------------------------------------------------------------------------
// Parser
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
enum Node {
    Text(String),
    Element(Element),
}

#[derive(Debug, Clone)]
struct Element {
    tag: String,
    attrs: Vec<(String, String)>,
    children: Vec<Node>,
}

fn parse(html: &str) -> Vec<Node> {
    let mut root: Vec<Node> = Vec::new();
    let mut stack: Vec<Element> = Vec::new();
    let len = html.len();
    let mut i = 0;

    while i < len {
        let rest = &html[i..];
        if rest.starts_with('<') {
            if rest.starts_with("<!--") {
                match rest.find("-->") {
                    Some(end) => i += end + 3,
                    None => break,
                }
                continue;
            }
            if rest.starts_with("<!") || rest.starts_with("<?") {
                match rest.find('>') {
                    Some(end) => i += end + 1,
                    None => break,
                }
                continue;
            }
            if let Some(after) = rest.strip_prefix("</") {
                if let Some(end) = after.find('>') {
                    let name = tag_name(&after[..end]);
                    close_element(&mut stack, &mut root, &name);
                    i += 2 + end + 1;
                } else {
                    break;
                }
                continue;
            }
            let second = rest.as_bytes().get(1).copied().unwrap_or(0);
            if second.is_ascii_alphabetic() {
                if let Some(end) = find_tag_end(rest) {
                    let (name, attrs, self_closing) = parse_open_tag(&rest[1..end]);
                    if !name.is_empty() {
                        if self_closing || is_void(&name) {
                            attach(
                                &mut stack,
                                &mut root,
                                Element {
                                    tag: name,
                                    attrs,
                                    children: Vec::new(),
                                },
                            );
                        } else {
                            stack.push(Element {
                                tag: name,
                                attrs,
                                children: Vec::new(),
                            });
                        }
                    }
                    i += end + 1;
                    continue;
                }
                break;
            }
        }

        // Literal text up to the next `<`. When the current character is a
        // literal `<` (not a tag), step past it by one byte; otherwise start at
        // the current (possibly multi-byte) character.
        let search_from = if rest.starts_with('<') { 1 } else { 0 };
        let next = rest[search_from..]
            .find('<')
            .map(|p| i + search_from + p)
            .unwrap_or(len);
        attach_text(&mut stack, &mut root, html[i..next].to_string());
        i = next;
    }

    while let Some(el) = stack.pop() {
        attach(&mut stack, &mut root, el);
    }
    root
}

fn find_tag_end(s: &str) -> Option<usize> {
    let bytes = s.as_bytes();
    let mut quote = 0u8;
    let mut i = 1;
    while i < bytes.len() {
        let b = bytes[i];
        if quote != 0 {
            if b == quote {
                quote = 0;
            }
        } else if b == b'"' || b == b'\'' {
            quote = b;
        } else if b == b'>' {
            return Some(i);
        }
        i += 1;
    }
    None
}

fn parse_open_tag(raw: &str) -> (String, Vec<(String, String)>, bool) {
    let trimmed = raw.trim();
    let self_closing = trimmed.ends_with('/');
    let body = trimmed.trim_end_matches('/');
    let name_end = body.find(char::is_whitespace).unwrap_or(body.len());
    let name = body[..name_end].to_ascii_lowercase();
    let attrs = parse_attrs(&body[name_end..]);
    (name, attrs, self_closing)
}

fn parse_attrs(mut s: &str) -> Vec<(String, String)> {
    let mut attrs = Vec::new();
    loop {
        s = s.trim_start();
        if s.is_empty() {
            break;
        }
        let key_end = s
            .find(|c: char| c.is_whitespace() || c == '=')
            .unwrap_or(s.len());
        let key = s[..key_end].to_ascii_lowercase();
        s = s[key_end..].trim_start();
        if let Some(rest) = s.strip_prefix('=') {
            s = rest.trim_start();
            let (value, after) = match s.chars().next() {
                Some(quote @ ('"' | '\'')) => {
                    let inner = &s[quote.len_utf8()..];
                    match inner.find(quote) {
                        Some(pos) => (inner[..pos].to_string(), &inner[pos + quote.len_utf8()..]),
                        None => (inner.to_string(), ""),
                    }
                }
                _ => {
                    let end = s.find(char::is_whitespace).unwrap_or(s.len());
                    (s[..end].to_string(), &s[end..])
                }
            };
            if !key.is_empty() {
                attrs.push((key, value));
            }
            s = after;
        } else if !key.is_empty() {
            attrs.push((key, String::new()));
        } else {
            // Nothing consumed; avoid an infinite loop on malformed input.
            s = &s[s.chars().next().map(char::len_utf8).unwrap_or(1)..];
        }
    }
    attrs
}

fn tag_name(raw: &str) -> String {
    raw.trim()
        .split(|c: char| c.is_whitespace() || c == '/')
        .next()
        .unwrap_or("")
        .to_ascii_lowercase()
}

fn attach(stack: &mut [Element], root: &mut Vec<Node>, el: Element) {
    match stack.last_mut() {
        Some(parent) => parent.children.push(Node::Element(el)),
        None => root.push(Node::Element(el)),
    }
}

fn attach_text(stack: &mut [Element], root: &mut Vec<Node>, text: String) {
    match stack.last_mut() {
        Some(parent) => parent.children.push(Node::Text(text)),
        None => root.push(Node::Text(text)),
    }
}

fn close_element(stack: &mut Vec<Element>, root: &mut Vec<Node>, name: &str) {
    let Some(position) = stack.iter().rposition(|el| el.tag == name) else {
        return;
    };
    while stack.len() > position {
        let el = stack.pop().expect("stack is non-empty");
        attach(stack, root, el);
    }
}

fn is_void(tag: &str) -> bool {
    matches!(
        tag,
        "area"
            | "base"
            | "br"
            | "col"
            | "embed"
            | "hr"
            | "img"
            | "input"
            | "link"
            | "meta"
            | "param"
            | "source"
            | "track"
            | "wbr"
    )
}

fn is_block_container(tag: &str) -> bool {
    matches!(
        tag,
        "html"
            | "body"
            | "div"
            | "section"
            | "article"
            | "main"
            | "header"
            | "footer"
            | "aside"
            | "nav"
            | "figure"
            | "figcaption"
            | "details"
            | "summary"
            | "address"
            | "fieldset"
            | "form"
            | "center"
            | "dialog"
            | "hgroup"
    )
}

// ---------------------------------------------------------------------------
// Renderer
// ---------------------------------------------------------------------------

fn render_nodes(nodes: &[Node], out: &mut String, mode: Format, pre: bool) {
    for node in nodes {
        match node {
            Node::Text(text) => {
                let decoded = decode_entities(text);
                if pre {
                    out.push_str(&decoded);
                } else {
                    out.push_str(&collapse_ws(&decoded));
                }
            }
            Node::Element(el) => render_element(el, out, mode, pre),
        }
    }
}

fn render_children(el: &Element, mode: Format) -> String {
    let mut s = String::new();
    render_nodes(&el.children, &mut s, mode, false);
    s
}

fn render_element(el: &Element, out: &mut String, mode: Format, pre: bool) {
    match el.tag.as_str() {
        "script" | "style" | "head" | "noscript" | "template" | "svg" | "iframe" | "canvas"
        | "object" | "embed" | "meta" | "link" | "base" | "title" | "input" | "select"
        | "textarea" | "button" | "option" => {}
        "br" => out.push('\n'),
        "hr" => push_block(out, "---"),
        "p" => push_block(out, &render_children(el, mode)),
        "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
            let level = (el.tag.as_bytes()[1] - b'0') as usize;
            let content = render_children(el, mode);
            let content = content.trim();
            if content.is_empty() {
                return;
            }
            blank_line(out);
            if mode == Format::Markdown {
                out.push_str(&"#".repeat(level));
                out.push(' ');
            }
            out.push_str(content);
            blank_line(out);
        }
        "ul" => render_list(el, out, mode, false, 1),
        "ol" => {
            let start = attr(el, "start")
                .and_then(|value| value.trim().parse::<usize>().ok())
                .unwrap_or(1);
            render_list(el, out, mode, true, start);
        }
        "blockquote" => render_quote(el, out, mode),
        "pre" => render_pre(el, out, mode),
        "table" => render_table(el, out, mode),
        "dl" => render_definition_list(el, out, mode),
        "strong" | "b" => wrap(el, out, mode, "**"),
        "em" | "i" => wrap(el, out, mode, "*"),
        "del" | "s" | "strike" => wrap(el, out, mode, "~~"),
        "mark" => wrap(el, out, mode, "=="),
        "code" | "kbd" | "samp" | "var" | "tt" => render_inline_code(el, out, mode, pre),
        "a" => render_link(el, out, mode),
        "img" => render_image(el, out, mode),
        "caption" => {
            let content = render_children(el, mode);
            let content = content.trim();
            if !content.is_empty() {
                blank_line(out);
                if mode == Format::Markdown {
                    out.push('*');
                    out.push_str(content);
                    out.push('*');
                } else {
                    out.push_str(content);
                }
                blank_line(out);
            }
        }
        _ if is_block_container(&el.tag) => push_block(out, &render_children(el, mode)),
        _ => render_nodes(&el.children, out, mode, pre),
    }
}

fn wrap(el: &Element, out: &mut String, mode: Format, marker: &str) {
    let content = render_children(el, mode);
    let content = content.trim();
    if content.is_empty() {
        return;
    }
    if mode == Format::Markdown {
        out.push_str(marker);
        out.push_str(content);
        out.push_str(marker);
    } else {
        out.push_str(content);
    }
}

fn render_inline_code(el: &Element, out: &mut String, mode: Format, pre: bool) {
    if pre {
        render_nodes(&el.children, out, mode, true);
        return;
    }
    let content = render_children(el, mode);
    let content = content.trim();
    if content.is_empty() {
        return;
    }
    if mode == Format::Markdown {
        let fence = "`".repeat(longest_backtick_run(content) + 1);
        out.push_str(&fence);
        out.push_str(content);
        out.push_str(&fence);
    } else {
        out.push_str(content);
    }
}

fn render_link(el: &Element, out: &mut String, mode: Format) {
    let href = attr(el, "href").unwrap_or_default();
    let content = render_children(el, mode);
    let text = content.trim();
    let usable = mode == Format::Markdown
        && !href.is_empty()
        && !href.starts_with('#')
        && !href
            .trim_start()
            .to_ascii_lowercase()
            .starts_with("javascript:");
    if usable {
        let label = if text.is_empty() { href.as_str() } else { text };
        out.push('[');
        out.push_str(label);
        out.push_str("](");
        out.push_str(&href);
        out.push(')');
    } else {
        out.push_str(text);
    }
}

fn render_image(el: &Element, out: &mut String, mode: Format) {
    let alt = attr(el, "alt").unwrap_or_default();
    let src = attr(el, "src").unwrap_or_default();
    let alt = alt.trim();
    if mode == Format::Markdown && !src.is_empty() {
        out.push_str("![");
        out.push_str(alt);
        out.push_str("](");
        out.push_str(&src);
        out.push(')');
    } else if !alt.is_empty() {
        out.push_str(alt);
    }
}

fn render_list(el: &Element, out: &mut String, mode: Format, ordered: bool, start: usize) {
    let mut index = start;
    let mut rendered = String::new();
    for child in &el.children {
        let Node::Element(li) = child else { continue };
        if li.tag != "li" {
            continue;
        }
        let marker = if ordered {
            let marker = format!("{index}. ");
            index += 1;
            marker
        } else {
            "- ".to_string()
        };

        // Nested lists sit directly under their parent item (no blank line);
        // everything else is inline/block content for the item text.
        let mut content = String::new();
        let mut nested = String::new();
        for grand in &li.children {
            if let Node::Element(g) = grand {
                if g.tag == "ul" || g.tag == "ol" {
                    let mut inner = String::new();
                    render_list(g, &mut inner, mode, g.tag == "ol", 1);
                    let inner = inner.trim();
                    if !inner.is_empty() {
                        if !nested.is_empty() {
                            nested.push('\n');
                        }
                        nested.push_str(inner);
                    }
                    continue;
                }
            }
            render_nodes(std::slice::from_ref(grand), &mut content, mode, false);
        }

        let mut item = content.trim().to_string();
        if !nested.is_empty() {
            if !item.is_empty() {
                item.push('\n');
            }
            item.push_str(&nested);
        }
        if item.trim().is_empty() {
            continue;
        }

        let indent = " ".repeat(marker.len());
        for (position, line) in item.lines().enumerate() {
            if position == 0 {
                rendered.push_str(&marker);
            } else {
                rendered.push_str(&indent);
            }
            rendered.push_str(line);
            rendered.push('\n');
        }
    }
    let rendered = rendered.trim_end();
    if !rendered.is_empty() {
        blank_line(out);
        out.push_str(rendered);
        blank_line(out);
    }
}

fn render_quote(el: &Element, out: &mut String, mode: Format) {
    let content = render_children(el, mode);
    let content = content.trim();
    if content.is_empty() {
        return;
    }
    blank_line(out);
    for line in content.lines() {
        if line.trim().is_empty() {
            out.push('>');
        } else {
            out.push_str("> ");
            out.push_str(line);
        }
        out.push('\n');
    }
    blank_line(out);
}

fn render_pre(el: &Element, out: &mut String, mode: Format) {
    let mut code = String::new();
    render_nodes(&el.children, &mut code, mode, true);
    let code = code.trim_matches('\n');
    blank_line(out);
    if mode == Format::Markdown && !code.is_empty() {
        let fence = "`".repeat((longest_backtick_run(code) + 1).max(3));
        out.push_str(&fence);
        out.push('\n');
        out.push_str(code);
        out.push('\n');
        out.push_str(&fence);
    } else {
        out.push_str(code);
    }
    blank_line(out);
}

fn render_table(el: &Element, out: &mut String, mode: Format) {
    let mut rows: Vec<Vec<String>> = Vec::new();
    collect_rows(el, &mut rows, mode);
    if rows.is_empty() {
        return;
    }
    let cols = rows.iter().map(Vec::len).max().unwrap_or(0);
    blank_line(out);
    for (index, row) in rows.iter().enumerate() {
        if mode == Format::Markdown {
            out.push('|');
            for column in 0..cols {
                out.push(' ');
                out.push_str(&escape_cell(
                    row.get(column).map(String::as_str).unwrap_or(""),
                ));
                out.push_str(" |");
            }
            out.push('\n');
            if index == 0 {
                out.push('|');
                for _ in 0..cols {
                    out.push_str(" --- |");
                }
                out.push('\n');
            }
        } else {
            out.push_str(&row.join(" | "));
            out.push('\n');
        }
    }
    blank_line(out);
}

fn collect_rows(el: &Element, rows: &mut Vec<Vec<String>>, mode: Format) {
    for child in &el.children {
        let Node::Element(e) = child else { continue };
        match e.tag.as_str() {
            "tr" => {
                let mut cells = Vec::new();
                for cell in &e.children {
                    let Node::Element(c) = cell else { continue };
                    if c.tag == "td" || c.tag == "th" {
                        let content = render_children(c, mode);
                        cells.push(content.split_whitespace().collect::<Vec<_>>().join(" "));
                    }
                }
                rows.push(cells);
            }
            "thead" | "tbody" | "tfoot" => collect_rows(e, rows, mode),
            _ => {}
        }
    }
}

fn render_definition_list(el: &Element, out: &mut String, mode: Format) {
    blank_line(out);
    for child in &el.children {
        let Node::Element(e) = child else { continue };
        match e.tag.as_str() {
            "dt" => {
                let content = render_children(e, mode);
                let content = content.trim();
                if content.is_empty() {
                    continue;
                }
                if mode == Format::Markdown {
                    out.push_str("**");
                    out.push_str(content);
                    out.push_str("**");
                } else {
                    out.push_str(content);
                }
                out.push('\n');
            }
            "dd" => {
                let content = render_children(e, mode);
                let content = content.trim();
                out.push_str(": ");
                out.push_str(content);
                out.push('\n');
            }
            _ => {}
        }
    }
    blank_line(out);
}

// ---------------------------------------------------------------------------
// Whitespace and entities
// ---------------------------------------------------------------------------

fn push_block(out: &mut String, content: &str) {
    let content = content.trim();
    if content.is_empty() {
        return;
    }
    blank_line(out);
    out.push_str(content);
    blank_line(out);
}

/// Ensures the buffer ends with exactly one blank line (two newlines).
fn blank_line(out: &mut String) {
    if out.is_empty() {
        return;
    }
    let trimmed = out.trim_end_matches('\n');
    let len = trimmed.len();
    out.truncate(len);
    out.push('\n');
    out.push('\n');
}

fn collapse_ws(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut space = false;
    for ch in text.chars() {
        if ch.is_whitespace() {
            if !space {
                out.push(' ');
                space = true;
            }
        } else {
            out.push(ch);
            space = false;
        }
    }
    out
}

/// Trims trailing whitespace per line and collapses runs of blank lines to one.
fn normalize(s: &str) -> String {
    let mut lines: Vec<String> = Vec::new();
    let mut previous_blank = false;
    for raw in s.replace("\r\n", "\n").replace('\r', "\n").lines() {
        let line = raw.trim_end();
        if line.trim().is_empty() {
            if !previous_blank && !lines.is_empty() {
                lines.push(String::new());
                previous_blank = true;
            }
            continue;
        }
        previous_blank = false;
        lines.push(line.to_string());
    }
    while lines.last().is_some_and(String::is_empty) {
        lines.pop();
    }
    lines.join("\n")
}

fn escape_cell(text: &str) -> String {
    text.replace('|', "\\|").replace('\n', " ")
}

fn longest_backtick_run(s: &str) -> usize {
    let mut max = 0;
    let mut run = 0;
    for ch in s.chars() {
        if ch == '`' {
            run += 1;
            max = max.max(run);
        } else {
            run = 0;
        }
    }
    max
}

fn attr(el: &Element, name: &str) -> Option<String> {
    el.attrs
        .iter()
        .find(|(key, _)| key == name)
        .map(|(_, value)| decode_entities(value))
}

/// Decodes HTML character references (`&amp;`, `&#233;`, `&#xE9;`).
pub fn decode_entities(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < text.len() {
        if text.as_bytes()[i] == b'&' {
            if let Some(rel) = text[i + 1..].find(';') {
                if rel <= 32 {
                    let entity = &text[i + 1..i + 1 + rel];
                    if let Some(value) = decode_entity(entity) {
                        out.push_str(&value);
                        i += 1 + rel + 1;
                        continue;
                    }
                }
            }
        }
        let ch = text[i..].chars().next().expect("valid char boundary");
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

fn decode_entity(entity: &str) -> Option<String> {
    if let Some(hex) = entity
        .strip_prefix("#x")
        .or_else(|| entity.strip_prefix("#X"))
    {
        return u32::from_str_radix(hex, 16)
            .ok()
            .and_then(char::from_u32)
            .map(|c| c.to_string());
    }
    if let Some(decimal) = entity.strip_prefix('#') {
        return decimal
            .parse::<u32>()
            .ok()
            .and_then(char::from_u32)
            .map(|c| c.to_string());
    }
    let value = match entity {
        "amp" => "&",
        "lt" => "<",
        "gt" => ">",
        "quot" => "\"",
        "apos" => "'",
        "nbsp" => " ",
        "copy" => "©",
        "reg" => "®",
        "trade" => "™",
        "hellip" => "…",
        "mdash" => "—",
        "ndash" => "–",
        "lsquo" => "‘",
        "rsquo" => "’",
        "ldquo" => "“",
        "rdquo" => "”",
        "laquo" => "«",
        "raquo" => "»",
        "times" => "×",
        "divide" => "÷",
        "deg" => "°",
        "plusmn" => "±",
        "frac12" => "½",
        "frac14" => "¼",
        "frac34" => "¾",
        "sup2" => "²",
        "sup3" => "³",
        "micro" => "µ",
        "para" => "¶",
        "sect" => "§",
        "middot" => "·",
        "bull" => "•",
        "dagger" => "†",
        "Dagger" => "‡",
        "euro" => "€",
        "pound" => "£",
        "yen" => "¥",
        "cent" => "¢",
        "aacute" => "á",
        "agrave" => "à",
        "acirc" => "â",
        "auml" => "ä",
        "aring" => "å",
        "ccedil" => "ç",
        "eacute" => "é",
        "egrave" => "è",
        "ecirc" => "ê",
        "euml" => "ë",
        "iacute" => "í",
        "igrave" => "ì",
        "icirc" => "î",
        "iuml" => "ï",
        "ntilde" => "ñ",
        "oacute" => "ó",
        "ograve" => "ò",
        "ocirc" => "ô",
        "ouml" => "ö",
        "otilde" => "õ",
        "uacute" => "ú",
        "ugrave" => "ù",
        "ucirc" => "û",
        "uuml" => "ü",
        "yacute" => "ý",
        "yuml" => "ÿ",
        "szlig" => "ß",
        _ => return None,
    };
    Some(value.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_headings_paragraphs_and_inline() {
        let html = "<h1>Title</h1><p>Hello <strong>world</strong> and <em>friends</em>.</p>";
        assert_eq!(
            to_markdown(html),
            "# Title\n\nHello **world** and *friends*."
        );
    }

    #[test]
    fn converts_links_and_images() {
        let html = r#"<p>See <a href="https://example.com">the docs</a> <img src="/x.png" alt="diagram"></p>"#;
        assert_eq!(
            to_markdown(html),
            "See [the docs](https://example.com) ![diagram](/x.png)"
        );
    }

    #[test]
    fn converts_lists_with_nesting_and_start() {
        let html = "<ul><li>one</li><li>two<ul><li>nested</li></ul></li></ul>";
        assert_eq!(to_markdown(html), "- one\n- two\n  - nested");

        let ol = "<ol start=\"3\"><li>three</li><li>four</li></ol>";
        assert_eq!(to_markdown(ol), "3. three\n4. four");
    }

    #[test]
    fn converts_blockquote_and_code() {
        let html = "<blockquote>quoted</blockquote><pre><code>let x = 1;\nlet y = 2;</code></pre>";
        assert_eq!(
            to_markdown(html),
            "> quoted\n\n```\nlet x = 1;\nlet y = 2;\n```"
        );
        assert_eq!(
            to_markdown("<p>use <code>cargo test</code></p>"),
            "use `cargo test`"
        );
    }

    #[test]
    fn converts_tables() {
        let html = "<table><thead><tr><th>a</th><th>b</th></tr></thead>\
                    <tbody><tr><td>1</td><td>2</td></tr></tbody></table>";
        assert_eq!(to_markdown(html), "| a | b |\n| --- | --- |\n| 1 | 2 |");
    }

    #[test]
    fn decodes_entities() {
        assert_eq!(
            to_markdown("<p>Tom &amp; Jerry &#233; &#x2764;</p>"),
            "Tom & Jerry é ❤"
        );
        assert_eq!(decode_entities("&unknown; &amp;"), "&unknown; &");
    }

    #[test]
    fn strips_script_style_and_head() {
        let html = "<head><title>t</title></head><style>a{}</style>\
                    <script>alert(1)</script><p>body</p>";
        assert_eq!(to_markdown(html), "body");
    }

    #[test]
    fn preserves_pre_whitespace_and_fences() {
        let html = "<pre>```\nfenced\n```</pre>";
        let md = to_markdown(html);
        assert!(md.starts_with("````"), "{md}");
        assert!(md.contains("fenced"), "{md}");
    }

    #[test]
    fn text_mode_omits_markdown_syntax() {
        let html = "<h2>Title</h2><p>See <a href=\"/x\">link</a> and <b>bold</b></p>";
        assert_eq!(to_text(html), "Title\n\nSee link and bold");
    }

    #[test]
    fn handles_literal_angle_brackets() {
        assert_eq!(to_markdown("<p>1 < 2 and 3 > 2</p>"), "1 < 2 and 3 > 2");
    }

    #[test]
    fn quoted_attribute_with_gt() {
        assert_eq!(
            to_markdown(r#"<a href="/a?b=1&amp;c=2">x</a>"#),
            "[x](/a?b=1&c=2)"
        );
    }

    #[test]
    fn full_document_normalizes_whitespace() {
        let html = "<html><body>\n  <p>one</p>\n\n\n  <p>two</p>\n</body></html>";
        assert_eq!(to_markdown(html), "one\n\ntwo");
    }

    #[test]
    fn renders_a_realistic_page() {
        let html = r#"
<!doctype html>
<html>
<head><title>Docs</title><style>body{}</style></head>
<body>
  <nav><a href="/">Home</a></nav>
  <main>
    <article>
      <h1>Getting started</h1>
      <p>Install with <code>cargo install</code> &mdash; then run <strong>oxide</strong>.</p>
      <h2>Options</h2>
      <ul><li>Fast</li><li>Small</li></ul>
      <table><tr><th>Flag</th><th>Meaning</th></tr><tr><td>-p</td><td>print</td></tr></table>
      <pre><code>oxide -p "hi"</code></pre>
    </article>
  </main>
  <footer>© 2024</footer>
</body>
</html>
"#;
        let md = to_markdown(html);
        assert!(md.contains("[Home](/)"), "{md}");
        assert!(md.contains("# Getting started"), "{md}");
        assert!(md.contains("`cargo install` — then run **oxide**"), "{md}");
        assert!(md.contains("## Options"), "{md}");
        assert!(md.contains("- Fast\n- Small"), "{md}");
        assert!(
            md.contains("| Flag | Meaning |\n| --- | --- |\n| -p | print |"),
            "{md}"
        );
        assert!(md.contains("```\noxide -p \"hi\"\n```"), "{md}");
        assert!(md.contains("© 2024"), "{md}");
        assert!(!md.contains("body{}"), "{md}");
        assert!(!md.contains("<title>"), "{md}");
    }
}
