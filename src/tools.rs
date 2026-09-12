use crate::llm::{ContentPart, FunctionSpec, ToolCall, ToolSpec};
use crate::mcp::McpRegistry;
use crate::media;
use anyhow::{Context, Result};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use tokio::time::{timeout, Duration};

const MAX_OUTPUT: usize = 30_000;

/// The result of running a tool: always a text payload, optionally plus media
/// parts (images/PDFs) that the model should see as content.
#[derive(Debug, Clone, Default)]
pub struct ToolOutput {
    pub text: String,
    pub media: Vec<ContentPart>,
}

impl ToolOutput {
    pub fn text(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            media: Vec::new(),
        }
    }

    pub fn with_media(text: impl Into<String>, media: Vec<ContentPart>) -> Self {
        Self {
            text: text.into(),
            media,
        }
    }
}

pub fn specs(mcp: &McpRegistry) -> Vec<ToolSpec> {
    let mut specs = vec![
        spec(
            "read_file",
            "Read a file from the project. Text files are returned with line numbers; image (png/jpg/gif/webp) and PDF files are returned as viewable attachments.",
            json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "File path relative to the project root" },
                    "offset": { "type": "integer", "description": "1-based line number to start from (text only)" },
                    "limit": { "type": "integer", "description": "Maximum number of lines to return (text only)" }
                },
                "required": ["path"]
            }),
        ),
        spec(
            "write_file",
            "Create or overwrite a file with the given content.",
            json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "File path relative to the project root" },
                    "content": { "type": "string", "description": "Full file content to write" }
                },
                "required": ["path", "content"]
            }),
        ),
        spec(
            "list_dir",
            "List the entries of a directory in the project.",
            json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Directory path relative to the project root (default: .)" }
                }
            }),
        ),
        spec(
            "bash",
            "Run a shell command from the project root and return its combined output.",
            json!({
                "type": "object",
                "properties": {
                    "command": { "type": "string", "description": "Shell command to execute" },
                    "timeout_secs": { "type": "integer", "description": "Timeout in seconds (default 120)" }
                },
                "required": ["command"]
            }),
        ),
        spec(
            "glob",
            "Find files by glob pattern (e.g. `**/*.rs`, `src/*.md`). Patterns match paths relative to the search directory; use `**` for recursive matching.",
            json!({
                "type": "object",
                "properties": {
                    "pattern": { "type": "string", "description": "Glob pattern to match" },
                    "path": { "type": "string", "description": "Directory to search in (default: .)" }
                },
                "required": ["pattern"]
            }),
        ),
        spec(
            "grep",
            "Search file contents for a substring and return matching `path:line: text` entries.",
            json!({
                "type": "object",
                "properties": {
                    "pattern": { "type": "string", "description": "Text to search for" },
                    "path": { "type": "string", "description": "Directory to search in (default: .)" },
                    "include": { "type": "string", "description": "Glob pattern to restrict which file names are searched (e.g. `*.rs`)" },
                    "ignore_case": { "type": "boolean", "description": "Case-insensitive search" }
                },
                "required": ["pattern"]
            }),
        ),
        spec(
            "patch",
            "Apply a unified diff (the `---`/`+++`/`@@` format) to one or more files.",
            json!({
                "type": "object",
                "properties": {
                    "diff": { "type": "string", "description": "Unified diff text to apply" }
                },
                "required": ["diff"]
            }),
        ),
        spec(
            "webfetch",
            "Fetch a URL and return its contents as text (HTML is stripped) or raw HTML.",
            json!({
                "type": "object",
                "properties": {
                    "url": { "type": "string", "description": "URL to fetch" },
                    "format": { "type": "string", "enum": ["text", "markdown", "html"], "description": "Output format (default: text)" }
                },
                "required": ["url"]
            }),
        ),
    ];
    specs.extend(mcp.tool_specs());
    specs
}

fn spec(name: &str, description: &str, parameters: Value) -> ToolSpec {
    ToolSpec {
        kind: "function",
        function: FunctionSpec {
            name: name.to_string(),
            description: description.to_string(),
            parameters,
        },
    }
}

