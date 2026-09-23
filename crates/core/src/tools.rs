use crate::diff;
use crate::llm::{ContentPart, FunctionSpec, ToolCall, ToolSpec};
use crate::mcp::McpRegistry;
use crate::media;
use anyhow::{Context, Result};
use serde_json::{json, Value};
use std::collections::{BTreeSet, VecDeque};
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use tokio::io::AsyncReadExt;
use tokio::time::{timeout, Duration};

const MAX_OUTPUT_BYTES: usize = 6_000;
const MAX_OUTPUT_LINES: usize = 250;
const MAX_LINE_LEN: usize = 1_000;
/// `read` gets a larger budget than the other tools: the prompt tells the model
/// to read a whole file in one call, and a 6 KB cap forced ordinary source
/// files into several paged round trips.
const READ_MAX_BYTES: usize = 16_000;
const READ_MAX_LINES: usize = 400;
const DEFAULT_READ_LINES: usize = READ_MAX_LINES;
const TRUNCATION_RETENTION_SECS: u64 = 7 * 24 * 60 * 60;
const PROGRESS_BATCH_BYTES: usize = 4_096;
const PROGRESS_INTERVAL: Duration = Duration::from_millis(50);
/// How long to wait for the output readers to drain after the shell exits. A
/// background descendant can keep the stdout/stderr pipe open indefinitely, so
/// this must be bounded or the tool hangs even though the shell is gone.
const STREAM_DRAIN_GRACE: Duration = Duration::from_secs(1);
/// Default timeout for an arbitrary shell command.
const DEFAULT_BASH_TIMEOUT_SECS: u64 = 120;
/// Build and test runners get a longer default: a cold Gradle, Maven or Cargo
/// build routinely runs past two minutes, and timing it out only makes the
/// agent re-run it (often several times).
const BUILD_BASH_TIMEOUT_SECS: u64 = 600;

static TRUNCATION_ID: AtomicU64 = AtomicU64::new(0);

