use crate::diff;
use crate::llm::{ContentPart, FunctionSpec, ToolCall, ToolSpec};
use crate::mcp::McpRegistry;
use crate::media;
use anyhow::{Context, Result};
use serde_json::{json, Value};
use std::collections::VecDeque;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::io::AsyncReadExt;
use tokio::time::{timeout, Duration};

const MAX_OUTPUT_BYTES: usize = 6_000;
const MAX_OUTPUT_LINES: usize = 250;
const MAX_LINE_LEN: usize = 1_000;
const DEFAULT_READ_LINES: usize = 250;
const TRUNCATION_RETENTION_SECS: u64 = 7 * 24 * 60 * 60;
const PROGRESS_BATCH_BYTES: usize = 4_096;
const PROGRESS_INTERVAL: Duration = Duration::from_millis(50);

static TRUNCATION_ID: AtomicU64 = AtomicU64::new(0);

/// Maps a tool name to its internal canonical form, accepting both the Pi-style
/// names (`read`, `write`, `edit`, `ls`, `find`, `grep`, `bash`) and the legacy
/// oxide names (`read_file`, `write_file`, `patch`, `list_dir`, `glob`). The
/// agent-level tools (`task`, `skill`, `memory`, `diagnostics`, `compress`) and
/// MCP names (`server__tool`) pass through unchanged.
pub fn canonical_tool_name(name: &str) -> &str {
    match name {
        "read" | "read_file" => "read_file",
        "write" | "write_file" => "write_file",
        "edit" => "edit",
        "patch" => "patch",
        "ls" | "list_dir" => "list_dir",
        "find" | "glob" => "glob",
        "grep" => "grep",
        "bash" => "bash",
        "webfetch" => "webfetch",
        other => other,
    }
}

/// A line-numbered diff of a file edit, carried alongside the tool result for
/// display only. It is never sent to the model (the text result is).
#[derive(Debug, Clone, Default)]
pub struct DiffPreview {
    pub path: String,
    pub text: String,
}

/// The result of running a tool: always a text payload, optionally plus media
/// parts (images/PDFs) that the model should see as content. `terminate` lets a
/// tool (or a `tool.execute.after` plugin hook) end the turn instead of asking
/// the model to react to the result.
#[derive(Debug, Clone, Default)]
pub struct ToolOutput {
    pub text: String,
    pub media: Vec<ContentPart>,
    pub terminate: bool,
    pub diff: Option<DiffPreview>,
}

impl ToolOutput {
    pub fn text(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            media: Vec::new(),
            terminate: false,
            diff: None,
        }
    }

    pub fn with_media(text: impl Into<String>, media: Vec<ContentPart>) -> Self {
        Self {
            text: text.into(),
            media,
            terminate: false,
            diff: None,
        }
    }

    pub fn with_diff(mut self, path: impl Into<String>, text: impl Into<String>) -> Self {
        self.diff = Some(DiffPreview {
            path: path.into(),
            text: text.into(),
        });
        self
    }
}

pub type ProgressSink = Arc<dyn Fn(&str) + Send + Sync>;

/// A best-effort sink for streaming tool progress to the UI. Tools that produce
/// incremental output (currently `bash`) report each line as it arrives.
#[derive(Clone, Default)]
pub struct Progress {
    sink: Option<ProgressSink>,
}

impl Progress {
    pub fn new(sink: ProgressSink) -> Self {
        Self { sink: Some(sink) }
    }

    pub fn report(&self, chunk: impl AsRef<str>) {
        if let Some(sink) = &self.sink {
            sink(chunk.as_ref());
        }
    }
}