pub async fn execute(call: &ToolCall, cwd: &Path, mcp: &McpRegistry) -> ToolOutput {
    let name = call.function.name.as_str();
    let args: Value = match serde_json::from_str(&call.function.arguments) {
        Ok(value) => value,
        Err(err) => return ToolOutput::text(format!("error: invalid arguments for {name}: {err}")),
    };

    let result = if mcp.is_tool(name) {
        mcp.call(name, args).await.map(ToolOutput::text)
    } else {
        match name {
            "read_file" => read_file(cwd, &args),
            "write_file" => write_file(cwd, &args).map(ToolOutput::text),
            "list_dir" => list_dir(cwd, &args).map(ToolOutput::text),
            "bash" => bash(cwd, &args).await.map(ToolOutput::text),
            "glob" => glob(cwd, &args).map(ToolOutput::text),
            "grep" => grep(cwd, &args).map(ToolOutput::text),
            "patch" => patch(cwd, &args).map(ToolOutput::text),
            "webfetch" => webfetch(&args).await.map(ToolOutput::text),
            other => Err(anyhow::anyhow!("unknown tool `{other}`")),
        }
    };

    match result {
        Ok(output) => ToolOutput {
            text: truncate(output.text),
            media: output.media,
        },
        Err(err) => ToolOutput::text(format!("error: {err:#}")),
    }
}

fn resolve(cwd: &Path, path: &str) -> PathBuf {
    let candidate = Path::new(path);
    if candidate.is_absolute() {
        candidate.to_path_buf()
    } else {
        cwd.join(candidate)
    }
}

fn read_file(cwd: &Path, args: &Value) -> Result<ToolOutput> {
    let path = args
        .get("path")
        .and_then(Value::as_str)
        .context("missing `path`")?;

    let full = resolve(cwd, path);
    if media::is_attachment_path(&full) {
        let part = media::load_attachment(&full)?;
        let kind = if media::is_pdf_path(&full) {
            "pdf"
        } else {
            "image"
        };
        return Ok(ToolOutput::with_media(
            format!("attached {kind} {}", full.display()),
            vec![part],
        ));
    }

    let offset = args
        .get("offset")
        .and_then(Value::as_u64)
        .unwrap_or(1)
        .max(1) as usize;
    let limit = args.get("limit").and_then(Value::as_u64).unwrap_or(2000) as usize;

    let content =
        std::fs::read_to_string(&full).with_context(|| format!("reading {}", full.display()))?;

    let numbered: Vec<String> = content
        .lines()
        .enumerate()
        .skip(offset - 1)
        .take(limit)
        .map(|(i, line)| format!("{:>6}\t{line}", i + 1))
        .collect();

    Ok(ToolOutput::text(numbered.join("\n")))
}

fn write_file(cwd: &Path, args: &Value) -> Result<String> {
    let path = args
        .get("path")
        .and_then(Value::as_str)
        .context("missing `path`")?;
    let content = args
        .get("content")
        .and_then(Value::as_str)
        .context("missing `content`")?;

    let full = resolve(cwd, path);
    if let Some(parent) = full.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    std::fs::write(&full, content).with_context(|| format!("writing {}", full.display()))?;
    Ok(format!(
        "wrote {} bytes to {}",
        content.len(),
        full.display()
    ))
}

fn list_dir(cwd: &Path, args: &Value) -> Result<String> {
    let path = args.get("path").and_then(Value::as_str).unwrap_or(".");
    let full = resolve(cwd, path);

    let mut entries: Vec<String> = std::fs::read_dir(&full)
        .with_context(|| format!("listing {}", full.display()))?
        .filter_map(|entry| entry.ok())
        .map(|entry| {
            let name = entry.file_name().to_string_lossy().to_string();
            let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
            if is_dir {
                format!("{name}/")
            } else {
                name
            }
        })
        .collect();
    entries.sort();
    Ok(entries.join("\n"))
}

const MAX_MATCHES: usize = 500;

fn glob(cwd: &Path, args: &Value) -> Result<String> {
    let pattern = args
        .get("pattern")
        .and_then(Value::as_str)
        .context("missing `pattern`")?;
    let base = args.get("path").and_then(Value::as_str).unwrap_or(".");
    let root = resolve(cwd, base);

    let mut matches = Vec::new();
    walk(&root, &mut |path| {
        let rel = path.strip_prefix(&root).unwrap_or(path);
        let rel = rel.to_string_lossy().replace('\\', "/");
        if glob_match(pattern, &rel) {
            matches.push(rel);
        }
    });
    matches.sort();
    if matches.is_empty() {
        return Ok("no matches".to_string());
    }
    let truncated = matches.len() > MAX_MATCHES;
    matches.truncate(MAX_MATCHES);
    let mut out = matches.join("\n");
    if truncated {
        out.push_str("\n... [truncated]");
    }
    Ok(out)
}