/// Maps a tool name to its internal canonical form, accepting both the Pi-style
/// names (`read`, `write`, `edit`, `ls`, `find`, `grep`, `bash`) and the legacy
/// oxide names (`read_file`, `write_file`, `patch`, `list_dir`, `glob`). The
/// agent-level tools (`task`, `skill`, `memory`, `diagnostics`) and
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
#[derive(Debug, Clone, Default, serde::Serialize)]
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
            "Read a file. Text files are returned with line numbers; image (png/jpg/gif/webp) and PDF files are returned as viewable attachments. If `path` is a directory, its entries are listed instead. Absolute paths and paths outside the project are allowed. A line longer than 1000 characters is split into continuation chunks (`N|`, `N+|`, …), and `offset`/`limit` count those display lines, so an over-long line (a minified JSON value) can be paged through instead of being cut off.",
            json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "File path; absolute paths are allowed" },
                    "offset": { "type": "integer", "description": "1-based display line to start from (text only; a long line counts once per chunk)" },
                    "limit": { "type": "integer", "description": "Maximum number of display lines to return (text only, default 400)" }
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
            "List the entries of a directory. Absolute paths and paths outside the project are allowed.",
            json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Directory path (default: .); absolute paths are allowed" },
                    "limit": { "type": "integer", "description": "Maximum number of entries to return" }
                }
            }),
        ),
        spec(
            "bash",
            "Run one focused shell command from the project root and return its combined output. Use it for programs, not for inspecting files: prefer `read`, `grep`, `find`, and `ls`, and never chain unrelated commands with `;`/`&&` or sweep the whole filesystem with `find /`.",
            json!({
                "type": "object",
                "properties": {
                    "command": { "type": "string", "description": "Shell command to execute" },
                    "timeout": { "type": "integer", "description": "Timeout in milliseconds (default 120000; build/test commands get 600000)" }
                },
                "required": ["command"]
            }),
        ),
        spec(
            "find",
            "Find files by glob pattern (e.g. `**/*.rs`, `src/*.md`). Patterns match paths relative to the search directory; use `**` for recursive matching. Absolute paths are allowed.",
            json!({
                "type": "object",
                "properties": {
                    "pattern": { "type": "string", "description": "Glob pattern to match" },
                    "path": { "type": "string", "description": "Directory to search in (default: .); absolute paths are allowed" },
                    "limit": { "type": "integer", "description": "Maximum number of results to return" }
                },
                "required": ["pattern"]
            }),
        ),
        spec(
            "grep",
            "Search file contents for a pattern and return matching `path:line: text` entries. The pattern matches a literal substring by default; set `regex` to treat it as a regular expression. Absolute paths are allowed.",
            json!({
                "type": "object",
                "properties": {
                    "pattern": { "type": "string", "description": "Text or regular expression to search for" },
                    "path": { "type": "string", "description": "Directory to search in (default: .)" },
                    "glob": { "type": "string", "description": "Glob pattern to restrict which file names are searched (e.g. `*.rs`)" },
                    "ignoreCase": { "type": "boolean", "description": "Case-insensitive search" },
                    "regex": { "type": "boolean", "description": "Treat `pattern` as a regular expression (default: literal substring)" },
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
            "Fetch a URL and return its contents as Markdown (default), readable plain text, or raw HTML. GitHub and GitLab pull requests, merge requests, and issues should be read with their `gh`/`glab` CLIs instead.",
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
    let configured = mcp.configured_servers();
    if !configured.is_empty() {
        let names: Vec<String> = configured.iter().map(|(name, _)| name.clone()).collect();
        let sources = configured
            .iter()
            .map(|(name, source)| {
                let domains = mcp
                    .server_domains(name)
                    .map(|d| d.join(", "))
                    .unwrap_or_default();
                let domain_hint = if domains.is_empty() {
                    String::new()
                } else {
                    format!(" (domains: {domains})")
                };
                format!("`{name}`{domain_hint} ({source})")
            })
            .collect::<Vec<_>>()
            .join(", ");
        specs.push(spec(
            "mcp_load",
            &format!(
                "Load one configured MCP server on demand and reveal its tools. Use this before answering requests that belong to a configured service, including when a document, ticket, or other service URL identifies the server. Route by URL host first: if a pasted link's domain matches a server's listed domains, call mcp_load for that server instead of webfetch. Configured servers: {sources}"
            ),
            json!({
                "type": "object",
                "properties": {
                    "server": {
                        "type": "string",
                        "enum": names,
                        "description": "Configured MCP server to load"
                    }
                },
                "required": ["server"]
            }),
        ));
    }
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
            "mcp_load" => match args.get("server").and_then(Value::as_str) {
                Some(server) => mcp.load(server).await.map(ToolOutput::text),
                None => Err(anyhow::anyhow!("missing `server`")),
            },
            "bash" => bash(cwd, &args, progress).await.map(ToolOutput::text),
            "webfetch" => webfetch_guarded(&args, mcp).await.map(ToolOutput::text),
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
    if full.is_dir() {
        let mut entries = dir_entries(&full)?;
        let truncated = entries.len() > MAX_MATCHES;
        entries.truncate(MAX_MATCHES);
        let mut out = format!("{} is a directory. Entries:\n", full.display());
        out.push_str(&entries.join("\n"));
        if truncated {
            out.push_str("\n... [truncated]");
        }
        return Ok(ToolOutput::text(out));
    }
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

    // A present-but-unparseable value (some models emit `"offset": ".130"`)
    // must fail instead of silently restarting at line 1.
    let offset = strict_int_arg(args, "offset")?.unwrap_or(1).max(1);
    let limit = strict_int_arg(args, "limit")?.unwrap_or(DEFAULT_READ_LINES);

    let bytes = std::fs::read(&full).with_context(|| format!("reading {}", full.display()))?;
    let content = match String::from_utf8(bytes) {
        Ok(content) => content,
        Err(err) => {
            return Ok(ToolOutput::text(format!(
                "binary file {} ({} bytes) — not shown",
                full.display(),
                err.as_bytes().len()
            )));
        }
    };

    let lines: Vec<&str> = content.lines().collect();
    // A long line is served in chunks so it can still be paged through; each
    // chunk is one display line, which is what `offset`/`limit` count.
    let total: usize = lines
        .iter()
        .map(|line| line.chars().count().max(1).div_ceil(MAX_LINE_LEN))
        .sum();
    let budget = READ_MAX_BYTES.saturating_sub(128);
    let mut numbered: Vec<String> = Vec::new();
    let mut used = 0usize;
    let mut display = 0usize;
    let mut next_offset: Option<usize> = None;
    'outer: for (file_index, line) in lines.iter().enumerate() {
        for (chunk_index, chunk) in line_chunks(line).into_iter().enumerate() {
            display += 1;
            if display < offset {
                continue;
            }
            if numbered.len() >= limit {
                next_offset = Some(display);
                break 'outer;
            }
            let prefix = if chunk_index == 0 {
                format!("{}|", file_index + 1)
            } else {
                format!("{}+|", file_index + 1)
            };
            let entry = format!("{prefix}{chunk}");
            if !numbered.is_empty() && used + entry.len() + 1 > budget {
                next_offset = Some(display);
                break 'outer;
            }
            used += entry.len() + 1;
            numbered.push(entry);
        }
    }

    let mut out = numbered.join("\n");
    if let Some(next) = next_offset {
        out.push_str(&format!(
            "\n... [{} more lines; use offset={}]",
            total + 1 - next,
            next
        ));
    }
    Ok(ToolOutput::text(out))
}

/// Splits a line into chunks of at most `MAX_LINE_LEN` characters so a very
/// long line (a minified JSON value, a wide table row) can be paged through
/// with `read`'s `offset`/`limit` instead of being cut off at the first chunk.
fn line_chunks(line: &str) -> Vec<&str> {
    if line.chars().count() <= MAX_LINE_LEN {
        return vec![line];
    }
    let mut chunks = Vec::new();
    let mut start = 0;
    let mut count = 0;
    for (index, _) in line.char_indices() {
        if count == MAX_LINE_LEN {
            chunks.push(&line[start..index]);
            start = index;
            count = 0;
        }
        count += 1;
    }
    chunks.push(&line[start..]);
    chunks
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
    let mut string_error = None;
    match args.get("edits") {
        Some(Value::String(text)) => match serde_json::from_str::<Value>(text) {
            Ok(parsed) => collect_edits(&parsed, &mut raw),
            Err(err) => string_error = Some(err),
        },
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
        if let Some(err) = string_error {
            anyhow::bail!(
                "`edits` is a JSON string that did not parse ({err}); pass `edits` as an array of {{oldText,newText}} objects"
            );
        }
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

fn dir_entries(full: &Path) -> Result<Vec<String>> {
    let mut entries: Vec<String> = std::fs::read_dir(full)
        .with_context(|| format!("listing {}", full.display()))?
        .filter_map(|entry| entry.ok())
        .map(|entry| {
            let name = entry.file_name().to_string_lossy().to_string();
            let file_type = entry.file_type();
            let is_symlink = file_type.as_ref().map(|t| t.is_symlink()).unwrap_or(false);
            if is_symlink {
                return match std::fs::read_link(entry.path()) {
                    Ok(target) => format!("{name} -> {}", target.display()),
                    Err(_) => name,
                };
            }
            if file_type.map(|t| t.is_dir()).unwrap_or(false) {
                format!("{name}/")
            } else {
                name
            }
        })
        .collect();
    entries.sort();
    Ok(entries)
}

fn list_dir(cwd: &Path, args: &Value) -> Result<String> {
    let path = args.get("path").and_then(Value::as_str).unwrap_or(".");
    let limit = int_arg(args, "limit").unwrap_or(MAX_MATCHES);
    let full = resolve(cwd, path);

    let mut entries = dir_entries(&full)?;
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
    let use_regex = bool_arg(args, "regex", "use_regex");
    let context = int_arg(args, "context").unwrap_or(0);
    let limit = int_arg(args, "limit").unwrap_or(MAX_MATCHES);
    let root = resolve(cwd, base);

    // `rg` applies the same search with far less I/O, so prefer it and fall
    // back to the dependency-free walker when it is unavailable.
    if let Some(output) = rg_grep(
        &root,
        pattern,
        include,
        ignore_case,
        context,
        limit,
        use_regex,
    ) {
        return Ok(output);
    }
    let matcher = GrepMatcher::new(pattern, use_regex, ignore_case)?;
    rust_grep(&root, &matcher, include, context, limit)
}

/// How a `grep` line is matched: a literal substring or a compiled regex.
enum GrepMatcher {
    Literal { needle: String, ignore_case: bool },
    Regex(regex::Regex),
}

impl GrepMatcher {
    fn new(pattern: &str, use_regex: bool, ignore_case: bool) -> Result<Self> {
        if use_regex {
            let regex = regex::RegexBuilder::new(pattern)
                .case_insensitive(ignore_case)
                .build()
                .map_err(|err| anyhow::anyhow!("invalid `pattern` regex: {err}"))?;
            Ok(GrepMatcher::Regex(regex))
        } else {
            Ok(GrepMatcher::Literal {
                needle: if ignore_case {
                    pattern.to_lowercase()
                } else {
                    pattern.to_string()
                },
                ignore_case,
            })
        }
    }

    fn is_match(&self, line: &str) -> bool {
        match self {
            GrepMatcher::Literal {
                needle,
                ignore_case,
            } => {
                if *ignore_case {
                    line.to_lowercase().contains(needle)
                } else {
                    line.contains(needle.as_str())
                }
            }
            GrepMatcher::Regex(regex) => regex.is_match(line),
        }
    }
}

/// Runs `grep` through `ripgrep` when it is on `PATH`, returning `None` so the
/// caller can fall back to the built-in walker.
fn rg_grep(
    root: &Path,
    pattern: &str,
    include: Option<&str>,
    ignore_case: bool,
    context: usize,
    limit: usize,
    use_regex: bool,
) -> Option<String> {
    if !command_exists("rg") {
        return None;
    }
    let (dir, target) = if root.is_dir() {
        (root.to_path_buf(), ".".to_string())
    } else {
        let parent = root.parent().unwrap_or(root).to_path_buf();
        let name = root.file_name()?.to_string_lossy().to_string();
        (parent, name)
    };
    let mut command = std::process::Command::new("rg");
    command.current_dir(&dir).args([
        "--no-require-git",
        "--hidden",
        "--json",
        "--no-messages",
        "--glob",
        "!.git",
        "--glob",
        "!node_modules",
        "--glob",
        "!target",
        "--glob",
        "!.venv",
    ]);
    if !use_regex {
        command.arg("--fixed-strings");
    }
    if ignore_case {
        command.arg("--ignore-case");
    }
    if context > 0 {
        command.arg("--context").arg(context.to_string());
    }
    if let Some(include) = include {
        command.arg("--glob").arg(include);
    }
    command.arg("--").arg(pattern).arg(&target);
    let output = command.output().ok()?;
    // rg exits 0 on matches, 1 on none, and 2 on errors we should fall back on.
    match output.status.code() {
        Some(0) | Some(1) => {}
        _ => return None,
    }
    let stdout = String::from_utf8(output.stdout).ok()?;
    let mut hits: Vec<String> = Vec::new();
    for line in stdout.lines() {
        if hits.len() > limit {
            break;
        }
        let Ok(event) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let kind = match event.get("type").and_then(Value::as_str) {
            Some(kind @ ("match" | "context")) => kind,
            _ => continue,
        };
        let data = &event["data"];
        let Some(path) = data.pointer("/path/text").and_then(Value::as_str) else {
            continue;
        };
        let Some(line_number) = data.get("line_number").and_then(Value::as_u64) else {
            continue;
        };
        let Some(text) = data.pointer("/lines/text").and_then(Value::as_str) else {
            continue;
        };
        let path = path
            .strip_prefix("./")
            .or_else(|| path.strip_prefix(".\\"))
            .unwrap_or(path);
        let text = text.trim_end_matches(['\n', '\r']);
        let marker = if kind == "match" { ':' } else { '-' };
        hits.push(format!("{path}{marker}{line_number}{marker} {text}"));
    }
    Some(finish_hits(hits, limit))
}

/// Greps the tree with the built-in parallel walker.
fn rust_grep(
    root: &Path,
    matcher: &GrepMatcher,
    include: Option<&str>,
    context: usize,
    limit: usize,
) -> Result<String> {
    // Walking is cheap compared to reading and scanning file contents, so
    // gather the candidates first and fan the reads across the available cores.
    let mut candidates: Vec<PathBuf> = Vec::new();
    walk(root, &mut |path| {
        if let Some(include) = include {
            if let Some(name) = path.file_name().and_then(|name| name.to_str()) {
                if !glob_match(include, name) {
                    return true;
                }
            }
        }
        candidates.push(path.to_path_buf());
        true
    });
    let hits = scan_files(root, &candidates, matcher, context, limit);
    Ok(finish_hits(hits, limit))
}

/// Truncates to `limit` hits and joins them, reporting truncation.
fn finish_hits(mut hits: Vec<String>, limit: usize) -> String {
    if hits.is_empty() {
        return "no matches".to_string();
    }
    let truncated = hits.len() > limit;
    hits.truncate(limit);
    let mut out = hits.join("\n");
    if truncated {
        out.push_str("\n... [truncated]");
    }
    out
}

/// Whether a binary is resolvable on `PATH`.
fn command_exists(name: &str) -> bool {
    let Some(path) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&path)
        .any(|dir| dir.join(name).is_file() || dir.join(format!("{name}.exe")).is_file())
}

/// Scans the candidate files for a matcher's hits across the available cores,
/// stopping once one more than `limit` matches have been collected so the
/// caller can report truncation.
fn scan_files(
    root: &Path,
    candidates: &[PathBuf],
    matcher: &GrepMatcher,
    context: usize,
    limit: usize,
) -> Vec<String> {
    if candidates.is_empty() {
        return Vec::new();
    }
    let stop_at = limit.saturating_add(1).max(1);
    let threads = std::thread::available_parallelism()
        .map(|count| count.get())
        .unwrap_or(1)
        .min(candidates.len());
    let next = AtomicUsize::new(0);
    let found = AtomicUsize::new(0);

    let mut results: Vec<(usize, Vec<String>)> = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..threads)
            .map(|_| {
                scope.spawn(|| {
                    let mut local: Vec<(usize, Vec<String>)> = Vec::new();
                    loop {
                        if found.load(Ordering::Relaxed) >= stop_at {
                            break;
                        }
                        let index = next.fetch_add(1, Ordering::Relaxed);
                        let Some(path) = candidates.get(index) else {
                            break;
                        };
                        let file_hits = scan_file(root, path, matcher, context, stop_at);
                        if file_hits.is_empty() {
                            continue;
                        }
                        found.fetch_add(file_hits.len(), Ordering::Relaxed);
                        local.push((index, file_hits));
                    }
                    local
                })
            })
            .collect();
        handles
            .into_iter()
            .flat_map(|handle| handle.join().unwrap_or_default())
            .collect()
    });
    results.sort_by_key(|(index, _)| *index);
    results.into_iter().flat_map(|(_, hits)| hits).collect()
}

