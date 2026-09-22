use serde_yaml::Value;

#[derive(Debug, Clone)]
pub struct Frontmatter {
    pub data: Value,
    pub body: String,
}

impl Frontmatter {
    pub fn get(&self, key: &str) -> Option<&Value> {
        self.data.get(key)
    }

    pub fn get_str(&self, key: &str) -> Option<String> {
        self.get(key).and_then(Value::as_str).map(str::to_string)
    }

    pub fn get_bool(&self, key: &str) -> Option<bool> {
        self.get(key).and_then(Value::as_bool)
    }
}

/// Splits a Markdown document into YAML frontmatter and body. Documents
/// without a leading `---` fence are returned with a null frontmatter.
pub fn parse(raw: &str) -> Frontmatter {
    let raw = raw.strip_prefix('\u{feff}').unwrap_or(raw);
    let Some(rest) = raw
        .strip_prefix("---\n")
        .or_else(|| raw.strip_prefix("---\r\n"))
    else {
        return Frontmatter {
            data: Value::Null,
            body: raw.to_string(),
        };
    };

    let Some(close) = closing_fence(rest) else {
        return Frontmatter {
            data: Value::Null,
            body: raw.to_string(),
        };
    };

    let (yaml, after) = rest.split_at(close);
    let body = after
        .split_once('\n')
        .map(|(_, rest)| rest)
        .unwrap_or("")
        .to_string();
    let data = serde_yaml::from_str(yaml).unwrap_or(Value::Null);
    Frontmatter { data, body }
}

fn closing_fence(text: &str) -> Option<usize> {
    let mut offset = 0;
    for line in text.split_inclusive('\n') {
        let trimmed = line.trim_end_matches(['\n', '\r']);
        if trimmed == "---" || trimmed == "..." {
            return Some(offset);
        }
        offset += line.len();
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_frontmatter_and_body() {
        let doc = parse("---\nname: reviewer\n---\nYou review code.\n");
        assert_eq!(doc.get_str("name").as_deref(), Some("reviewer"));
        assert_eq!(doc.body.trim(), "You review code.");
    }

    #[test]
    fn parses_document_without_frontmatter() {
        let doc = parse("just a body\n");
        assert!(doc.data.is_null());
        assert_eq!(doc.body, "just a body\n");
    }
}