fn grep(cwd: &Path, args: &Value) -> Result<String> {
    let pattern = args
        .get("pattern")
        .and_then(Value::as_str)
        .context("missing `pattern`")?;
    if pattern.is_empty() {
        anyhow::bail!("`pattern` must not be empty");
    }
    let base = args.get("path").and_then(Value::as_str).unwrap_or(".");
    let include = args.get("include").and_then(Value::as_str);
    let ignore_case = args
        .get("ignore_case")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let root = resolve(cwd, base);

    let needle = if ignore_case {
        pattern.to_lowercase()
    } else {
        pattern.to_string()
    };

    let mut hits: Vec<String> = Vec::new();
    walk(&root, &mut |path| {
        if hits.len() > MAX_MATCHES {
            return;
        }
        let name = path.file_name().map(|n| n.to_string_lossy().to_string());
        if let (Some(include), Some(name)) = (include, name.as_deref()) {
            if !glob_match(include, name) {
                return;
            }
        }
        let Ok(content) = std::fs::read_to_string(path) else {
            return;
        };
        let rel = path.strip_prefix(&root).unwrap_or(path);
        let rel = rel.to_string_lossy().replace('\\', "/");
        for (index, line) in content.lines().enumerate() {
            let haystack = if ignore_case {
                line.to_lowercase()
            } else {
                line.to_string()
            };
            if haystack.contains(&needle) {
                hits.push(format!("{rel}:{}: {}", index + 1, line.trim_end()));
                if hits.len() > MAX_MATCHES {
                    return;
                }
            }
        }
    });

    if hits.is_empty() {
        return Ok("no matches".to_string());
    }
    let truncated = hits.len() > MAX_MATCHES;
    hits.truncate(MAX_MATCHES);
    let mut out = hits.join("\n");
    if truncated {
        out.push_str("\n... [truncated]");
    }
    Ok(out)
}

fn patch(cwd: &Path, args: &Value) -> Result<String> {
    let diff = args
        .get("diff")
        .and_then(Value::as_str)
        .context("missing `diff`")?;
    let lines: Vec<&str> = diff.lines().collect();
    let mut applied: Vec<String> = Vec::new();
    let mut i = 0;

    while i < lines.len() {
        if !lines[i].starts_with("--- ") {
            i += 1;
            continue;
        }
        i += 1;
        if i >= lines.len() || !lines[i].starts_with("+++ ") {
            anyhow::bail!("malformed diff: `---` without `+++`");
        }
        let target = diff_target(lines[i]);
        i += 1;

        let full = resolve(cwd, &target);
        let original = std::fs::read_to_string(&full).unwrap_or_default();
        let trailing_newline = original.ends_with('\n');
        let mut file_lines: Vec<String> = original.lines().map(str::to_string).collect();
        let mut offset: isize = 0;

        while i < lines.len() && lines[i].starts_with("@@") {
            let (old_start, old_count) = parse_hunk_header(lines[i])?;
            i += 1;
            let mut old_lines: Vec<String> = Vec::new();
            let mut new_lines: Vec<String> = Vec::new();
            while i < lines.len() {
                let line = lines[i];
                if line.starts_with("@@") || line.starts_with("--- ") {
                    break;
                }
                if let Some(rest) = line.strip_prefix(' ') {
                    old_lines.push(rest.to_string());
                    new_lines.push(rest.to_string());
                } else if let Some(rest) = line.strip_prefix('-') {
                    old_lines.push(rest.to_string());
                } else if let Some(rest) = line.strip_prefix('+') {
                    new_lines.push(rest.to_string());
                } else if line.is_empty() {
                    old_lines.push(String::new());
                    new_lines.push(String::new());
                } else if line.starts_with('\\') {
                } else {
                    break;
                }
                i += 1;
            }

            let start = ((old_start as isize - 1) + offset).max(0) as usize;
            let position = find_lines(&file_lines, &old_lines, start)
                .with_context(|| format!("hunk at line {old_start} did not match {target}"))?;
            let _ = old_count;
            file_lines.splice(
                position..position + old_lines.len(),
                new_lines.iter().cloned(),
            );
            offset += new_lines.len() as isize - old_lines.len() as isize;
        }

        let mut out = file_lines.join("\n");
        if trailing_newline {
            out.push('\n');
        }
        std::fs::write(&full, out).with_context(|| format!("writing {}", full.display()))?;
        applied.push(target);
    }

    if applied.is_empty() {
        anyhow::bail!("no file patches found in diff");
    }
    Ok(format!("patched {}", applied.join(", ")))
}