/// Reads a single file and returns its formatted matches, stopping at `max`.
fn scan_file(
    root: &Path,
    path: &Path,
    matcher: &GrepMatcher,
    context: usize,
    max: usize,
) -> Vec<String> {
    let Some(content) = read_text_file(path) else {
        return Vec::new();
    };
    let rel = path
        .strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/");
    let lines: Vec<&str> = content.lines().collect();
    let mut hits = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        if !matcher.is_match(line) {
            continue;
        }
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
        if hits.len() >= max {
            break;
        }
    }
    hits
}

/// Sniffed bytes inspected before committing to a full file read.
const BINARY_SNIFF_BYTES: usize = 8 * 1024;

/// Reads a file as UTF-8 text, skipping binaries. Only a prefix is inspected
/// first, so a large binary is abandoned without reading it all.
fn read_text_file(path: &Path) -> Option<String> {
    use std::io::Read;
    let mut file = std::fs::File::open(path).ok()?;
    let mut prefix = [0u8; BINARY_SNIFF_BYTES];
    let read = file.read(&mut prefix).ok()?;
    if read == 0 {
        return Some(String::new());
    }
    if prefix[..read].contains(&0) {
        return None;
    }
    let mut bytes = Vec::with_capacity(read + 1024);
    bytes.extend_from_slice(&prefix[..read]);
    file.read_to_end(&mut bytes).ok()?;
    String::from_utf8(bytes).ok()
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

/// Like [`int_arg`], but distinguishes an absent value from one that is present
/// and unparseable, so the latter (including explicit JSON `null`) can be
/// reported instead of silently ignored.
fn strict_int_arg(args: &Value, key: &str) -> Result<Option<usize>> {
    match args.get(key) {
        None => Ok(None),
        Some(value) => match int_arg(args, key) {
            Some(number) => Ok(Some(number)),
            None => anyhow::bail!("`{key}` must be an integer, got {value}"),
        },
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

/// Fetches a URL unless a configured MCP server owns it, in which case the
/// server is loaded on demand and the model is pointed at its tools instead of
/// an unauthenticated fetch.
async fn webfetch_guarded(args: &Value, mcp: &McpRegistry) -> Result<String> {
    let url = args
        .get("url")
        .and_then(Value::as_str)
        .context("missing `url`")?;
    if let Some(server) = mcp.url_owned(url) {
        let loaded = mcp.load(&server).await.with_context(|| {
            format!("loading MCP server `{server}` for URL owned by that service")
        })?;
        let hint = format!(
            "This URL belongs to the `{server}` MCP server. Prefer its tools over webfetch. {loaded}"
        );
        return Ok(hint);
    }
    if let Some((route, item)) = forge_route(url) {
        if command_exists(route.cli) {
            return Ok(forge_hint(route, item, url));
        }
    }
    webfetch(args).await
}

/// A hosted forge whose pull/merge requests and issues are better read through
/// its official CLI, which authenticates for private repositories and surfaces
/// review threads, checks, and diffs that a plain page fetch cannot.
struct ForgeRoute {
    service: &'static str,
    host: &'static str,
    cli: &'static str,
    items: &'static [ForgeItem],
}

/// One kind of forge work item (a request or an issue) and the CLI commands
/// that read it, show its diff, and post a reply.
struct ForgeItem {
    /// URL path segment that starts the item, e.g. `pull`/`merge_requests`.
    path: &'static str,
    noun: &'static str,
    /// `<cli>` subcommand plus flags that reads the item.
    view: &'static str,
    /// `<cli>` subcommand that shows the diff, for requests only.
    diff: Option<&'static str>,
    /// `<cli>` subcommand that posts a reply.
    comment: &'static str,
}

const FORGE_ROUTES: &[ForgeRoute] = &[
    ForgeRoute {
        service: "GitHub",
        host: "github.com",
        cli: "gh",
        items: &[
            ForgeItem {
                path: "pull",
                noun: "pull request",
                view: "pr view --comments",
                diff: Some("pr diff"),
                comment: "pr comment",
            },
            ForgeItem {
                path: "issues",
                noun: "issue",
                view: "issue view --comments",
                diff: None,
                comment: "issue comment",
            },
        ],
    },
    ForgeRoute {
        service: "GitLab",
        host: "gitlab.com",
        cli: "glab",
        items: &[
            ForgeItem {
                path: "merge_requests",
                noun: "merge request",
                view: "mr view",
                diff: Some("mr diff"),
                comment: "mr note",
            },
            ForgeItem {
                path: "issues",
                noun: "issue",
                view: "issue view",
                diff: None,
                comment: "issue note",
            },
        ],
    },
];

/// Resolves a forge work-item URL (a pull/merge request or issue) to the route
/// and item describing it, so the caller can steer the model at the forge CLI.
fn forge_route(url: &str) -> Option<(&'static ForgeRoute, &'static ForgeItem)> {
    let trimmed = url.trim_end_matches('/');
    let rest = trimmed
        .strip_prefix("https://")
        .or_else(|| trimmed.strip_prefix("http://"))?;
    let (host, path) = rest.split_once('/')?;
    let route = FORGE_ROUTES.iter().find(|route| route.host == host)?;
    let segments: Vec<&str> = path
        .split('/')
        .filter(|segment| !segment.is_empty())
        .collect();
    let item = route.items.iter().find(|item| {
        segments
            .iter()
            .position(|segment| *segment == item.path)
            .and_then(|start| segments.get(start + 1))
            .is_some_and(|number| !number.trim_start_matches('#').is_empty())
    })?;
    Some((route, item))
}

/// Builds the hint that steers a forge work-item URL to its CLI.
fn forge_hint(route: &ForgeRoute, item: &ForgeItem, url: &str) -> String {
    let url = url.trim_end_matches('/');
    let changes = item
        .diff
        .map(|diff| format!(" Inspect the changes with `{} {diff} {url}`.", route.cli))
        .unwrap_or_default();
    format!(
        "This URL is a {service} {noun}. Use the `{cli}` CLI instead of webfetch — it \
         authenticates for private repositories and exposes review threads, checks, and diffs. \
         Read it with `{cli} {view} {url}`.{changes} Reply with \
         `{cli} {comment} {url} --body \"...\"`.",
        service = route.service,
        noun = item.noun,
        cli = route.cli,
        view = item.view,
        comment = item.comment,
    )
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

/// Returns project-relative files and folders for interactive path completion.
///
/// This shares the tool walker's ignore behavior so the TUI does not suggest
/// build output, dependencies, or files excluded by project `.gitignore`s.
pub fn workspace_paths(root: &Path) -> Vec<String> {
    let mut paths = BTreeSet::new();
    walk(root, &mut |path| {
        let rel = path.strip_prefix(root).unwrap_or(path);
        paths.insert(rel.to_string_lossy().replace('\\', "/"));
        let mut parent = rel.parent();
        while let Some(dir) = parent.filter(|dir| !dir.as_os_str().is_empty()) {
            paths.insert(format!("{}/", dir.to_string_lossy().replace('\\', "/")));
            parent = dir.parent();
        }
        true
    });
    paths.into_iter().collect()
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

/// The timeout for a shell command. An explicit `timeout` (milliseconds, Pi's
/// form) or `timeout_secs` wins; otherwise build and test runners get a longer
/// budget than an arbitrary command.
fn bash_timeout_secs(args: &Value, command: &str) -> u64 {
    if let Some(millis) = int_arg(args, "timeout") {
        return (millis as u64).div_ceil(1000).max(1);
    }
    if let Some(secs) = args.get("timeout_secs").and_then(Value::as_u64) {
        return secs;
    }
    if is_build_command(command) {
        BUILD_BASH_TIMEOUT_SECS
    } else {
        DEFAULT_BASH_TIMEOUT_SECS
    }
}

/// Whether a command invokes a build, test or lint runner. Detection is
/// deliberately loose: a longer timeout on a command that did not need it costs
/// nothing, while timing out a real build costs a full re-run.
fn is_build_command(command: &str) -> bool {
    const RUNNERS: &[&str] = &[
        "gradlew",
        "gradle",
        "mvn",
        "mvnw",
        "cargo",
        "make",
        "bazel",
        "buck",
        "dotnet",
        "npm",
        "yarn",
        "pnpm",
        "pytest",
        "tox",
        "rake",
        "go",
        "bundle",
        "swift",
        "xcodebuild",
        "ctest",
        "meson",
        "ninja",
        "sbt",
    ];
    command.split(['\n', ';', '&', '|']).any(|segment| {
        segment.split_whitespace().any(|word| {
            // Windows invokes ` .\gradlew.bat`, so normalize both separators
            // and the batch/executable suffix before matching.
            let word = word.trim_matches(['\'', '"']);
            let word = word.rsplit(['/', '\\']).next().unwrap_or(word);
            let word = word
                .strip_suffix(".bat")
                .or_else(|| word.strip_suffix(".cmd"))
                .or_else(|| word.strip_suffix(".exe"))
                .unwrap_or(word);
            RUNNERS.contains(&word)
        })
    })
}

async fn bash(cwd: &Path, args: &Value, progress: &Progress) -> Result<String> {
    let command = args
        .get("command")
        .and_then(Value::as_str)
        .context("missing `command`")?;
    let secs = bash_timeout_secs(args, command);

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
    let stdout_path = stream_temp_path("stdout");
    let stderr_path = stream_temp_path("stderr");
    let stdout_task = tokio::spawn(read_stream(stdout, progress.clone(), stdout_path.clone()));
    let stderr_task = tokio::spawn(read_stream(stderr, progress.clone(), stderr_path.clone()));

    let status = match timeout(Duration::from_secs(secs), child.wait()).await {
        Ok(status) => Some(status.context("waiting for shell")?),
        Err(_) => {
            child.kill().await.ok();
            None
        }
    };
    let stdout = join_stream(stdout_task, stdout_path).await?;
    let stderr = join_stream(stderr_task, stderr_path).await?;
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

    /// Rebuild a capture from whatever the reader managed to spool before it
    /// was aborted, so a pipe held open by a background process cannot stall
    /// the tool.
    fn from_spool(path: PathBuf) -> Self {
        let (max_bytes, _) = output_limits("bash");
        let mut bytes = 0usize;
        let mut lines = 0usize;
        let mut tail = VecDeque::new();
        let mut last_byte = None;
        if let Ok(file) = std::fs::File::open(&path) {
            let mut reader = BufReader::new(file);
            let mut buffer = [0u8; 8_192];
            while let Ok(read) = reader.read(&mut buffer) {
                if read == 0 {
                    break;
                }
                let chunk = &buffer[..read];
                bytes = bytes.saturating_add(read);
                lines = lines.saturating_add(chunk.iter().filter(|byte| **byte == b'\n').count());
                last_byte = chunk.last().copied();
                tail.extend(chunk);
                let excess = tail.len().saturating_sub(max_bytes);
                tail.drain(..excess);
            }
        }
        if bytes > 0 && last_byte != Some(b'\n') {
            lines = lines.saturating_add(1);
        }
        StreamCapture {
            path,
            tail,
            bytes,
            lines,
        }
    }
}

/// Wait for a command-output reader to finish, but never block forever: a
/// background process can hold the pipe open after the shell exits. On timeout
/// the reader is aborted and whatever it spooled so far is salvaged.
async fn join_stream(
    mut handle: tokio::task::JoinHandle<Result<StreamCapture>>,
    path: PathBuf,
) -> Result<StreamCapture> {
    match timeout(STREAM_DRAIN_GRACE, &mut handle).await {
        Ok(result) => result.context("joining command output reader")?,
        Err(_) => {
            handle.abort();
            Ok(StreamCapture::from_spool(path))
        }
    }
}

async fn read_stream<R>(mut reader: R, progress: Progress, path: PathBuf) -> Result<StreamCapture>
where
    R: tokio::io::AsyncRead + Unpin,
{
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
            progress.report(sanitize_terminal_output(text.trim_end()));
            progress_batch.clear();
            last_progress = std::time::Instant::now();
        }
    }
    if !progress_batch.is_empty() {
        let bytes: Vec<u8> = progress_batch.iter().copied().collect();
        let text = String::from_utf8_lossy(&bytes);
        progress.report(sanitize_terminal_output(text.trim_end()));
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

/// Removes terminal control sequences from captured command output so it can be
/// rendered or replayed without corrupting the screen. A carriage return
/// overwrites the current line, so only the text after the last one is kept,
/// which also collapses `\r`-based progress output to its final state.
pub fn sanitize_terminal_output(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for (index, line) in text.split('\n').enumerate() {
        if index > 0 {
            out.push('\n');
        }
        // A carriage return overwrites the line, so the visible text is the
        // first non-empty segment counting back from the last one. A trailing
        // return (CRLF) leaves the preceding text untouched.
        let line = line
            .rsplit('\r')
            .find(|segment| !segment.is_empty())
            .unwrap_or("");
        let mut chars = line.chars().peekable();
        while let Some(ch) = chars.next() {
            match ch {
                '\t' => out.push('\t'),
                '\u{1b}' => {
                    if chars.peek() == Some(&'[') {
                        chars.next();
                        for c in chars.by_ref() {
                            if ('\u{40}'..='\u{7e}').contains(&c) {
                                break;
                            }
                        }
                    }
                }
                c if c.is_control() => {}
                c => out.push(c),
            }
        }
    }
    out
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
        return Ok(sanitize_terminal_output(&output));
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
    Ok(sanitize_terminal_output(&output))
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
        "read_file" => (READ_MAX_BYTES, READ_MAX_LINES),
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
    fn sanitize_terminal_output_collapses_progress_and_drops_escapes() {
        assert_eq!(
            sanitize_terminal_output("a\rbb\rccc"),
            "ccc",
            "a carriage return overwrites earlier text on the line"
        );
        assert_eq!(
            sanitize_terminal_output("one\ntwo\rthree\n"),
            "one\nthree\n"
        );
        assert_eq!(sanitize_terminal_output("value\r"), "value");
        assert_eq!(sanitize_terminal_output("a\r\nb\r\n"), "a\nb\n");
        assert_eq!(sanitize_terminal_output("\u{1b}[31mred\u{1b}[0m"), "red");
        assert_eq!(sanitize_terminal_output("keep\ttabs\n"), "keep\ttabs\n");
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

    #[test]
    fn specs_expose_compact_loader_for_configured_mcp_servers() {
        let server = crate::ecosystem::McpServer {
            name: "atlassian".to_string(),
            enabled: true,
            kind: crate::ecosystem::McpKind::Remote {
                url: "https://mcp.atlassian.com/v1/mcp".to_string(),
                headers: Default::default(),
                oauth: None,
            },
            domains: vec![],
        };
        let mcp = McpRegistry::new(&[server]);
        let loader = specs(&mcp)
            .into_iter()
            .find(|spec| spec.function.name == "mcp_load")
            .unwrap();
        assert!(loader.function.description.contains("atlassian"));
        assert!(loader
            .function
            .description
            .contains("https://mcp.atlassian.com/v1/mcp"));
        assert_eq!(
            loader.function.parameters["properties"]["server"]["enum"][0],
            "atlassian"
        );
        assert_eq!(mcp.server_count(), 0);
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
    async fn grep_truncates_and_skips_binary_files() {
        let dir = std::env::temp_dir().join(format!("oxide_grep_binary_{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        for name in ["a.txt", "b.txt", "c.txt"] {
            std::fs::write(dir.join(name), "needle here\n").unwrap();
        }
        std::fs::write(dir.join("bin.dat"), b"\0needle in a binary\n").unwrap();
        let mcp = McpRegistry::default();
        let progress = Progress::default();

        let out = execute(
            &call("grep", json!({ "pattern": "needle", "limit": 2 })),
            &dir,
            &mcp,
            &progress,
        )
        .await;
        assert!(out.text.contains("... [truncated]"), "{}", out.text);
        assert!(!out.text.contains("bin.dat"), "{}", out.text);
        assert_eq!(out.text.matches("needle").count(), 2, "{}", out.text);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn grep_supports_regex_patterns() {
        let dir = std::env::temp_dir().join(format!("oxide_grep_regex_{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("a.java"),
            "class A {}\n@Placeholder(name = \"cheapest_boutique_hotel\")\n",
        )
        .unwrap();
        let mcp = McpRegistry::default();
        let progress = Progress::default();

        // A literal search does not match the pattern as a whole...
        let literal = execute(
            &call("grep", json!({ "pattern": "@Placeholder.*hotel" })),
            &dir,
            &mcp,
            &progress,
        )
        .await;
        assert!(!literal.text.contains("a.java"), "{}", literal.text);

        // ...but `regex: true` does.
        let regex = execute(
            &call(
                "grep",
                json!({ "pattern": "@Placeholder.*hotel", "regex": true }),
            ),
            &dir,
            &mcp,
            &progress,
        )
        .await;
        assert!(regex.text.contains("a.java"), "{}", regex.text);

        let bad = execute(
            &call("grep", json!({ "pattern": "([", "regex": true })),
            &dir,
            &mcp,
            &progress,
        )
        .await;
        assert!(bad.text.contains("invalid"), "{}", bad.text);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn list_dir_shows_symlink_targets() {
        let dir = std::env::temp_dir().join(format!("oxide_ls_symlink_{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("target.txt"), "hi").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink("target.txt", dir.join("link.txt")).unwrap();

        let out = list_dir(&dir, &json!({})).unwrap();
        assert!(out.contains("target.txt"), "{out}");
        #[cfg(unix)]
        assert!(out.contains("link.txt -> target.txt"), "{out}");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn rust_grep_fallback_scans_and_truncates() {
        let dir = std::env::temp_dir().join(format!("oxide_rust_grep_{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(dir.join("nested")).unwrap();
        std::fs::write(dir.join("a.txt"), "needle one\n").unwrap();
        std::fs::write(dir.join("b.txt"), "needle two\n").unwrap();
        std::fs::write(dir.join("nested/c.txt"), "needle three\n").unwrap();

        let literal = GrepMatcher::new("needle", false, false).unwrap();
        let out = rust_grep(&dir, &literal, None, 0, 1).unwrap();
        assert!(out.contains("... [truncated]"), "{out}");
        assert_eq!(out.matches("needle").count(), 1, "{out}");

        let out = rust_grep(&dir, &literal, Some("*.txt"), 0, 10).unwrap();
        assert_eq!(out.matches("needle").count(), 3, "{out}");

        // The built-in walker also does regex when `rg` is unavailable.
        let regex = GrepMatcher::new("n[aeiou]+dle tw[o]", true, false).unwrap();
        let out = rust_grep(&dir, &regex, Some("*.txt"), 0, 10).unwrap();
        assert_eq!(out.matches("needle").count(), 1, "{out}");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn forge_hint_routes_work_items_to_their_cli() {
        let (route, item) =
            forge_route("https://github.com/Skyscanner/landing-pages-api/pull/848").unwrap();
        let pr = forge_hint(
            route,
            item,
            "https://github.com/Skyscanner/landing-pages-api/pull/848",
        );
        assert!(pr.contains("GitHub pull request"), "{pr}");
        assert!(
            pr.contains(
                "gh pr view --comments https://github.com/Skyscanner/landing-pages-api/pull/848"
            ),
            "{pr}"
        );
        assert!(
            pr.contains("gh pr diff https://github.com/Skyscanner/landing-pages-api/pull/848"),
            "{pr}"
        );
        assert!(
            pr.contains(
                "gh pr comment https://github.com/Skyscanner/landing-pages-api/pull/848 --body"
            ),
            "{pr}"
        );

        let (route, item) = forge_route("http://github.com/owner/repo/issues/12/").unwrap();
        let issue = forge_hint(route, item, "http://github.com/owner/repo/issues/12/");
        assert!(issue.contains("GitHub issue"), "{issue}");
        assert!(issue.contains("gh issue view --comments"), "{issue}");
        assert!(!issue.contains("gh issue diff"), "{issue}");

        let (route, item) =
            forge_route("https://gitlab.com/group/sub/repo/-/merge_requests/7").unwrap();
        let mr = forge_hint(
            route,
            item,
            "https://gitlab.com/group/sub/repo/-/merge_requests/7",
        );
        assert!(mr.contains("GitLab merge request"), "{mr}");
        assert!(mr.contains("glab mr view"), "{mr}");
        assert!(mr.contains("glab mr diff"), "{mr}");
        assert!(mr.contains("glab mr note"), "{mr}");

        assert!(forge_route("https://github.com/owner/repo").is_none());
        assert!(forge_route("https://github.com/owner/repo/issues").is_none());
        assert!(forge_route("https://example.com/owner/repo/pull/1").is_none());
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

    #[test]
    fn build_commands_get_a_longer_default_timeout() {
        assert!(is_build_command("./gradlew test"));
        assert!(is_build_command(
            "cd /repo && ./gradlew clean build test 2>&1 | tail -30"
        ));
        assert!(is_build_command("npm run lint"));
        assert!(is_build_command("cargo test --all"));
        // Windows wrappers keep their path separator and `.bat`/`.cmd` suffix.
        assert!(is_build_command(r".\gradlew.bat test"));
        assert!(is_build_command(r".\mvnw.cmd verify"));
        assert!(is_build_command(r"C:\tools\gradle.bat test"));
        assert!(!is_build_command("git log --grep latest"));
        assert!(!is_build_command("echo hello"));

        assert_eq!(
            bash_timeout_secs(&json!({ "command": "./gradlew test" }), "./gradlew test"),
            BUILD_BASH_TIMEOUT_SECS
        );
        assert_eq!(
            bash_timeout_secs(&json!({ "command": "echo hi" }), "echo hi"),
            DEFAULT_BASH_TIMEOUT_SECS
        );
        // An explicit timeout (Pi's milliseconds or the legacy seconds) wins.
        assert_eq!(
            bash_timeout_secs(
                &json!({ "command": "./gradlew test", "timeout": 5_000 }),
                "./gradlew test"
            ),
            5
        );
        assert_eq!(
            bash_timeout_secs(
                &json!({ "command": "./gradlew test", "timeout_secs": 42 }),
                "./gradlew test"
            ),
            42
        );
    }

    #[tokio::test]
    async fn bash_does_not_wait_for_a_background_output_holder() {
        let dir = std::env::temp_dir().join(format!("oxide_bash_bg_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mcp = McpRegistry::default();
        let progress = Progress::new(Arc::new(|_: &str| {}));

        let start = std::time::Instant::now();
        let out = execute(
            &call("bash", json!({ "command": "echo started; sleep 10 &" })),
            &dir,
            &mcp,
            &progress,
        )
        .await;
        assert!(out.text.contains("started"), "{}", out.text);
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "bash waited {:?} for a background process",
            start.elapsed()
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn bash_timeout_returns_despite_a_background_output_holder() {
        let dir = std::env::temp_dir().join(format!("oxide_bash_timeout_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mcp = McpRegistry::default();
        let progress = Progress::new(Arc::new(|_: &str| {}));

        let start = std::time::Instant::now();
        let out = execute(
            &call(
                "bash",
                json!({ "command": "sleep 10 & wait", "timeout": 500 }),
            ),
            &dir,
            &mcp,
            &progress,
        )
        .await;
        assert!(out.text.contains("timed out"), "{}", out.text);
        assert!(
            start.elapsed() < Duration::from_secs(6),
            "bash hung for {:?} after timing out",
            start.elapsed()
        );

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
        let capture = read_stream(reader, progress, stream_temp_path("test"))
            .await
            .unwrap();

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
        for i in 0..(READ_MAX_LINES + 50) {
            output.push_str(&format!("line {i}\n"));
        }

        let result = truncate_into("read_file", output, Some(&dir));
        assert!(result.contains("line 0\n"), "{result}");
        assert!(
            !result.contains(&format!("line {}\n", READ_MAX_LINES + 40)),
            "{result}"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn truncate_saves_full_output() {
        let dir = std::env::temp_dir().join(format!("oxide_trunc_save_{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();

        let output = "x".repeat(READ_MAX_BYTES + 100);
        let result = truncate_into("read_file", output.clone(), Some(&dir));
        assert!(result.contains("; full:"), "{result}");

        let saved: Vec<_> = std::fs::read_dir(&dir).unwrap().flatten().collect();
        assert_eq!(saved.len(), 1, "{saved:?}");
        assert_eq!(std::fs::read_to_string(saved[0].path()).unwrap(), output);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn read_file_pages_through_long_lines() {
        let dir = std::env::temp_dir().join(format!("oxide_read_cap_{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        let line = "a".repeat(READ_MAX_BYTES + MAX_LINE_LEN * 4);
        std::fs::write(dir.join("long.txt"), &line).unwrap();

        // The line is served in continuation chunks, not cut off, and the
        // footer points at the next display line.
        let out = read_file(&dir, &json!({ "path": "long.txt" })).unwrap();
        assert!(out.text.starts_with("1|"), "{}", out.text);
        assert!(out.text.contains("1+|"), "{}", out.text);
        assert!(out.text.contains("use offset="), "{}", out.text);

        // Paging through the footer's offset recovers the whole line.
        let mut offset = 1usize;
        let mut assembled = String::new();
        loop {
            let page = read_file(&dir, &json!({ "path": "long.txt", "offset": offset })).unwrap();
            for entry in page.text.lines() {
                if entry.contains("more lines; use offset=") {
                    break;
                }
                if let Some((_, rest)) = entry.split_once('|') {
                    assembled.push_str(rest);
                }
            }
            match page
                .text
                .split("use offset=")
                .nth(1)
                .and_then(|next| next.trim_end_matches(']').parse::<usize>().ok())
            {
                Some(next) => offset = next,
                None => break,
            }
        }
        assert_eq!(assembled, line);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn read_file_lists_directories_instead_of_erroring() {
        let dir = std::env::temp_dir().join(format!("oxide_read_dir_{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(dir.join("components/nested")).unwrap();
        std::fs::write(dir.join("components/index.tsx"), "export {}").unwrap();

        let out = read_file(&dir, &json!({ "path": "components" })).unwrap();
        assert!(out.text.contains("is a directory"), "{}", out.text);
        assert!(out.text.contains("index.tsx"), "{}", out.text);
        assert!(out.text.contains("nested/"), "{}", out.text);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn read_file_reports_binary_files_without_erroring() {
        let dir = std::env::temp_dir().join(format!("oxide_read_bin_{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("blob.bin"), [0xff, 0xfe, 0x00, 0x01]).unwrap();

        let out = read_file(&dir, &json!({ "path": "blob.bin" })).unwrap();
        assert!(out.text.contains("binary file"), "{}", out.text);
        assert!(out.text.contains("4 bytes"), "{}", out.text);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn read_file_rejects_an_unparseable_offset() {
        let dir =
            std::env::temp_dir().join(format!("oxide_read_bad_offset_{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.txt"), "one\ntwo\nthree\n").unwrap();

        let err = read_file(&dir, &json!({ "path": "a.txt", "offset": ".2" })).unwrap_err();
        assert!(
            err.to_string().contains("`offset` must be an integer"),
            "{err}"
        );

        // An explicit `null` is a present, non-integer value, not an omission.
        let err = read_file(&dir, &json!({ "path": "a.txt", "limit": null })).unwrap_err();
        assert!(
            err.to_string().contains("`limit` must be an integer"),
            "{err}"
        );

        // A numeric string is still accepted.
        let out = read_file(
            &dir,
            &json!({ "path": "a.txt", "offset": "2", "limit": "1" }),
        )
        .unwrap();
        assert!(out.text.contains("2|two"), "{}", out.text);
        assert!(!out.text.contains("1|one"), "{}", out.text);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn edit_reports_an_unparseable_edits_string() {
        let dir = std::env::temp_dir().join(format!("oxide_edit_string_{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.txt"), "hello\n").unwrap();

        let err = edit(
            &dir,
            &json!({ "path": "a.txt", "edits": "[{\"oldText\": \"hello\"" }),
        )
        .unwrap_err();
        assert!(err.to_string().contains("did not parse"), "{err}");

        std::fs::remove_dir_all(&dir).ok();
    }
}