pub fn specs(mcp: &McpRegistry) -> Vec<ToolSpec> {
    let mut specs = vec![
        spec(
            "read",
            "Read a file from the project. Text files are returned with line numbers; image (png/jpg/gif/webp) and PDF files are returned as viewable attachments.",
            json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "File path relative to the project root" },
                    "offset": { "type": "integer", "description": "1-based line number to start from (text only)" },
                    "limit": { "type": "integer", "description": "Maximum number of lines to return (text only, default 400)" }
                },
                "required": ["path"]
            }),
        ),
        spec(
            "write",
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
            "edit",
            "Edit a single file using exact text replacement. Every edits[].oldText must match a unique, non-overlapping region of the original file. If two changes affect the same block or nearby lines, merge them into one edit instead of emitting overlapping edits. Do not include large unchanged regions just to connect distant changes.",
            json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Path to the file to edit (relative or absolute)" },
                    "edits": {
                        "type": "array",
                        "description": "One or more targeted replacements. Each edit is matched against the original file, not incrementally. Do not include overlapping or nested edits. If two changes touch the same block or nearby lines, merge them into one edit instead.",
                        "items": {
                            "type": "object",
                            "properties": {
                                "oldText": { "type": "string", "description": "Exact text for one targeted replacement. It must be unique in the original file and must not overlap with any other edits[].oldText in the same call." },
                                "newText": { "type": "string", "description": "Replacement text for this targeted edit." }
                            },
                            "required": ["oldText", "newText"]
                        }
                    }
                },
                "required": ["path", "edits"]
            }),
        ),
        spec(
            "ls",
            "List the entries of a directory in the project.",
            json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Directory path relative to the project root (default: .)" },
                    "limit": { "type": "integer", "description": "Maximum number of entries to return" }
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
                    "timeout": { "type": "integer", "description": "Timeout in milliseconds (default 120000)" }
                },
                "required": ["command"]
            }),
        ),
        spec(
            "find",
            "Find files by glob pattern (e.g. `**/*.rs`, `src/*.md`). Patterns match paths relative to the search directory; use `**` for recursive matching.",
            json!({
                "type": "object",
                "properties": {
                    "pattern": { "type": "string", "description": "Glob pattern to match" },
                    "path": { "type": "string", "description": "Directory to search in (default: .)" },
                    "limit": { "type": "integer", "description": "Maximum number of results to return" }
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
                    "glob": { "type": "string", "description": "Glob pattern to restrict which file names are searched (e.g. `*.rs`)" },
                    "ignoreCase": { "type": "boolean", "description": "Case-insensitive search" },
                    "literal": { "type": "boolean", "description": "Treat the pattern as a literal string instead of a regular expression" },
                    "context": { "type": "integer", "description": "Number of context lines to include around each match" },
                    "limit": { "type": "integer", "description": "Maximum number of matches to return" }
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
            "Fetch a URL and return its contents as Markdown (default), readable plain text, or raw HTML.",
            json!({
                "type": "object",
                "properties": {
                    "url": { "type": "string", "description": "URL to fetch" },
                    "format": { "type": "string", "enum": ["markdown", "text", "html"], "description": "Output format (default: markdown)" }
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

pub async fn execute(
    call: &ToolCall,
    cwd: &Path,
    mcp: &McpRegistry,
    progress: &Progress,
) -> ToolOutput {
    let name = call.function.name.as_str();
    let args: Value = match serde_json::from_str(&call.function.arguments) {
        Ok(value) => value,
        Err(err) => return ToolOutput::text(format!("error: invalid arguments for {name}: {err}")),
    };

    let canonical = canonical_tool_name(name);
    let result = if mcp.is_tool(name) {
        mcp.call(name, args).await.map(ToolOutput::text)
    } else {
        match canonical {
            "bash" => bash(cwd, &args, progress).await.map(ToolOutput::text),
            "webfetch" => webfetch(&args).await.map(ToolOutput::text),
            "read_file" | "write_file" | "edit" | "list_dir" | "glob" | "grep" | "patch" => {
                let cwd = cwd.to_path_buf();
                let args = args.clone();
                let tool = canonical.to_string();
                match tokio::task::spawn_blocking(move || match tool.as_str() {
                    "read_file" => read_file(&cwd, &args),
                    "write_file" => write_file(&cwd, &args),
                    "edit" => edit(&cwd, &args),
                    "list_dir" => list_dir(&cwd, &args).map(ToolOutput::text),
                    "glob" => glob(&cwd, &args).map(ToolOutput::text),
                    "grep" => grep(&cwd, &args).map(ToolOutput::text),
                    "patch" => patch(&cwd, &args),
                    _ => unreachable!(),
                })
                .await
                {
                    Ok(result) => result,
                    Err(err) => Err(anyhow::anyhow!("{canonical} worker failed: {err}")),
                }
            }
            other => Err(anyhow::anyhow!("unknown tool `{other}`")),
        }
    };

    match result {
        Ok(output) => ToolOutput {
            text: if canonical == "bash" {
                output.text
            } else {
                truncate(canonical, output.text)
            },
            media: output.media,
            terminate: output.terminate,
            diff: output.diff,
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
    let limit = args
        .get("limit")
        .and_then(Value::as_u64)
        .unwrap_or(DEFAULT_READ_LINES as u64) as usize;

    let content =
        std::fs::read_to_string(&full).with_context(|| format!("reading {}", full.display()))?;

    let total = content.lines().count();
    let budget = MAX_OUTPUT_BYTES.saturating_sub(128);
    let mut numbered: Vec<String> = Vec::new();
    let mut used = 0usize;
    for (i, line) in content.lines().enumerate().skip(offset - 1).take(limit) {
        let shown = if line.chars().count() > MAX_LINE_LEN {
            let prefix: String = line.chars().take(MAX_LINE_LEN).collect();
            format!("{prefix} …")
        } else {
            line.to_string()
        };
        let entry = format!("{}|{shown}", i + 1);
        if !numbered.is_empty() && used + entry.len() + 1 > budget {
            break;
        }
        used += entry.len() + 1;
        numbered.push(entry);
    }

    let mut out = numbered.join("\n");
    let read_to = (offset - 1) + numbered.len();
    if read_to < total {
        out.push_str(&format!(
            "\n... [{} more lines; use offset={}]",
            total - read_to,
            read_to + 1
        ));
    }
    Ok(ToolOutput::text(out))
}

fn write_file(cwd: &Path, args: &Value) -> Result<ToolOutput> {
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
    let previous = std::fs::read_to_string(&full).unwrap_or_default();
    std::fs::write(&full, content).with_context(|| format!("writing {}", full.display()))?;
    let output = ToolOutput::text(format!("wrote {} bytes", content.len()));
    match diff::preview(&previous, content) {
        Some(diff) => Ok(output.with_diff(path, diff)),
        None => Ok(output),
    }
}

/// One exact-text replacement from an `edit` call.
struct Replacement {
    old: String,
    new: String,
}

/// Normalizes the many shapes models send for `edit` into a list of
/// replacements, mirroring Pi's `prepareArguments`: `edits` may be a JSON
/// string, a single `{oldText,newText}` object, or an array; legacy top-level
/// `oldText`/`newText` are folded into the list.
fn parse_edits(args: &Value) -> Result<Vec<Replacement>> {
    let mut raw: Vec<Replacement> = Vec::new();
    match args.get("edits") {
        Some(Value::String(text)) => {
            if let Ok(parsed) = serde_json::from_str::<Value>(text) {
                collect_edits(&parsed, &mut raw);
            }
        }
        Some(value) => collect_edits(value, &mut raw),
        None => {}
    }
    if let (Some(old), Some(new)) = (
        args.get("oldText").and_then(Value::as_str),
        args.get("newText").and_then(Value::as_str),
    ) {
        raw.push(Replacement {
            old: old.to_string(),
            new: new.to_string(),
        });
    }
    if raw.is_empty() {
        anyhow::bail!("edits must contain at least one replacement");
    }
    Ok(raw)
}

fn collect_edits(value: &Value, out: &mut Vec<Replacement>) {
    match value {
        Value::Array(items) => {
            for item in items {
                collect_edits(item, out);
            }
        }
        Value::Object(_) => {
            if let (Some(old), Some(new)) = (
                value.get("oldText").and_then(Value::as_str),
                value.get("newText").and_then(Value::as_str),
            ) {
                out.push(Replacement {
                    old: old.to_string(),
                    new: new.to_string(),
                });
            }
        }
        _ => {}
    }
}

/// Applies Pi-style exact text replacements. Each `oldText` is matched against
/// the original file (never incrementally) and must be unique; overlapping or
/// non-unique matches are rejected so a bad edit cannot silently corrupt a
/// file.
fn edit(cwd: &Path, args: &Value) -> Result<ToolOutput> {
    let path = args
        .get("path")
        .and_then(Value::as_str)
        .context("missing `path`")?;
    let edits = parse_edits(args)?;
    let full = resolve(cwd, path);
    let raw = std::fs::read_to_string(&full)
        .with_context(|| format!("reading {} (use write to create new files)", full.display()))?;

    let (bom, content) = split_bom(&raw);
    let crlf = content.contains("\r\n");
    let mut base = content.replace("\r\n", "\n");

    for (index, replacement) in edits.iter().enumerate() {
        let old = replacement.old.replace("\r\n", "\n");
        if old.is_empty() {
            anyhow::bail!("edits[{index}].oldText must not be empty");
        }
        let matches = base.matches(&old).count();
        match matches {
            0 => anyhow::bail!(
                "edits[{index}].oldText did not match anything in {path}; check the exact text"
            ),
            1 => {
                base = base.replacen(&old, &replacement.new.replace("\r\n", "\n"), 1);
            }
            n => anyhow::bail!(
                "edits[{index}].oldText matched {n} times in {path}; include more surrounding text to make it unique"
            ),
        }
    }

    let final_content = if crlf {
        format!("{bom}{}", base.replace('\n', "\r\n"))
    } else {
        format!("{bom}{base}")
    };
    std::fs::write(&full, &final_content).with_context(|| format!("writing {}", full.display()))?;

    let output = ToolOutput::text(format!(
        "Successfully replaced {} block(s) in {path}.",
        edits.len()
    ));
    match diff::preview(
        &raw.replace("\r\n", "\n"),
        &final_content.replace("\r\n", "\n"),
    ) {
        Some(diff) => Ok(output.with_diff(path, diff)),
        None => Ok(output),
    }
}

/// Splits a leading UTF-8 BOM from file content so edits match text the model
/// actually sees. Returns the BOM (empty when absent) and the remaining text.
fn split_bom(text: &str) -> (&str, &str) {
    match text.strip_prefix('\u{feff}') {
        Some(rest) => ("\u{feff}", rest),
        None => ("", text),
    }
}

fn list_dir(cwd: &Path, args: &Value) -> Result<String> {
    let path = args.get("path").and_then(Value::as_str).unwrap_or(".");
    let limit = int_arg(args, "limit").unwrap_or(MAX_MATCHES);
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
    let truncated = entries.len() > limit;
    entries.truncate(limit);
    let mut out = entries.join("\n");
    if truncated {
        out.push_str("\n... [truncated]");
    }
    Ok(out)
}

const MAX_MATCHES: usize = 200;

fn glob(cwd: &Path, args: &Value) -> Result<String> {
    let pattern = args
        .get("pattern")
        .and_then(Value::as_str)
        .context("missing `pattern`")?;
    let base = args.get("path").and_then(Value::as_str).unwrap_or(".");
    let limit = int_arg(args, "limit").unwrap_or(MAX_MATCHES);
    let root = resolve(cwd, base);

    let mut matches = Vec::new();
    walk(&root, &mut |path| {
        let rel = path.strip_prefix(&root).unwrap_or(path);
        let rel = rel.to_string_lossy().replace('\\', "/");
        if glob_match(pattern, &rel) {
            matches.push(rel);
        }
        true
    });
    matches.sort();
    if matches.is_empty() {
        return Ok("no matches".to_string());
    }
    let truncated = matches.len() > limit;
    matches.truncate(limit);
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
    // Pi names these `glob`/`ignoreCase`; the legacy names are still accepted.
    let include = args
        .get("glob")
        .and_then(Value::as_str)
        .or_else(|| args.get("include").and_then(Value::as_str));
    let ignore_case = bool_arg(args, "ignoreCase", "ignore_case");
    let context = int_arg(args, "context").unwrap_or(0);
    let limit = int_arg(args, "limit").unwrap_or(MAX_MATCHES);
    let root = resolve(cwd, base);

    let needle = if ignore_case {
        pattern.to_lowercase()
    } else {
        pattern.to_string()
    };

    let mut hits: Vec<String> = Vec::new();
    walk(&root, &mut |path| {
        if hits.len() > limit {
            return false;
        }
        let name = path.file_name().map(|n| n.to_string_lossy().to_string());
        if let (Some(include), Some(name)) = (include, name.as_deref()) {
            if !glob_match(include, name) {
                return true;
            }
        }
        let Ok(content) = std::fs::read_to_string(path) else {
            return true;
        };
        let rel = path.strip_prefix(&root).unwrap_or(path);
        let rel = rel.to_string_lossy().replace('\\', "/");
        let lines: Vec<&str> = content.lines().collect();
        for (index, line) in lines.iter().enumerate() {
            let matched = if ignore_case {
                line.to_lowercase().contains(&needle)
            } else {
                line.contains(&needle)
            };
            if matched {
                if context > 0 {
                    let start = index.saturating_sub(context);
                    let end = (index + context + 1).min(lines.len());
                    for (ctx_index, ctx_line) in lines.iter().enumerate().take(end).skip(start) {
                        let marker = if ctx_index == index { ':' } else { '-' };
                        hits.push(format!(
                            "{rel}{marker}{}{marker} {}",
                            ctx_index + 1,
                            ctx_line.trim_end()
                        ));
                    }
                } else {
                    hits.push(format!("{rel}:{}: {}", index + 1, line.trim_end()));
                }
                if hits.len() > limit {
                    return false;
                }
            }
        }
        true
    });

    if hits.is_empty() {
        return Ok("no matches".to_string());
    }
    let truncated = hits.len() > limit;
    hits.truncate(limit);
    let mut out = hits.join("\n");
    if truncated {
        out.push_str("\n... [truncated]");
    }
    Ok(out)
}

/// Reads an integer argument, accepting both a JSON number and a numeric
/// string (some models stringify numbers).
fn int_arg(args: &Value, key: &str) -> Option<usize> {
    match args.get(key) {
        Some(Value::Number(number)) => number.as_u64().map(|value| value as usize),
        Some(Value::String(text)) => text.trim().parse().ok(),
        _ => None,
    }
}

/// Reads a boolean argument under either of two names (Pi name first).
fn bool_arg(args: &Value, primary: &str, legacy: &str) -> bool {
    args.get(primary)
        .or_else(|| args.get(legacy))
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

fn patch(cwd: &Path, args: &Value) -> Result<ToolOutput> {
    let diff = args
        .get("diff")
        .and_then(Value::as_str)
        .context("missing `diff`")?;
    let lines: Vec<&str> = diff.lines().collect();
    let mut applied: Vec<String> = Vec::new();
    let mut previews: Vec<(String, String)> = Vec::new();
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
        std::fs::write(&full, &out).with_context(|| format!("writing {}", full.display()))?;
        if let Some(preview) = diff::preview(&original, &out) {
            previews.push((target.clone(), preview));
        }
        applied.push(target);
    }

    if applied.is_empty() {
        anyhow::bail!("no file patches found in diff");
    }
    let output = ToolOutput::text(format!("patched {}", applied.join(", ")));
    if previews.is_empty() {
        return Ok(output);
    }
    let path = if previews.len() == 1 {
        previews[0].0.clone()
    } else {
        format!("{} files", previews.len())
    };
    let text = previews
        .iter()
        .map(|(file, diff)| {
            if previews.len() == 1 {
                diff.clone()
            } else {
                format!("  {file}\n{diff}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    Ok(output.with_diff(path, text))
}

async fn webfetch(args: &Value) -> Result<String> {
    let url = args
        .get("url")
        .and_then(Value::as_str)
        .context("missing `url`")?;
    let format = args
        .get("format")
        .and_then(Value::as_str)
        .unwrap_or("markdown");

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
        "text" => crate::html::to_text(&body),
        _ => crate::html::to_markdown(&body),
    };
    Ok(text)
}

fn walk(root: &Path, visit: &mut impl FnMut(&Path) -> bool) {
    let mut stack = vec![(root.to_path_buf(), Arc::new(Vec::new()))];
    while let Some((dir, inherited_ignores)) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        let mut ignores = (*inherited_ignores).clone();
        ignores.extend(gitignore_patterns(&dir));
        let ignores = Arc::new(ignores);
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if matches!(name.as_str(), ".git" | "node_modules" | "target" | ".venv") {
                continue;
            }
            let path = entry.path();
            let rel = path
                .strip_prefix(root)
                .unwrap_or(&path)
                .to_string_lossy()
                .replace('\\', "/");
            let is_dir = path.is_dir();
            if is_ignored(&ignores, &rel, is_dir) {
                continue;
            }
            if is_dir {
                stack.push((path, Arc::clone(&ignores)));
            } else if !visit(&path) {
                return;
            }
        }
    }
}

/// Reads the `.gitignore` rules introduced by one directory. The walker carries
/// inherited rules forward so ancestor files are not reopened for every child.
fn gitignore_patterns(dir: &Path) -> Vec<String> {
    let mut patterns = Vec::new();
    if let Ok(content) = std::fs::read_to_string(dir.join(".gitignore")) {
        for line in content.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            patterns.push(line.to_string());
        }
    }
    patterns
}

/// Applies the common subset of gitignore semantics: `!` negation, leading `/`
/// anchoring, trailing `/` for directories, and `*`/`**` globs.
fn is_ignored(patterns: &[String], rel: &str, is_dir: bool) -> bool {
    let mut ignored = false;
    for pattern in patterns {
        let (negated, raw) = match pattern.strip_prefix('!') {
            Some(rest) => (true, rest),
            None => (false, pattern.as_str()),
        };
        let mut pattern = raw;
        let dir_only = pattern.ends_with('/');
        pattern = pattern.trim_end_matches('/');
        if dir_only && !is_dir {
            continue;
        }
        let anchored = pattern.starts_with('/');
        let pattern = pattern.trim_start_matches('/');
        let name = rel.rsplit('/').next().unwrap_or(rel);
        let matched = if anchored || pattern.contains('/') {
            glob_match(pattern, rel)
        } else {
            glob_match(pattern, name)
        };
        if matched {
            ignored = !negated;
        }
    }
    ignored
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

async fn bash(cwd: &Path, args: &Value, progress: &Progress) -> Result<String> {
    let command = args
        .get("command")
        .and_then(Value::as_str)
        .context("missing `command`")?;
    // Pi passes `timeout` in milliseconds; the legacy `timeout_secs` is still
    // accepted for backward compatibility.
    let secs = if let Some(millis) = int_arg(args, "timeout") {
        (millis as u64).div_ceil(1000).max(1)
    } else {
        args.get("timeout_secs")
            .and_then(Value::as_u64)
            .unwrap_or(120)
    };

    #[cfg(windows)]
    let mut shell = {
        let mut shell = tokio::process::Command::new("cmd");
        shell.arg("/C").arg(command);
        shell
    };
    #[cfg(not(windows))]
    let mut shell = {
        let mut shell = tokio::process::Command::new("sh");
        shell.arg("-c").arg(command);
        shell
    };

    let mut child = shell
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("spawning shell")?;

    let stdout = child.stdout.take().context("capturing stdout")?;
    let stderr = child.stderr.take().context("capturing stderr")?;
    let stdout_task = tokio::spawn(read_stream(stdout, progress.clone(), "stdout"));
    let stderr_task = tokio::spawn(read_stream(stderr, progress.clone(), "stderr"));

    let status = match timeout(Duration::from_secs(secs), child.wait()).await {
        Ok(status) => Some(status.context("waiting for shell")?),
        Err(_) => {
            child.kill().await.ok();
            None
        }
    };
    let stdout = stdout_task.await.context("joining stdout reader")??;
    let stderr = stderr_task.await.context("joining stderr reader")??;
    let Some(status) = status else {
        remove_stream_files(&[&stdout, &stderr]);
        anyhow::bail!("command timed out after {secs}s");
    };
    finish_bash_output(stdout, stderr, status.code().unwrap_or(-1))
}

struct StreamCapture {
    path: PathBuf,
    tail: VecDeque<u8>,
    bytes: usize,
    lines: usize,
}

impl StreamCapture {
    fn tail_text(&self) -> String {
        String::from_utf8_lossy(&self.tail.iter().copied().collect::<Vec<_>>()).into_owned()
    }
}

async fn read_stream<R>(mut reader: R, progress: Progress, label: &str) -> Result<StreamCapture>
where
    R: tokio::io::AsyncRead + Unpin,
{
    let path = stream_temp_path(label);
    let file = std::fs::File::create(&path)
        .with_context(|| format!("creating command output spool {}", path.display()))?;
    let mut spool = BufWriter::new(file);
    let (max_bytes, _) = output_limits("bash");
    let mut tail = VecDeque::new();
    let mut total_bytes = 0usize;
    let mut total_lines = 0usize;
    let mut last_byte = None;
    let mut progress_batch = VecDeque::new();
    let mut last_progress = std::time::Instant::now();
    let mut buffer = [0u8; 8_192];

    loop {
        let read = reader
            .read(&mut buffer)
            .await
            .context("reading command output")?;
        if read == 0 {
            break;
        }
        let chunk = &buffer[..read];
        spool.write_all(chunk).context("spooling command output")?;
        total_bytes = total_bytes.saturating_add(read);
        total_lines =
            total_lines.saturating_add(chunk.iter().filter(|byte| **byte == b'\n').count());
        last_byte = chunk.last().copied();

        tail.extend(chunk);
        let excess = tail.len().saturating_sub(max_bytes);
        tail.drain(..excess);

        progress_batch.extend(chunk);
        let excess = progress_batch.len().saturating_sub(PROGRESS_BATCH_BYTES);
        progress_batch.drain(..excess);
        if last_progress.elapsed() >= PROGRESS_INTERVAL {
            let bytes: Vec<u8> = progress_batch.iter().copied().collect();
            let text = String::from_utf8_lossy(&bytes);
            progress.report(text.trim_end());
            progress_batch.clear();
            last_progress = std::time::Instant::now();
        }
    }
    if !progress_batch.is_empty() {
        let bytes: Vec<u8> = progress_batch.iter().copied().collect();
        let text = String::from_utf8_lossy(&bytes);
        progress.report(text.trim_end());
    }
    if total_bytes > 0 && last_byte != Some(b'\n') {
        total_lines = total_lines.saturating_add(1);
    }
    spool.flush().context("flushing command output spool")?;
    Ok(StreamCapture {
        path,
        tail,
        bytes: total_bytes,
        lines: total_lines,
    })
}

fn stream_temp_path(label: &str) -> PathBuf {
    let id = TRUNCATION_ID.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "oxide-command-{}-{label}-{id}.log",
        std::process::id()
    ))
}

fn finish_bash_output(stdout: StreamCapture, stderr: StreamCapture, exit: i32) -> Result<String> {
    let suffix = format!("[exit: {exit}]");
    let marker_bytes = usize::from(stderr.lines > 0) * "[stderr]\n".len();
    let total_bytes = stdout.bytes + stderr.bytes + marker_bytes + suffix.len();
    let total_lines = stdout.lines + stderr.lines + usize::from(stderr.lines > 0) + 1;
    let (max_bytes, max_lines) = output_limits("bash");

    if total_bytes <= max_bytes && total_lines <= max_lines {
        let mut output =
            String::from_utf8_lossy(&std::fs::read(&stdout.path).unwrap_or_default()).into_owned();
        if stderr.lines > 0 {
            output.push_str("[stderr]\n");
            output.push_str(&String::from_utf8_lossy(
                &std::fs::read(&stderr.path).unwrap_or_default(),
            ));
        }
        output.push_str(&suffix);
        remove_stream_files(&[&stdout, &stderr]);
        return Ok(output);
    }

    let mut preview = stdout.tail_text();
    if stderr.lines > 0 {
        preview.push_str("[stderr]\n");
        preview.push_str(&stderr.tail_text());
    }
    preview.push_str(&suffix);
    let preview = tail_preview(&preview, max_bytes, max_lines);
    let saved = truncation_dir().and_then(|dir| {
        save_bash_spool(&dir, &stdout.path, &stderr.path, stderr.lines > 0, &suffix)
    });
    remove_stream_files(&[&stdout, &stderr]);

    let dropped_lines = total_lines.saturating_sub(preview.lines().count());
    let dropped_bytes = total_bytes.saturating_sub(preview.len());
    let mut output = format!("[truncated: {dropped_lines} lines, {dropped_bytes} bytes");
    if let Some(path) = saved {
        output.push_str(&format!("; full: {}", path.display()));
    }
    output.push_str("]\n");
    output.push_str(&preview);
    Ok(output)
}

fn tail_preview(text: &str, max_bytes: usize, max_lines: usize) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let start = lines.len().saturating_sub(max_lines);
    let mut preview = lines[start..].join("\n");
    if preview.len() > max_bytes {
        let mut cut = preview.len() - max_bytes;
        while !preview.is_char_boundary(cut) {
            cut += 1;
        }
        preview = preview[cut..].to_string();
    }
    preview
}

fn save_bash_spool(
    dir: &Path,
    stdout: &Path,
    stderr: &Path,
    has_stderr: bool,
    suffix: &str,
) -> Option<PathBuf> {
    std::fs::create_dir_all(dir).ok()?;
    cleanup_truncated(dir);
    let id = TRUNCATION_ID.fetch_add(1, Ordering::Relaxed);
    let path = dir.join(format!("tool_{}_{}.txt", now_millis(), id));
    let mut output = BufWriter::new(std::fs::File::create(&path).ok()?);
    std::io::copy(&mut std::fs::File::open(stdout).ok()?, &mut output).ok()?;
    if has_stderr {
        output.write_all(b"[stderr]\n").ok()?;
        std::io::copy(&mut std::fs::File::open(stderr).ok()?, &mut output).ok()?;
    }
    output.write_all(suffix.as_bytes()).ok()?;
    output.flush().ok()?;
    Some(path)
}

fn remove_stream_files(captures: &[&StreamCapture]) {
    for capture in captures {
        let _ = std::fs::remove_file(&capture.path);
    }
}

fn now_millis() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or(0)
}

/// Cap tool output so a single result cannot dominate the context window. The
/// preview uses a tool-specific line and byte budget. `bash` keeps its tail
/// (where errors and the exit code live), everything else
/// keeps its head. When content is dropped, the full text is saved under the
/// oxide config dir and the result points at it so the model can inspect the
/// full output without re-running the tool.
fn truncate(name: &str, output: String) -> String {
    truncate_into(name, output, truncation_dir().as_deref())
}

fn truncate_into(name: &str, output: String, dir: Option<&Path>) -> String {
    let (max_bytes, max_lines) = output_limits(name);
    let lines: Vec<&str> = output.lines().collect();
    if output.len() <= max_bytes && lines.len() <= max_lines {
        return output;
    }

    let tail = name == "bash";
    let keep = max_lines.min(lines.len());
    let start = if tail { lines.len() - keep } else { 0 };
    let kept = &lines[start..start + keep];
    let dropped_lines = lines.len() - keep;

    let mut preview = kept.join("\n");
    let mut dropped_bytes = 0;
    if preview.len() > max_bytes {
        if tail {
            let mut cut = preview.len() - max_bytes;
            while !preview.is_char_boundary(cut) {
                cut += 1;
            }
            dropped_bytes = cut;
            preview = preview[cut..].to_string();
        } else {
            let mut cut = max_bytes;
            while !preview.is_char_boundary(cut) {
                cut -= 1;
            }
            dropped_bytes = preview.len() - cut;
            preview.truncate(cut);
        }
    }

    let mut result = format!("[truncated: {dropped_lines} lines, {dropped_bytes} bytes");
    if let Some(path) = dir.and_then(|dir| save_truncated(dir, &output)) {
        result.push_str(&format!("; full: {}", path.display()));
    }
    result.push_str("]\n");
    result.push_str(&preview);
    result
}

fn output_limits(name: &str) -> (usize, usize) {
    match name {
        "bash" => (5_000, 160),
        "grep" | "glob" | "list_dir" => (4_000, 160),
        "webfetch" => (6_000, 200),
        "write_file" | "patch" | "edit" => (3_000, 120),
        _ => (MAX_OUTPUT_BYTES, MAX_OUTPUT_LINES),
    }
}

fn truncation_dir() -> Option<PathBuf> {
    if let Ok(dir) = std::env::var("OXIDE_TRUNCATION_DIR") {
        return Some(PathBuf::from(dir));
    }
    Some(dirs::config_dir()?.join("oxide").join("truncated"))
}

fn save_truncated(dir: &Path, text: &str) -> Option<PathBuf> {
    std::fs::create_dir_all(dir).ok()?;
    cleanup_truncated(dir);
    let stamp = now_millis();
    let id = TRUNCATION_ID.fetch_add(1, Ordering::Relaxed);
    let path = dir.join(format!("tool_{stamp}_{id}.txt"));
    std::fs::write(&path, text).ok()?;
    Some(path)
}

fn cleanup_truncated(dir: &Path) {
    let cutoff = std::time::SystemTime::now()
        .checked_sub(std::time::Duration::from_secs(TRUNCATION_RETENTION_SECS));
    let Some(cutoff) = cutoff else {
        return;
    };
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let Ok(meta) = entry.metadata() else {
            continue;
        };
        if meta.modified().map(|m| m < cutoff).unwrap_or(false) {
            let _ = std::fs::remove_file(entry.path());
        }
    }
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

    #[test]
    fn canonical_tool_names_accept_pi_and_legacy_aliases() {
        assert_eq!(canonical_tool_name("read"), "read_file");
        assert_eq!(canonical_tool_name("read_file"), "read_file");
        assert_eq!(canonical_tool_name("write"), "write_file");
        assert_eq!(canonical_tool_name("edit"), "edit");
        assert_eq!(canonical_tool_name("patch"), "patch");
        assert_eq!(canonical_tool_name("ls"), "list_dir");
        assert_eq!(canonical_tool_name("find"), "glob");
        assert_eq!(canonical_tool_name("bash"), "bash");
        assert_eq!(canonical_tool_name("server__tool"), "server__tool");
        assert_eq!(canonical_tool_name("task"), "task");
    }

    #[test]
    fn specs_expose_pi_tool_names() {
        let mcp = McpRegistry::default();
        let names: Vec<String> = specs(&mcp).into_iter().map(|s| s.function.name).collect();
        for name in ["read", "write", "edit", "bash", "grep", "find", "ls"] {
            assert!(
                names.iter().any(|n| n == name),
                "missing {name} in {names:?}"
            );
        }
    }

    #[tokio::test]
    async fn pi_named_tools_execute() {
        let dir = std::env::temp_dir().join(format!("oxide_pi_names_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mcp = McpRegistry::default();
        let progress = Progress::default();

        let out = execute(
            &call("write", json!({ "path": "a.txt", "content": "hello" })),
            &dir,
            &mcp,
            &progress,
        )
        .await;
        assert!(out.text.contains("wrote"), "{}", out.text);

        let out = execute(
            &call("read", json!({ "path": "a.txt" })),
            &dir,
            &mcp,
            &progress,
        )
        .await;
        assert!(out.text.contains("hello"), "{}", out.text);

        let out = execute(&call("ls", json!({})), &dir, &mcp, &progress).await;
        assert!(out.text.contains("a.txt"), "{}", out.text);

        let out = execute(
            &call("find", json!({ "pattern": "*.txt", "limit": 10 })),
            &dir,
            &mcp,
            &progress,
        )
        .await;
        assert!(out.text.contains("a.txt"), "{}", out.text);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn edit_replaces_unique_text_and_reports_diff() {
        let dir = std::env::temp_dir().join(format!("oxide_edit_{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        let mcp = McpRegistry::default();
        let progress = Progress::default();

        execute(
            &call(
                "write",
                json!({ "path": "f.rs", "content": "fn main() {\n    let x = 1;\n}\n" }),
            ),
            &dir,
            &mcp,
            &progress,
        )
        .await;

        let out = execute(
            &call(
                "edit",
                json!({
                    "path": "f.rs",
                    "edits": [
                        { "oldText": "let x = 1;", "newText": "let x = 2;" },
                        { "oldText": "fn main", "newText": "fn run" }
                    ]
                }),
            ),
            &dir,
            &mcp,
            &progress,
        )
        .await;
        assert!(out.text.contains("2 block(s)"), "{}", out.text);
        assert!(out.diff.is_some());
        let content = std::fs::read_to_string(dir.join("f.rs")).unwrap();
        assert!(content.contains("let x = 2;"));
        assert!(content.contains("fn run"));

        // Non-unique oldText is rejected rather than corrupting the file.
        execute(
            &call("write", json!({ "path": "g.rs", "content": "aa\naa\n" })),
            &dir,
            &mcp,
            &progress,
        )
        .await;
        let out = execute(
            &call(
                "edit",
                json!({ "path": "g.rs", "edits": [{ "oldText": "aa", "newText": "bb" }] }),
            ),
            &dir,
            &mcp,
            &progress,
        )
        .await;
        assert!(out.text.contains("matched 2 times"), "{}", out.text);
        assert_eq!(
            std::fs::read_to_string(dir.join("g.rs")).unwrap(),
            "aa\naa\n"
        );

        // A single edit object and top-level oldText/newText are both accepted.
        let out = execute(
            &call(
                "edit",
                json!({ "path": "f.rs", "oldText": "fn run", "newText": "fn go" }),
            ),
            &dir,
            &mcp,
            &progress,
        )
        .await;
        assert!(out.text.contains("1 block(s)"), "{}", out.text);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn find_and_grep_respect_gitignore() {
        let dir = std::env::temp_dir().join(format!("oxide_gitignore_{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(dir.join("ignored")).unwrap();
        std::fs::write(dir.join(".gitignore"), "ignored/\n*.log\n").unwrap();
        std::fs::write(dir.join("ignored/hidden.rs"), "needle\n").unwrap();
        std::fs::write(dir.join("skip.log"), "needle\n").unwrap();
        std::fs::write(dir.join("kept.rs"), "needle\n").unwrap();
        let mcp = McpRegistry::default();
        let progress = Progress::default();

        let out = execute(
            &call("find", json!({ "pattern": "**/*.rs" })),
            &dir,
            &mcp,
            &progress,
        )
        .await;
        assert!(out.text.contains("kept.rs"), "{}", out.text);
        assert!(!out.text.contains("hidden.rs"), "{}", out.text);

        let out = execute(
            &call("grep", json!({ "pattern": "needle" })),
            &dir,
            &mcp,
            &progress,
        )
        .await;
        assert!(out.text.contains("kept.rs"), "{}", out.text);
        assert!(!out.text.contains("hidden.rs"), "{}", out.text);
        assert!(!out.text.contains("skip.log"), "{}", out.text);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn grep_accepts_pi_parameter_names() {
        let dir = std::env::temp_dir().join(format!("oxide_grep_{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.rs"), "let Foo = 1;\nlet foo = 2;\n").unwrap();
        let mcp = McpRegistry::default();
        let progress = Progress::default();

        let out = execute(
            &call(
                "grep",
                json!({ "pattern": "foo", "glob": "*.rs", "ignoreCase": true, "limit": 50 }),
            ),
            &dir,
            &mcp,
            &progress,
        )
        .await;
        assert!(out.text.contains("let Foo"), "{}", out.text);
        assert!(out.text.contains("let foo"), "{}", out.text);

        let out = execute(
            &call("grep", json!({ "pattern": "Foo", "context": 1 })),
            &dir,
            &mcp,
            &progress,
        )
        .await;
        assert!(out.text.contains("a.rs:1: let Foo"), "{}", out.text);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn webfetch_converts_html_to_markdown() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let body = "<h1>Title</h1><p>Hello <b>world</b></p>";
        let _server = tokio::spawn(async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                tokio::spawn(async move {
                    // Drain the request first: closing a socket that still has
                    // unread data sends an RST on Windows (os error 10053).
                    let mut buf = [0u8; 1024];
                    let _ = socket.read(&mut buf).await;
                    let response = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = socket.write_all(response.as_bytes()).await;
                    let _ = socket.shutdown().await;
                });
            }
        });

        let mcp = McpRegistry::default();
        let progress = Progress::default();
        let dir = std::env::temp_dir();
        let url = format!("http://{addr}/");

        let out = execute(
            &call("webfetch", json!({ "url": url })),
            &dir,
            &mcp,
            &progress,
        )
        .await;
        assert_eq!(out.text, "# Title\n\nHello **world**", "{}", out.text);

        let out = execute(
            &call("webfetch", json!({ "url": url, "format": "text" })),
            &dir,
            &mcp,
            &progress,
        )
        .await;
        assert_eq!(out.text, "Title\n\nHello world", "{}", out.text);

        let out = execute(
            &call("webfetch", json!({ "url": url, "format": "html" })),
            &dir,
            &mcp,
            &progress,
        )
        .await;
        assert!(out.text.contains("<h1>Title</h1>"), "{}", out.text);
    }

    #[tokio::test]
    async fn tools_round_trip() {
        let dir = std::env::temp_dir().join(format!("oxide_test_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mcp = McpRegistry::default();

        let progress = Progress::default();
        let out = execute(
            &call(
                "write_file",
                json!({ "path": "a.txt", "content": "one\ntwo\nthree" }),
            ),
            &dir,
            &mcp,
            &progress,
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
            &progress,
        )
        .await;
        assert!(out.text.contains("two"), "{}", out.text);

        let out = execute(&call("list_dir", json!({})), &dir, &mcp, &progress).await;
        assert!(out.text.contains("a.txt"), "{}", out.text);

        let out = execute(
            &call("bash", json!({ "command": "echo hi" })),
            &dir,
            &mcp,
            &progress,
        )
        .await;
        assert!(
            out.text.contains("hi") && out.text.contains("[exit: 0]"),
            "{}",
            out.text
        );

        let out = execute(
            &call("read_file", json!({ "path": "missing.txt" })),
            &dir,
            &mcp,
            &progress,
        )
        .await;
        assert!(out.text.starts_with("error:"), "{}", out.text);

        std::fs::write(dir.join("shot.png"), b"f").unwrap();
        let out = execute(
            &call("read_file", json!({ "path": "shot.png" })),
            &dir,
            &mcp,
            &progress,
        )
        .await;
        assert_eq!(out.media.len(), 1, "{out:?}");
        assert!(out.text.contains("attached image"), "{}", out.text);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn bash_streams_progress() {
        let dir = std::env::temp_dir().join(format!("oxide_bash_stream_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mcp = McpRegistry::default();

        let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
        let sink = Arc::clone(&seen);
        let progress = Progress::new(Arc::new(move |chunk: &str| {
            sink.lock().unwrap().push(chunk.to_string());
        }));

        let out = execute(
            &call("bash", json!({ "command": "echo one && echo two" })),
            &dir,
            &mcp,
            &progress,
        )
        .await;
        assert!(
            out.text.contains("one") && out.text.contains("two"),
            "{}",
            out.text
        );

        let seen = seen.lock().unwrap();
        let streamed = seen.join("\n");
        assert!(streamed.contains("one"), "{seen:?}");
        assert!(streamed.contains("two"), "{seen:?}");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn stream_capture_spools_full_output_and_bounds_memory() {
        use tokio::io::AsyncWriteExt;

        let (mut writer, reader) = tokio::io::duplex(32_768);
        let payload = vec![b'x'; 20_000];
        writer.write_all(&payload).await.unwrap();
        drop(writer);

        let updates = Arc::new(std::sync::Mutex::new(Vec::new()));
        let sink = Arc::clone(&updates);
        let progress = Progress::new(Arc::new(move |chunk: &str| {
            sink.lock().unwrap().push(chunk.to_string());
        }));
        let capture = read_stream(reader, progress, "test").await.unwrap();

        assert_eq!(capture.bytes, payload.len());
        assert!(capture.tail.len() <= output_limits("bash").0);
        assert_eq!(std::fs::read(&capture.path).unwrap(), payload);
        assert!(updates
            .lock()
            .unwrap()
            .iter()
            .all(|chunk| chunk.len() <= PROGRESS_BATCH_BYTES));
        remove_stream_files(&[&capture]);
    }

    #[test]
    fn walk_stops_when_visitor_requests_it() {
        let dir = std::env::temp_dir().join(format!("oxide_walk_stop_{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        for name in ["a", "b", "c"] {
            std::fs::write(dir.join(name), name).unwrap();
        }

        let mut visited = 0;
        walk(&dir, &mut |_| {
            visited += 1;
            false
        });
        assert_eq!(visited, 1);
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
    fn converts_html_to_markdown_for_webfetch() {
        let md = crate::html::to_markdown("<h1>Title</h1><p>Hello &amp; bye</p>");
        assert_eq!(md, "# Title\n\nHello & bye");
        let text = crate::html::to_text("<h1>Title</h1><p>Hello &amp; bye</p>");
        assert_eq!(text, "Title\n\nHello & bye");
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
        assert!(out.text.contains("f.txt"), "{}", out.text);
        assert!(out.diff.is_some());
        assert_eq!(
            std::fs::read_to_string(dir.join("f.txt")).unwrap(),
            "one\nTWO\nthree\n"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn write_file_reports_a_diff() {
        let dir = std::env::temp_dir().join(format!("oxide_write_diff_{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("f.txt"), "one\ntwo\n").unwrap();

        let out = write_file(&dir, &json!({ "path": "f.txt", "content": "one\nTWO\n" })).unwrap();
        let diff = out.diff.expect("write_file should report a diff");
        assert!(diff.text.contains("TWO"), "{}", diff.text);
        assert!(diff.text.contains("two"), "{}", diff.text);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn truncate_keeps_bash_tail_and_exit_code() {
        let dir = std::env::temp_dir().join(format!("oxide_trunc_bash_{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();

        let mut output = String::new();
        for i in 0..(MAX_OUTPUT_LINES + 50) {
            output.push_str(&format!("line {i}\n"));
        }
        output.push_str("[exit: 7]");

        let result = truncate_into("bash", output, Some(&dir));
        assert!(result.contains("[exit: 7]"), "{result}");
        assert!(result.contains("[truncated:"), "{result}");
        assert!(!result.contains("line 0\n"), "{result}");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn truncate_keeps_head_for_reads() {
        let dir = std::env::temp_dir().join(format!("oxide_trunc_head_{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();

        let mut output = String::new();
        for i in 0..(MAX_OUTPUT_LINES + 50) {
            output.push_str(&format!("line {i}\n"));
        }

        let result = truncate_into("read_file", output, Some(&dir));
        assert!(result.contains("line 0\n"), "{result}");
        assert!(
            !result.contains(&format!("line {}\n", MAX_OUTPUT_LINES + 40)),
            "{result}"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn truncate_saves_full_output() {
        let dir = std::env::temp_dir().join(format!("oxide_trunc_save_{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();

        let output = "x".repeat(MAX_OUTPUT_BYTES + 100);
        let result = truncate_into("read_file", output.clone(), Some(&dir));
        assert!(result.contains("; full:"), "{result}");

        let saved: Vec<_> = std::fs::read_dir(&dir).unwrap().flatten().collect();
        assert_eq!(saved.len(), 1, "{saved:?}");
        assert_eq!(std::fs::read_to_string(saved[0].path()).unwrap(), output);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn read_file_caps_long_lines() {
        let dir = std::env::temp_dir().join(format!("oxide_read_cap_{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("long.txt"), "a".repeat(MAX_LINE_LEN + 500)).unwrap();

        let out = read_file(&dir, &json!({ "path": "long.txt" })).unwrap();
        assert!(out.text.contains('…'), "{}", out.text);
        assert!(out.text.len() < MAX_LINE_LEN + 100, "{}", out.text.len());

        std::fs::remove_dir_all(&dir).ok();
    }
}