async fn webfetch(args: &Value) -> Result<String> {
    let url = args
        .get("url")
        .and_then(Value::as_str)
        .context("missing `url`")?;
    let format = args.get("format").and_then(Value::as_str).unwrap_or("text");

    let response = reqwest::Client::new()
        .get(url)
        .header("User-Agent", "oxide")
        .send()
        .await
        .with_context(|| format!("fetching {url}"))?;
    let status = response.status();
    if !status.is_success() {
        anyhow::bail!("fetch returned {status}");
    }
    let body = response.text().await.context("reading response body")?;

    let text = match format {
        "html" => body,
        _ => html_to_text(&body),
    };
    Ok(text)
}

fn walk(root: &Path, visit: &mut impl FnMut(&Path)) {
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if matches!(name.as_str(), ".git" | "node_modules" | "target" | ".venv") {
                continue;
            }
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else {
                visit(&path);
            }
        }
    }
}

fn glob_match(pattern: &str, path: &str) -> bool {
    let pattern: Vec<&str> = pattern.split('/').filter(|part| !part.is_empty()).collect();
    let path: Vec<&str> = path.split('/').filter(|part| !part.is_empty()).collect();
    match_segments(&pattern, &path)
}

fn match_segments(pattern: &[&str], path: &[&str]) -> bool {
    if pattern.is_empty() {
        return path.is_empty();
    }
    if pattern[0] == "**" {
        return (0..=path.len()).any(|skip| match_segments(&pattern[1..], &path[skip..]));
    }
    if path.is_empty() {
        return false;
    }
    wildcard(
        &pattern[0].chars().collect::<Vec<_>>(),
        &path[0].chars().collect::<Vec<_>>(),
    ) && match_segments(&pattern[1..], &path[1..])
}

fn wildcard(pattern: &[char], text: &[char]) -> bool {
    if pattern.is_empty() {
        return text.is_empty();
    }
    match pattern[0] {
        '*' => wildcard(&pattern[1..], text) || (!text.is_empty() && wildcard(pattern, &text[1..])),
        '?' => !text.is_empty() && wildcard(&pattern[1..], &text[1..]),
        ch => !text.is_empty() && text[0] == ch && wildcard(&pattern[1..], &text[1..]),
    }
}

fn diff_target(header: &str) -> String {
    let rest = header.trim_start_matches("+++ ").trim();
    let path = rest.split('\t').next().unwrap_or(rest);
    path.strip_prefix("b/").unwrap_or(path).to_string()
}

fn parse_hunk_header(header: &str) -> Result<(usize, usize)> {
    let body = header
        .trim_start_matches("@@")
        .split("@@")
        .next()
        .unwrap_or("")
        .trim();
    let old = body
        .split_whitespace()
        .find(|part| part.starts_with('-'))
        .context("malformed hunk header")?;
    let mut parts = old.trim_start_matches('-').split(',');
    let start = parts
        .next()
        .and_then(|value| value.parse::<usize>().ok())
        .context("malformed hunk start")?;
    let count = parts
        .next()
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(1);
    Ok((start, count))
}

fn find_lines(haystack: &[String], needle: &[String], start: usize) -> Option<usize> {
    if needle.is_empty() {
        return Some(start.min(haystack.len()));
    }
    if needle.len() > haystack.len() {
        return None;
    }
    (start..=haystack.len() - needle.len())
        .find(|&index| haystack[index..index + needle.len()] == *needle)
}

fn html_to_text(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut in_tag = false;
    for ch in html.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => {
                in_tag = false;
                out.push('\n');
            }
            _ if !in_tag => out.push(ch),
            _ => {}
        }
    }
    let decoded = out
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&nbsp;", " ");
    let mut lines: Vec<&str> = decoded.lines().map(str::trim_end).collect();
    lines.retain(|line| !line.trim().is_empty());
    lines.join("\n")
}

async fn bash(cwd: &Path, args: &Value) -> Result<String> {
    let command = args
        .get("command")
        .and_then(Value::as_str)
        .context("missing `command`")?;
    let secs = args
        .get("timeout_secs")
        .and_then(Value::as_u64)
        .unwrap_or(120);

    let child = tokio::process::Command::new("sh")
        .arg("-c")
        .arg(command)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output();

    let output = timeout(Duration::from_secs(secs), child)
        .await
        .with_context(|| format!("command timed out after {secs}s"))?
        .context("spawning shell")?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let mut combined = String::new();
    if !stdout.trim().is_empty() {
        combined.push_str(stdout.trim_end());
    }
    if !stderr.trim().is_empty() {
        if !combined.is_empty() {
            combined.push('\n');
        }
        combined.push_str("[stderr]\n");
        combined.push_str(stderr.trim_end());
    }
    if combined.is_empty() {
        combined.push_str("(no output)");
    }
    combined.push_str(&format!(
        "\n[exit code: {}]",
        output.status.code().unwrap_or(-1)
    ));
    Ok(combined)
}

fn truncate(mut output: String) -> String {
    if output.len() > MAX_OUTPUT {
        let mut cut = MAX_OUTPUT;
        while !output.is_char_boundary(cut) {
            cut -= 1;
        }
        output.truncate(cut);
        output.push_str("\n... [output truncated]");
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::FunctionCall;

    fn call(name: &str, args: Value) -> ToolCall {
        ToolCall {
            id: "test".to_string(),
            kind: "function".to_string(),
            function: FunctionCall {
                name: name.to_string(),
                arguments: args.to_string(),
            },
        }
    }

    #[tokio::test]
    async fn tools_round_trip() {
        let dir = std::env::temp_dir().join(format!("oxide_test_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mcp = McpRegistry::default();

        let out = execute(
            &call(
                "write_file",
                json!({ "path": "a.txt", "content": "one\ntwo\nthree" }),
            ),
            &dir,
            &mcp,
        )
        .await;
        assert!(out.text.contains("wrote"), "{}", out.text);

        let out = execute(
            &call(
                "read_file",
                json!({ "path": "a.txt", "offset": 2, "limit": 1 }),
            ),
            &dir,
            &mcp,
        )
        .await;
        assert!(out.text.contains("two"), "{}", out.text);

        let out = execute(&call("list_dir", json!({})), &dir, &mcp).await;
        assert!(out.text.contains("a.txt"), "{}", out.text);

        let out = execute(&call("bash", json!({ "command": "echo hi" })), &dir, &mcp).await;
        assert!(
            out.text.contains("hi") && out.text.contains("exit code: 0"),
            "{}",
            out.text
        );

        let out = execute(
            &call("read_file", json!({ "path": "missing.txt" })),
            &dir,
            &mcp,
        )
        .await;
        assert!(out.text.starts_with("error:"), "{}", out.text);

        std::fs::write(dir.join("shot.png"), b"f").unwrap();
        let out = execute(
            &call("read_file", json!({ "path": "shot.png" })),
            &dir,
            &mcp,
        )
        .await;
        assert_eq!(out.media.len(), 1, "{out:?}");
        assert!(out.text.contains("attached image"), "{}", out.text);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn glob_matches_recursively() {
        assert!(glob_match("**/*.rs", "src/main.rs"));
        assert!(glob_match("src/*.md", "src/readme.md"));
        assert!(!glob_match("src/*.md", "src/nested/readme.md"));
        assert!(glob_match("*.txt", "notes.txt"));
        assert!(!glob_match("*.txt", "a/b.txt"));
        assert!(glob_match("a/**/b.rs", "a/b.rs"));
        assert!(glob_match("a/**/b.rs", "a/x/y/b.rs"));
    }

    #[test]
    fn strips_html_tags() {
        let text = html_to_text("<h1>Title</h1><p>Hello &amp; bye</p>");
        assert_eq!(text, "Title\nHello & bye");
    }

    #[test]
    fn applies_unified_diff() {
        let dir = std::env::temp_dir().join(format!("oxide_patch_{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("f.txt"), "one\ntwo\nthree\n").unwrap();

        let diff = "\
--- a/f.txt
+++ b/f.txt
@@ -1,3 +1,3 @@
 one
-two
+TWO
 three
";
        let out = patch(&dir, &json!({ "diff": diff })).unwrap();
        assert!(out.contains("f.txt"), "{out}");
        assert_eq!(
            std::fs::read_to_string(dir.join("f.txt")).unwrap(),
            "one\nTWO\nthree\n"
        );

        std::fs::remove_dir_all(&dir).ok();
    }
}
