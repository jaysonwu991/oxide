use crate::diff;
use crate::llm::{ContentPart, FunctionSpec, ToolCall, ToolSpec};
use crate::mcp::McpRegistry;
use crate::media;
use anyhow::{Context, Result};
use serde_json::{json, Value};
use std::collections::{BTreeSet, HashMap, VecDeque};
use std::ffi::OsString;
use std::io::{BufRead, BufReader, BufWriter, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};
use tokio::io::AsyncReadExt;
use tokio::time::{timeout, Duration};

/// Pi's truncation budget (`core/tools/truncate.ts`): 2000 lines or 50 KB,
/// whichever is hit first. A smaller cap is not a speed win — it turns one
/// call into several paged round trips and hides the tail of a failing build
/// from the model, which is exactly the output the next step needs.
const MAX_OUTPUT_BYTES: usize = 50 * 1024;
const MAX_OUTPUT_LINES: usize = 2_000;
const MAX_LINE_LEN: usize = 1_000;
const READ_MAX_BYTES: usize = MAX_OUTPUT_BYTES;
const READ_MAX_LINES: usize = MAX_OUTPUT_LINES;
const DEFAULT_READ_LINES: usize = READ_MAX_LINES;
/// An MCP result is structured data a server chose the shape of, so unlike a
/// file read the model cannot narrow it with `offset`/`limit`, and unlike
/// `grep` it cannot re-ask for less: whatever the server returned is the only
/// answer available. It gets the same budget as the built-ins, with the record
/// elision below keeping a JSON answer readable rather than cut mid-token.
const MCP_OUTPUT_BYTES: usize = MAX_OUTPUT_BYTES;
const MCP_OUTPUT_LINES: usize = MAX_OUTPUT_LINES;
const TRUNCATION_RETENTION_SECS: u64 = 7 * 24 * 60 * 60;
/// What a search that found nothing answers with, in Pi's own words
/// (`coding-agent/src/core/tools/{grep,find}.ts`). Short, but never an empty
/// string: an empty result reads as a failure and invites a pointless retry.
pub const NO_MATCHES: &str = "No matches found";
pub const NO_FILES_FOUND: &str = "No files found matching pattern";
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
/// What a shell reports for a command killed by `SIGPIPE` (128 + 13). A
/// pipeline that writes more than the reader asked for — `ls -lt | head` is
/// the everyday one — ends this way when the reader exits and the writer is
/// cut off, which is the command doing exactly what it was asked rather than
/// failing.
#[cfg(unix)]
const SIGPIPE_EXIT: i32 = 141;

/// Appended to a shell command that ran past its timeout. The usual cause is an
/// unbounded search that walked build output, so the message steers the retry
/// to a narrower command rather than letting the same one time out again.
const BASH_TIMEOUT_HINT: &str = "[the output above is what the command printed before it was \
killed. Retry narrower rather than repeating it: scope it to a directory, exclude build output \
(`target/`, `node_modules/`, `.venv/`, `.git/`), or use the `grep`/`find` tools, which skip those \
and honor .gitignore. Raise `timeout` only for a known-slow build.]";

/// Appended to a failed shell command that required a relative module from a
/// script outside the project. Node resolves `require('./...')` against the
/// script's own directory, not the working directory, so the retry should move
/// the script into the project or use an absolute path.
const BASH_MODULE_HINT: &str = "[a relative import could not be resolved: Node resolves \
`require('./...')`/`import './...'` against the script's own directory, not the working \
directory. This script lives outside the tree it imports from — create it inside the project \
with the `write` tool, or point the import at an absolute path, then run it again.]";

/// Appended to a failed shell command whose relative import could not be
/// resolved from a script inside the project or with no locatable script, where
/// the usual cause is a wrong specifier rather than a script run from outside
/// the tree.
const BASH_MODULE_PATH_HINT: &str = "[a relative import could not be resolved: Node resolves \
`require('./...')`/`import './...'` against the script's own directory, not the working \
directory. Check the specifier and that the file it names exists at that path, then run it \
again.]";

/// Appended to a failed shell command whose output shows the request never
/// completed over the network. `gh`, `curl` and `git` report a TLS handshake
/// timeout, a reset or a DNS lookup failure this way, and the command itself is
/// fine, so the hint asks for the same command again rather than sending the
/// run looking for a repository, authentication or syntax problem that is not
/// there.
const BASH_NETWORK_HINT: &str = "[hint: the command failed on a network request, not on the \
 command itself. That is usually transient — run the same command again before changing \
 approach, and report the failure only if the retry fails too.]";

/// Whether a failed command's output shows a transport-level failure rather
/// than an answer from the service.
fn transient_network_failure(output: &str) -> bool {
    const SIGNALS: &[&str] = &[
        "tls handshake timeout",
        "tls: handshake failure",
        "i/o timeout",
        "connection reset by peer",
        "network is unreachable",
        "temporary failure in name resolution",
        "could not resolve host",
        "eof occurred in violation of protocol",
    ];
    let output = output.to_ascii_lowercase();
    SIGNALS.iter().any(|signal| output.contains(signal))
}

/// Whether a failed command's output shows a relative module specifier that
/// could not be found. Absolute specifiers (a genuinely missing dependency) are
/// excluded, since the fix there is not a path inside the project.
fn unresolved_relative_import(output: &str) -> bool {
    if !(output.contains("Cannot find module")
        || output.contains("Cannot find package")
        || output.contains("MODULE_NOT_FOUND")
        || output.contains("ERR_MODULE_NOT_FOUND"))
    {
        return false;
    }
    ["'./", "\"./", "'../", "\"../"]
        .iter()
        .any(|specifier| output.contains(specifier))
}

/// Chooses the relative-import hint: a script that lives outside `cwd` gets the
/// move-it-into-the-project advice, anything else is told to check the path. A
/// Node require stack lists the requiring script as `- <abs path>`.
fn module_failure_hint(output: &str, cwd: &Path) -> &'static str {
    let outside = output.lines().any(|line| {
        let Some(entry) = line.trim().strip_prefix("- ") else {
            return false;
        };
        let entry = Path::new(entry.trim());
        entry.is_absolute() && !entry.starts_with(cwd)
    });
    if outside {
        BASH_MODULE_HINT
    } else {
        BASH_MODULE_PATH_HINT
    }
}

static TRUNCATION_ID: AtomicU64 = AtomicU64::new(0);

/// A compact, human-readable subject for a built-in search call. Search cards
/// must lead with the pattern: leading with the project path makes several
/// different searches look identical in every front-end.
pub fn search_tool_summary(name: &str, args: &str) -> Option<String> {
    if !matches!(name, "grep" | "find") {
        return None;
    }
    let value: Value = serde_json::from_str(args).ok()?;
    let pattern = value.get("pattern")?.as_str()?.trim();
    if pattern.is_empty() {
        return None;
    }
    let mut summary = serde_json::to_string(pattern).ok()?;
    if let Some(path) = value
        .get("path")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|path| !path.is_empty() && *path != ".")
    {
        summary.push_str(" · ");
        summary.push_str(path);
    }
    Some(summary)
}

/// Whether a successful built-in search returned its exact no-hit sentinel.
/// The raw text remains in the event/session stream for the model; renderers
/// use this classification to replace a large body with a compact count.
pub fn is_empty_search_result(name: &str, output: &str) -> bool {
    matches!(name, "grep" | "find") && matches!(output.trim(), NO_MATCHES | NO_FILES_FOUND)
}

/// A line-numbered diff of a file edit, carried alongside the tool result for
/// display only. It is never sent to the model (the text result is).
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct DiffPreview {
    pub path: String,
    pub text: String,
}

/// The result of running a tool: always a text payload, optionally plus media
/// content parts (an image, a PDF, or a file's own text) that the model should
/// see as content. `terminate` lets a tool (or a `tool.execute.after` plugin
/// hook) end the turn instead of asking the model to react to the result.
#[derive(Debug, Clone, Default)]
pub struct ToolOutput {
    pub text: String,
    pub media: Vec<ContentPart>,
    /// Whether the tool failed. This is deliberately separate from `text` so
    /// front-ends never have to infer state from user-visible wording.
    pub is_error: bool,
    pub terminate: bool,
    pub diff: Option<DiffPreview>,
}

impl ToolOutput {
    pub fn text(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            media: Vec::new(),
            is_error: false,
            terminate: false,
            diff: None,
        }
    }

    pub fn with_media(text: impl Into<String>, media: Vec<ContentPart>) -> Self {
        Self {
            text: text.into(),
            media,
            is_error: false,
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

    pub fn error(text: impl Into<String>) -> Self {
        let text = text.into();
        Self {
            text: text
                .strip_prefix("error:")
                .map(str::trim_start)
                .unwrap_or(&text)
                .to_string(),
            media: Vec::new(),
            is_error: true,
            terminate: false,
            diff: None,
        }
    }

    /// Tool messages do not yet carry a provider-neutral error bit, so retain
    /// the familiar marker in the model/session transcript while keeping it
    /// out of the event text rendered to people.
    pub fn model_text(&self) -> String {
        if self.is_error {
            format!("error: {}", self.text)
        } else {
            self.text.clone()
        }
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
    specs_for_scope(mcp, PathScope::Unrestricted)
}

/// Backward-compatible tool descriptions for callers that only distinguish a
/// current-project boundary from unrestricted access.
pub fn specs_scoped(mcp: &McpRegistry, project_only: bool) -> Vec<ToolSpec> {
    specs_for_scope(
        mcp,
        if project_only {
            PathScope::CurrentProject
        } else {
            PathScope::Unrestricted
        },
    )
}

#[derive(Clone, Copy)]
pub enum PathScope<'a> {
    Unrestricted,
    CurrentProject,
    RegisteredProjects(&'a [PathBuf]),
}

pub fn specs_for_scope(mcp: &McpRegistry, scope: PathScope<'_>) -> Vec<ToolSpec> {
    let project_only = matches!(scope, PathScope::CurrentProject);
    let registered_only = matches!(scope, PathScope::RegisteredProjects(_));
    let read_scope = if project_only {
        " Paths must stay inside the current project."
    } else if registered_only {
        " Paths must stay inside a project registered in Oxide."
    } else {
        " Absolute paths and paths outside the project are allowed."
    };
    let path_description = if project_only {
        "File path relative to the project root"
    } else if registered_only {
        "File path inside a project registered in Oxide; absolute paths are allowed"
    } else {
        "File path; absolute paths are allowed"
    };
    let directory_scope = if project_only {
        " Paths must stay inside the current project."
    } else if registered_only {
        " Paths must stay inside a project registered in Oxide."
    } else {
        " Absolute paths and paths outside the project are allowed."
    };
    let mut specs = vec![
        spec(
            "read",
            &format!("Read a file. Text files are returned with line numbers; image (png/jpg/gif/webp) and PDF files are returned as viewable attachments. If `path` is a directory, its entries are listed instead.{read_scope} A line longer than 1000 characters is split into continuation chunks (`N|`, `N+|`, …), and `offset`/`limit` count those display lines, so an over-long line (a minified JSON value) can be paged through instead of being cut off."),
            json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": path_description },
                    "offset": { "type": "integer", "description": "1-based display line to start from (text only; a long line counts once per chunk)" },
                    "limit": { "type": "integer", "description": "Maximum number of display lines to return (text only, default 2000)" }
                },
                "required": ["path"]
            }),
        ),
        spec(
            "write",
            if project_only { "Create or overwrite a file inside the current project." } else if registered_only { "Create or overwrite a file inside a project registered in Oxide." } else { "Create or overwrite a file with the given content." },
            json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": path_description },
                    "content": { "type": "string", "description": "Full file content to write" }
                },
                "required": ["path", "content"]
            }),
        ),
        spec(
            "edit",
            "Edit a single file using targeted text replacement. Put the file path once in the top-level `path`; use separate calls for different files. Copy every edits[].oldText from a recent read of that same path, and read it again after a formatter or another tool changes it. Every target must be unique in the original file, before any replacements in this call. Omit edits whose oldText equals newText; they are still validated if supplied. The batch is atomic: if any edit fails, none are written, so correct and resend the whole batch. A match is exact first; trailing whitespace differences and the `N|` / `N+|` line numbers that `read` prints are tolerated. If the text has genuinely changed, read the reported region before retrying. Merge overlapping or nested changes into one replacement. Do not include large unchanged regions just to connect distant changes.",
            json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": path_description },
                    "edits": {
                        "type": "array",
                        "description": "One or more replacements for the top-level path only. Each edit is matched against the original file, not incrementally. Omit no-op edits. Do not include overlapping or nested edits. If any edit fails, nothing in this batch is written.",
                        "items": {
                            "type": "object",
                            "properties": {
                                "oldText": { "type": "string", "description": "Text to replace, copied from the file or a read result. It must be unique in the file and must not overlap any other edits[].oldText in the same call; trailing whitespace and `N|` line-number prefixes are ignored when matching." },
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
            &format!("List directory entries, or report a file and its size. Use `read` for file contents.{directory_scope}"),
            json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": if project_only { "Directory inside the current project (default: .)" } else if registered_only { "Directory inside a project registered in Oxide (default: .); absolute paths are allowed" } else { "Directory path (default: .); absolute paths are allowed" } },
                    "limit": { "type": "integer", "description": "Maximum number of entries to return" }
                }
            }),
        ),
        spec(
            "bash",
            if project_only { "Run one focused shell command from the project root and return its combined output. The command must stay inside the current project: do not change to another directory or name paths outside the project. Use it for programs, not for inspecting files. For content searches, call the `grep` tool directly instead of running `grep` or `rg` here; that tool uses ripgrep when available, bounds results, skips dependencies and generated files, and respects ignore files. Prefer `read`, `find`, and `ls` for other inspection. Recursive shell `grep` is refused. Never chain unrelated commands with `;`/`&&` or sweep the whole filesystem with `find /`." } else if registered_only { "Run one focused shell command from the project root and return its combined output. The command may name another project registered in Oxide, but must not access unregistered paths. Use it for programs, not for inspecting files. For content searches, call the `grep` tool directly instead of running `grep` or `rg` here; that tool uses ripgrep when available, bounds results, skips dependencies and generated files, and respects ignore files. Prefer `read`, `find`, and `ls` for other inspection. Recursive shell `grep` is refused. Never chain unrelated commands with `;`/`&&` or sweep the whole filesystem with `find /`." } else { "Run one focused shell command from the project root and return its combined output. Use it for programs, not for inspecting files. For content searches, call the `grep` tool directly instead of running `grep` or `rg` here; that tool uses ripgrep when available, bounds results, skips dependencies and generated files, and respects ignore files. Prefer `read`, `find`, and `ls` for other inspection. Recursive shell `grep` is refused. Never chain unrelated commands with `;`/`&&` or sweep the whole filesystem with `find /`." },
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
            if project_only { "Find files by glob pattern (e.g. `**/*.rs`, `src/*.md`). Patterns match paths relative to the search directory; use `**` for recursive matching. The search directory must stay inside the current project." } else if registered_only { "Find files by glob pattern (e.g. `**/*.rs`, `src/*.md`). Patterns match paths relative to the search directory; use `**` for recursive matching. The search directory must stay inside a project registered in Oxide." } else { "Find files by glob pattern (e.g. `**/*.rs`, `src/*.md`). Patterns match paths relative to the search directory; use `**` for recursive matching. Absolute paths are allowed." },
            json!({
                "type": "object",
                "properties": {
                    "pattern": { "type": "string", "description": "Glob pattern to match" },
                    "path": { "type": "string", "description": if project_only { "Directory inside the current project (default: .)" } else if registered_only { "Directory inside a project registered in Oxide (default: .); absolute paths are allowed" } else { "Directory to search in (default: .); absolute paths are allowed" } },
                    "limit": { "type": "integer", "description": "Maximum number of results to return" }
                },
                "required": ["pattern"]
            }),
        ),
        spec(
            "grep",
            if project_only { "Search file contents inside the current project and return matching `path:line: text` entries. Use this directly for repository searches instead of calling `bash` with `grep` or `rg`. It runs ripgrep when available, respects ignore files, and skips dependency and generated directories. The pattern is a regular expression by default; set `literal: true` to search for plain text. Output is truncated to 2000 matches or 50KB (whichever is hit first)." } else if registered_only { "Search file contents inside projects registered in Oxide and return matching `path:line: text` entries. Use this directly for repository searches instead of calling `bash` with `grep` or `rg`. It runs ripgrep when available, respects ignore files, and skips dependency and generated directories. The pattern is a regular expression by default; set `literal: true` to search for plain text. Output is truncated to 2000 matches or 50KB (whichever is hit first). Absolute paths inside those projects are allowed." } else { "Search file contents and return matching `path:line: text` entries. Use this directly for repository searches instead of calling `bash` with `grep` or `rg`. It runs ripgrep when available, respects ignore files, and skips dependency and generated directories. The pattern is a regular expression by default; set `literal: true` to search for plain text. Output is truncated to 2000 matches or 50KB (whichever is hit first). Absolute paths are allowed." },
            json!({
                "type": "object",
                "properties": {
                    "pattern": { "type": "string", "description": "Search pattern (regular expression by default, or literal text when `literal` is true)" },
                    "path": { "type": "string", "description": "Directory to search in (default: .)" },
                    "glob": { "type": "string", "description": "Glob pattern to restrict which file names are searched (e.g. `*.rs`)" },
                    "ignoreCase": { "type": "boolean", "description": "Case-insensitive search" },
                    "literal": { "type": "boolean", "description": "Treat the pattern as a literal string instead of a regular expression (default: false)" },
                    "context": { "type": "integer", "description": "Number of context lines to include around each match" },
                    "limit": { "type": "integer", "description": "Maximum number of matches to return" }
                },
                "required": ["pattern"]
            }),
        ),
        spec(
            "webfetch",
            "Fetch a public URL and return its contents as Markdown (default), readable plain text, or raw HTML. Do not use this for an authenticated service or console URL: prefer a configured MCP server, then the service's authenticated CLI. In particular, use `aws` for AWS Console URLs and `gh`/`glab` for GitHub or GitLab URLs, because an unauthenticated fetch can disguise a private resource as missing.",
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
        specs.push(spec(
            "tool_search",
            "Search deferred MCP tool metadata with BM25 and declare matching tools for your next call. Always use tool_search for MCP tool discovery, including private links and services behind generic company gateways. Search by capability, service, or task; no domain mapping is needed. Previously discovered tools remain available.",
            json!({"type":"object","properties":{
                "query":{"type":"string","description":"Required capabilities, service, or task"},
                "limit":{"type":"integer","minimum":1,"maximum":64,"description":"Maximum matches; default 8"}
            },"required":["query"]}),
        ));
        // A server may be asked for by a service it answers to as well as by its
        // own name, so the schema lists both and `mcp_load` resolves either.
        let mut names: Vec<String> = configured.iter().map(|(name, _)| name.clone()).collect();
        for (name, _) in &configured {
            for alias in mcp.service_names(name) {
                if !names
                    .iter()
                    .any(|existing| existing.eq_ignore_ascii_case(&alias))
                {
                    names.push(alias);
                }
            }
        }
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
                let aliases: Vec<String> = mcp
                    .service_names(name)
                    .into_iter()
                    .filter(|alias| !name.eq_ignore_ascii_case(alias))
                    .collect();
                let alias_hint = if aliases.is_empty() {
                    String::new()
                } else {
                    format!(" (also answers to: {})", aliases.join(", "))
                };
                format!("`{name}`{alias_hint}{domain_hint} ({source})")
            })
            .collect::<Vec<_>>()
            .join(", ");
        specs.push(spec(
            "mcp_load",
            &format!(
                "Connect or retry one configured MCP server. Tools remain deferred: use tool_search to discover and declare the capabilities you need. A server may be named by a service it answers to as well as by its own name. Configured servers: {sources}"
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
    execute_with_scope(call, cwd, mcp, progress, PathScope::Unrestricted).await
}

pub async fn execute_scoped(
    call: &ToolCall,
    cwd: &Path,
    mcp: &McpRegistry,
    progress: &Progress,
    project_only: bool,
) -> ToolOutput {
    execute_with_scope(
        call,
        cwd,
        mcp,
        progress,
        if project_only {
            PathScope::CurrentProject
        } else {
            PathScope::Unrestricted
        },
    )
    .await
}

pub async fn execute_with_scope(
    call: &ToolCall,
    cwd: &Path,
    mcp: &McpRegistry,
    progress: &Progress,
    scope: PathScope<'_>,
) -> ToolOutput {
    let name = call.function.name.as_str();
    let args = match parse_tool_arguments(name, &call.function.arguments) {
        Ok(args) => args,
        Err(message) => return ToolOutput::error(message),
    };

    let canonical = name;
    if let Err(error) = enforce_path_scope(cwd, canonical, &args, scope) {
        return ToolOutput::error(format!("{error:#}"));
    }
    let result = if mcp.is_tool(name) {
        mcp.call(name, args).await.map(|result| {
            if result.is_error {
                ToolOutput::error(result.text)
            } else {
                ToolOutput::text(result.text)
            }
        })
    } else {
        match canonical {
            "mcp_load" => match args.get("server").and_then(Value::as_str) {
                Some(server) => mcp.load(server).await.map(ToolOutput::text),
                None => Err(anyhow::anyhow!(
                    "mcp_load requires a `server` string, for example {{\"server\":\"github\"}}"
                )),
            },
            "bash" => bash_with_network_retry(cwd, &args, progress).await,
            "webfetch" => webfetch_guarded(&args, mcp).await.map(ToolOutput::text),
            "read" | "write" | "edit" | "ls" | "find" | "grep" => {
                let cwd = cwd.to_path_buf();
                let args = args.clone();
                // `spawn_blocking` requires a `'static` closure, so the owned tool
                // name has to be moved in.
                let tool = canonical.to_string();
                match tokio::task::spawn_blocking(move || match tool.as_str() {
                    "read" => read(&cwd, &args),
                    "write" => write(&cwd, &args),
                    "edit" => edit(&cwd, &args),
                    "ls" => ls(&cwd, &args).map(ToolOutput::text),
                    "find" => find(&cwd, &args).map(ToolOutput::text),
                    "grep" => grep(&cwd, &args).map(ToolOutput::text),
                    _ => unreachable!(),
                })
                .await
                {
                    Ok(result) => result,
                    Err(_) => Ok(ToolOutput::error(format!(
                        "{name} stopped before reporting its result; completion is unknown. \
                         Inspect the target state before deciding whether to retry"
                    ))),
                }
            }
            other => Err(anyhow::anyhow!(
                "unknown tool `{other}`. For an MCP tool, use `tool_search` to discover its \
                 capabilities, or `mcp_load` to retry a connection. Then use the exact tool name \
                 and schema declared on the next step. Do not guess tool names or resource IDs"
            )),
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
            is_error: output.is_error,
            terminate: output.terminate,
            diff: output.diff,
        },
        Err(err) => ToolOutput::error(format!("{err:#}")),
    }
}

/// Parses a tool call's `arguments`.
///
/// A call with no arguments arrives as an empty string from some providers and
/// `{}` from others, so an empty value means an empty object rather than
/// malformed JSON. Without this an MCP tool whose schema has no properties
/// (`atlassianUserInfo`) failed every call with a JSON `EOF` before the server
/// was ever reached.
fn parse_tool_arguments(name: &str, arguments: &str) -> Result<Value, String> {
    let arguments = arguments.trim();
    if arguments.is_empty() {
        return Ok(Value::Object(serde_json::Map::new()));
    }
    serde_json::from_str(arguments).map_err(|_| {
        format!(
            "{name} received invalid JSON arguments. Expected a JSON object{}.",
            expected_arguments(name)
        )
    })
}

fn expected_arguments(name: &str) -> &'static str {
    match name {
        "read" => " with a string `path` and optional integer `offset` and `limit`",
        "write" => " with string `path` and `content` fields",
        "edit" => " with a string `path` and an `edits` array of {oldText,newText} objects",
        "find" => " with a string `pattern` and optional `path`",
        "grep" => " with a string `pattern` and optional `path`, `glob`, and search options",
        "bash" => " with a shell command string in `command`",
        "webfetch" => " with a URL string in `url`",
        "mcp_load" => " with an MCP server name string in `server`",
        _ => "",
    }
}

fn required_string<'a>(args: &'a Value, tool: &str, key: &str, example: &str) -> Result<&'a str> {
    args.get(key).and_then(Value::as_str).ok_or_else(|| {
        anyhow::anyhow!(
            "{tool} requires a `{key}` string, for example {example}{}",
            received_keys(args)
        )
    })
}

fn received_keys(args: &Value) -> String {
    let Some(object) = args.as_object() else {
        return "; received a non-object value".to_string();
    };
    if object.is_empty() {
        "; received an empty object".to_string()
    } else {
        format!(
            "; received keys: {}",
            object
                .keys()
                .map(String::as_str)
                .collect::<Vec<_>>()
                .join(", ")
        )
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

/// Enforces the front-end's filesystem boundary before a built-in tool is
/// dispatched. Existing paths are canonicalized so a symlink inside an allowed
/// root cannot lead outside it; new paths canonicalize their nearest existing
/// parent for the same reason.
fn enforce_path_scope(cwd: &Path, tool: &str, args: &Value, scope: PathScope<'_>) -> Result<()> {
    if matches!(scope, PathScope::Unrestricted) {
        return Ok(());
    }
    let path = match tool {
        "read" | "write" | "ls" => args.get("path").and_then(Value::as_str),
        "find" | "grep" => args.get("path").and_then(Value::as_str).or(Some(".")),
        "edit" => edit_path_value(args),
        "bash" => {
            let command = args
                .get("command")
                .and_then(Value::as_str)
                .unwrap_or_default();
            return enforce_scoped_shell(cwd, command, scope);
        }
        _ => None,
    };
    if let Some(path) = path {
        ensure_path_scope(cwd, path, scope)?;
    }
    Ok(())
}

/// Checks that one path resolves inside `cwd`, including through symlinks. CLI
/// front-ends use this for context and attachments as well as tool calls.
pub fn ensure_project_path(cwd: &Path, path: &str) -> Result<()> {
    ensure_path_scope(cwd, path, PathScope::CurrentProject)
}

pub fn ensure_scoped_path(cwd: &Path, path: &str, scope: PathScope<'_>) -> Result<()> {
    ensure_path_scope(cwd, path, scope)
}

fn ensure_path_scope(cwd: &Path, path: &str, scope: PathScope<'_>) -> Result<()> {
    let candidate = canonical_or_normalized(&resolve(cwd, path))?;
    let allowed = match scope {
        PathScope::Unrestricted => true,
        PathScope::CurrentProject => candidate.starts_with(canonical_or_normalized(cwd)?),
        PathScope::RegisteredProjects(roots) => roots.iter().any(|root| {
            canonical_or_normalized(root).is_ok_and(|root| candidate.starts_with(root))
        }),
    };
    if allowed {
        return Ok(());
    }
    match scope {
        PathScope::CurrentProject => anyhow::bail!(
            "project boundary refused path `{path}`; CLI and VS Code may access only {}. Open the other folder as the active project to work there",
            cwd.display(),
        ),
        PathScope::RegisteredProjects(_) => anyhow::bail!(
            "project boundary refused path `{path}`; Desktop may access only projects added to Oxide. Add the folder as a project before working in it"
        ),
        PathScope::Unrestricted => Ok(()),
    }
}

fn canonical_or_normalized(path: &Path) -> Result<PathBuf> {
    if let Ok(path) = std::fs::canonicalize(path) {
        return Ok(path);
    }

    // A write target may not exist yet. Canonicalize its closest existing
    // ancestor, then restore the missing suffix and normalize `..` components.
    let mut ancestor = path;
    let mut suffix = Vec::new();
    while !ancestor.exists() {
        let Some(name) = ancestor.file_name() else {
            break;
        };
        suffix.push(name.to_os_string());
        let Some(parent) = ancestor.parent() else {
            break;
        };
        ancestor = parent;
    }
    let mut resolved = std::fs::canonicalize(ancestor).unwrap_or_else(|_| normalize_path(ancestor));
    for part in suffix.iter().rev() {
        resolved.push(part);
    }
    Ok(normalize_path(&resolved))
}

fn normalize_path(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// A quoted cat heredoc used as a GitHub PR body is literal text, not a path
/// operand or executable script. Keep the command around it subject to checks.
fn shell_scope_source(command: &str) -> String {
    let mut source = command.as_bytes().to_vec();
    for (start, _) in command.match_indices("$(cat <<") {
        let prefix = &command[..start];
        if !prefix.ends_with("--body \"") {
            continue;
        }
        let segments = shell_segments(prefix);
        let words: Vec<_> = segments.last().unwrap_or(&"").split_whitespace().collect();
        let Some(index) = shell_command_index(&words) else {
            continue;
        };
        if shell_word_name(words[index]) != "gh"
            || words.get(index + 1) != Some(&"pr")
            || !matches!(words.get(index + 2), Some(&"create" | &"edit"))
        {
            continue;
        }
        let rest = &command[start + "$(cat <<".len()..];
        let Some((header, body)) = rest.split_once('\n') else {
            continue;
        };
        let header = header.trim_end();
        let Some(quote) = header.chars().next().filter(|ch| matches!(ch, '\'' | '"')) else {
            continue;
        };
        let Some(delimiter) = header
            .strip_prefix(quote)
            .and_then(|text| text.strip_suffix(quote))
        else {
            continue;
        };
        if delimiter.is_empty()
            || !delimiter
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
        {
            continue;
        }
        let body_start = command.len() - body.len();
        let mut offset = 0;
        for line in body.split_inclusive('\n') {
            if line.trim_end_matches(['\r', '\n']) == delimiter {
                if body[offset + line.len()..].trim_start().starts_with(")\"") {
                    for byte in &mut source[body_start..body_start + offset] {
                        if *byte != b'\n' && *byte != b'\r' {
                            *byte = b' ';
                        }
                    }
                }
                break;
            }
            offset += line.len();
        }
    }
    String::from_utf8(source).expect("masking literal data preserves UTF-8")
}

/// Shells cannot be made into a portable OS sandbox here, but deliberate
/// cross-project commands have concrete path operands. Refuse absolute/home
/// paths and parent traversal before starting the process. This catches the
/// `cd /sibling/repo` and absolute grep/read forms while leaving ordinary build
/// commands, URLs, and `/dev/null` redirection alone.
fn enforce_scoped_shell(cwd: &Path, command: &str, scope: PathScope<'_>) -> Result<()> {
    let source = shell_scope_source(command);
    let command = source.as_str();
    if command.contains("$HOME")
        || command.contains("${HOME}")
        || command.contains("$PWD/..")
        || command.contains("${PWD}/..")
    {
        anyhow::bail!(
            "project boundary refused a home-directory expansion; shell commands must stay inside the allowed project roots"
        );
    }
    for raw in command.split_whitespace() {
        let raw = raw.trim_matches(|ch: char| {
            matches!(
                ch,
                '\'' | '"' | '(' | ')' | '[' | ']' | '{' | '}' | ';' | ',' | '|' | '&'
            )
        });
        let value = raw
            .split_once('=')
            .map(|(_, value)| value)
            .unwrap_or(raw)
            .trim_matches(['\'', '"']);
        let value = value
            .trim_start_matches(|ch: char| ch.is_ascii_digit())
            .strip_prefix('>')
            .or_else(|| value.strip_prefix('<'))
            .unwrap_or(value);
        if value == "/dev/null" || value.contains("://") {
            continue;
        }
        if value.starts_with('~') {
            anyhow::bail!(
                "project boundary refused path `{value}`; shell commands must stay inside the allowed project roots"
            );
        }
        if Path::new(value).is_absolute()
            || value == ".."
            || value.starts_with("../")
            || value.starts_with("..\\")
        {
            ensure_path_scope(cwd, value, scope)?;
        }
    }
    // Also catch paths inside a quoted program argument, for example
    // `python -c "open('/sibling/file')"` or its Windows drive-path form.
    for path in embedded_absolute_paths(command) {
        if path == "/dev/null" || path.starts_with("//") {
            continue;
        }
        ensure_path_scope(cwd, path, scope)?;
    }
    Ok(())
}

/// Finds Unix-rooted and Windows drive-rooted paths embedded in a quoted
/// script or expression. `Path::is_absolute` only understands the host syntax,
/// so recognizing both shapes here also keeps this parser directly testable on
/// every platform.
fn embedded_absolute_paths(command: &str) -> Vec<&str> {
    let chars: Vec<(usize, char)> = command.char_indices().collect();
    let mut paths = Vec::new();
    for (position, (start, ch)) in chars.iter().enumerate() {
        let unix_rooted = *ch == '/';
        let windows_rooted = ch.is_ascii_alphabetic()
            && chars.get(position + 1).is_some_and(|(_, ch)| *ch == ':')
            && chars
                .get(position + 2)
                .is_some_and(|(_, ch)| matches!(ch, '/' | '\\'));
        if !unix_rooted && !windows_rooted {
            continue;
        }
        let previous = position.checked_sub(1).map(|index| chars[index].1);
        if previous.is_some_and(|ch| !matches!(ch, '\'' | '"' | '(' | '[' | '{' | '=' | ',')) {
            continue;
        }
        let end = chars
            .iter()
            .skip(position + 1)
            .find(|(_, ch)| {
                ch.is_whitespace()
                    || matches!(ch, '\'' | '"' | ')' | ']' | '}' | ',' | ';' | '|' | '&')
            })
            .map(|(index, _)| *index)
            .unwrap_or(command.len());
        let path = &command[*start..end];
        paths.push(path);
    }
    paths
}

fn read(cwd: &Path, args: &Value) -> Result<ToolOutput> {
    let path = required_string(args, "read", "path", r#"{"path":"src/main.rs"}"#)?;

    let full = resolve(cwd, path);
    if full.is_dir() {
        let mut entries = dir_entries(&full)?;
        let truncated = entries.len() > DEFAULT_LS_ENTRIES;
        entries.truncate(DEFAULT_LS_ENTRIES);
        let mut out = format!("{} is a directory. Entries:\n", full.display());
        out.push_str(&entries.join("\n"));
        if truncated {
            out.push_str("\n... [truncated]");
        }
        return Ok(ToolOutput::text(out));
    }
    if media::is_attachment_path(&full) {
        let part = media::load_attachment(&full)?;
        let kind = media::part_kind(&part).to_string();
        return Ok(ToolOutput::with_media(
            format!("attached {kind} {}", full.display()),
            vec![part],
        ));
    }

    // A present-but-unparseable value (some models emit `"offset": ".130"`)
    // must fail instead of silently restarting at line 1.
    let offset = strict_int_arg(args, "offset")?.unwrap_or(1).max(1);
    // Clamp to the read budget so the page (plus the `more lines` pointer the
    // caller appends) never exceeds what `output_limits` allows and gets the
    // continuation hint cut off by the central truncation. A model that asks
    // for a range (`"offset": [820, 850]`) gets that span rather than the
    // default page.
    let limit = match strict_int_arg(args, "limit")? {
        Some(limit) => limit,
        None => args
            .get("offset")
            .and_then(range_len)
            .unwrap_or(DEFAULT_READ_LINES),
    }
    .clamp(1, READ_MAX_LINES);

    let bytes = std::fs::read(&full).map_err(|err| media::read_error(&full, &err))?;
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

/// Reject increases in U+FFFD as likely encoding corruption while allowing
/// existing replacement characters to survive a rewrite.
const REPLACEMENT_CHARACTER: char = '\u{FFFD}';

/// How many U+FFFD characters `after` adds over `before`.
fn introduced_replacement_characters(before: &str, after: &str) -> usize {
    after
        .matches(REPLACEMENT_CHARACTER)
        .count()
        .saturating_sub(before.matches(REPLACEMENT_CHARACTER).count())
}

fn write(cwd: &Path, args: &Value) -> Result<ToolOutput> {
    let path = required_string(
        args,
        "write",
        "path",
        r#"{"path":"notes.txt","content":"..."}"#,
    )?;
    let content = required_string(
        args,
        "write",
        "content",
        r#"{"path":"notes.txt","content":"..."}"#,
    )?;

    let full = resolve(cwd, path);
    let previous = std::fs::read_to_string(&full).unwrap_or_default();
    let introduced = introduced_replacement_characters(&previous, content);
    if introduced > 0 {
        anyhow::bail!(
            "write refused: `content` adds {introduced} U+FFFD replacement character(s) and \
             increases the count in {path}. That character marks text that was truncated or decoded from \
             invalid UTF-8 and may indicate corruption; resend `content` with the real \
             characters. A file whose content is deliberately U+FFFD can be written with `bash`."
        );
    }
    if let Some(parent) = full.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
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
    /// Some models repeat the file on each replacement and omit the tool's
    /// top-level `path`. Keep it so `edit` can recover that call when every
    /// replacement names the same file.
    path: Option<String>,
}

/// Normalizes the many shapes models send for `edit` into a list of
/// replacements, mirroring Pi's `prepareArguments`: `edits` may be a JSON
/// string, a single `{oldText,newText}` object, or an array; legacy top-level
/// `oldText`/`newText` are folded into the list.
fn parse_edits(args: &Value) -> Result<Vec<Replacement>> {
    let mut raw: Vec<Replacement> = Vec::new();
    let mut invalid_string: Option<String> = None;
    match args.get("edits") {
        Some(Value::String(text)) => match parse_edits_string(text) {
            Some(parsed) => collect_edits(&parsed, &mut raw)?,
            None if args.get("newText").and_then(Value::as_str).is_some()
                && text
                    .trim_start()
                    .starts_with("<parameter name=\"oldText\">") =>
            {
                let old = text
                    .trim_start()
                    .strip_prefix("<parameter name=\"oldText\">")
                    .unwrap();
                raw.push(Replacement {
                    old: old.strip_suffix("</parameter>").unwrap_or(old).to_string(),
                    new: args["newText"].as_str().unwrap().to_string(),
                    path: None,
                });
            }
            None => {
                let mut snippet: String = text.chars().take(80).collect();
                if text.chars().count() > 80 {
                    snippet.push('…');
                }
                invalid_string = Some(snippet.replace(['\n', '\r', '\t'], " "));
            }
        },
        Some(value) => collect_edits(value, &mut raw)?,
        None => {}
    }
    if let (Some(old), Some(new)) = (
        args.get("oldText").and_then(Value::as_str),
        args.get("newText").and_then(Value::as_str),
    ) {
        raw.push(Replacement {
            old: old.to_string(),
            new: new.to_string(),
            path: None,
        });
    }
    if raw.is_empty() {
        if let Some(snippet) = invalid_string {
            anyhow::bail!(
                "edit received `edits` as a string that is not valid JSON. Pass an array like \
                 [{{\"oldText\":\"before\",\"newText\":\"after\"}}]. Received: {snippet:?}"
            );
        }
        anyhow::bail!(
            "edit requires at least one replacement in `edits`, for example \
             [{{\"oldText\":\"before\",\"newText\":\"after\"}}]"
        );
    }
    Ok(raw)
}

/// Parses a stringified `edits` value, tolerating the extra `}` some models
/// emit when they double-encode the array (`[{"oldText":"...","newText":
/// "..."}}]`). Strict parsing is tried first, so a well-formed string is
/// never rewritten.
fn parse_edits_string(text: &str) -> Option<Value> {
    if let Ok(value) = serde_json::from_str::<Value>(text) {
        return Some(value);
    }
    let mut candidate = text.trim().to_string();
    while candidate.ends_with("}}]") {
        candidate.truncate(candidate.len() - 2);
        candidate.push(']');
        if let Ok(value) = serde_json::from_str::<Value>(&candidate) {
            return Some(value);
        }
    }
    None
}

fn collect_edits(value: &Value, out: &mut Vec<Replacement>) -> Result<()> {
    collect_edits_with_path(value, out, None, "edits")
}

fn edit_path_value(value: &Value) -> Option<&str> {
    ["path", "file_path", "filePath"]
        .into_iter()
        .find_map(|key| value.get(key).and_then(Value::as_str))
}

fn collect_edits_with_path(
    value: &Value,
    out: &mut Vec<Replacement>,
    inherited: Option<&str>,
    location: &str,
) -> Result<()> {
    match value {
        Value::Array(items) => {
            for (index, item) in items.iter().enumerate() {
                collect_edits_with_path(item, out, inherited, &format!("{location}[{index}]"))?;
            }
        }
        Value::Object(_) => {
            let path = edit_path_value(value).or(inherited);
            let old = value.get("oldText");
            let new = value.get("newText");
            if old.is_some() || new.is_some() {
                let (Some(old), Some(new)) =
                    (old.and_then(Value::as_str), new.and_then(Value::as_str))
                else {
                    anyhow::bail!("{location} requires string `oldText` and `newText` fields");
                };
                out.push(Replacement {
                    old: old.to_string(),
                    new: new.to_string(),
                    path: path.map(str::to_string),
                });
            } else if let Some(edits) = value.get("edits") {
                collect_edits_with_path(edits, out, path, &format!("{location}.edits"))?;
            } else {
                anyhow::bail!(
                    "{location} must be a replacement with string `oldText` and `newText` fields"
                );
            }
        }
        _ => anyhow::bail!(
            "{location} must be a replacement object or an array of replacement objects"
        ),
    }
    Ok(())
}

/// One line of a file with its byte range, used by the whitespace-insensitive
/// matcher and the "closest region" hint.
struct SourceLine<'a> {
    start: usize,
    /// End of the line including its trailing newline, when present.
    end: usize,
    /// The line without its trailing newline.
    text: &'a str,
}

fn source_lines(text: &str) -> Vec<SourceLine<'_>> {
    let mut lines = Vec::new();
    let mut start = 0;
    for segment in text.split_inclusive('\n') {
        let content = segment.strip_suffix('\n').unwrap_or(segment);
        let content = content.strip_suffix('\r').unwrap_or(content);
        lines.push(SourceLine {
            start,
            end: start + segment.len(),
            text: content,
        });
        start += segment.len();
    }
    lines
}

/// A unique region of a file to replace.
struct Located {
    start: usize,
    end: usize,
}

/// The comparison key for one line of an `oldText` block: trailing whitespace
/// and the typographic characters a model substitutes when it retypes a block
/// are invisible to the intent, so two lines that differ only in them are the
/// same line. The typographic set matches Pi's `normalizeForFuzzyMatch`
/// (`coding-agent/src/core/tools/edit-diff.ts`). NFKC folding is deliberately
/// left out: it is lossy in ways a file rewrite should not be.
fn match_key(text: &str) -> String {
    text.trim_end()
        .chars()
        .map(|ch| match ch {
            '\u{2018}' | '\u{2019}' | '\u{201a}' | '\u{201b}' => '\'',
            '\u{201c}' | '\u{201d}' | '\u{201e}' | '\u{201f}' => '"',
            '\u{2010}' | '\u{2011}' | '\u{2012}' | '\u{2013}' | '\u{2014}' | '\u{2015}'
            | '\u{2212}' => '-',
            '\u{00a0}' | '\u{2002}'..='\u{200a}' | '\u{202f}' | '\u{205f}' | '\u{3000}' => ' ',
            other => other,
        })
        .collect()
}

/// Why an `oldText` could not be applied: it is absent (with the closest region
/// when one was found), or it matched on several lines.
enum MatchFailure {
    /// The closest region, as `(first line, last line, raw text)`. The text is
    /// rendered without `read`'s numbering so copying it back applies exactly.
    Missing(Option<(usize, usize, String)>),
    /// 1-based line numbers of every occurrence, so the retry can add context.
    Ambiguous(Vec<usize>),
}

/// Locates `old` in `base`: byte-exact first, then a whole-line match that
/// ignores trailing whitespace (which is invisible to the model and often the
/// only difference). Either must be unique, so the tolerance repairs a trailing
/// space without letting a fuzzy hit rewrite the wrong region. Leading
/// indentation is deliberately not ignored: a replacement that silently lost
/// its indentation would corrupt the file's formatting.
fn locate(base: &str, old: &str) -> Result<Located, MatchFailure> {
    let exact = overlapping_match_indices(base, old);
    match exact.len() {
        1 => {
            let start = exact[0];
            return Ok(Located {
                start,
                end: start + old.len(),
            });
        }
        n if n > 1 => {
            let base_lines = source_lines(base);
            let lines = exact
                .iter()
                .map(|&offset| {
                    base_lines
                        .iter()
                        .take_while(|line| line.start <= offset)
                        .count()
                })
                .collect();
            return Err(MatchFailure::Ambiguous(lines));
        }
        _ => {}
    }

    let base_lines = source_lines(base);
    let old_lines = source_lines(old);
    if old_lines.is_empty() || old_lines.len() > base_lines.len() {
        return Err(MatchFailure::Missing(closest_region(
            &base_lines,
            &old_lines,
        )));
    }
    let width = old_lines.len();
    let range_at = |i: usize| {
        let last = &base_lines[i + width - 1];
        // Keep the file's trailing newline unless the matched text replaced it.
        let end = if old.ends_with('\n') {
            last.end
        } else {
            last.start + last.text.len()
        };
        (
            i + 1,
            Located {
                start: base_lines[i].start,
                end,
            },
        )
    };

    // The fast pass: trailing whitespace is invisible to the model and is often
    // the only difference, so it is ignored before anything more expensive.
    let mut hits: Vec<(usize, Located)> = (0..=base_lines.len() - width)
        .filter(|&i| {
            (0..width).all(|k| base_lines[i + k].text.trim_end() == old_lines[k].text.trim_end())
        })
        .map(range_at)
        .collect();

    // A block retyped with typographic characters — smart quotes, en/em dashes,
    // non-breaking spaces — reads as the same block, so those still land. This
    // runs only when the exact and whitespace passes found nothing, and the
    // replacement still writes the file's own line range, so a loose match can
    // never change text the model did not name.
    if hits.is_empty() {
        let base_keys: Vec<String> = base_lines.iter().map(|line| match_key(line.text)).collect();
        let old_keys: Vec<String> = old_lines.iter().map(|line| match_key(line.text)).collect();
        hits = (0..=base_lines.len() - width)
            .filter(|&i| (0..width).all(|k| base_keys[i + k] == old_keys[k]))
            .map(range_at)
            .collect();
    }
    match hits.len() {
        1 => Ok(hits.remove(0).1),
        n if n > 1 => Err(MatchFailure::Ambiguous(
            hits.into_iter().map(|(line, _)| line).collect(),
        )),
        _ => Err(MatchFailure::Missing(closest_region(
            &base_lines,
            &old_lines,
        ))),
    }
}

/// Every byte offset at which `needle` starts, including overlapping matches.
/// `str::match_indices` advances by the whole match and would call `aa` unique
/// in `aaa`, even though either pair of characters could be the intended edit.
fn overlapping_match_indices(haystack: &str, needle: &str) -> Vec<usize> {
    if needle.is_empty() {
        return Vec::new();
    }
    let mut offsets = Vec::new();
    let mut from = 0;
    while let Some(relative) = haystack[from..].find(needle) {
        let start = from + relative;
        offsets.push(start);
        let next = haystack[start..]
            .chars()
            .next()
            .expect("a non-empty match starts on a character boundary")
            .len_utf8();
        from = start + next;
    }
    offsets
}

/// The file region most similar to a failed `oldText`, as its 1-based line
/// range and raw text, so the model can copy the exact text on a retry. The
/// text is deliberately not numbered: a copied `N|` prefix on a single line
/// would fail the same way again.
fn closest_region(
    base_lines: &[SourceLine<'_>],
    old_lines: &[SourceLine<'_>],
) -> Option<(usize, usize, String)> {
    if old_lines.is_empty() || base_lines.is_empty() {
        return None;
    }
    let width = old_lines.len().min(base_lines.len());
    let base_keys: Vec<String> = base_lines
        .iter()
        .map(|line| match_key(line.text.trim()))
        .collect();
    let old_keys: Vec<String> = old_lines
        .iter()
        .map(|line| match_key(line.text.trim()))
        .collect();
    // A line counts as shared when it is equal apart from trailing whitespace,
    // or when one side begins the other for a run long enough to mean it: a
    // block cut off mid-line — the newline written where the file holds `\n` —
    // would otherwise score nothing and leave the failure with no region to
    // name at all.
    let shared = |base: &str, old: &str| {
        // Blank lines and punctuation-only delimiters do not locate a block:
        // a Chinese paragraph must not be "closest" to an English heading
        // just because both have a blank line under them.
        if !old.chars().any(char::is_alphanumeric) {
            return false;
        }
        let width = base.len().min(old.len());
        base == old || (width >= 8 && (base.starts_with(old) || old.starts_with(base)))
    };
    let mut best: Option<(usize, usize)> = None;
    for start in 0..=base_lines.len() - width {
        let score = (0..width)
            .filter(|&k| shared(&base_keys[start + k], &old_keys[k]))
            .count();
        let better = match best {
            Some((best_score, _)) => score > best_score,
            None => true,
        };
        if score > 0 && better {
            best = Some((score, start));
        }
    }
    let (_, start) = best?;
    let end = (start + old_lines.len()).min(base_lines.len());
    let region = base_lines[start..end]
        .iter()
        .map(|line| line.text)
        .collect::<Vec<_>>()
        .join("\n");
    Some((start + 1, end, region))
}

/// The first line where a failed `oldText` differs from the region the file
/// holds, as its 1-based position and both renderings with control characters
/// escaped. Two blocks can look identical and still differ — a `\n` typed into
/// a string literal against the newline the model wrote instead, a tab against
/// spaces — and a reader comparing them in a transcript cannot see either.
fn first_differing_line(old: &str, region: &str) -> Option<(usize, String, String)> {
    let mut old_lines = old.split('\n');
    let mut region_lines = region.split('\n');
    let mut position = 0;
    loop {
        position += 1;
        let old_line = old_lines.next();
        let base_line = region_lines.next();
        match (old_line, base_line) {
            (None, None) => return None,
            (Some(old), Some(base)) if match_key(old) == match_key(base) => continue,
            _ => {
                return Some((
                    position,
                    base_line.map_or_else(|| "nothing".to_string(), |line| format!("{line:?}")),
                    old_line.map_or_else(|| "nothing".to_string(), |line| format!("{line:?}")),
                ))
            }
        }
    }
}

/// Strips the `N|` / `N+|` prefixes `read` adds, so a block copied straight out
/// of a numbered listing still matches. A single numbered line is accepted too
/// (`read` numbers its first line as `1|`). Continuations repeat their physical
/// line's number and join without a newline; new lines must increase it.
fn strip_line_prefixes(text: &str) -> Option<String> {
    let mut previous = None;
    let mut stripped = String::new();
    let mut found = false;
    for (index, line) in text.split('\n').enumerate() {
        if line.trim().is_empty() {
            if index > 0 {
                stripped.push('\n');
            }
            stripped.push_str(line);
            continue;
        }
        let (number, continuation, rest) = line_prefix(line)?;
        if let Some(previous) = previous {
            if (continuation && number != previous) || (!continuation && number <= previous) {
                return None;
            }
        }
        if index > 0 && !continuation {
            stripped.push('\n');
        }
        previous = Some(number);
        found = true;
        stripped.push_str(rest);
    }
    found.then_some(stripped)
}

/// Splits a `12|text` or `12+|text` read line into its number and text.
fn line_prefix(line: &str) -> Option<(u64, bool, &str)> {
    let digits = line
        .find(|ch: char| !ch.is_ascii_digit())
        .unwrap_or(line.len());
    if digits == 0 {
        return None;
    }
    let number = line[..digits].parse().ok()?;
    let trailing = &line[digits..];
    let continuation = trailing.starts_with('+');
    let rest = trailing.strip_prefix('+').unwrap_or(trailing);
    Some((number, continuation, rest.strip_prefix('|')?))
}

/// Removes `N|` / `N+|` prefixes from every line, used for the replacement text
/// once the old text was recognized as a copied numbered block.
fn strip_all_line_prefixes(text: &str) -> String {
    let mut stripped = String::new();
    let mut previous = None;
    for (index, line) in text.split('\n').enumerate() {
        let prefix = line_prefix(line);
        let joins = prefix
            .is_some_and(|(number, continuation, _)| continuation && previous == Some(number));
        if index > 0 && !joins {
            stripped.push('\n');
        }
        stripped.push_str(prefix.map(|(_, _, rest)| rest).unwrap_or(line));
        previous = prefix.map(|(number, _, _)| number);
    }
    stripped
}

/// Applies targeted text replacements. Each `oldText` is matched against the
/// original file (never incrementally) and must be unique; a non-unique match
/// is rejected instead of guessed. Beyond a byte-exact match, a line-wise
/// comparison ignores trailing whitespace, and a block copied from a numbered
/// `read` listing has its `N|` prefixes removed, so the common "cosmetically
/// different" edit still lands without a round trip.
fn edit(cwd: &Path, args: &Value) -> Result<ToolOutput> {
    // Pi accepts `file_path` beside `path` (`coding-agent/src/core/tools/edit.ts`),
    // and some models spell it that way; either names the file to change.
    let top_level_path = edit_path_value(args);
    let edits = match parse_edits(args) {
        Ok(edits) => edits,
        Err(_) if top_level_path.is_none() => anyhow::bail!(
            "edit requires a `path` string, for example {{\"path\":\"src/main.rs\",\
             \"edits\":[{{\"oldText\":\"...\",\"newText\":\"...\"}}]}}{}",
            received_keys(args)
        ),
        Err(error) => return Err(error),
    };
    let nested_paths: BTreeSet<&str> = edits
        .iter()
        .filter_map(|replacement| replacement.path.as_deref())
        .collect();
    let path = match (top_level_path, nested_paths.len()) {
        (Some(path), 0) => path.to_string(),
        (Some(path), _) if nested_paths.iter().all(|nested| *nested == path) => path.to_string(),
        (Some(path), _) => anyhow::bail!(
            "edit received conflicting file paths: top-level path {path:?}, nested paths {}. Each \
             edit call changes one file; split changes for different files into separate calls.",
            nested_paths
                .iter()
                .map(|path| format!("{path:?}"))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        (None, 1) => nested_paths
            .iter()
            .next()
            .expect("one nested path")
            .to_string(),
        (None, count) if count > 1 => anyhow::bail!(
            "edit received replacements for multiple files ({}). Each edit call changes one file; \
             split them into separate calls.",
            nested_paths
                .iter()
                .map(|path| format!("{path:?}"))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        (None, _) => anyhow::bail!(
            "edit requires a `path` string, for example {{\"path\":\"src/main.rs\",\
             \"edits\":[{{\"oldText\":\"...\",\"newText\":\"...\"}}]}}{}",
            received_keys(args)
        ),
    };
    let full = resolve(cwd, &path);
    let raw = std::fs::read_to_string(&full)
        .with_context(|| format!("reading {} (use write to create new files)", full.display()))?;

    let (bom, content) = split_bom(&raw);
    let mut base = content.to_string();
    let mut failures: Vec<String> = Vec::new();
    // Every edit is matched against the original file, so a later edit cannot
    // be broken by an earlier one rewriting nearby text (or introducing a
    // second copy of its target). The located ranges are applied at the end.
    let mut located: Vec<(usize, Located, String)> = Vec::new();

    for (index, replacement) in edits.iter().enumerate() {
        let raw_old = replacement.old.replace("\r\n", "\n");
        // Only a truly empty target is rejected; a whitespace-only one is
        // allowed as long as it matches uniquely, as the exact matcher decides.
        if raw_old.is_empty() {
            failures.push(format!("edits[{index}].oldText must not be empty"));
            continue;
        }
        // A literal `1|text` can genuinely exist in a file. Try the text as
        // supplied before treating it as a numbered `read` listing; only a
        // missing literal target enables the prefix-stripping fallback.
        let mut searched = raw_old;
        let (found, copied_listing) = match locate(&base, &searched) {
            ok @ Ok(_) => (ok, false),
            ambiguous @ Err(MatchFailure::Ambiguous(_)) => (ambiguous, false),
            missing @ Err(MatchFailure::Missing(_)) => match strip_line_prefixes(&searched) {
                Some(stripped) if stripped != searched => {
                    let retried = locate(&base, &stripped);
                    searched = stripped;
                    (retried, true)
                }
                _ => (missing, false),
            },
        };
        let failures_before = failures.len();
        match found {
            Ok(range) => {
                let mut new = replacement.new.replace("\r\n", "\n");
                if copied_listing {
                    new = strip_all_line_prefixes(&new);
                }
                new = replacement_with_matching_line_endings(
                    &new,
                    &base[range.start..range.end],
                    content,
                );
                if base[range.start..range.end] != new {
                    let introduced =
                        introduced_replacement_characters(&base[range.start..range.end], &new);
                    if introduced > 0 {
                        failures.push(format!(
                            "edits[{index}].newText adds {introduced} U+FFFD replacement \
                             character(s) beyond the count in the text it replaces. That character marks \
                             text that was truncated or decoded from invalid UTF-8 and may indicate \
                             corruption; re-read {path} and resend the block with \
                             the real characters."
                        ));
                        continue;
                    }
                    located.push((index, range, new));
                }
            }
            Err(MatchFailure::Ambiguous(lines)) => failures.push(format!(
                "edits[{index}].oldText matched {} times in {path} (lines {}); include more \
                 surrounding text to make it unique",
                lines.len(),
                lines
                    .iter()
                    .map(usize::to_string)
                    .collect::<Vec<_>>()
                    .join(", ")
            )),
            Err(MatchFailure::Missing(Some((first, last, region)))) => {
                let mut message = format!(
                    "edits[{index}].oldText did not match anything in {path}. The closest text \
                     (lines {first}-{last}) is:\n{region}"
                );
                // A block that reads identically in a transcript can still carry
                // a newline where the file holds `\n`, or a tab where it holds
                // spaces. Naming the first line that differs, with control
                // characters escaped, is what makes that visible.
                if let Some((line, in_file, in_block)) = first_differing_line(&searched, &region) {
                    message.push_str(&format!(
                        "\nYour line {line} differs: the file holds {in_file} where the block \
                         holds {in_block}"
                    ));
                }
                message.push_str("\nRead this region from the target file before correcting oldText; the closest text is a hint, not an automatic replacement.");
                failures.push(message);
            }
            Err(MatchFailure::Missing(None)) => failures.push(format!(
                "edits[{index}].oldText did not match anything in {path}. No similar text was \
                 found. Check that this replacement belongs to this file; use separate edit \
                 calls for different files. Read the target file again rather than guessing."
            )),
        }
        if failures.len() > failures_before && replacement.old == replacement.new {
            failures.last_mut().expect("a failure was added").push_str(
                "\nThis replacement is a no-op (oldText equals newText); remove it from the batch.",
            );
        }
    }

    // Ranges are matched against the same original, so two of them can overlap
    // or duplicate. Applying both would be ambiguous, so the whole batch is
    // rejected and the pair is named.
    located.sort_by_key(|(_, range, _)| (range.start, range.end));
    for pair in located.windows(2) {
        let (first_index, first, _) = &pair[0];
        let (second_index, second, _) = &pair[1];
        if second.start < first.end {
            failures.push(format!(
                "edits[{first_index}].oldText and edits[{second_index}].oldText overlap in {path}; \
                 merge them into one edit"
            ));
        }
    }

    if !failures.is_empty() {
        anyhow::bail!(
            "No changes were written to {path}; the entire edit batch was rejected.\n\n{}\n\n\
             Read the current target with `read` {}. Correct the failed replacements, omit \
             no-ops, merge overlaps, and resend the entire batch; even matching replacements \
             were not applied.",
            failures.join("\n\n"),
            json!({ "path": path }),
        );
    }

    // Apply from the end so the ranges computed against the original stay
    // valid: each replacement shifts only text after the ranges already done.
    for (_, range, new) in located.iter().rev() {
        base.replace_range(range.start..range.end, new);
    }

    let final_content = format!("{bom}{base}");
    if final_content != raw {
        std::fs::write(&full, &final_content)
            .with_context(|| format!("writing {}", full.display()))?;
    }

    // Pi's own wording (`coding-agent/src/core/tools/edit.ts`). The redundancy a
    // reader sees is the renderer's to drop, not this result's: Pi keeps the
    // sentence here and has `renderers/edit.ts::formatEditResult` return nothing
    // on success, so a display never restates the path its header already names.
    let output = ToolOutput::text(format!(
        "Successfully replaced {} block(s) in {path}.",
        located.len()
    ));
    match diff::preview(
        &raw.replace("\r\n", "\n"),
        &final_content.replace("\r\n", "\n"),
    ) {
        Some(diff) => Ok(output.with_diff(&path, diff)),
        None => Ok(output),
    }
}

/// Adapts newly inserted lines to the target region without normalizing line
/// endings elsewhere in the file. If the target has no newline of its own, a
/// consistently-CRLF file supplies the default; mixed files retain their
/// untouched mixture byte for byte.
fn replacement_with_matching_line_endings(new: &str, target: &str, file: &str) -> String {
    let target_has_newline = target.contains('\n');
    let use_crlf = if target_has_newline {
        target.contains("\r\n") && !has_lone_lf(target)
    } else {
        file.contains("\r\n") && !has_lone_lf(file)
    };
    if use_crlf {
        new.replace('\n', "\r\n")
    } else {
        new.to_string()
    }
}

fn has_lone_lf(text: &str) -> bool {
    let bytes = text.as_bytes();
    bytes
        .iter()
        .enumerate()
        .any(|(index, byte)| *byte == b'\n' && (index == 0 || bytes[index - 1] != b'\r'))
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

fn ls(cwd: &Path, args: &Value) -> Result<String> {
    let path = args.get("path").and_then(Value::as_str).unwrap_or(".");
    let limit = int_arg(args, "limit").unwrap_or(DEFAULT_LS_ENTRIES).max(1);
    let full = resolve(cwd, path);

    let metadata =
        std::fs::metadata(&full).with_context(|| format!("listing {}", full.display()))?;
    if metadata.is_file() {
        return Ok(format!(
            "{} (file, {} bytes; use `read` to inspect its contents)",
            full.display(),
            metadata.len()
        ));
    }
    let mut entries = dir_entries(&full)?;
    let truncated = entries.len() > limit;
    entries.truncate(limit);
    let mut out = entries.join("\n");
    if truncated {
        out.push_str("\n... [truncated]");
    }
    Ok(out)
}

/// Default result counts, Pi's own (`tools/{grep,find,ls}.js`); each answer is
/// still bounded by the shared byte and line budget.
const DEFAULT_GREP_MATCHES: usize = 100;
const DEFAULT_FIND_MATCHES: usize = 1_000;
const DEFAULT_LS_ENTRIES: usize = 500;

fn find(cwd: &Path, args: &Value) -> Result<String> {
    let pattern = required_string(args, "find", "pattern", r#"{"pattern":"**/*.rs"}"#)?;
    let base = args.get("path").and_then(Value::as_str).unwrap_or(".");
    let limit = int_arg(args, "limit")
        .unwrap_or(DEFAULT_FIND_MATCHES)
        .max(1);
    let root = resolve(cwd, base);

    let metadata = std::fs::metadata(&root)
        .with_context(|| format!("find could not access search root {}", root.display()))?;
    if !metadata.is_dir() {
        anyhow::bail!("find search root is not a directory: {}", root.display());
    }

    // Use the same native fd fast path as Pi when it is installed, then
    // ripgrep's parallel file lister. The built-in walker stays available on a
    // minimal system with neither binary.
    if let Some(output) = fast_find(&root, pattern, limit) {
        return Ok(output);
    }

    let mut matches = Vec::new();
    walk(&root, &mut |path| {
        let rel = path.strip_prefix(&root).unwrap_or(path);
        let rel = rel.to_string_lossy().replace('\\', "/");
        if find_pattern_matches(pattern, &rel) {
            matches.push(rel);
        }
        matches.len() <= limit
    });
    matches.sort();
    if matches.is_empty() {
        return Ok(NO_FILES_FOUND.to_string());
    }
    let truncated = matches.len() > limit;
    matches.truncate(limit);
    let mut out = matches.join("\n");
    if truncated {
        out.push_str("\n... [truncated]");
    }
    Ok(out)
}

fn fast_find(root: &Path, pattern: &str, limit: usize) -> Option<String> {
    if !root.is_dir() {
        return None;
    }
    // fd matches path-containing globs against an absolute/./-prefixed path,
    // which changes root-relative semantics. rg handles those patterns exactly;
    // fd remains the faster basename-pattern path.
    if !pattern.contains('/') && !pattern.contains('\\') {
        if let Some(fd) = command_path("fd").or_else(|| command_path("fdfind")) {
            if let Some(output) = fd_find(&fd, root, pattern, limit) {
                return Some(output);
            }
        }
    }
    rg_find(root, pattern, limit)
}

/// Lists files with fd, matching Pi's preferred implementation. Debian-based
/// distributions package the same binary as `fdfind`, accepted above.
fn fd_find(fd: &Path, root: &Path, pattern: &str, limit: usize) -> Option<String> {
    if pattern.contains('/') || pattern.contains('\\') {
        return None;
    }
    let mut args = vec![
        "--glob".to_string(),
        "--color=never".to_string(),
        "--hidden".to_string(),
        "--no-require-git".to_string(),
        "--type".to_string(),
        "f".to_string(),
        "--max-results".to_string(),
        limit.saturating_add(1).to_string(),
    ];
    for ignored in [".git", "node_modules", "target", ".venv"] {
        args.push("--exclude".to_string());
        args.push(ignored.to_string());
    }
    args.extend(["--".to_string(), pattern.to_string(), ".".to_string()]);
    collect_find_child(
        std::process::Command::new(fd).current_dir(root).args(args),
        limit,
    )
}

/// Lists files through ripgrep when fd is absent. `Command::output` is
/// intentionally avoided: a broad pattern in a large checkout must not collect
/// every path before the model can see the first page.
fn rg_find(root: &Path, pattern: &str, limit: usize) -> Option<String> {
    let rg = command_path("rg")?;
    collect_find_child(
        std::process::Command::new(rg).current_dir(root).args([
            "--files",
            "--hidden",
            "--no-require-git",
            "--glob",
            "!.git",
            "--glob",
            "!node_modules",
            "--glob",
            "!target",
            "--glob",
            "!.venv",
            "--glob",
            pattern,
        ]),
        limit,
    )
}

/// Collects at most one result beyond the requested limit, then stops the
/// native walker. The extra result is enough to report truncation accurately.
fn collect_find_child(command: &mut std::process::Command, limit: usize) -> Option<String> {
    let mut child = command
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let stdout = child.stdout.take()?;
    let mut matches = Vec::with_capacity(limit.saturating_add(1));
    let mut kill_requested = false;
    let mut read_failed = false;
    for line in BufReader::new(stdout).lines() {
        let line = match line {
            Ok(line) => line,
            Err(_) => {
                read_failed = true;
                break;
            }
        };
        let line = line
            .strip_prefix("./")
            .or_else(|| line.strip_prefix(".\\"))
            .unwrap_or(&line)
            .replace('\\', "/");
        matches.push(line);
        if matches.len() > limit {
            kill_requested = child.kill().is_ok();
            break;
        }
    }
    let status = child.wait().ok()?;
    let killed_at_limit = child_killed_at_limit(&status, kill_requested);
    // Never return partial output from a native tool that failed by itself.
    // That would look correct while silently omitting results. Let the caller
    // try the next implementation instead. A process Oxide deliberately killed
    // after the limit is the sole non-success status we accept.
    if read_failed || (!status.success() && !killed_at_limit) {
        return None;
    }
    if matches.is_empty() {
        return Some(NO_FILES_FOUND.to_string());
    }
    let truncated = matches.len() > limit;
    matches.truncate(limit);
    let mut out = matches.join("\n");
    if truncated {
        out.push_str("\n... [truncated]");
    }
    Some(out)
}

/// fd (and Pi's `find`) treats a pattern without a path separator as a
/// basename pattern at every depth. Keep the Rust fallback consistent with
/// that behavior; path-containing patterns still match from the search root.
fn find_pattern_matches(pattern: &str, path: &str) -> bool {
    if pattern.contains('/') || pattern.contains('\\') {
        glob_match(&pattern.replace('\\', "/"), path)
    } else {
        glob_match(pattern, path.rsplit('/').next().unwrap_or(path))
    }
}

fn grep(cwd: &Path, args: &Value) -> Result<String> {
    let pattern = required_string(args, "grep", "pattern", r#"{"pattern":"ToolOutput"}"#)?;
    if pattern.is_empty() {
        anyhow::bail!("grep requires a non-empty `pattern` string");
    }
    let base = args.get("path").and_then(Value::as_str).unwrap_or(".");
    // Pi names these `glob`/`ignoreCase`; the legacy names are still accepted.
    let include = args
        .get("glob")
        .and_then(Value::as_str)
        .or_else(|| args.get("include").and_then(Value::as_str));
    let ignore_case = bool_arg(args, "ignoreCase", "ignore_case");
    let literal = opt_bool_arg(args, "literal").unwrap_or(false);
    // `regex`/`use_regex` are the previous spelling. A call that still sends
    // `regex: false` keeps its old literal meaning; a call that sends neither
    // gets Pi's default, a regular expression.
    let mut use_regex = if literal {
        false
    } else {
        opt_bool_arg(args, "regex")
            .or_else(|| opt_bool_arg(args, "use_regex"))
            .unwrap_or(true)
    };
    let mut note = String::new();
    if use_regex {
        if let Err(err) = regex::RegexBuilder::new(pattern)
            .case_insensitive(ignore_case)
            .build()
        {
            // A pattern that is not valid regex syntax is almost always a
            // literal string copied from source (a line with parentheses or
            // brackets). Search it literally and say so instead of failing the
            // call and spending a whole turn on the retry.
            use_regex = false;
            note = format!(
                "[note: `{pattern}` is not a valid regular expression ({err}); searched it as literal text instead]\n"
            );
        }
    }
    let context = int_arg(args, "context").unwrap_or(0);
    let limit = int_arg(args, "limit")
        .unwrap_or(DEFAULT_GREP_MATCHES)
        .max(1);
    let root = resolve(cwd, base);

    std::fs::metadata(&root)
        .with_context(|| format!("grep could not access search root {}", root.display()))?;

    // `rg` applies the same search with far less I/O, so prefer it and fall
    // back to the dependency-free walker when it is unavailable.
    let output = if let Some(output) = rg_grep(
        &root,
        pattern,
        include,
        ignore_case,
        context,
        limit,
        use_regex,
    ) {
        output
    } else {
        let matcher = GrepMatcher::new(pattern, use_regex, ignore_case)?;
        rust_grep(&root, &matcher, include, context, limit)?
    };
    Ok(format!("{note}{output}"))
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
                .map_err(|err| {
                    anyhow::anyhow!(
                        "grep could not compile `pattern` as a regular expression: {err}. \
                         Fix the expression or set `literal` to true for a plain-text search"
                    )
                })?;
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
    let rg = command_path("rg")?;
    let (dir, target) = if root.is_dir() {
        (root.to_path_buf(), ".".to_string())
    } else {
        let parent = root.parent().unwrap_or(root).to_path_buf();
        let name = root.file_name()?.to_string_lossy().to_string();
        (parent, name)
    };
    let mut command = std::process::Command::new(rg);
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
    command.stdout(Stdio::piped()).stderr(Stdio::null());
    let mut child = command.spawn().ok()?;
    let stdout = child.stdout.take()?;
    let mut hits: Vec<String> = Vec::new();
    let mut kill_requested = false;
    let mut read_failed = false;
    for line in BufReader::new(stdout).lines() {
        let line = match line {
            Ok(line) => line,
            Err(_) => {
                read_failed = true;
                break;
            }
        };
        let Ok(event) = serde_json::from_str::<Value>(&line) else {
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
        if hits.len() > limit {
            kill_requested = child.kill().is_ok();
            break;
        }
    }
    let status = child.wait().ok()?;
    let killed_at_limit = child_killed_at_limit(&status, kill_requested);
    // rg exits 1 for an ordinary no-match result. Every other unsolicited
    // failure falls back rather than returning plausible-looking partial data.
    let no_matches = status.code() == Some(1) && hits.is_empty();
    if read_failed || (!status.success() && !no_matches && !killed_at_limit) {
        return None;
    }
    Some(finish_hits(hits, limit))
}

/// Confirms that a non-success status came from Oxide's limit stop rather than
/// an accelerator that had already failed after filling its stdout pipe.
fn child_killed_at_limit(status: &std::process::ExitStatus, requested: bool) -> bool {
    if !requested {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        status.signal() == Some(libc::SIGKILL)
    }
    #[cfg(windows)]
    {
        // std::process::Child::kill uses TerminateProcess with exit code 1.
        status.code() == Some(1)
    }
    #[cfg(not(any(unix, windows)))]
    {
        false
    }
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
    let mut collect = |path: &Path| {
        if let Some(include) = include {
            if let Some(name) = path.file_name().and_then(|name| name.to_str()) {
                if !glob_match(include, name) {
                    return true;
                }
            }
        }
        candidates.push(path.to_path_buf());
        true
    };
    let display_root = if root.is_file() {
        collect(root);
        root.parent().unwrap_or(root)
    } else {
        walk(root, &mut collect);
        root
    };
    let hits = scan_files(display_root, &candidates, matcher, context, limit);
    Ok(finish_hits(hits, limit))
}

/// Truncates to `limit` hits and joins them, reporting truncation.
fn finish_hits(mut hits: Vec<String>, limit: usize) -> String {
    if hits.is_empty() {
        return NO_MATCHES.to_string();
    }
    let truncated = hits.len() > limit;
    hits.truncate(limit);
    let mut out = hits.join("\n");
    if truncated {
        out.push_str("\n... [truncated]");
    }
    out
}

/// Caches a resolved binary per `(PATH, name)` pair. Tool-heavy turns call this
/// for every `find`, `grep`, and forge URL; successful lookups avoid repeatedly
/// splitting and probing PATH, while misses remain visible to a later install.
fn command_path(name: &str) -> Option<PathBuf> {
    // Use the same enriched PATH as `bash`: a desktop process may not inherit
    // the terminal's package-manager directories, and treating an installed
    // `gh`, `rg`, or `fd` as absent sends the agent down a slower fallback.
    let path = bash_path().or_else(|| std::env::var_os("PATH"))?;
    type CommandCache = HashMap<(std::ffi::OsString, String), PathBuf>;
    static CACHE: OnceLock<std::sync::Mutex<CommandCache>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| std::sync::Mutex::new(HashMap::new()));
    let key = (path.clone(), name.to_string());
    if let Ok(cache) = cache.lock() {
        if let Some(found) = cache.get(&key) {
            return Some(found.clone());
        }
    }
    let found = std::env::split_paths(&path).find_map(|dir| {
        [dir.join(name), dir.join(format!("{name}.exe"))]
            .into_iter()
            .find(|candidate| is_executable_file(candidate))
    });
    // Do not cache misses: a package manager or an Oxide-managed installer may
    // add the binary to an existing PATH directory while this process lives.
    if let (Some(found), Ok(mut cache)) = (&found, cache.lock()) {
        cache.insert(key, found.clone());
    }
    found
}

fn is_executable_file(path: &Path) -> bool {
    let Ok(metadata) = path.metadata() else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

fn command_exists(name: &str) -> bool {
    command_path(name).is_some()
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

/// Reads an integer argument, accepting a JSON number, a numeric string (some
/// models stringify numbers), or an array of either (some models pass a line
/// range like `[820, 850]`, where the first element is the offset).
fn int_arg(args: &Value, key: &str) -> Option<usize> {
    int_value(args.get(key)?)
}

fn int_value(value: &Value) -> Option<usize> {
    match value {
        Value::Number(number) => number.as_u64().map(|value| value as usize),
        Value::String(text) => text.trim().parse().ok().or_else(|| {
            let parsed: Value = serde_json::from_str(text).ok()?;
            parsed.as_array()?.iter().find_map(int_value)
        }),
        Value::Array(items) => items.iter().find_map(int_value),
        _ => None,
    }
}

/// The number of lines covered by an array offset like `[820, 850]`, so a
/// model that asks for a range gets that span instead of the default page.
fn range_len(value: &Value) -> Option<usize> {
    if let Value::String(text) = value {
        let parsed: Value = serde_json::from_str(text).ok()?;
        return parsed.as_array().and_then(|_| range_len(&parsed));
    }
    let Value::Array(items) = value else {
        return None;
    };
    let mut numbers = items.iter().filter_map(int_value);
    let first = numbers.next()?;
    let last = numbers.next()?;
    // `checked_sub` rejects a reversed range and `checked_add` a span that
    // would overflow, so a malformed model argument cannot panic `read`.
    last.checked_sub(first)?.checked_add(1)
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
    opt_bool_arg(args, primary)
        .or_else(|| opt_bool_arg(args, legacy))
        .unwrap_or(false)
}

/// The same read, leaving an absent argument absent so a caller can tell
/// "not sent" from an explicit `false`.
fn opt_bool_arg(args: &Value, key: &str) -> Option<bool> {
    args.get(key).and_then(Value::as_bool)
}

/// Fetches a URL unless a configured MCP server owns it, in which case the
/// server is loaded on demand and the model is pointed at its tools instead of
/// an unauthenticated fetch.
async fn webfetch_guarded(args: &Value, mcp: &McpRegistry) -> Result<String> {
    let url = required_string(args, "webfetch", "url", r#"{"url":"https://example.com"}"#)?;
    if let Some(server) = mcp.url_owned(url) {
        let loaded = mcp.load(&server).await.with_context(|| {
            format!("loading MCP server `{server}` for URL owned by that service")
        })?;
        let hint = format!(
            "This URL belongs to the `{server}` MCP server. Prefer its tools over webfetch. {loaded}"
        );
        return Ok(hint);
    }
    if let Some(hint) = aws_console_hint(url) {
        return Ok(hint);
    }
    if let Some((route, item)) = forge_route(url) {
        return Ok(forge_hint(route, item, url));
    }
    if let Some(hint) = github_file_hint(url) {
        return Ok(hint);
    }
    if let Some(hint) = forge_host_hint(url) {
        return Ok(hint);
    }
    webfetch(args).await
}

/// Keeps account-scoped AWS Console links on an authenticated route. This is
/// intentionally limited to console hosts: an arbitrary `amazonaws.com` URL
/// may be a public object or a time-limited presigned download that webfetch
/// should still be allowed to read.
fn aws_console_hint(url: &str) -> Option<String> {
    let parsed = reqwest::Url::parse(url).ok()?;
    let host = parsed.host_str()?.to_ascii_lowercase();
    let console_suffix = [
        ".console.aws.amazon.com",
        ".console.amazonaws.cn",
        ".console.amazonaws-us-gov.com",
    ]
    .into_iter()
    .find(|suffix| host.ends_with(suffix));
    let is_console = host == "console.aws.amazon.com"
        || host == "console.amazonaws.cn"
        || host == "console.amazonaws-us-gov.com"
        || console_suffix.is_some();
    if !is_console {
        return None;
    }

    let query_region = parsed
        .query_pairs()
        .find(|(key, _)| key == "region")
        .map(|(_, value)| value.into_owned())
        .filter(|value| aws_region_name(value));
    let host_region = console_suffix
        .and_then(|suffix| host.strip_suffix(suffix))
        .filter(|value| aws_region_name(value))
        .map(str::to_string);
    let region = query_region.or(host_region);
    let region_flag = region.as_deref().map_or_else(
        || " --region <region>".to_string(),
        |region| format!(" --region {region}"),
    );
    let decoded_fragment = parsed.fragment().map(decode_aws_console_fragment);
    let command = aws_console_command(&parsed, decoded_fragment.as_deref(), &region_flag);
    let availability = if command_exists("aws") {
        String::new()
    } else {
        " The `aws` CLI is not currently available; install AWS CLI v2 and authenticate the intended profile before retrying.".to_string()
    };

    Some(format!(
        "This is an account-scoped AWS Console URL. Do not use webfetch: it cannot reuse the browser's AWS session and may turn an authorization failure into an empty page, redirect, or 404. Use the authenticated AWS CLI with the same account and region. Confirm the account first with `aws sts get-caller-identity --profile <profile>`, then read the resource with `{command}`. If the profile uses IAM Identity Center and its session expired, run `aws sso login --profile <profile>` first.{availability}"
    ))
}

fn aws_region_name(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        && value.bytes().filter(|byte| *byte == b'-').count() >= 2
}

fn aws_console_command(parsed: &reqwest::Url, fragment: Option<&str>, region: &str) -> String {
    let path = parsed.path().to_ascii_lowercase();
    let fragment_lower = fragment.unwrap_or_default().to_ascii_lowercase();
    let route = format!("{path}#{fragment_lower}");

    if route.contains("cloudwatch") || route.contains("log-groups") {
        if let Some((group, stream)) = aws_cloudwatch_log_target(fragment.unwrap_or_default()) {
            return format!(
                "aws logs get-log-events --log-group-name {} --log-stream-name {}{region} --profile <profile>",
                shell_quote_hint(&group),
                shell_quote_hint(&stream)
            );
        }
        return format!(
            "aws logs get-log-events --log-group-name <log-group> --log-stream-name <log-stream>{region} --profile <profile>"
        );
    }
    if route.contains("batch") {
        if let Some(job_id) = aws_fragment_value(fragment.unwrap_or_default(), "jobs/detail/") {
            return format!(
                "aws batch describe-jobs --jobs {}{region} --profile <profile>",
                shell_quote_hint(job_id)
            );
        }
        return format!("aws batch describe-jobs --jobs <job-id>{region} --profile <profile>");
    }
    if route.contains("/s3/") || path.starts_with("/s3") {
        if let Some(bucket) = parsed
            .path_segments()
            .and_then(|mut segments| {
                segments
                    .find(|segment| *segment == "buckets")
                    .and_then(|_| segments.next())
            })
            .filter(|bucket| !bucket.is_empty())
        {
            return format!(
                "aws s3 ls {}{region} --profile <profile>",
                shell_quote_hint(&format!("s3://{bucket}"))
            );
        }
        return format!("aws s3 ls s3://<bucket>/<prefix>{region} --profile <profile>");
    }
    if route.contains("lambda") {
        return format!(
            "aws lambda get-function --function-name <function>{region} --profile <profile>"
        );
    }
    if route.contains("ecs") {
        return format!(
            "aws ecs describe-services --cluster <cluster> --services <service>{region} --profile <profile>"
        );
    }
    format!("aws <service> <read-command>{region} --profile <profile>")
}

fn aws_cloudwatch_log_target(fragment: &str) -> Option<(String, String)> {
    let rest = fragment.split("log-group/").nth(1)?;
    let (group, stream) = rest.split_once("/log-events/")?;
    let stream = stream.split(['?', '#']).next().unwrap_or(stream);
    if group.is_empty() || stream.is_empty() {
        return None;
    }
    Some((group.to_string(), stream.to_string()))
}

fn aws_fragment_value<'a>(fragment: &'a str, marker: &str) -> Option<&'a str> {
    let value = fragment.split(marker).nth(1)?;
    let value = value.split(['?', '#', '/']).next().unwrap_or(value);
    (!value.is_empty()).then_some(value)
}

fn decode_url_component(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            let high = (bytes[index + 1] as char).to_digit(16);
            let low = (bytes[index + 2] as char).to_digit(16);
            if let (Some(high), Some(low)) = (high, low) {
                decoded.push(((high << 4) | low) as u8);
                index += 3;
                continue;
            }
        }
        decoded.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&decoded).into_owned()
}

/// CloudWatch's fragment router uses `$252F` for a double-encoded slash (and
/// the same `$` prefix for other percent escapes). Normalize that convention,
/// then decode twice so both ordinary `%2F` links and CloudWatch's `$252F`
/// links yield the resource names expected by the CLI.
fn decode_aws_console_fragment(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut normalized = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'$'
            && index + 2 < bytes.len()
            && (bytes[index + 1] as char).is_ascii_hexdigit()
            && (bytes[index + 2] as char).is_ascii_hexdigit()
        {
            normalized.push(b'%');
        } else {
            normalized.push(bytes[index]);
        }
        index += 1;
    }
    let once = decode_url_component(&String::from_utf8_lossy(&normalized));
    decode_url_component(&once)
}

fn shell_quote_hint(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
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

/// Routes a GitHub `blob` link through the authenticated contents API. A
/// browser-style fetch cannot distinguish a private repository from a missing
/// page (both commonly answer 404), while `gh api` carries the user's GitHub
/// credential and can return the raw file at the revision named by the URL.
fn github_file_hint(url: &str) -> Option<String> {
    let parsed = reqwest::Url::parse(url).ok()?;
    if parsed.host_str()? != "github.com" {
        return None;
    }
    let segments: Vec<&str> = parsed.path_segments()?.collect();
    if segments.len() < 5 || segments[2] != "blob" {
        return None;
    }
    let owner = segments[0];
    let repo = segments[1];
    let revision = segments[3];
    let path = segments[4..].join("/");
    if owner.is_empty() || repo.is_empty() || revision.is_empty() || path.is_empty() {
        return None;
    }
    let availability = if command_exists("gh") {
        String::new()
    } else {
        " The `gh` CLI is not currently available; use an existing local checkout or install and authenticate `gh` before retrying.".to_string()
    };
    // Only a full commit id fixes the ref/path boundary. For a named ref,
    // `/blob/feature/foo/src/lib.rs` may mean branch `feature/foo` and path
    // `src/lib.rs`; treating the first segment as the ref produces a plausible
    // but wrong authenticated request.
    if revision.len() != 40 || !revision.chars().all(|ch| ch.is_ascii_hexdigit()) {
        return Some(format!(
            "This URL is a GitHub repository file, which may be private. Do not use webfetch or raw.githubusercontent.com: both are unauthenticated and a private file commonly looks like a 404. Use an existing authenticated checkout or `gh repo clone {owner}/{repo}` and resolve the complete ref and file path from the original URL. Do not assume `{revision}` is the whole ref: GitHub branch and tag names can contain `/`.{availability}"
        ));
    }
    let endpoint = format!("repos/{owner}/{repo}/contents/{path}?ref={revision}");
    let line_hint = github_line_range(parsed.fragment()).map_or_else(String::new, |range| {
        format!(" To read only the linked lines, pipe it to `sed -n '{range}p'`.")
    });
    Some(format!(
        "This URL is a GitHub repository file, which may be private. Do not use webfetch or raw.githubusercontent.com: both are unauthenticated and a private file commonly looks like a 404. Read it with `gh api -H 'Accept: application/vnd.github.raw+json' '{endpoint}'`.{line_hint}{availability}"
    ))
}

/// Keeps every GitHub/GitLab page on an authenticated route, not just the
/// work-item and blob shapes for which Oxide can print an exact command. A
/// repository root, commit, comparison, action or tree can be private too, and
/// falling through to webfetch turns an authentication problem into a
/// misleading 404.
fn forge_host_hint(url: &str) -> Option<String> {
    let parsed = reqwest::Url::parse(url).ok()?;
    let host = parsed.host_str()?;
    let (service, cli, example) = if host == "github.com"
        || host.ends_with(".github.com")
        || host == "raw.githubusercontent.com"
    {
        ("GitHub", "gh", "gh repo view OWNER/REPO")
    } else if host == "gitlab.com" || host.ends_with(".gitlab.com") {
        ("GitLab", "glab", "glab repo view OWNER/REPO")
    } else {
        return None;
    };
    let availability = if command_exists(cli) {
        String::new()
    } else {
        format!(
            " The `{cli}` CLI is not currently available; install and authenticate it before retrying."
        )
    };
    Some(format!(
        "This {service} URL may be private. Use the authenticated `{cli}` CLI instead of webfetch; start with `{example}` or the matching `{cli} api` endpoint for the URL.{availability}"
    ))
}

fn github_line_range(fragment: Option<&str>) -> Option<String> {
    let fragment = fragment?;
    let (start, end) = fragment
        .split_once("-L")
        .map_or((fragment, None), |(start, end)| (start, Some(end)));
    let start = start.strip_prefix('L')?;
    if start.is_empty() || !start.chars().all(|ch| ch.is_ascii_digit()) {
        return None;
    }
    match end {
        Some(end) if !end.is_empty() && end.chars().all(|ch| ch.is_ascii_digit()) => {
            Some(format!("{start},{end}"))
        }
        Some(_) => None,
        None => Some(start.to_string()),
    }
}

/// Give recovery guidance without inferring why a server denied access.
fn fetch_status_error(status: reqwest::StatusCode) -> anyhow::Error {
    let hint = match status.as_u16() {
        401 => " — authentication is required; use the service's authenticated CLI or a configured MCP server",
        403 => " — the site denied access to this fetch; authentication, permissions, or automated-fetch restrictions may apply; use the service's authenticated CLI or a configured MCP server instead of retrying unchanged",
        429 => " — the site is rate limiting this client; wait before retrying",
        _ => "",
    };
    anyhow::anyhow!("fetch returned {status}{hint}")
}

async fn webfetch(args: &Value) -> Result<String> {
    let url = required_string(args, "webfetch", "url", r#"{"url":"https://example.com"}"#)?;
    let format = args
        .get("format")
        .and_then(Value::as_str)
        .unwrap_or("markdown");

    // Keep one pool for the process so consecutive fetches to the same host do
    // not repeat DNS, TCP, and TLS setup. `Client` is internally shared and is
    // intended to be reused across requests.
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    let client = CLIENT.get_or_init(reqwest::Client::new);
    let response = client
        .get(url)
        .header("User-Agent", "oxide")
        .send()
        .await
        .with_context(|| format!("fetching {url}"))?;
    let status = response.status();
    if !status.is_success() {
        return Err(fetch_status_error(status));
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
            let file_name = entry.file_name();
            let name_lossy = file_name.to_string_lossy();
            if matches!(
                name_lossy.as_ref(),
                ".git" | "node_modules" | "target" | ".venv"
            ) {
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
    let (mut p, mut t) = (0, 0);
    let mut star = None;
    let mut retry = 0;
    while t < text.len() {
        if p < pattern.len() && pattern[p] == '*' {
            star = Some(p);
            p += 1;
            retry = t;
        } else if p < pattern.len() && (pattern[p] == '?' || pattern[p] == text[t]) {
            p += 1;
            t += 1;
        } else if let Some(index) = star {
            retry += 1;
            t = retry;
            p = index + 1;
        } else {
            return false;
        }
    }
    pattern[p..].iter().all(|ch| *ch == '*')
}

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

/// One `grep` invocation read out of a shell command.
struct ShellGrep {
    /// The pattern searched for, when the command names it literally. A `-f`
    /// pattern file leaves this empty, since the tool call cannot restate it.
    pattern: Option<String>,
    /// The search operands: the paths grep was pointed at.
    operands: Vec<String>,
    /// A `--include` file-name filter, which maps to the tool's `glob`.
    glob: Option<String>,
    recursive: bool,
    restatable: bool,
    fixed: bool,
    /// The directory the operands resolve against, when the command's own `cd`s
    /// could be followed (`cd ~/crate && grep -rn needle src/app.rs`). `None`
    /// leaves a relative operand as it was written.
    base: Option<PathBuf>,
}

impl ShellGrep {
    /// Only offer a call when its pattern, scope and matching mode can be
    /// represented without evaluating the shell or dropping search operands.
    fn equivalent_call(&self) -> Option<String> {
        if !self.restatable || self.operands.len() > 1 {
            return None;
        }
        let pattern = literal_grep_word(self.pattern.as_ref()?)?;
        let mut call = serde_json::Map::new();
        call.insert("pattern".to_string(), json!(pattern));
        if let Some(path) = self.operands.first() {
            let path = literal_grep_word(path)?;
            // The tool starts in the project root, so an operand written after a
            // `cd` has to be named where the grep really searched.
            let path = match &self.base {
                Some(base) if Path::new(&path).is_relative() => {
                    base.join(&path).to_string_lossy().into_owned()
                }
                _ => path,
            };
            call.insert("path".to_string(), json!(path));
        }
        if let Some(glob) = &self.glob {
            call.insert("glob".to_string(), json!(literal_grep_word(glob)?));
        }
        if self.fixed {
            call.insert("literal".to_string(), json!(true));
        } else if pattern.contains([
            '\\', '[', ']', '^', '$', '.', '*', '+', '?', '(', ')', '{', '}', '|',
        ]) {
            // BRE/ERE and the tool's regex dialect do not always agree.
            return None;
        }
        Some(Value::Object(call).to_string())
    }
}

/// Keep quotes and escapes so decoding a suggestion never evaluates shell code.
fn grep_shell_words(segment: &str) -> Vec<&str> {
    let mut words = Vec::new();
    let mut start = None;
    let mut quote = None;
    let mut escaped = false;
    for (index, ch) in segment.char_indices() {
        if start.is_none() && !ch.is_whitespace() {
            start = Some(index);
        }
        if escaped {
            escaped = false;
        } else if ch == '\\' && quote != Some('\'') {
            escaped = true;
        } else {
            match quote {
                Some(open) if ch == open => quote = None,
                Some(_) => {}
                None if matches!(ch, '\'' | '"') => quote = Some(ch),
                None if ch.is_whitespace() => {
                    if let Some(start) = start.take() {
                        words.push(&segment[start..index]);
                    }
                }
                None => {}
            }
        }
    }
    if let Some(start) = start {
        words.push(&segment[start..]);
    }
    words
}

/// Decode literal words only; expansions and redirects need the generic hint.
fn literal_grep_word(word: &str) -> Option<String> {
    let mut result = String::new();
    let mut quote = None;
    let mut chars = word.chars();
    while let Some(ch) = chars.next() {
        match (quote, ch) {
            (Some('\''), '\'') | (Some('"'), '"') => quote = None,
            (Some('\''), _) => result.push(ch),
            (_, '$' | '`') => return None,
            (None, '\'' | '"') => quote = Some(ch),
            (None, '*' | '?' | '[' | '~' | '{' | '}' | '<' | '>') => return None,
            (_, '\\') => {
                let next = chars.next()?;
                if quote == Some('"') && !matches!(next, '$' | '`' | '"' | '\\' | '\n') {
                    result.push('\\');
                }
                if next != '\n' {
                    result.push(next);
                }
            }
            _ => result.push(ch),
        }
    }
    quote.is_none().then_some(result)
}

/// Reads one shell segment as a `grep` invocation, or `None` when the segment
/// does not run grep.
fn parse_shell_grep(segment: &str) -> Option<ShellGrep> {
    let words = grep_shell_words(segment);
    let grep_index = shell_command_index(&words)
        .filter(|index| matches!(shell_word_name(words[*index]), "grep" | "grep.exe"))?;

    let mut grep = ShellGrep {
        pattern: None,
        operands: Vec::new(),
        glob: None,
        recursive: false,
        restatable: grep_index == 0,
        fixed: false,
        base: None,
    };
    // Whether an option already supplied the pattern, so the next positional
    // argument is a search operand rather than the pattern.
    let mut saw_pattern = false;
    let mut literal = false;
    let mut arguments = words[grep_index + 1..].iter();
    while let Some(word) = arguments.next() {
        if !literal {
            if *word == "--" {
                literal = true;
                continue;
            }
            if matches!(*word, "--recursive" | "--dereference-recursive") {
                grep.recursive = true;
                continue;
            }
            if let Some(glob) = word.strip_prefix("--include=") {
                grep.restatable &= grep.glob.is_none();
                grep.glob = Some(glob.to_string());
                continue;
            }
            if *word == "--include" {
                grep.restatable &= grep.glob.is_none();
                grep.glob = arguments.next().map(|glob| glob.to_string());
                continue;
            }
            if matches!(*word, "--regexp" | "--file") {
                grep.restatable &= !saw_pattern && *word == "--regexp";
                saw_pattern = true;
                if *word == "--regexp" {
                    grep.pattern = arguments.next().map(|value| value.to_string());
                } else {
                    arguments.next();
                }
                continue;
            }
            if matches!(
                *word,
                "--after-context"
                    | "--before-context"
                    | "--context"
                    | "--max-count"
                    | "--label"
                    | "--exclude"
                    | "--exclude-dir"
                    | "--binary-files"
                    | "--directories"
                    | "--devices"
            ) {
                grep.restatable = false;
                arguments.next();
                continue;
            }
            if let Some(flags) = word.strip_prefix('-') {
                if !flags.starts_with('-') {
                    let mut flags = flags.chars().peekable();
                    while let Some(flag) = flags.next() {
                        if flag == 'F' {
                            grep.fixed = true;
                        }
                        if !matches!(flag, 'r' | 'R' | 'n' | 'H' | 'h' | 'E' | 'F' | 'e') {
                            grep.restatable = false;
                        }
                        if flag == 'r' || flag == 'R' {
                            grep.recursive = true;
                        }
                        if matches!(flag, 'e' | 'f') {
                            grep.restatable &= !saw_pattern && flag == 'e';
                            saw_pattern = true;
                            let value = if flags.peek().is_none() {
                                arguments.next().map(|value| value.to_string())
                            } else {
                                Some(flags.collect::<String>())
                            };
                            if flag == 'e' {
                                grep.pattern = value;
                            }
                            break;
                        }
                        if matches!(flag, 'A' | 'B' | 'C' | 'm' | 'd' | 'D') {
                            if flags.peek().is_none() {
                                arguments.next();
                            }
                            break;
                        }
                    }
                } else {
                    grep.restatable = false;
                }
                continue;
            }
        }
        if word.starts_with(|ch: char| ch.is_ascii_digit()) && word.contains('>') {
            continue;
        }
        // The first positional argument is the pattern unless an option
        // already supplied one; everything after it is a search operand.
        if saw_pattern {
            grep.operands.push(word.to_string());
        } else {
            saw_pattern = true;
            grep.pattern = Some(word.to_string());
        }
    }
    Some(grep)
}

/// The first recursive grep in `command` that could walk a directory.
fn recursive_shell_grep(command: &str, cwd: &Path) -> Option<ShellGrep> {
    let directories = segment_directories(command, cwd);
    shell_segments(command)
        .into_iter()
        .enumerate()
        .find_map(|(index, segment)| {
            let mut grep = parse_shell_grep(segment)?;
            // `restatable` is already set to `grep_index == 0` by parse_shell_grep,
            // so we can infer if this is the first shell command in this segment.
            // A segment whose directory could be followed is judged against it,
            // so a glob that expands to files there is a search of named files
            // rather than a sweep. Anything else keeps the conservative rule: the
            // grep tool starts in the project root, and this reader cannot say
            // which directory the shell was in.
            let (base, verified) = match directories.get(index).and_then(Option::as_deref) {
                Some(base) => (base, true),
                None => {
                    grep.restatable &= index == 0;
                    (cwd, grep.restatable && index == 0)
                }
            };
            if !grep.recursive || !recursive_grep_sweeps(&grep.operands, base, verified) {
                return None;
            }
            // A relative operand is restated where the grep really searched, so
            // only a directory the command moved to changes what it says.
            grep.base = (verified && base != cwd).then(|| base.to_path_buf());
            Some(grep)
        })
}

/// Whether a segment could change the shell's directory in a way this reader
/// cannot follow, which makes its own and every later relative operand
/// unverifiable.
fn changes_directory_opaquely(segment: &str) -> bool {
    if segment.contains(['(', ')', '{', '}', '`']) {
        return true;
    }
    let words = grep_shell_words(segment);
    let Some(first) = words
        .iter()
        .find(|word| !shell_assignment(word) && !word.starts_with(|ch: char| ch.is_ascii_digit()))
    else {
        return false;
    };
    matches!(
        shell_word_name(first),
        "pushd"
            | "popd"
            | "exec"
            | "su"
            | "sudo"
            | "for"
            | "while"
            | "until"
            | "case"
            | "if"
            | "do"
            | "eval"
            | "source"
            | "."
            | "env"
            | "chroot"
            | "chdir"
    )
}

/// The directory each shell segment runs in, from the `cd`s written before it.
/// `None` means the directory is no longer known, and the segment keeps the
/// conservative rule.
fn segment_directories(command: &str, cwd: &Path) -> Vec<Option<PathBuf>> {
    let mut base = Some(cwd.to_path_buf());
    let mut directories = Vec::new();
    for segment in shell_segments(command) {
        // A segment that could move itself — a subshell, `env -C`, `pushd` —
        // cannot be placed, so its own operands keep the conservative rule.
        let opaque = changes_directory_opaquely(segment);
        directories.push(if opaque { None } else { base.clone() });
        base = match cd_target(segment) {
            Some(Some(target)) => base
                .as_deref()
                .and_then(|base| resolved_directory(base, &target)),
            Some(None) => None,
            None if opaque => None,
            None => base,
        };
    }
    directories
}

/// The directory a segment `cd`s to: `Some(Some(path))` for a literal target,
/// `Some(None)` for a `cd` this reader cannot follow (`cd -`, an expansion), and
/// `None` when the segment is not a `cd` at all.
fn cd_target(segment: &str) -> Option<Option<String>> {
    let words = grep_shell_words(segment);
    let index = shell_command_index(&words)?;
    if shell_word_name(words[index]) != "cd" {
        return None;
    }
    let Some(target) = words.get(index + 1) else {
        return Some(None);
    };
    if target.starts_with('-') {
        return Some(None);
    }
    Some(decode_directory(target))
}

/// A `cd` target decoded as a literal path, with `~` kept as the marker it is
/// so the resolution happens on this machine's home rather than the shell's.
fn decode_directory(word: &str) -> Option<String> {
    if let Some(rest) = word.strip_prefix('~') {
        if !(rest.is_empty() || rest.starts_with('/')) {
            return None;
        }
        return literal_grep_word(rest).map(|rest| format!("~{rest}"));
    }
    literal_grep_word(word)
}

/// The directory a `cd` lands in, when it can be resolved on this machine. A
/// target that does not exist is not followed: the grep it precedes is then
/// judged conservatively rather than against a guess.
fn resolved_directory(base: &Path, target: &str) -> Option<PathBuf> {
    let target = match target.strip_prefix('~') {
        Some(rest) => dirs::home_dir()?.join(rest.trim_start_matches('/')),
        None => base.join(target),
    };
    std::fs::canonicalize(target)
        .ok()
        .filter(|path| path.is_dir())
}

/// Refuses the recursive shell-grep pattern that most often turns a narrow
/// code search into a walk through dependency and build-output trees. The
/// dedicated grep tool applies the repository's ignore rules, bounds its
/// results, and is the right place for a repository-wide content search.
///
/// This intentionally permits non-recursive grep over explicit files, grep
/// used as a pipe filter, and a recursive grep whose operands are all named
/// files (`grep -rn needle README.md docs/guide.md`): walking named files
/// cannot reach a build-output or dependency tree, so refusing it only costs
/// the run a step. It also inspects every shell segment so a leading `cd`,
/// environment assignment, or pipeline cannot hide the grep.
/// The boolean form of [`recursive_shell_grep`], kept for the guard's own tests.
#[cfg(test)]
fn uses_recursive_shell_grep(command: &str, cwd: &Path) -> bool {
    recursive_shell_grep(command, cwd).is_some()
}

/// The refusal text for a recursive shell grep, naming the grep-tool call that
/// searches the same pattern and path when the command spells them out.
fn recursive_shell_grep_refusal(grep: &ShellGrep) -> String {
    let mut message = String::from(
        "recursive shell grep refused; call the `grep` tool directly with `pattern` and \
         optional `path`, `glob`, and `limit` instead. The tool uses ripgrep when \
         available, skips dependency/generated directories, and respects ignore files",
    );
    if let Some(call) = grep.equivalent_call() {
        message.push_str(&format!(". For this command that call is {call}"));
    }
    message
}

/// Only expand a simple filename glob in a literal directory. Whether that
/// directory is the project root or the one the command's own `cd` reached is
/// the caller's to say.
fn grep_glob_names_files(operand: &str, cwd: &Path) -> bool {
    if operand.starts_with('~')
        || operand.ends_with('/')
        || operand.contains(['\'', '"', '\\', '$', '`', '[', '{', '}'])
    {
        return false;
    }
    let path = Path::new(operand);
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let Some(pattern) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    if parent.to_string_lossy().contains(['*', '?']) || !pattern.contains(['*', '?']) {
        return false;
    }
    let Ok(entries) = std::fs::read_dir(cwd.join(parent)) else {
        return false;
    };
    let pattern_chars: Vec<char> = pattern.chars().collect();
    let mut matched = false;
    for entry in entries {
        let Ok(entry) = entry else { return false };
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            return false;
        };
        if name.starts_with('.') && !pattern.starts_with('.') {
            continue;
        }
        if wildcard(&pattern_chars, &name.chars().collect::<Vec<_>>()) {
            if !entry.path().is_file() {
                return false;
            }
            matched = true;
        }
    }
    matched
}

fn recursive_grep_sweeps(operands: &[String], cwd: &Path, verified: bool) -> bool {
    operands.is_empty()
        || operands.iter().any(|operand| {
            if verified && grep_glob_names_files(operand, cwd) {
                return false;
            }
            let operand = operand.trim_matches(['\'', '"']);
            operand.is_empty()
                || operand.ends_with('/')
                || operand.contains(['*', '?', '['])
                || !cwd.join(operand).is_file()
        })
}

/// A pipeline that only reads over the network: a read command from `gh`, a
/// GET-shaped `curl`, or `git ls-remote`, followed by text filters. A transport
/// failure on one of these is safe to retry, since running it again cannot
/// repeat a change the first attempt did not make.
fn read_only_network_pipeline(command: &str) -> bool {
    if writes_a_file(command) {
        return false;
    }
    let command = strip_redirections(command);
    let mut segments = shell_segments(&command)
        .into_iter()
        .map(str::trim)
        .filter(|segment| !segment.is_empty());
    let Some(first) = segments.next() else {
        return false;
    };
    network_read_command(first) && segments.all(text_filter_command)
}

/// Whether the command sends its output into a file. Redirecting stdout makes
/// a read a write whatever it runs, so such a command is never retried; a
/// discard (`2>/dev/null`, `2>&1`) and an input redirect (`< file`) are not.
fn writes_a_file(command: &str) -> bool {
    for word in command.split_whitespace() {
        let trimmed = word.trim_start_matches(|ch: char| ch.is_ascii_digit());
        let target = if let Some(target) = trimmed.strip_prefix('>') {
            target
        } else if let Some(target) = trimmed.strip_prefix("&>") {
            target
        } else {
            continue;
        };
        if target.starts_with('&') {
            continue;
        }
        if target.is_empty() || target != "/dev/null" {
            return true;
        }
    }
    false
}

/// Removes redirection tokens (`2>&1`, `>/dev/null`) so a shell separator
/// inside one does not split the pipeline at the wrong place.
fn strip_redirections(command: &str) -> String {
    command
        .split_whitespace()
        .filter(|word| {
            let word = word.trim_start_matches(|ch: char| ch.is_ascii_digit());
            !(word.starts_with('>') || word.starts_with('<') || word.starts_with("&>"))
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn network_read_command(segment: &str) -> bool {
    let words: Vec<&str> = segment.split_whitespace().collect();
    let Some(index) = shell_command_index(&words) else {
        return false;
    };
    let args = &words[index + 1..];
    match shell_word_name(words[index]) {
        "gh" => gh_reads(args),
        "curl" => curl_reads(args),
        "git" => git_reads(args),
        _ => false,
    }
}

fn text_filter_command(segment: &str) -> bool {
    let words: Vec<&str> = segment.split_whitespace().collect();
    shell_command_index(&words)
        .map(|index| {
            matches!(
                shell_word_name(words[index]),
                "head"
                    | "tail"
                    | "grep"
                    | "egrep"
                    | "fgrep"
                    | "rg"
                    | "cat"
                    | "jq"
                    | "wc"
                    | "sort"
                    | "uniq"
            )
        })
        .unwrap_or(false)
}

fn gh_reads(args: &[&str]) -> bool {
    let Some(at) = args.iter().position(|word| !word.starts_with('-')) else {
        return false;
    };
    let rest = &args[at + 1..];
    match args[at] {
        "api" => !gh_api_mutates(rest),
        "search" => true,
        "pr" => matches!(
            rest.first().copied(),
            Some("view" | "list" | "diff" | "checks" | "status")
        ),
        "issue" | "run" | "release" | "repo" => {
            matches!(rest.first().copied(), Some("view" | "list"))
        }
        "auth" => matches!(rest.first().copied(), Some("status")),
        _ => false,
    }
}

/// Whether a `gh api` call names a change rather than a read. A field flag
/// makes the request a POST even when no method is given, so it counts.
fn gh_api_mutates(args: &[&str]) -> bool {
    let mut words = args.iter().copied();
    while let Some(word) = words.next() {
        if matches!(word, "-f" | "--field" | "-F" | "--raw-field" | "--input")
            || word.starts_with("--field=")
            || word.starts_with("--raw-field=")
            || word.starts_with("--input=")
            || (!word.starts_with("--") && (word.starts_with("-f") || word.starts_with("-F")))
        {
            return true;
        }
        if let Some(method) = word
            .strip_prefix("--method=")
            .or_else(|| word.strip_prefix("-X").filter(|value| !value.is_empty()))
        {
            return !method_reads(method);
        }
        if matches!(word, "-X" | "--method") {
            return !words.next().is_some_and(method_reads);
        }
    }
    false
}

fn curl_reads(args: &[&str]) -> bool {
    let mut words = args.iter().copied();
    while let Some(word) = words.next() {
        if !word.starts_with('-') || word == "-" {
            continue;
        }
        if let Some(long) = word.strip_prefix("--") {
            let (name, value) = match long.split_once('=') {
                Some((name, value)) => (name, Some(value)),
                None => (long, None),
            };
            match name {
                "request" => {
                    if !method_reads(value.or_else(|| words.next()).unwrap_or("")) {
                        return false;
                    }
                }
                "data" | "data-raw" | "data-binary" | "data-urlencode" | "data-ascii" | "form"
                | "form-string" | "upload-file" | "json" | "output" | "output-dir"
                | "remote-name" | "remote-name-all" | "post301" | "post302" | "post303" => {
                    return false;
                }
                _ => {}
            }
            continue;
        }
        let mut flags = word.trim_start_matches('-').chars().peekable();
        while let Some(flag) = flags.next() {
            match flag {
                'd' | 'F' | 'T' | 'o' | 'O' => return false,
                'X' => {
                    let inline: String = flags.by_ref().collect();
                    let method = if inline.is_empty() {
                        words.next().unwrap_or("")
                    } else {
                        &inline
                    };
                    if !method_reads(method) {
                        return false;
                    }
                    break;
                }
                _ => {}
            }
        }
    }
    true
}

fn git_reads(args: &[&str]) -> bool {
    let mut words = args.iter().copied();
    while let Some(word) = words.next() {
        if matches!(
            word,
            "-C" | "-c" | "--git-dir" | "--work-tree" | "--namespace" | "--config-env"
        ) {
            words.next();
            continue;
        }
        if word.starts_with('-') {
            continue;
        }
        return word == "ls-remote";
    }
    false
}

fn method_reads(method: &str) -> bool {
    matches!(
        method.trim().to_ascii_uppercase().as_str(),
        "" | "GET" | "HEAD"
    )
}

/// Splits commands only at shell separators outside quotes. Treating a quoted
/// `| grep -r` example as another pipeline stage would turn data into a command.
pub(crate) fn shell_segments(command: &str) -> Vec<&str> {
    let mut segments = Vec::new();
    let mut start = 0;
    let mut quote = None;
    let mut escaped = false;
    for (index, ch) in command.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        if ch == '\\' && quote != Some('\'') {
            escaped = true;
            continue;
        }
        match quote {
            Some(open) if ch == open => quote = None,
            Some(_) => {}
            None if ch == '\'' || ch == '"' => quote = Some(ch),
            None if matches!(ch, '\n' | ';' | '&' | '|') => {
                segments.push(&command[start..index]);
                start = index + ch.len_utf8();
            }
            None => {}
        }
    }
    segments.push(&command[start..]);
    segments
}

/// Finds the executable token rather than any argument that happens to spell
/// `grep`. Shell assignments and the common wrappers an agent may put before a
/// command are skipped; wrapper options are skipped conservatively too.
pub(crate) fn shell_command_index(words: &[&str]) -> Option<usize> {
    let mut index = 0;
    while index < words.len() && shell_assignment(words[index]) {
        index += 1;
    }
    loop {
        let name = shell_word_name(words.get(index)?);
        if !matches!(name, "command" | "exec" | "env" | "nohup" | "sudo") {
            return Some(index);
        }
        let wrapper = name;
        index += 1;
        while let Some(word) = words.get(index) {
            if *word == "--" {
                index += 1;
                break;
            }
            if shell_assignment(word) || word.starts_with('-') {
                if wrapper_option_takes_value(wrapper, word) {
                    index += 1;
                }
                index += 1;
                continue;
            }
            break;
        }
    }
}

fn wrapper_option_takes_value(wrapper: &str, option: &str) -> bool {
    if option.contains('=') {
        return false;
    }
    match wrapper {
        "env" => matches!(
            option,
            "-u" | "--unset" | "-C" | "--chdir" | "-S" | "--split-string"
        ),
        "sudo" => matches!(
            option,
            "-u" | "--user"
                | "-g"
                | "--group"
                | "-h"
                | "--host"
                | "-p"
                | "--prompt"
                | "-C"
                | "--close-from"
                | "-R"
                | "--chroot"
                | "-D"
                | "--chdir"
        ),
        _ => false,
    }
}

fn shell_word_name(word: &str) -> &str {
    let word = word.trim_matches(['\'', '"']);
    word.rsplit(['/', '\\']).next().unwrap_or(word)
}

fn shell_assignment(word: &str) -> bool {
    let Some((name, _)) = word.split_once('=') else {
        return false;
    };
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|ch| ch == '_' || ch.is_ascii_alphabetic())
        && chars.all(|ch| ch == '_' || ch.is_ascii_alphanumeric())
}

/// The PATH inherited by a desktop app is commonly much smaller than the one
/// in the user's terminal. Keep that inherited order, but put conventional
/// user tool directories in front when they exist so a `bash` call can find
/// the same Node, Rust, Python and Homebrew-installed commands without every
/// prompt first having to rediscover and export their locations.
fn bash_path() -> Option<OsString> {
    let inherited = std::env::var_os("PATH").unwrap_or_default();
    let home = dirs::home_dir()?;
    Some(augment_path(&inherited, &home))
}

fn project_bash_path(cwd: &Path) -> Option<OsString> {
    let home = dirs::home_dir()?;
    let inherited = std::env::var_os("PATH").unwrap_or_default();
    Some(project_runtime_path(cwd, &home, &inherited))
}

fn project_runtime_path(cwd: &Path, home: &Path, inherited: &std::ffi::OsStr) -> OsString {
    let augmented = augment_path(inherited, home);
    let Some(version) = std::fs::read_to_string(cwd.join(".nvmrc")).ok() else {
        return augmented;
    };
    let version = version.trim().trim_start_matches('v');
    let parts: Vec<_> = version.split('.').collect();
    if parts.len() != 3
        || !parts
            .iter()
            .all(|part| !part.is_empty() && part.bytes().all(|ch| ch.is_ascii_digit()))
    {
        return augmented;
    }
    let bin = home
        .join(".nvm/versions/node")
        .join(format!("v{version}"))
        .join("bin");
    if !bin
        .join(if cfg!(windows) { "node.exe" } else { "node" })
        .is_file()
    {
        return augmented;
    }
    let mut paths = vec![bin.clone()];
    paths.extend(std::env::split_paths(&augmented).filter(|path| path != &bin));
    std::env::join_paths(paths).unwrap_or(augmented)
}

fn augment_path(inherited: &std::ffi::OsStr, home: &Path) -> OsString {
    let mut paths = Vec::new();
    let candidates = [
        home.join(".local/bin"),
        home.join(".volta/bin"),
        home.join(".cargo/bin"),
        home.join(".pyenv/shims"),
        home.join(".pyenv/bin"),
        home.join(".asdf/shims"),
        home.join(".local/share/mise/shims"),
        home.join(".bun/bin"),
        home.join("Library/pnpm"),
        home.join("go/bin"),
        PathBuf::from("/opt/homebrew/bin"),
        PathBuf::from("/opt/homebrew/sbin"),
        PathBuf::from("/home/linuxbrew/.linuxbrew/bin"),
        PathBuf::from("/home/linuxbrew/.linuxbrew/sbin"),
        PathBuf::from("/usr/local/bin"),
    ];
    for path in candidates
        .into_iter()
        .filter(|path| path.is_dir())
        .chain(std::env::split_paths(inherited))
    {
        if !paths.contains(&path) {
            paths.push(path);
        }
    }
    std::env::join_paths(paths).unwrap_or_else(|_| inherited.to_os_string())
}

/// Runs a shell command, trying a read-only network fetch a second time when
/// the first attempt failed on the transport. A TLS handshake timeout, a reset
/// or a DNS failure usually clears on its own, and re-running something that
/// only reads cannot repeat a change the first attempt did not make. Anything
/// else is returned as it is, with the retry hint `bash_failure_hint` appends.
async fn bash_with_network_retry(
    cwd: &Path,
    args: &Value,
    progress: &Progress,
) -> Result<ToolOutput> {
    let first = bash(cwd, args, progress).await?;
    let command = args
        .get("command")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if first.is_error
        && transient_network_failure(&first.text)
        && read_only_network_pipeline(command)
    {
        let mut retried = bash(cwd, args, progress).await?;
        retried
            .text
            .push_str(" (retried once after a network failure)");
        return Ok(retried);
    }
    Ok(first)
}

async fn bash(cwd: &Path, args: &Value, progress: &Progress) -> Result<ToolOutput> {
    let command = required_string(args, "bash", "command", r#"{"command":"cargo test"}"#)?;
    if let Some(grep) = recursive_shell_grep(command, cwd) {
        anyhow::bail!("{}", recursive_shell_grep_refusal(&grep));
    }
    let secs = bash_timeout_secs(args, command);
    #[cfg(unix)]
    let pipeline_status_path = stream_temp_path("pipeline-status");

    #[cfg(windows)]
    let mut shell = {
        let mut shell = tokio::process::Command::new("cmd");
        shell.arg("/C").arg(command);
        shell
    };
    #[cfg(not(windows))]
    let mut shell = {
        let mut shell = tokio::process::Command::new("bash");
        shell.args(["--noprofile", "--norc", "-o", "pipefail", "-c"]);
        #[cfg(unix)]
        {
            // Capture the last pipeline before another command overwrites
            // PIPESTATUS. Explicit exit/exec and errexit skip this suffix;
            // without evidence of a closed reader their failures stay failures.
            let script = format!(
                "{command}\n__oxide_pipeline_status=(\"$?\" \"${{PIPESTATUS[@]}}\")\n\
                 if (( __oxide_pipeline_status[0] == {SIGPIPE_EXIT} || __oxide_pipeline_status[0] == 1 )); then\n\
                 builtin printf '%s\\n' \"${{__oxide_pipeline_status[@]}}\" > \"$OXIDE_BASH_STATUS\"\n\
                 fi\n\
                 exit \"${{__oxide_pipeline_status[0]}}\""
            );
            shell
                .arg(script)
                .env("OXIDE_BASH_STATUS", &pipeline_status_path);
        }
        #[cfg(not(unix))]
        shell.arg(command);
        shell
    };
    if let Some(path) = project_bash_path(cwd) {
        shell.env("PATH", path);
    }
    crate::child::detach_terminal(&mut shell);

    let mut child = shell
        .current_dir(cwd)
        .kill_on_drop(true)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("spawning shell")?;
    let mut process_tree = crate::child::ProcessTree::new(&child);

    let stdout = child.stdout.take().context("capturing stdout")?;
    let stderr = child.stderr.take().context("capturing stderr")?;
    let stdout_path = stream_temp_path("stdout");
    let stderr_path = stream_temp_path("stderr");
    let stdout_task = tokio::spawn(read_stream(stdout, progress.clone(), stdout_path.clone()));
    let stderr_task = tokio::spawn(read_stream(stderr, progress.clone(), stderr_path.clone()));

    let status = match timeout(Duration::from_secs(secs), child.wait()).await {
        Ok(status) => {
            let status = status.context("waiting for shell")?;
            process_tree.disarm();
            Some(status)
        }
        Err(_) => {
            process_tree.terminate();
            child.kill().await.ok();
            None
        }
    };
    let stdout = join_stream(stdout_task, stdout_path).await?;
    let stderr = join_stream(stderr_task, stderr_path).await?;
    #[cfg(unix)]
    let pipeline_status = {
        if matches!(
            status.as_ref().and_then(std::process::ExitStatus::code),
            Some(SIGPIPE_EXIT | 1)
        ) {
            let text = std::fs::read_to_string(&pipeline_status_path).unwrap_or_default();
            std::fs::remove_file(&pipeline_status_path).ok();
            text
        } else {
            String::new()
        }
    };
    let Some(status) = status else {
        // Keep whatever the command printed before the timeout: a search or
        // build that was killed mid-run may already have produced the answer,
        // and re-running it from scratch is the expensive path.
        let captured = finish_bash_output(stdout, stderr, -1)
            .unwrap_or_default()
            .replace("\n[exit: -1]", "");
        anyhow::bail!("command timed out after {secs}s\n{captured}\n{BASH_TIMEOUT_HINT}");
    };
    let exit = status.code().unwrap_or(-1);
    #[cfg(unix)]
    let reader_closed = exit == SIGPIPE_EXIT && closed_pipeline_reader(&pipeline_status);
    #[cfg(not(unix))]
    let reader_closed = false;
    #[cfg(unix)]
    let no_matches = exit == 1 && search_returned_no_matches(command, &pipeline_status);
    #[cfg(not(unix))]
    let no_matches =
        exit == 1 && last_pipeline_is_search(command) && stderr.tail_text().trim().is_empty();
    let mut output = finish_bash_output(stdout, stderr, if reader_closed { 0 } else { exit })?;
    if reader_closed {
        output.push_str(" (a command in the pipeline stopped because its reader closed)");
    }
    if no_matches {
        output.push_str(" (search returned no matches)");
    }
    if exit != 0 && !reader_closed && unresolved_relative_import(&output) {
        output.push('\n');
        output.push_str(module_failure_hint(&output, cwd));
    }
    if exit != 0 && !reader_closed && !no_matches {
        if let Some(hint) = bash_failure_hint(&output) {
            output.push('\n');
            output.push_str(hint);
        }
    }
    Ok(ToolOutput {
        is_error: exit != 0 && !reader_closed && !no_matches,
        ..ToolOutput::text(output)
    })
}

fn bash_failure_hint(output: &str) -> Option<&'static str> {
    if output.contains("[rejected]")
        && (output.contains("(stale info)")
            || output.contains("(fetch first)")
            || output.contains("(non-fast-forward)"))
    {
        Some("[hint: the remote branch or its tracking ref differs from your expectation. Inspect the remote head, fetch into the relevant remote-tracking ref, and compare remote-only commits before reconciling. Do not bypass this rejection with --force, -f, a + refspec, or a blindly refreshed lease; prefer a normal follow-up commit and push.]")
    } else if output.contains("grep: maximum repetition exceeds")
        || output.contains("grep: invalid repetition count")
    {
        Some("[hint: this grep limits regex repetition counts. Use the `grep` tool with `pattern` and `path` instead; its regex engine supports larger repetition counts.]")
    } else if output.contains("unexpected EOF while looking for matching") {
        Some("[hint: close the shell's quotes. Put literal search patterns in single quotes; backticks inside double quotes start command substitution.]")
    } else if output.contains("package ID specification")
        && output.contains("did not match any packages")
    {
        Some("[hint: the package may be outside this Cargo workspace. Run Cargo with `--manifest-path <package-directory>/Cargo.toml` instead of selecting it with `-p`.]")
    } else if transient_network_failure(output) {
        Some(BASH_NETWORK_HINT)
    } else {
        None
    }
}

#[cfg(unix)]
fn search_returned_no_matches(command: &str, text: &str) -> bool {
    let Some(stages) = simple_search_pipeline(command).or_else(|| compound_search_tail(command))
    else {
        return false;
    };
    let Ok(statuses) = text
        .lines()
        .map(str::parse::<i32>)
        .collect::<Result<Vec<_>, _>>()
    else {
        return false;
    };
    statuses.len() == stages.len() + 1
        && statuses[0] == 1
        && statuses[1..].contains(&1)
        && stages.iter().zip(&statuses[1..]).all(|(stage, status)| {
            *status == 0
                || (*status == 1
                    && matches!(
                        shell_word_name(stage.split_whitespace().next().unwrap_or("")),
                        "grep" | "egrep" | "fgrep" | "rg"
                    ))
        })
}

/// Whether the last stage of a `cmd` pipeline is a search. Windows has no
/// `pipefail`, so the process exit status is already the last stage's; a search
/// that found nothing exits 1, which is a result rather than a failure.
#[cfg(not(unix))]
fn last_pipeline_is_search(command: &str) -> bool {
    let Some(last) = shell_segments(command)
        .into_iter()
        .rev()
        .find(|segment| !segment.trim().is_empty())
    else {
        return false;
    };
    let words: Vec<&str> = last.split_whitespace().collect();
    shell_command_index(&words)
        .map(|index| {
            matches!(
                shell_word_name(words[index]),
                "grep" | "egrep" | "fgrep" | "rg"
            )
        })
        .unwrap_or(false)
}

#[cfg(unix)]
fn simple_search_pipeline(command: &str) -> Option<Vec<&str>> {
    let command = command.trim();
    let mut stages = Vec::new();
    let mut start = 0;
    let mut quote = None;
    let mut escaped = false;
    let mut chars = command.char_indices().peekable();
    let mut previous = None;
    let mut changed_dir = false;
    while let Some((index, ch)) = chars.next() {
        if escaped {
            escaped = false;
            previous = Some(ch);
            continue;
        }
        if ch == '\\' && quote != Some('\'') {
            escaped = true;
            previous = Some(ch);
            continue;
        }
        match quote {
            Some(open) if ch == open => quote = None,
            Some('\'') => {}
            Some(_) if ch == '$' || ch == '`' => return None,
            Some(_) => {}
            None if ch == '\'' || ch == '"' => quote = Some(ch),
            None if ch == '&' && previous == Some('>') => {}
            None if ch == '&'
                && chars.peek().is_some_and(|(_, next)| *next == '&')
                && start == 0 =>
            {
                let prefix: Vec<_> = command[..index].split_whitespace().collect();
                if prefix.len() != 2 || prefix[0] != "cd" {
                    return None;
                }
                chars.next();
                start = index + 2;
                changed_dir = true;
            }
            None if ch == '|' => {
                if chars
                    .peek()
                    .is_some_and(|(_, next)| *next == '|' || *next == '&')
                {
                    return None;
                }
                stages.push(command[start..index].trim());
                start = index + 1;
            }
            None if matches!(
                ch,
                ';' | '&' | '\n' | '\r' | '(' | ')' | '{' | '}' | '$' | '`' | '#' | '<'
            ) =>
            {
                return None
            }
            None => {}
        }
        previous = Some(ch);
    }
    if quote.is_some() || escaped {
        return None;
    }
    stages.push(command[start..].trim());
    // A failed leading `cd` has one status. Require a real pipeline so that
    // its failure cannot be mistaken for the search that never executed.
    if stages.iter().any(|stage| stage.is_empty()) || (changed_dir && stages.len() < 2) {
        return None;
    }
    Some(stages)
}

/// The stages of the last command in a `;`- or newline-separated script, when
/// the script is nothing but plain commands.
///
/// A shell reports the exit status of its last command, so an earlier command
/// in a script is invisible to the status this tool reads: a script that ends
/// in a search which matched nothing is the empty result a lone search is, and
/// the last pipeline's own stage statuses are the evidence. Anything that could
/// make the last command something other than what was written — a comment, a
/// heredoc, a subshell, an expansion, `&&`/`||`, or an earlier command that
/// ends the shell (`exit`, `exec`, `return`, `set`, `trap`, `eval`, `source`)
/// — keeps the ordinary failure behavior.
#[cfg(unix)]
fn compound_search_tail(command: &str) -> Option<Vec<&str>> {
    let command = command.trim();
    let mut segments = Vec::new();
    let mut start = 0;
    let mut quote = None;
    let mut escaped = false;
    let mut previous = None;
    let mut chars = command.char_indices().peekable();
    while let Some((index, ch)) = chars.next() {
        if escaped {
            escaped = false;
            previous = Some(ch);
            continue;
        }
        if ch == '\\' && quote != Some('\'') {
            escaped = true;
            previous = Some(ch);
            continue;
        }
        match quote {
            Some(open) if ch == open => quote = None,
            Some('\'') => {}
            Some(_) if ch == '$' || ch == '`' => return None,
            Some(_) => {}
            None if ch == '\'' || ch == '"' => quote = Some(ch),
            // A redirection's `&` (`2>&1`, `&>out`) is not a control operator.
            None if ch == '&'
                && (previous == Some('>')
                    || chars.peek().is_some_and(|(_, next)| *next == '>')) => {}
            None if ch == ';' || ch == '\n' => {
                segments.push(command[start..index].trim());
                start = index + ch.len_utf8();
            }
            None if ch == '|' => {
                if chars
                    .peek()
                    .is_some_and(|(_, next)| *next == '|' || *next == '&')
                {
                    return None;
                }
            }
            None if matches!(
                ch,
                '&' | '\r' | '(' | ')' | '{' | '}' | '$' | '`' | '#' | '<'
            ) =>
            {
                return None
            }
            None => {}
        }
        previous = Some(ch);
    }
    if quote.is_some() || escaped {
        return None;
    }
    segments.push(command[start..].trim());
    let mut segments: Vec<&str> = segments
        .into_iter()
        .filter(|segment| !segment.is_empty())
        .collect();
    if segments.len() < 2 {
        return None;
    }
    let last = segments.pop()?;
    if segments.iter().any(|segment| ends_the_shell(segment)) {
        return None;
    }
    simple_search_pipeline(last)
}

/// Whether a segment could end the shell before the last command runs, which
/// would make the last command's status the script's for a reason other than a
/// search that matched nothing.
#[cfg(unix)]
fn ends_the_shell(segment: &str) -> bool {
    let words: Vec<&str> = segment.split_whitespace().collect();
    let mut index = 0;
    while index < words.len() && shell_assignment(words[index]) {
        index += 1;
    }
    for _ in 0..4 {
        let Some(word) = words.get(index) else {
            return false;
        };
        let name = shell_word_name(word);
        if matches!(
            name,
            "exit" | "exec" | "return" | "set" | "trap" | "eval" | "source" | "." | "builtin"
        ) {
            return true;
        }
        if !matches!(name, "command" | "env" | "nohup" | "sudo") {
            return false;
        }
        index += 1;
        while index < words.len()
            && (words[index].starts_with('-') || shell_assignment(words[index]))
        {
            index += 1;
        }
    }
    false
}

#[cfg(unix)]
fn closed_pipeline_reader(text: &str) -> bool {
    let Some(statuses) = text
        .lines()
        .map(str::parse::<i32>)
        .collect::<Result<Vec<_>, _>>()
        .ok()
    else {
        return false;
    };
    statuses.len() >= 3
        && statuses[0] == SIGPIPE_EXIT
        && statuses.last() == Some(&0)
        && statuses[1..]
            .iter()
            .all(|&status| status == 0 || status == SIGPIPE_EXIT)
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

/// Keep a successful command's result visible when dependency warnings were
/// appended on stderr. Expanded output still contains every diagnostic.
pub fn bash_warning_preview(output: &str, limit: usize) -> Option<(String, usize)> {
    let all: Vec<&str> = output.trim_end().lines().collect();
    if all.last().copied() != Some("[exit: 0]") || limit == 0 {
        return None;
    }
    let stderr = all.iter().position(|line| *line == "[stderr]")?;
    if !all[..stderr].iter().any(|line| !line.trim().is_empty()) {
        return None;
    }
    let diagnostics = &all[stderr + 1..all.len() - 1];
    let warnings = diagnostics
        .iter()
        .any(|line| line.contains("Warning:") || line.contains("warnings.warn("));
    let errors = diagnostics.iter().any(|line| {
        line.split_whitespace().any(|word| {
            let word = word.to_ascii_lowercase();
            word.split(':').next().is_some_and(|level| {
                level.ends_with("error") || level.ends_with("exception") || level == "fatal"
            }) || word == "traceback"
        })
    });
    if !warnings || errors {
        return None;
    }
    let mut preferred = all[..stderr].to_vec();
    preferred.push("[exit: 0]");
    let shown = &preferred[preferred.len().saturating_sub(limit)..];
    Some((shown.join("\n"), all.len() - shown.len()))
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

/// The tail of a spooled `bash` result, within both budgets.
///
/// The byte cut lands on a line boundary where one exists, so the preview opens
/// where a line opens rather than mid-token. Snapping is skipped when it would
/// spend most of the budget on alignment — a single line longer than the budget
/// has no useful boundary, and dropping it would answer with a marker and
/// nothing to read — in which case the cut falls back to a character boundary.
fn tail_preview(text: &str, max_bytes: usize, max_lines: usize) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let start = lines.len().saturating_sub(max_lines);
    let mut preview = lines[start..].join("\n");
    if preview.len() > max_bytes {
        let cut = tail_cut(&preview, max_bytes);
        preview = preview[cut..].to_string();
    }
    preview
}

/// Where to start a tail preview that must lose `text.len() - max_bytes` bytes:
/// the next line boundary when that still keeps most of the budget, else a
/// character boundary at the minimum cut.
fn tail_cut(text: &str, max_bytes: usize) -> usize {
    let least = text.len() - max_bytes;
    if let Some(offset) = text.as_bytes()[least..]
        .iter()
        .position(|byte| *byte == b'\n')
    {
        let cut = least + offset + 1;
        if text.len() - cut >= max_bytes / 2 {
            return cut;
        }
    }
    let mut cut = least;
    while !text.is_char_boundary(cut) {
        cut += 1;
    }
    cut
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

/// Which end of an over-long result is worth keeping.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Keep {
    /// `bash`: errors and the exit code are at the end.
    Tail,
    /// A file read or a listing: the beginning is the answer.
    Head,
    /// An MCP result: the summary is at the top and the detail at the bottom,
    /// so half the budget is taken from each end rather than discarding one.
    Middle,
}

/// Cap tool output so a single result cannot dominate the context window. The
/// preview uses a tool-specific line and byte budget, kept from the end
/// [`Keep`] names for that tool, and a server's JSON answer is elided on record
/// boundaries rather than at a byte offset. When content is dropped, the full
/// text is saved under the Oxide config dir and the result points at it so the
/// model can inspect the full output without re-running the tool.
fn truncate(name: &str, output: String) -> String {
    truncate_into(name, output, truncation_dir().as_deref())
}

fn truncate_into(name: &str, output: String, dir: Option<&Path>) -> String {
    let (max_bytes, max_lines) = output_limits(name);
    let lines: Vec<&str> = output.lines().collect();
    if output.len() <= max_bytes && lines.len() <= max_lines {
        return output;
    }

    if is_mcp_tool(name) {
        if let Some(elision) = elide_records(&output, max_bytes) {
            if elision.clause.is_empty() {
                return elision.text;
            }
            let mut result = format!("[truncated: {}", elision.clause);
            if let Some(path) = dir.and_then(|dir| save_truncated(dir, &output)) {
                result.push_str(&format!("; full: {}", path.display()));
            }
            result.push_str("]\n");
            result.push_str(&elision.text);
            return result;
        }
    }

    let keep = keep_end(name);
    let (preview, dropped_lines, dropped_bytes) = match keep {
        Keep::Middle => middle_preview(&lines, max_bytes, max_lines),
        Keep::Tail | Keep::Head => end_preview(&lines, max_bytes, max_lines, keep == Keep::Tail),
    };

    let mut result = format!("[truncated: {dropped_lines} lines, {dropped_bytes} bytes");
    if let Some(path) = dir.and_then(|dir| save_truncated(dir, &output)) {
        result.push_str(&format!("; full: {}", path.display()));
    }
    result.push_str("]\n");
    result.push_str(&preview);
    result
}

/// Keeps one end of `lines` within both budgets, returning the preview and how
/// many lines and bytes it left out.
///
/// The byte cut lands on a line boundary where one exists, so a preview never
/// opens or closes with a fragment of a line the model cannot see the rest of.
/// Snapping is skipped when it would spend most of the budget on alignment: a
/// line longer than the budget has no useful boundary, and dropping it would
/// answer with a marker and nothing to read.
fn end_preview(
    lines: &[&str],
    max_bytes: usize,
    max_lines: usize,
    tail: bool,
) -> (String, usize, usize) {
    let keep = max_lines.min(lines.len());
    let start = if tail { lines.len() - keep } else { 0 };
    let mut preview = lines[start..start + keep].join("\n");
    let mut dropped_lines = lines.len() - keep;
    let mut dropped_bytes = 0;
    if preview.len() > max_bytes {
        if tail {
            let cut = tail_cut(&preview, max_bytes);
            dropped_bytes = cut;
            dropped_lines += preview[..cut].matches('\n').count();
            preview = preview[cut..].to_string();
        } else {
            let cut = head_cut(&preview, max_bytes);
            dropped_bytes = preview.len() - cut;
            dropped_lines += preview[cut..].matches('\n').count();
            preview.truncate(cut);
        }
    }
    (preview, dropped_lines, dropped_bytes)
}

/// Where to end a head preview held to `max_bytes`: the last line boundary
/// within budget when that still keeps most of it, else a character boundary
/// at the budget.
fn head_cut(text: &str, max_bytes: usize) -> usize {
    if let Some(offset) = text.as_bytes()[..max_bytes]
        .iter()
        .rposition(|byte| *byte == b'\n')
    {
        if offset >= max_bytes / 2 {
            return offset;
        }
    }
    let mut cut = max_bytes;
    while !text.is_char_boundary(cut) {
        cut -= 1;
    }
    cut
}

/// Keeps the head and the tail, half of each budget apiece, with a marker
/// naming what the middle left out. A server's result is usually a summary
/// followed by its detail and totals, and keeping one end alone discards the
/// half that answers the call.
fn middle_preview(lines: &[&str], max_bytes: usize, max_lines: usize) -> (String, usize, usize) {
    let (head_lines, tail_lines, dropped_lines) = if lines.len() > max_lines {
        let head = max_lines / 2;
        let tail = max_lines - head;
        (head, tail, lines.len() - max_lines)
    } else {
        (lines.len(), 0, 0)
    };

    let head_text = lines[..head_lines].join("\n");
    let tail_text = if tail_lines > 0 {
        lines[lines.len() - tail_lines..].join("\n")
    } else {
        String::new()
    };

    // No lines dropped and all text fits: keep as-is.
    if dropped_lines == 0 && head_text.len() + tail_text.len() <= max_bytes {
        return (head_text, 0, 0);
    }

    // The byte cut is taken on the text rather than on line boundaries: a
    // server commonly answers with a single long line of JSON, which no
    // line-wise split can divide into a head and a tail. When lines were
    // dropped, cut head and tail separately so the marker sits between them.
    let (head, tail, dropped_bytes) = if dropped_lines > 0 {
        cut_middle_parts(&head_text, &tail_text, max_bytes)
    } else {
        // No lines dropped but bytes exceed: cut the combined text in the middle.
        let text = format!("{}\n{}", head_text, tail_text);
        let (h, t, d) = cut_middle(&text, max_bytes);
        (h.to_string(), t.to_string(), d)
    };

    if dropped_bytes == 0 && dropped_lines == 0 {
        return (
            if tail_lines > 0 {
                format!("{}\n{}", head, tail)
            } else {
                head
            },
            0,
            0,
        );
    }

    let marker = format!("\n…{dropped_lines} lines, {dropped_bytes} bytes truncated…\n");
    (
        format!("{head}{marker}{tail}"),
        dropped_lines,
        dropped_bytes,
    )
}

/// Cuts the head and tail of two separate parts to fit a byte budget,
/// preserving the separation so the marker appears between them. The head gets
/// three-quarters of the budget and the tail gets one-quarter (a server's
/// summary matters more than its footer), cutting on character boundaries.
fn cut_middle_parts(head_text: &str, tail_text: &str, max_bytes: usize) -> (String, String, usize) {
    let head_budget = (max_bytes * 3) / 4;
    let tail_budget = max_bytes - head_budget;

    let head = if head_text.len() <= head_budget {
        head_text.to_string()
    } else {
        let mut cut = head_budget;
        while cut > 0 && !head_text.is_char_boundary(cut) {
            cut -= 1;
        }
        head_text[..cut].to_string()
    };

    let tail = if tail_text.len() <= tail_budget {
        tail_text.to_string()
    } else {
        let mut start = tail_text.len() - tail_budget;
        while start < tail_text.len() && !tail_text.is_char_boundary(start) {
            start += 1;
        }
        tail_text[start..].to_string()
    };

    let dropped = head_text.len() + tail_text.len() - head.len() - tail.len();
    (head, tail, dropped)
}

/// Splits `text` into the head and tail that fit `max_bytes` between them,
/// cutting on character boundaries, and reports the bytes dropped between.
fn cut_middle(text: &str, max_bytes: usize) -> (&str, &str, usize) {
    if text.len() <= max_bytes {
        return (text, "", 0);
    }
    let mut end = max_bytes / 2;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    let mut start = text.len() - (max_bytes - max_bytes / 2);
    while start < text.len() && !text.is_char_boundary(start) {
        start += 1;
    }
    if start <= end {
        return (text, "", 0);
    }
    (&text[..end], &text[start..], start - end)
}

/// Which end of an MCP result's JSON answer to elide around. Returned by
/// [`elide_records`].
struct RecordElision {
    /// The `[truncated: …]` clause naming what the document left out, empty when
    /// every record survived and only whitespace was dropped.
    clause: String,
    /// The document that stands in for the whole one.
    text: String,
}

/// One step along the path to an array inside a parsed document.
#[derive(Clone)]
enum Step {
    Key(String),
    Index(usize),
}

/// Elides a JSON document at its largest array, keeping the records from both
/// ends that fit `max_bytes` and naming the ones between them.
///
/// A server answers with one document, so cutting it at a byte offset leaves a
/// fragment that parses as nothing: it starts and ends mid-token, carries no
/// record count, and says nothing about how much of the answer is missing. A
/// model that cannot tell how much it is looking at narrows the question and
/// pays for the answer a second time, which costs far more than the elision
/// saves. Cutting on record boundaries keeps the envelope, both ends of the list
/// and the number left out.
///
/// `None` when the text is not JSON, holds no array, or cannot be brought inside
/// the budget with one record kept.
fn elide_records(text: &str, max_bytes: usize) -> Option<RecordElision> {
    let original = text.len();
    let value: Value = serde_json::from_str(text).ok()?;
    let (path, total) = largest_array(&value)?;
    if total == 0 {
        return None;
    }

    // A document that is over budget only because the server pretty-printed it
    // comes back whole and compact, with nothing dropped and no marker. It is
    // checked before the search rather than by it: dropping the marker is what
    // makes the last candidate smaller than the one below it, so a search that
    // still had `total` in range could walk past the answer and down to zero.
    let whole = elided(&value, &path, total)?;
    if whole.len() <= max_bytes {
        return Some(RecordElision {
            clause: String::new(),
            text: whole,
        });
    }

    // The most records that still fit. Every candidate here keeps a record out
    // and puts the marker in, so the length only grows with the count the search
    // walks up to.
    let (mut low, mut high) = (0, total - 1);
    while low < high {
        let mid = low + (high - low).div_ceil(2);
        if elided(&value, &path, mid)?.len() <= max_bytes {
            low = mid;
        } else {
            high = mid - 1;
        }
    }
    if low == 0 {
        return None;
    }

    let text = elided(&value, &path, low)?;
    Some(RecordElision {
        clause: format!(
            "{} of {} records omitted, {} bytes",
            total - low,
            total,
            original.saturating_sub(text.len())
        ),
        text,
    })
}

/// The document with `keep` of the array's records kept — three quarters from
/// the front, the rest from the back — and a marker between them naming what was
/// left out. The marker is a string element, so what comes back is still the
/// same document rather than a fragment of one.
fn elided(value: &Value, path: &[Step], keep: usize) -> Option<String> {
    let mut document = value.clone();
    let items = stepped(&mut document, path)?.as_array_mut()?;
    let total = items.len();
    let head = (keep * 3).div_ceil(4);
    let tail = keep - head;

    let mut kept: Vec<Value> = Vec::with_capacity(keep + 1);
    kept.extend_from_slice(&items[..head]);
    if keep < total {
        kept.push(Value::String(format!(
            "… {} of {total} records omitted …",
            total - keep
        )));
    }
    kept.extend_from_slice(&items[total - tail..]);
    *items = kept;

    serde_json::to_string(&document).ok()
}

/// The largest array in a document and how many records it holds, by the bytes
/// it takes to serialize.
fn largest_array(value: &Value) -> Option<(Vec<Step>, usize)> {
    fn walk(value: &Value, path: &mut Vec<Step>, best: &mut (usize, Option<Vec<Step>>, usize)) {
        match value {
            Value::Array(items) => {
                let size = serde_json::to_string(items)
                    .map(|text| text.len())
                    .unwrap_or(0);
                if best.1.is_none() || size > best.0 {
                    best.0 = size;
                    best.1 = Some(path.clone());
                    best.2 = items.len();
                }
                for (index, item) in items.iter().enumerate() {
                    path.push(Step::Index(index));
                    walk(item, path, best);
                    path.pop();
                }
            }
            Value::Object(map) => {
                for (key, item) in map {
                    path.push(Step::Key(key.clone()));
                    walk(item, path, best);
                    path.pop();
                }
            }
            _ => {}
        }
    }

    let mut best = (0, None, 0);
    walk(value, &mut Vec::new(), &mut best);
    best.1.map(|path| (path, best.2))
}

/// Follows a [`Step`] path into a parsed document.
fn stepped<'a>(value: &'a mut Value, path: &[Step]) -> Option<&'a mut Value> {
    let mut node = value;
    for step in path {
        node = match step {
            Step::Key(key) => node.as_object_mut()?.get_mut(key)?,
            Step::Index(index) => node.as_array_mut()?.get_mut(*index)?,
        };
    }
    Some(node)
}

/// Which end of a tool's output to keep. An MCP tool is named
/// `<server>__<tool>` by `mcp::expose`, and no built-in name holds `__`.
fn keep_end(name: &str) -> Keep {
    match name {
        "bash" => Keep::Tail,
        _ if is_mcp_tool(name) => Keep::Middle,
        _ => Keep::Head,
    }
}

fn is_mcp_tool(name: &str) -> bool {
    name.contains("__")
}

fn output_limits(name: &str) -> (usize, usize) {
    match name {
        "bash" => (MAX_OUTPUT_BYTES, MAX_OUTPUT_LINES),
        "grep" | "find" | "ls" => (MAX_OUTPUT_BYTES, MAX_OUTPUT_LINES),
        "webfetch" => (MAX_OUTPUT_BYTES, MAX_OUTPUT_LINES),
        // A write or an edit answers with a confirmation and a diff the model
        // already knows; only the built-ins whose output it must reason from
        // need the full budget.
        "write" | "edit" => (3_000, 120),
        // One extra line for the `… more lines; use offset=` pointer, so a
        // full page of `read` output is never trimmed by the central cap.
        "read" => (READ_MAX_BYTES, READ_MAX_LINES + 1),
        // A server's result is structured data, not a file: it needs more room
        // than the generic cap, and it is the one output a reader cannot
        // narrow with `offset`/`limit` or a tighter query.
        _ if is_mcp_tool(name) => (MCP_OUTPUT_BYTES, MCP_OUTPUT_LINES),
        _ => (MAX_OUTPUT_BYTES, MAX_OUTPUT_LINES),
    }
}

fn truncation_dir() -> Option<PathBuf> {
    if let Ok(dir) = std::env::var("OXIDE_TRUNCATION_DIR") {
        return Some(PathBuf::from(dir));
    }
    Some(crate::config::config_dir()?.join("truncated"))
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
            signature: None,
            function: FunctionCall {
                name: name.to_string(),
                arguments: args.to_string(),
            },
        }
    }

    // `awk` is the generator here, so these two drive the shell path on Unix
    // only; the cut itself is covered on every platform by the `end_preview`
    // and `tail_preview` tests above, which need no child process.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_real_oversized_diff_preview_opens_on_a_line() {
        // The shape that exposed this: `gh pr diff | head`, whose 270 KB of
        // patch text is spooled and previewed from its tail. Driven through
        // the public tool path with a diff generated here, so the assertion is
        // about the preview the model receives rather than about an internal.
        let dir = std::env::temp_dir().join(format!("oxide_real_diff_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mcp = McpRegistry::default();
        let progress = Progress::default();

        // A patch's own line prefixes are what a reader needs intact.
        let command = "awk 'BEGIN { \
            for (i = 0; i < 2000; i++) { \
                printf \"@@ -%d,7 +%d,7 @@ fn region_%d\\n\", i, i, i; \
                printf \"-    const previous = readTheOldValue(%d);\\n\", i; \
                printf \"+    const next = readTheReplacementValue(%d);\\n\", i; \
            } }'";

        let out = execute(
            &call("bash", json!({ "command": command })),
            &dir,
            &mcp,
            &progress,
        )
        .await;

        assert!(out.text.contains("[truncated:"), "expected a capped result");
        let body = out.text.split_once("]\n").expect("a marker").1;
        let first = body.lines().next().expect("a preview keeps lines");
        assert!(
            first.starts_with("@@ ") || first.starts_with('-') || first.starts_with('+'),
            "a diff preview must open on a patch line, not mid-line: {first:?}"
        );
        // The exit code still has to survive at the other end.
        assert!(out.text.trim_end().ends_with("[exit: 0]"), "{}", out.text);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_spooled_tail_keeps_whole_lines() {
        let mut text = String::new();
        for i in 0..400 {
            text.push_str(&format!("line {i}: {}\n", "payload ".repeat(40)));
        }
        text.push_str("[exit: 2]");

        let preview = tail_preview(&text, MAX_OUTPUT_BYTES, MAX_OUTPUT_LINES);
        assert!(preview.ends_with("[exit: 2]"), "{preview}");
        for line in preview.lines() {
            assert!(
                line.is_empty() || line.starts_with("line ") || line.starts_with("[exit:"),
                "a spooled tail must not begin mid-line: {line:?}"
            );
        }

        // One line with no boundary to snap to still yields readable content.
        let huge = format!("{}\n[exit: 0]", "y".repeat(MAX_OUTPUT_BYTES * 2));
        let preview = tail_preview(&huge, MAX_OUTPUT_BYTES, MAX_OUTPUT_LINES);
        assert!(preview.ends_with("[exit: 0]"), "{}", preview.len());
        assert!(preview.contains('y'), "a tail must keep readable content");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn an_oversized_bash_result_keeps_whole_lines() {
        // The end-to-end path: real command output over the budget, capped by
        // `execute` the way a model receives it. A diff is the shape that
        // exposed the mid-line cut, since its lines are long and its leading
        // characters (`+`/`-`/`@@`) are what a reader needs to tell apart.
        let dir = std::env::temp_dir().join(format!("oxide_bash_lines_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mcp = McpRegistry::default();
        let progress = Progress::default();

        let out = execute(
            &call(
                "bash",
                json!({
                    "command": "awk 'BEGIN { for (i = 0; i < 1200; i++) \
                        printf \"+line %d: %s\\n\", i, \
                        \"const value = computeSomethingRatherLong(argument);\" }'"
                }),
            ),
            &dir,
            &mcp,
            &progress,
        )
        .await;

        assert!(out.text.contains("[truncated:"), "{}", out.text);
        let body = out.text.split_once("]\n").expect("a marker").1;
        for line in body.lines() {
            assert!(
                line.is_empty() || line.starts_with("+line ") || line.starts_with("[exit:"),
                "an oversized bash result must keep whole lines: {line:?}"
            );
        }

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn bash_previews_prioritize_results_over_successful_command_warnings() {
        let output = "POST http://service/v1/copy\nCreated: 6\nFailed: 0\n[stderr]\nrequests/__init__.py:113: RequestsDependencyWarning: unsupported dependency\n  warnings.warn(\n[exit: 0]";
        let (preview, more) = bash_warning_preview(output, 3).unwrap();
        assert_eq!(preview, "Created: 6\nFailed: 0\n[exit: 0]");
        assert_eq!(more, 4);
        assert!(bash_warning_preview(&output.replace("[exit: 0]", "[exit: 1]"), 3).is_none());
        assert!(bash_warning_preview(
            &output.replace("  warnings.warn(", "Traceback (most recent call last):"),
            3
        )
        .is_none());
        assert!(bash_warning_preview(
            &output.replace("  warnings.warn(", "ERROR:root:submission failed"),
            3
        )
        .is_none());
        assert!(bash_warning_preview("[stderr]\nRuntimeWarning: warning\n[exit: 0]", 3).is_none());
        assert!(bash_warning_preview(
            &output.replace("  warnings.warn(", "ValueError: bad payload"),
            3
        )
        .is_none());
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
    fn search_display_leads_with_pattern_and_recognizes_only_exact_no_hit_results() {
        let args = r#"{"path":"/work/falcon","pattern":"generateUrl|ums-generation"}"#;
        assert_eq!(
            search_tool_summary("grep", args).as_deref(),
            Some(r#""generateUrl|ums-generation" · /work/falcon"#)
        );
        assert_eq!(
            search_tool_summary("find", r#"{"pattern":"**/*.rs"}"#).as_deref(),
            Some(r#""**/*.rs""#)
        );
        assert!(is_empty_search_result("grep", NO_MATCHES));
        assert!(is_empty_search_result("find", NO_FILES_FOUND));
        assert!(!is_empty_search_result("read", NO_MATCHES));
        assert!(!is_empty_search_result(
            "grep",
            "No matches found\nadditional detail"
        ));
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
    fn project_only_specs_do_not_advertise_cross_project_paths() {
        let specs = specs_scoped(&McpRegistry::default(), true);
        let descriptions = specs
            .iter()
            .map(|spec| spec.function.description.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(descriptions.contains("current project"), "{descriptions}");
        assert!(!descriptions.contains("outside the project are allowed"));
    }

    #[test]
    fn project_boundary_refuses_parent_and_absolute_sibling_paths() {
        let base = std::env::temp_dir().join(format!(
            "oxide_project_boundary_{}_{}",
            std::process::id(),
            TRUNCATION_ID.fetch_add(1, Ordering::Relaxed)
        ));
        let project = base.join("project");
        let sibling = base.join("sibling");
        std::fs::create_dir_all(&project).unwrap();
        std::fs::create_dir_all(&sibling).unwrap();
        std::fs::write(project.join("inside.txt"), "inside").unwrap();
        std::fs::write(sibling.join("outside.txt"), "outside").unwrap();

        assert!(ensure_project_path(&project, "inside.txt").is_ok());
        assert!(ensure_project_path(&project, "new/nested.txt").is_ok());
        assert!(ensure_project_path(&project, "../sibling/outside.txt").is_err());
        assert!(ensure_project_path(&project, sibling.to_str().unwrap()).is_err());
        assert!(enforce_scoped_shell(&project, "cargo test", PathScope::CurrentProject).is_ok());
        assert!(enforce_scoped_shell(
            &project,
            &format!("cd {} && git status", sibling.display()),
            PathScope::CurrentProject
        )
        .is_err());
        assert!(enforce_scoped_shell(
            &project,
            &format!(
                "python3 -c \"open('{}')\"",
                sibling.join("outside.txt").display()
            ),
            PathScope::CurrentProject
        )
        .is_err());

        std::fs::remove_dir_all(base).ok();
    }

    #[test]
    fn desktop_scope_allows_registered_projects_and_refuses_other_paths() {
        let base = std::env::temp_dir().join(format!(
            "oxide_registered_boundary_{}_{}",
            std::process::id(),
            TRUNCATION_ID.fetch_add(1, Ordering::Relaxed)
        ));
        let current = base.join("current");
        let registered = base.join("registered");
        let outside = base.join("outside");
        for path in [&current, &registered, &outside] {
            std::fs::create_dir_all(path).unwrap();
        }
        let roots = vec![current.clone(), registered.clone()];
        let scope = PathScope::RegisteredProjects(&roots);

        assert!(ensure_scoped_path(&current, "inside.txt", scope).is_ok());
        assert!(ensure_scoped_path(&current, registered.to_str().unwrap(), scope).is_ok());
        assert!(ensure_scoped_path(&current, outside.to_str().unwrap(), scope).is_err());
        assert!(enforce_scoped_shell(
            &current,
            &format!("git -C {} status", registered.display()),
            scope
        )
        .is_ok());
        assert!(enforce_scoped_shell(
            &current,
            &format!("git -C {} status", outside.display()),
            scope
        )
        .is_err());

        let descriptions = specs_for_scope(&McpRegistry::default(), scope)
            .into_iter()
            .map(|spec| spec.function.description)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            descriptions.contains("registered in Oxide"),
            "{descriptions}"
        );
        assert!(!descriptions.contains("outside the project are allowed"));

        std::fs::remove_dir_all(base).ok();
    }

    #[test]
    fn project_boundary_treats_quoted_pr_heredoc_as_text_but_checks_executable_paths() {
        let cwd = std::env::current_dir().unwrap();
        let secret = cwd.parent().unwrap().join("outside/secret");
        let secret = secret.to_string_lossy().replace('\\', "/");
        let quoted = "gh pr create --body \"$(cat <<'EOF'\n### Screenshot / GIF\n$HOME /outside/file $(cat /outside/secret) `cat /outside/secret`\nEOF\n)\"".replace("/outside/secret", &secret);
        assert!(enforce_scoped_shell(&cwd, &quoted, PathScope::CurrentProject).is_ok());
        for command in [
            quoted.replace("<<'EOF'", "<<EOF"),
            quoted.replace("gh pr create --body", "python -c"),
            quoted.replace("EOF\n)\"", &format!("EOF\ncat {secret}\n)\"")),
            format!("{quoted} && cat {secret}"),
            format!("python <<'EOF'\nopen('{secret}')\nEOF"),
        ] {
            assert!(
                enforce_scoped_shell(&cwd, &command, PathScope::CurrentProject).is_err(),
                "{command}"
            );
        }
    }

    #[test]
    fn embedded_shell_paths_recognize_unix_and_windows_roots() {
        assert_eq!(
            embedded_absolute_paths(
                "python -c \"open('/private/project/a')\" && python -c \"open('D:\\\\work\\\\other\\\\b')\""
            ),
            vec!["/private/project/a", r"D:\\work\\other\\b"]
        );
        assert!(embedded_absolute_paths("curl https://example.com/a").is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn project_boundary_refuses_a_symlink_into_a_sibling() {
        use std::os::unix::fs::symlink;

        let base = std::env::temp_dir().join(format!(
            "oxide_project_boundary_link_{}_{}",
            std::process::id(),
            TRUNCATION_ID.fetch_add(1, Ordering::Relaxed)
        ));
        let project = base.join("project");
        let sibling = base.join("sibling");
        std::fs::create_dir_all(&project).unwrap();
        std::fs::create_dir_all(&sibling).unwrap();
        std::fs::write(sibling.join("outside.txt"), "outside").unwrap();
        symlink(&sibling, project.join("linked")).unwrap();

        assert!(ensure_project_path(&project, "linked/outside.txt").is_err());

        std::fs::remove_dir_all(base).ok();
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
            .contains("(also answers to: confluence, jira)"));
        assert!(loader
            .function
            .description
            .contains("https://mcp.atlassian.com/v1/mcp"));
        assert_eq!(
            loader.function.parameters["properties"]["server"]["enum"][0],
            "atlassian"
        );
        // An advertised alias has to be a value `mcp_load` accepts, not just a
        // hint in the description.
        let names: Vec<&str> = loader.function.parameters["properties"]["server"]["enum"]
            .as_array()
            .unwrap()
            .iter()
            .map(|value| value.as_str().unwrap())
            .collect();
        assert_eq!(names, vec!["atlassian", "confluence", "jira"]);
        assert_eq!(mcp.server_count(), 0);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_refused_read_names_the_grant_to_give_back() {
        use std::os::unix::fs::PermissionsExt;

        let dir = std::env::temp_dir().join(format!("oxide_read_denied_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("notes.txt");
        std::fs::write(&file, b"hello").unwrap();
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o000)).unwrap();
        // Root reads a file whatever its mode says, and a runner that does so
        // has nothing to assert.
        if std::fs::read(&file).is_ok() {
            std::fs::remove_dir_all(&dir).ok();
            return;
        }

        let out = execute(
            &call("read", json!({ "path": "notes.txt" })),
            &dir,
            &McpRegistry::default(),
            &Progress::default(),
        )
        .await;
        assert!(out.is_error, "{}", out.text);
        assert!(out.text.starts_with("reading "), "{}", out.text);
        assert!(out.text.contains("may not read that file"), "{}", out.text);
        // The app to allow is named by the platform's own settings pane, so
        // only macOS has that half of the hint to assert.
        #[cfg(target_os = "macos")]
        {
            assert!(out.text.contains("System Settings"), "{}", out.text);
        }
        #[cfg(not(target_os = "macos"))]
        {
            assert!(out.text.contains("check its permissions"), "{}", out.text);
        }

        std::fs::remove_dir_all(&dir).ok();
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
        assert!(out.text.contains("(lines 1, 2)"), "{}", out.text);
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
    async fn edit_tolerates_trailing_whitespace_and_copied_line_numbers() {
        let dir = std::env::temp_dir().join(format!("oxide_edit_loose_{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        let mcp = McpRegistry::default();
        let progress = Progress::default();

        execute(
            &call(
                "write",
                json!({ "path": "f.rs", "content": "fn main() {\n    let x = 1;   \n    let y = 2;\n}\n" }),
            ),
            &dir,
            &mcp,
            &progress,
        )
        .await;
        // The file has trailing spaces the model cannot see; the edit still
        // lands, and the replacement keeps its original indentation.
        let out = execute(
            &call(
                "edit",
                json!({
                    "path": "f.rs",
                    "edits": [{
                        "oldText": "    let x = 1;\n    let y = 2;",
                        "newText": "    let x = 9;\n    let y = 9;"
                    }]
                }),
            ),
            &dir,
            &mcp,
            &progress,
        )
        .await;
        assert!(out.text.contains("1 block(s)"), "{}", out.text);
        assert_eq!(
            std::fs::read_to_string(dir.join("f.rs")).unwrap(),
            "fn main() {\n    let x = 9;\n    let y = 9;\n}\n"
        );

        // A block copied from a numbered `read` result has its prefixes
        // stripped from both the old and the replacement text.
        std::fs::write(dir.join("g.rs"), "alpha\nbeta\ngamma\n").unwrap();
        let out = execute(
            &call(
                "edit",
                json!({
                    "path": "g.rs",
                    "edits": [{
                        "oldText": "1|alpha\n2|beta",
                        "newText": "1|ALPHA\n2|BETA"
                    }]
                }),
            ),
            &dir,
            &mcp,
            &progress,
        )
        .await;
        assert!(out.text.contains("1 block(s)"), "{}", out.text);
        assert_eq!(
            std::fs::read_to_string(dir.join("g.rs")).unwrap(),
            "ALPHA\nBETA\ngamma\n"
        );

        // A single numbered line is stripped too, since `read` numbers its
        // first line as `1|`.
        std::fs::write(dir.join("h.rs"), "let x = 1;\nlet y = 2;\n").unwrap();
        let out = execute(
            &call(
                "edit",
                json!({
                    "path": "h.rs",
                    "edits": [{ "oldText": "2|let y = 2;", "newText": "2|let y = 3;" }]
                }),
            ),
            &dir,
            &mcp,
            &progress,
        )
        .await;
        assert!(out.text.contains("1 block(s)"), "{}", out.text);
        assert_eq!(
            std::fs::read_to_string(dir.join("h.rs")).unwrap(),
            "let x = 1;\nlet y = 3;\n"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn copied_read_continuations_rejoin_physical_lines() {
        let numbered = "8|first\n8+| continuation\n8+| end\n9|next\n";
        let expected = "first continuation end\nnext\n";
        assert_eq!(strip_line_prefixes(numbered).as_deref(), Some(expected));
        assert_eq!(strip_all_line_prefixes(numbered), expected);
        assert_eq!(
            strip_line_prefixes("8+|partial\n9|next").as_deref(),
            Some("partial\nnext")
        );
        for invalid in ["8|first\n8|next", "8|first\n9+|next", "8|first\n7|next"] {
            assert!(strip_line_prefixes(invalid).is_none(), "{invalid}");
        }
    }

    #[test]
    fn edit_round_trips_long_unicode_lines_from_read() {
        let dir = std::env::temp_dir().join(format!("oxide_edit_chunks_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let original = format!("start {} end\nnext line\n", "界".repeat(MAX_LINE_LEN * 2));
        let replacement = format!("changed {} end\nnext line\n", "新".repeat(MAX_LINE_LEN * 2));
        std::fs::write(dir.join("original.txt"), &original).unwrap();
        std::fs::write(dir.join("replacement.txt"), &replacement).unwrap();
        let old = read(&dir, &json!({ "path": "original.txt" })).unwrap().text;
        let new = read(&dir, &json!({ "path": "replacement.txt" }))
            .unwrap()
            .text;
        assert!(old.contains("1+|"));
        let output = edit(
            &dir,
            &json!({
                "path": "original.txt", "edits": [{ "oldText": old, "newText": new }]
            }),
        )
        .unwrap();
        assert!(!output.is_error);
        assert_eq!(
            std::fs::read_to_string(dir.join("original.txt")).unwrap(),
            replacement
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn edit_accepts_file_path_and_typographic_variants() {
        let dir = std::env::temp_dir().join(format!("oxide_edit_alias_{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        let mcp = McpRegistry::default();
        let progress = Progress::default();

        // The file holds smart quotes and an em dash; the model retyped the
        // block with ASCII characters and spelled the path `file_path`.
        std::fs::write(
            dir.join("a.txt"),
            "He said \u{201c}hello\u{201d} \u{2014} today\n",
        )
        .unwrap();
        let out = execute(
            &call(
                "edit",
                json!({
                    "file_path": "a.txt",
                    "edits": [{
                        "oldText": "He said \"hello\" - today",
                        "newText": "She said \"hi\" - today"
                    }]
                }),
            ),
            &dir,
            &mcp,
            &progress,
        )
        .await;
        assert!(!out.is_error, "{}", out.text);
        assert_eq!(
            std::fs::read_to_string(dir.join("a.txt")).unwrap(),
            "She said \"hi\" - today\n"
        );

        // Some providers put the path on each replacement and omit the
        // top-level field. Recover the call when all replacements agree.
        std::fs::write(dir.join("b.txt"), "before\n").unwrap();
        let out = execute(
            &call(
                "edit",
                json!({
                    "edits": [{
                        "file_path": "b.txt",
                        "oldText": "before",
                        "newText": "after"
                    }]
                }),
            ),
            &dir,
            &mcp,
            &progress,
        )
        .await;
        assert!(!out.is_error, "{}", out.text);
        assert_eq!(
            std::fs::read_to_string(dir.join("b.txt")).unwrap(),
            "after\n"
        );

        let out = execute(
            &call(
                "edit",
                json!({
                    "edits": [
                        { "path": "a.txt", "oldText": "one", "newText": "ONE" },
                        { "path": "b.txt", "oldText": "two", "newText": "TWO" }
                    ]
                }),
            ),
            &dir,
            &mcp,
            &progress,
        )
        .await;
        assert!(out.is_error);
        assert!(out.text.contains("multiple files"), "{}", out.text);

        // A wrong `path` is still named so the model can fix it.
        let out = execute(&call("edit", json!({ "edits": [] })), &dir, &mcp, &progress).await;
        assert!(out.is_error);
        assert!(
            out.text.contains("edit requires a `path` string"),
            "{}",
            out.text
        );
        assert!(out.text.contains("received keys: edits"), "{}", out.text);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn edit_failure_names_the_closest_region_and_collects_every_edit() {
        let dir = std::env::temp_dir().join(format!("oxide_edit_hint_{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        let mcp = McpRegistry::default();
        let progress = Progress::default();

        execute(
            &call(
                "write",
                json!({ "path": "f.rs", "content": "fn main() {\n    let x = 1;\n    let y = 2;\n}\n" }),
            ),
            &dir,
            &mcp,
            &progress,
        )
        .await;

        // Both edits are stale. The result names the region closest to each so
        // the next attempt can copy it exactly, and nothing is written.
        let out = execute(
            &call(
                "edit",
                json!({
                    "path": "f.rs",
                    "edits": [
                        { "oldText": "    let x = 1;\n    let z = 9;", "newText": "    let x = 9;" },
                        { "oldText": "    let q = 7;", "newText": "    let q = 8;" }
                    ]
                }),
            ),
            &dir,
            &mcp,
            &progress,
        )
        .await;
        assert!(out.text.contains("edits[0]"), "{}", out.text);
        assert!(out.text.contains("edits[1]"), "{}", out.text);
        assert!(
            out.text.contains("The closest text (lines 2-3)"),
            "{}",
            out.text
        );
        assert!(out.text.contains("    let x = 1;"), "{}", out.text);
        assert!(
            out.text.contains("entire edit batch was rejected"),
            "{}",
            out.text
        );
        assert!(out.text.contains("resend the entire batch"), "{}", out.text);
        assert_eq!(
            std::fs::read_to_string(dir.join("f.rs")).unwrap(),
            "fn main() {\n    let x = 1;\n    let y = 2;\n}\n"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn closest_edit_hint_requires_shared_content() {
        let english = "---\ntitle: Agent\n---\n\n# How to Build an AI Coding Agent Like Pi\n\nEnglish paragraph\n";
        let chinese =
            "## 加入基础工具（read/write/bash……）\n\n工具是 Agent 停止空谈、开始改动你仓库的地方。";
        assert!(closest_region(&source_lines(english), &source_lines(chinese)).is_none());
        assert!(closest_region(&source_lines("{\n}\n"), &source_lines("{\nmissing\n}")).is_none());
        let hint = closest_region(
            &source_lines("unrelated\n\nHe said “hello” — today\nold content\n"),
            &source_lines("He said \"hello\" - today\nnew content"),
        )
        .unwrap();
        assert_eq!((hint.0, hint.1), (3, 4));
    }

    #[tokio::test]
    async fn mixed_file_edits_reject_atomically_then_recover_as_separate_calls() {
        let dir = std::env::temp_dir().join(format!("oxide_edit_languages_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let english =
            "---\n\n# Build an Agent\n\n## Add basic tools\n\nTools bridge text and the world.\n";
        let chinese = "## 加入基础工具\n\n工具连接文本与世界。\n";
        std::fs::write(dir.join("en.md"), english).unwrap();
        std::fs::write(dir.join("zh.md"), chinese).unwrap();
        let en_edit = json!({ "oldText": "## Add basic tools", "newText": "## Tools explained" });
        let zh_edit = json!({ "oldText": "## 加入基础工具\n\n工具连接文本与世界。", "newText": "## 工具详解\n\n工具连接文本与世界。" });
        let output = execute(
            &call(
                "edit",
                json!({ "path": "en.md", "edits": [en_edit.clone(), zh_edit.clone()] }),
            ),
            &dir,
            &McpRegistry::default(),
            &Progress::default(),
        )
        .await;
        assert!(output.is_error);
        assert!(!output.terminate);
        assert!(output.diff.is_none());
        assert!(output.model_text().starts_with("error:"));
        assert!(output.text.contains("edits[1].oldText"), "{}", output.text);
        assert!(
            output.text.contains("No changes were written"),
            "{}",
            output.text
        );
        assert!(
            output.text.contains("separate edit calls"),
            "{}",
            output.text
        );
        assert!(!output.text.contains("closest text"), "{}", output.text);
        assert_eq!(std::fs::read_to_string(dir.join("en.md")).unwrap(), english);
        assert_eq!(std::fs::read_to_string(dir.join("zh.md")).unwrap(), chinese);
        for (path, replacement) in [("en.md", en_edit), ("zh.md", zh_edit)] {
            edit(&dir, &json!({ "path": path, "edits": [replacement] })).unwrap();
        }
        assert!(std::fs::read_to_string(dir.join("en.md"))
            .unwrap()
            .contains("## Tools explained"));
        assert!(std::fs::read_to_string(dir.join("zh.md"))
            .unwrap()
            .contains("## 工具详解"));
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// A block that looks identical in a transcript can still differ by
    /// whitespace or by an escape, and "copy it exactly" is not advice a reader
    /// can follow when the two renderings look the same. The failure names the
    /// first line that differs, with both sides escaped.
    #[tokio::test]
    async fn edit_names_the_line_that_differs_invisibly() {
        let dir = std::env::temp_dir().join(format!("oxide_edit_differs_{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        let mcp = McpRegistry::default();
        let progress = Progress::default();

        // Tab-indented in the file, space-indented in the block: leading
        // indentation is deliberately not ignored, so the block is stale.
        std::fs::write(dir.join("t.rs"), "fn main() {\n\tlet x = 1;\n}\n").unwrap();
        let out = execute(
            &call(
                "edit",
                json!({
                    "path": "t.rs",
                    "edits": [{ "oldText": "fn main() {\n    let x = 1;\n}", "newText": "x" }]
                }),
            ),
            &dir,
            &mcp,
            &progress,
        )
        .await;
        assert!(out.is_error, "{}", out.text);
        assert!(out.text.contains("Your line 2 differs:"), "{}", out.text);
        // Both sides escaped, so a tab is told apart from four spaces.
        assert!(
            out.text.contains(r#"the file holds "\tlet x = 1;""#),
            "{}",
            out.text
        );
        assert!(
            out.text.contains(r#"the block holds "    let x = 1;""#),
            "{}",
            out.text
        );

        // A `\n` inside a string literal against the newline written instead:
        // the block carries one more line than the file, which is itself the
        // difference worth naming.
        std::fs::write(dir.join("e.rs"), "let s = \"a\\nb\";\n").unwrap();
        let out = execute(
            &call(
                "edit",
                json!({
                    "path": "e.rs",
                    "edits": [{ "oldText": "let s = \"a\nb\";", "newText": "x" }]
                }),
            ),
            &dir,
            &mcp,
            &progress,
        )
        .await;
        assert!(out.is_error, "{}", out.text);
        assert!(out.text.contains("Your line 1 differs:"), "{}", out.text);
        assert!(out.text.contains(r#"a\\nb"#), "{}", out.text);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn edit_batches_match_the_original_and_reject_overlaps() {
        let dir = std::env::temp_dir().join(format!("oxide_edit_batch_{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        let mcp = McpRegistry::default();
        let progress = Progress::default();

        // Applying the first edit would make the second's `oldText` ambiguous,
        // so both are matched against the original and applied together.
        execute(
            &call("write", json!({ "path": "f.rs", "content": "Foo\nBar\n" })),
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
                        { "oldText": "Foo", "newText": "Bar" },
                        { "oldText": "Bar", "newText": "Baz" }
                    ]
                }),
            ),
            &dir,
            &mcp,
            &progress,
        )
        .await;
        assert!(out.text.contains("2 block(s)"), "{}", out.text);
        assert_eq!(
            std::fs::read_to_string(dir.join("f.rs")).unwrap(),
            "Bar\nBaz\n"
        );

        // Overlapping edits are rejected instead of corrupting the file.
        execute(
            &call(
                "write",
                json!({ "path": "g.rs", "content": "let x = 1;\n" }),
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
                    "path": "g.rs",
                    "edits": [
                        { "oldText": "let x = 1;", "newText": "let x = 2;" },
                        { "oldText": "let x", "newText": "let y" }
                    ]
                }),
            ),
            &dir,
            &mcp,
            &progress,
        )
        .await;
        assert!(out.text.contains("overlap"), "{}", out.text);
        assert_eq!(
            std::fs::read_to_string(dir.join("g.rs")).unwrap(),
            "let x = 1;\n"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn edit_validates_noops_without_blocking_real_replacements() {
        let dir = std::env::temp_dir().join(format!("oxide_edit_noop_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("f.rs");
        std::fs::write(&path, "fn run() {\n    work();\n}\n").unwrap();
        let mcp = McpRegistry::default();
        let progress = Progress::default();
        let out = execute(
            &call(
                "edit",
                json!({"path": "f.rs", "edits": [
                    {"oldText": "fn run() {", "newText": "fn run() {"},
                    {"oldText": "fn run() {\n    work();", "newText": "fn run() {\n    finish();"}
                ]}),
            ),
            &dir,
            &mcp,
            &progress,
        )
        .await;
        assert!(!out.is_error, "{}", out.text);
        assert!(out.text.contains("1 block(s)"), "{}", out.text);
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "fn run() {\n    finish();\n}\n"
        );

        let before = std::fs::metadata(&path).unwrap().modified().unwrap();
        let out = execute(
            &call(
                "edit",
                json!({"path": "f.rs", "edits": [
                    {"oldText": "fn run() {", "newText": "fn run() {"}
                ]}),
            ),
            &dir,
            &mcp,
            &progress,
        )
        .await;
        assert!(!out.is_error, "{}", out.text);
        assert!(out.text.contains("0 block(s)"), "{}", out.text);
        assert!(out.diff.is_none());
        assert_eq!(
            std::fs::metadata(&path).unwrap().modified().unwrap(),
            before
        );

        // A stale no-op is still an invalid assertion about the original file.
        let out = execute(
            &call(
                "edit",
                json!({"path": "f.rs", "edits": [
                    {"oldText": "stale", "newText": "stale"},
                    {"oldText": "finish();", "newText": "oops();"}
                ]}),
            ),
            &dir,
            &mcp,
            &progress,
        )
        .await;
        assert!(out.is_error);
        assert!(
            out.text.contains("This replacement is a no-op"),
            "{}",
            out.text
        );
        assert!(!std::fs::read_to_string(&path).unwrap().contains("oops"));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn edit_rejects_overlapping_occurrences_and_prefers_literal_numbered_text() {
        let dir = std::env::temp_dir().join(format!("oxide_edit_exact_{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        let mcp = McpRegistry::default();
        let progress = Progress::default();

        // Both occurrences of `aa` count even though they overlap. Choosing
        // the first one would silently turn `aaa` into `ba`.
        std::fs::write(dir.join("overlap.txt"), "aaa\n").unwrap();
        let out = execute(
            &call(
                "edit",
                json!({
                    "path": "overlap.txt",
                    "edits": [{ "oldText": "aa", "newText": "b" }]
                }),
            ),
            &dir,
            &mcp,
            &progress,
        )
        .await;
        assert!(out.is_error, "{}", out.text);
        assert!(out.text.contains("matched 2 times"), "{}", out.text);
        assert_eq!(
            std::fs::read_to_string(dir.join("overlap.txt")).unwrap(),
            "aaa\n"
        );

        // Advancing overlap detection by a Unicode scalar boundary keeps the
        // same protection for multibyte text without slicing UTF-8 mid-codepoint.
        std::fs::write(dir.join("unicode.txt"), "ééé\n").unwrap();
        let out = execute(
            &call(
                "edit",
                json!({
                    "path": "unicode.txt",
                    "edits": [{ "oldText": "éé", "newText": "x" }]
                }),
            ),
            &dir,
            &mcp,
            &progress,
        )
        .await;
        assert!(out.is_error, "{}", out.text);
        assert!(out.text.contains("matched 2 times"), "{}", out.text);
        assert_eq!(
            std::fs::read_to_string(dir.join("unicode.txt")).unwrap(),
            "ééé\n"
        );

        // A real line beginning with digits and `|` wins as an exact literal;
        // prefix stripping is only a fallback for text copied from `read`.
        std::fs::write(dir.join("numbered.txt"), "1|alpha\nbeta\n").unwrap();
        let out = execute(
            &call(
                "edit",
                json!({
                    "path": "numbered.txt",
                    "edits": [{ "oldText": "1|alpha", "newText": "literal" }]
                }),
            ),
            &dir,
            &mcp,
            &progress,
        )
        .await;
        assert!(!out.is_error, "{}", out.text);
        assert_eq!(
            std::fs::read_to_string(dir.join("numbered.txt")).unwrap(),
            "literal\nbeta\n"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn edit_preserves_mixed_line_endings_and_adapts_crlf_replacements() {
        let dir = std::env::temp_dir().join(format!("oxide_edit_endings_{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        let mcp = McpRegistry::default();
        let progress = Progress::default();

        let mixed = "one\r\ntwo\nthree\r\n";
        std::fs::write(dir.join("mixed.txt"), mixed).unwrap();
        let out = execute(
            &call(
                "edit",
                json!({
                    "path": "mixed.txt",
                    "edits": [{ "oldText": "two", "newText": "TWO" }]
                }),
            ),
            &dir,
            &mcp,
            &progress,
        )
        .await;
        assert!(!out.is_error, "{}", out.text);
        assert_eq!(
            std::fs::read_to_string(dir.join("mixed.txt")).unwrap(),
            "one\r\nTWO\nthree\r\n"
        );

        std::fs::write(dir.join("crlf.txt"), "one\r\ntwo\r\n").unwrap();
        let out = execute(
            &call(
                "edit",
                json!({
                    "path": "crlf.txt",
                    "edits": [{ "oldText": "two", "newText": "two-a\ntwo-b" }]
                }),
            ),
            &dir,
            &mcp,
            &progress,
        )
        .await;
        assert!(!out.is_error, "{}", out.text);
        assert_eq!(
            std::fs::read_to_string(dir.join("crlf.txt")).unwrap(),
            "one\r\ntwo-a\r\ntwo-b\r\n"
        );

        std::fs::write(dir.join("bom.txt"), "\u{feff}alpha\n").unwrap();
        let out = execute(
            &call(
                "edit",
                json!({
                    "path": "bom.txt",
                    "edits": [{ "oldText": "alpha", "newText": "beta" }]
                }),
            ),
            &dir,
            &mcp,
            &progress,
        )
        .await;
        assert!(!out.is_error, "{}", out.text);
        assert_eq!(
            std::fs::read_to_string(dir.join("bom.txt")).unwrap(),
            "\u{feff}beta\n"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn edit_rejects_each_malformed_replacement_instead_of_silently_dropping_it() {
        let dir = std::env::temp_dir().join(format!("oxide_edit_shape_{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.txt"), "one\ntwo\n").unwrap();

        let err = edit(
            &dir,
            &json!({
                "path": "a.txt",
                "edits": [
                    { "oldText": "one", "newText": "ONE" },
                    { "oldText": "two" }
                ]
            }),
        )
        .unwrap_err();
        assert!(err.to_string().contains("edits[1]"), "{err}");
        assert!(err.to_string().contains("`newText`"), "{err}");
        assert_eq!(
            std::fs::read_to_string(dir.join("a.txt")).unwrap(),
            "one\ntwo\n"
        );

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
    async fn grep_indexes_regex_by_default_and_searches_literally_on_request() {
        let dir = std::env::temp_dir().join(format!("oxide_grep_mode_hint_{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("noindex-resolver.test.ts"),
            "it.each([PAGE_TYPE.BOUTIQUE_HOTELS_IN_CITY, PAGE_TYPE.VILLAS_IN_CITY])\n",
        )
        .unwrap();
        let mcp = McpRegistry::default();
        let progress = Progress::default();
        let pattern = "BOUTIQUE_HOTELS_IN_CITY|VILLAS_IN_CITY";

        // Pi's default is a regular expression, so a pattern a model writes
        // from knowledge of its tools matches without a second call.
        let matched = execute(
            &call("grep", json!({ "pattern": pattern })),
            &dir,
            &mcp,
            &progress,
        )
        .await;
        assert!(!matched.is_error, "{}", matched.text);
        assert!(
            matched.text.contains("noindex-resolver.test.ts:1:"),
            "{}",
            matched.text
        );
        assert!(!matched.text.contains("[note:"), "{}", matched.text);

        // A caller that means the characters themselves asks for a literal
        // search; the old `regex: false` spelling keeps working.
        for args in [
            json!({ "pattern": pattern, "literal": true }),
            json!({ "pattern": pattern, "regex": false }),
            json!({ "pattern": pattern, "use_regex": false }),
        ] {
            let literal = execute(&call("grep", args), &dir, &mcp, &progress).await;
            assert_eq!(literal.text, NO_MATCHES);
        }

        assert_eq!(
            grep(&dir, &json!({ "pattern": "missing" })).unwrap(),
            NO_MATCHES
        );

        std::fs::write(dir.join("literal.txt"), pattern).unwrap();
        let literal = grep(&dir, &json!({ "pattern": pattern, "literal": true })).unwrap();
        assert!(literal.contains("literal.txt:1:"), "{literal}");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[tokio::test]
    async fn grep_falls_back_to_literal_text_for_an_invalid_regex() {
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

        // A pattern written as a regex matches by default.
        let regex = execute(
            &call("grep", json!({ "pattern": "@Placeholder.*hotel" })),
            &dir,
            &mcp,
            &progress,
        )
        .await;
        assert!(regex.text.contains("a.java"), "{}", regex.text);

        // The same pattern read literally finds nothing.
        let literal = execute(
            &call(
                "grep",
                json!({ "pattern": "@Placeholder.*hotel", "literal": true }),
            ),
            &dir,
            &mcp,
            &progress,
        )
        .await;
        assert!(!literal.text.contains("a.java"), "{}", literal.text);

        // An unbalanced bracket is source text, not a regex. Searching it
        // literally is what the caller meant, and one saved round trip.
        std::fs::write(dir.join("b.txt"), "before ([ after\n").unwrap();
        let bad = execute(
            &call("grep", json!({ "pattern": "([" })),
            &dir,
            &mcp,
            &progress,
        )
        .await;
        assert!(!bad.is_error, "{}", bad.text);
        assert!(
            bad.text.contains("not a valid regular expression"),
            "{}",
            bad.text
        );
        assert!(bad.text.contains("b.txt:1:"), "{}", bad.text);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn ls_shows_symlink_targets() {
        let dir = std::env::temp_dir().join(format!("oxide_ls_symlink_{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("target.txt"), "hi").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink("target.txt", dir.join("link.txt")).unwrap();

        let out = ls(&dir, &json!({})).unwrap();
        assert!(out.contains("target.txt"), "{out}");
        #[cfg(unix)]
        assert!(out.contains("link.txt -> target.txt"), "{out}");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn rust_grep_fallback_searches_a_single_file() {
        let dir = std::env::temp_dir().join(format!("oxide_rust_grep_file_{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("resolver.test.ts");
        std::fs::write(&file, "before\nPAGE_TYPE.BOUTIQUE_HOTELS_IN_CITY\nafter\n").unwrap();

        let literal = GrepMatcher::new("BOUTIQUE_HOTELS_IN_CITY", false, false).unwrap();
        assert_eq!(
            rust_grep(&file, &literal, None, 0, 10).unwrap(),
            "resolver.test.ts:2: PAGE_TYPE.BOUTIQUE_HOTELS_IN_CITY"
        );
        let regex =
            GrepMatcher::new("BOUTIQUE_HOTELS_IN_CITY|VILLAS_IN_CITY", true, false).unwrap();
        assert_eq!(
            rust_grep(&file, &regex, Some("*.ts"), 1, 10).unwrap(),
            "resolver.test.ts-1- before\nresolver.test.ts:2: PAGE_TYPE.BOUTIQUE_HOTELS_IN_CITY\nresolver.test.ts-3- after"
        );
        assert_eq!(
            rust_grep(&file, &literal, Some("*.rs"), 0, 10).unwrap(),
            NO_MATCHES
        );
        std::fs::write(&file, b"\0BOUTIQUE_HOTELS_IN_CITY").unwrap();
        assert_eq!(rust_grep(&file, &literal, None, 0, 10).unwrap(), NO_MATCHES);
        std::fs::remove_dir_all(&dir).unwrap();
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
        let (route, item) = forge_route("https://github.com/acme/landing-pages/pull/848").unwrap();
        let pr = forge_hint(
            route,
            item,
            "https://github.com/acme/landing-pages/pull/848",
        );
        assert!(pr.contains("GitHub pull request"), "{pr}");
        assert!(
            pr.contains("gh pr view --comments https://github.com/acme/landing-pages/pull/848"),
            "{pr}"
        );
        assert!(
            pr.contains("gh pr diff https://github.com/acme/landing-pages/pull/848"),
            "{pr}"
        );
        assert!(
            pr.contains("gh pr comment https://github.com/acme/landing-pages/pull/848 --body"),
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

    #[test]
    fn github_blob_links_route_to_the_authenticated_contents_api() {
        let url = "https://github.com/Skyscanner/web-platform/blob/\
                   e704e161bb89ff6a75f3e5c8ea54982d43712df3/\
                   libs/shared/footer/searchPatternData.ts#L4699-L4702";
        let hint = github_file_hint(url).unwrap();

        assert!(hint.contains("may be private"), "{hint}");
        assert!(hint.contains("Do not use webfetch"), "{hint}");
        assert!(hint.contains("gh api"), "{hint}");
        assert!(
            hint.contains(
                "repos/Skyscanner/web-platform/contents/libs/shared/footer/\
                           searchPatternData.ts?ref=e704e161bb89ff6a75f3e5c8ea54982d43712df3"
            ),
            "{hint}"
        );
        assert!(hint.contains("sed -n '4699,4702p'"), "{hint}");

        let named_ref =
            github_file_hint("https://github.com/owner/repo/blob/feature/foo/src/lib.rs#L12")
                .unwrap();
        assert!(named_ref.contains("branch and tag names can contain `/`"));
        assert!(named_ref.contains("gh repo clone owner/repo"));
        assert!(
            !named_ref.contains("contents/foo/src/lib.rs?ref=feature"),
            "{named_ref}"
        );

        assert!(github_file_hint("https://github.com/owner/repo").is_none());
        assert!(github_file_hint("https://example.com/owner/repo/blob/main/a.rs").is_none());
        assert_eq!(github_line_range(Some("L12")), Some("12".to_string()));
        assert_eq!(github_line_range(Some("heading")), None);
    }

    #[test]
    fn every_public_forge_host_uses_its_authenticated_cli() {
        let github = forge_host_hint("https://github.com/acme/private-repo/commit/abc").unwrap();
        assert!(github.contains("may be private"), "{github}");
        assert!(github.contains("`gh` CLI instead of webfetch"), "{github}");
        let raw =
            forge_host_hint("https://raw.githubusercontent.com/acme/private/main/a.rs").unwrap();
        assert!(raw.contains("`gh` CLI instead of webfetch"), "{raw}");

        let gitlab = forge_host_hint("https://gitlab.com/acme/private-repo/-/pipelines/7").unwrap();
        assert!(gitlab.contains("may be private"), "{gitlab}");
        assert!(
            gitlab.contains("`glab` CLI instead of webfetch"),
            "{gitlab}"
        );

        assert!(forge_host_hint("https://example.com/acme/repo").is_none());
    }

    #[test]
    fn aws_console_urls_use_the_authenticated_cli() {
        let logs = aws_console_hint(
            "https://eu-west-1.console.aws.amazon.com/cloudwatch/home?region=eu-west-1\
             #logsV2:log-groups/log-group/%2Faws%2Fbatch%2Fjob/log-events/run%2F123",
        )
        .unwrap();
        assert!(logs.contains("account-scoped AWS Console URL"), "{logs}");
        assert!(logs.contains("Do not use webfetch"), "{logs}");
        assert!(logs.contains("aws sts get-caller-identity"), "{logs}");
        assert!(logs.contains("aws logs get-log-events"), "{logs}");
        assert!(logs.contains("'/aws/batch/job'"), "{logs}");
        assert!(logs.contains("'run/123'"), "{logs}");
        assert!(logs.contains("--region eu-west-1"), "{logs}");
        assert!(logs.contains("--profile <profile>"), "{logs}");

        let cloudwatch_escaped = aws_console_hint(
            "https://eu-west-1.console.aws.amazon.com/cloudwatch/home\
             #logsV2:log-groups/log-group/$252Faws$252Flambda$252Fworker/log-events/2026$252F10$252F14",
        )
        .unwrap();
        assert!(
            cloudwatch_escaped.contains("'/aws/lambda/worker'"),
            "{cloudwatch_escaped}"
        );
        assert!(
            cloudwatch_escaped.contains("'2026/10/14'"),
            "{cloudwatch_escaped}"
        );

        let batch = aws_console_hint(
            "https://console.aws.amazon.com/batch/home?region=us-east-2#jobs/detail/abc-123",
        )
        .unwrap();
        assert!(batch.contains("aws batch describe-jobs"), "{batch}");
        assert!(batch.contains("'abc-123'"), "{batch}");
        assert!(batch.contains("--region us-east-2"), "{batch}");

        let s3 = aws_console_hint(
            "https://us-west-2.console.aws.amazon.com/s3/buckets/private-bucket?region=us-west-2",
        )
        .unwrap();
        assert!(s3.contains("aws s3 ls 's3://private-bucket'"), "{s3}");

        let no_region = aws_console_hint("https://console.aws.amazon.com/lambda/home").unwrap();
        assert!(no_region.contains("--region <region>"), "{no_region}");

        // AWS API/object endpoints are not necessarily private. In particular,
        // public and presigned S3 URLs still belong on the normal fetch path.
        assert!(aws_console_hint("https://bucket.s3.amazonaws.com/public.txt").is_none());
        assert!(aws_console_hint("https://example.amazonaws.com/resource").is_none());
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
                "write",
                json!({ "path": "a.txt", "content": "one\ntwo\nthree" }),
            ),
            &dir,
            &mcp,
            &progress,
        )
        .await;
        assert!(out.text.contains("wrote"), "{}", out.text);

        let out = execute(
            &call("read", json!({ "path": "a.txt", "offset": 2, "limit": 1 })),
            &dir,
            &mcp,
            &progress,
        )
        .await;
        assert!(out.text.contains("two"), "{}", out.text);

        let out = execute(&call("ls", json!({})), &dir, &mcp, &progress).await;
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
            &call("read", json!({ "path": "missing.txt" })),
            &dir,
            &mcp,
            &progress,
        )
        .await;
        assert!(out.is_error, "{}", out.text);
        assert!(out.text.contains("reading") && out.text.contains("missing.txt"));

        // The bytes decide, not the name: a PNG attaches, and a text file
        // that merely ends in `.png` is read as the file it is.
        let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
        png.extend_from_slice(&13u32.to_be_bytes());
        png.extend_from_slice(b"IHDR");
        png.extend_from_slice(&[0u8; 17]);
        std::fs::write(dir.join("shot.png"), &png).unwrap();
        let out = execute(
            &call("read", json!({ "path": "shot.png" })),
            &dir,
            &mcp,
            &progress,
        )
        .await;
        assert_eq!(out.media.len(), 1, "{out:?}");
        assert!(out.text.contains("attached image/png"), "{}", out.text);

        std::fs::write(dir.join("notes.png"), b"f").unwrap();
        let out = execute(
            &call("read", json!({ "path": "notes.png" })),
            &dir,
            &mcp,
            &progress,
        )
        .await;
        assert!(out.media.is_empty(), "{out:?}");
        assert_eq!(out.text, "1|f");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn argument_errors_are_structured_and_actionable() {
        let mcp = McpRegistry::default();
        let progress = Progress::default();
        let cwd = std::env::temp_dir();

        let missing = execute(&call("edit", json!({ "edits": [] })), &cwd, &mcp, &progress).await;
        assert!(missing.is_error);
        assert!(missing.text.contains("edit requires a `path` string"));
        assert!(missing.text.contains("received keys: edits"));
        assert!(!missing.text.starts_with("error:"));

        let malformed = ToolCall {
            id: "test".to_string(),
            kind: "function".to_string(),
            signature: None,
            function: FunctionCall {
                name: "edit".to_string(),
                arguments: "{not json".to_string(),
            },
        };
        let malformed = execute(&malformed, &cwd, &mcp, &progress).await;
        assert!(malformed.is_error);
        assert!(malformed.text.contains("received invalid JSON arguments"));
        assert!(malformed.text.contains("oldText,newText"));
        assert!(!malformed.text.contains("line 1 column"));

        assert!(malformed.model_text().starts_with("error: "));
    }

    #[tokio::test]
    async fn a_no_argument_call_is_not_a_json_error() {
        let dir = std::env::temp_dir().join(format!("oxide_no_args_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.txt"), "hello").unwrap();
        let mcp = McpRegistry::default();
        let progress = Progress::default();

        // Some providers send an empty `arguments` string for a call with no
        // arguments (an MCP tool whose schema has no properties). It is an
        // empty object, not malformed JSON, so a call whose arguments are all
        // optional still runs.
        let call = ToolCall {
            id: "test".to_string(),
            kind: "function".to_string(),
            signature: None,
            function: FunctionCall {
                name: "ls".to_string(),
                arguments: String::new(),
            },
        };
        let out = execute(&call, &dir, &mcp, &progress).await;
        assert!(!out.is_error, "{}", out.text);
        assert!(out.text.contains("a.txt"), "{}", out.text);

        // A tool that does need arguments still names the missing one rather
        // than reporting a parse failure.
        let call = ToolCall {
            id: "test".to_string(),
            kind: "function".to_string(),
            signature: None,
            function: FunctionCall {
                name: "bash".to_string(),
                arguments: "   ".to_string(),
            },
        };
        let out = execute(&call, &dir, &mcp, &progress).await;
        assert!(out.is_error);
        assert!(out.text.contains("`command`"), "{}", out.text);
        assert!(!out.text.contains("invalid JSON arguments"), "{}", out.text);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn bash_reports_pipeline_failures_as_tool_errors() {
        let cwd = std::env::current_dir().unwrap();
        let mcp = McpRegistry::default();
        let out = execute(
            &call(
                "bash",
                json!({ "command": "(printf 'test failed\\n'; exit 7) | tail -1" }),
            ),
            &cwd,
            &mcp,
            &Progress::default(),
        )
        .await;
        assert!(out.is_error, "{}", out.text);
        assert!(
            out.text.contains("test failed") && out.text.ends_with("[exit: 7]"),
            "{}",
            out.text
        );
        let ok = execute(
            &call("bash", json!({ "command": "printf 'passed\\n' | tail -1" })),
            &cwd,
            &mcp,
            &Progress::default(),
        )
        .await;
        assert!(!ok.is_error, "{}", ok.text);
        assert!(ok.text.ends_with("[exit: 0]"));
    }

    /// A pipeline whose reader stops early is the command doing what it was
    /// asked, not a failure: `head` exiting kills the writer with SIGPIPE and
    /// `-o pipefail` reports 141, which used to paint a good listing red in
    /// every front-end. Unix-only, since neither a signal nor `pipefail` exists
    /// in the shell this tool runs on Windows.
    #[cfg(unix)]
    #[tokio::test]
    async fn bash_recognizes_a_search_with_no_matches_without_hiding_other_failures() {
        let dir = std::env::temp_dir().join(format!("oxide_search_status_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("input.txt"), "build succeeded\n").unwrap();
        for command in [
            "grep '^error' input.txt",
            "printf 'build succeeded\\n' | grep '^error' | head -40",
            "printf 'build succeeded\\n' | grep -v 'succeeded'",
            "cd . && printf 'build succeeded\\n' | grep '^error' | head -40",
            // A script reports its last command's status, so a search that ends
            // one is the empty result a lone search is.
            "printf 'build succeeded\\n'; grep '^error' input.txt",
            "printf 'a\\n' | head -1; printf 'build succeeded\\n' | grep '^error'",
            "printf 'x\\n'\nprintf 'build succeeded\\n' | grep '^error' | head -40",
        ] {
            let out = execute(
                &call("bash", json!({ "command": command })),
                &dir,
                &McpRegistry::default(),
                &Progress::default(),
            )
            .await;
            assert!(!out.is_error, "{command}: {}", out.text);
            assert!(
                out.text.ends_with("[exit: 1] (search returned no matches)"),
                "{}",
                out.text
            );
        }
        for command in [
            "bash -c 'exit 1' | grep '^error' | head -40",
            "bash -c 'exit 7' | grep '^error'",
            "grep '[' input.txt",
            "cd missing && printf x | grep '^error'",
            "false && grep '^error' input.txt",
            "! printf x | grep x",
            "exit 1",
            // A script whose last command is not a search, or whose text could
            // hide which command ran last, keeps its ordinary failure.
            "printf x; exit 1",
            "printf x; set -e; grep '^error' input.txt",
            "printf x; false && grep '^error' input.txt",
            "printf x; false || exit 1; grep '^error' input.txt",
            "printf x; grep '[' input.txt",
            "printf x; grep '^error' input.txt # ; printf y",
            "printf x; $CMD; grep '^error' input.txt",
            "printf x; bash -c 'exit 7'",
        ] {
            let out = execute(
                &call("bash", json!({ "command": command })),
                &dir,
                &McpRegistry::default(),
                &Progress::default(),
            )
            .await;
            assert!(out.is_error, "{command}: {}", out.text);
            assert!(
                !out.text.contains("search returned no matches"),
                "{command}: {}",
                out.text
            );
        }
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn rejected_git_lease_reports_recovery_without_retrying() {
        let dir = std::env::temp_dir().join(format!("oxide_git_lease_{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        let git = |args: &[&str]| {
            let output = std::process::Command::new("git")
                .current_dir(&dir)
                .args(args)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            String::from_utf8_lossy(&output.stdout).trim().to_string()
        };
        git(&["init", "--bare", "remote.git"]);
        git(&["init", "--initial-branch=feature", "local"]);
        git(&["-C", "local", "config", "user.name", "Test"]);
        git(&["-C", "local", "config", "user.email", "test@example.com"]);
        git(&[
            "-C",
            "local",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "--allow-empty",
            "-m",
            "base",
        ]);
        git(&["-C", "local", "remote", "add", "origin", "../remote.git"]);
        git(&["-C", "local", "push", "-u", "origin", "feature"]);
        let base = git(&["-C", "local", "rev-parse", "HEAD"]);
        git(&[
            "-C",
            "local",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "--allow-empty",
            "-m",
            "remote update",
        ]);
        git(&["-C", "local", "push", "origin", "feature"]);
        let remote_head = git(&["-C", "local", "rev-parse", "HEAD"]);
        git(&["-C", "local", "reset", "--hard", &base]);
        git(&[
            "-C",
            "local",
            "update-ref",
            "refs/remotes/origin/feature",
            &base,
        ]);

        let output = execute(
            &call(
                "bash",
                json!({"command": "git push --force-with-lease origin feature"}),
            ),
            &dir.join("local"),
            &McpRegistry::default(),
            &Progress::default(),
        )
        .await;
        assert!(output.is_error, "{}", output.text);
        assert!(output.text.contains("(stale info)"), "{}", output.text);
        assert!(
            output.text.contains("Do not bypass this rejection"),
            "{}",
            output.text
        );
        assert!(!output.text.contains("retried once"), "{}", output.text);
        assert_eq!(
            git(&["--git-dir=remote.git", "rev-parse", "feature"]),
            remote_head
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn shell_errors_from_sessions_have_specific_recovery_hints() {
        let repetition = bash_failure_hint("grep: maximum repetition exceeds 255").unwrap();
        assert!(repetition.contains("`grep` tool"));
        assert!(!repetition.contains("regex: true"), "{repetition}");
        assert!(
            bash_failure_hint("bash: unexpected EOF while looking for matching ``'")
                .unwrap()
                .contains("single quotes")
        );
        assert!(bash_failure_hint(
            "error: package ID specification `desktop` did not match any packages"
        )
        .unwrap()
        .contains("--manifest-path"));
        // A transport failure is the network, not the command, and the retry
        // is what usually fixes it.
        assert!(bash_failure_hint(
            "Get \"https://api.github.com/repos/x/y/contents/docs\": net/http: TLS handshake timeout"
        )
        .unwrap()
        .contains("run the same command again"));
        assert!(bash_failure_hint(
            "fatal: unable to access 'https://github.com/x/y.git/': Could not resolve host: github.com"
        )
        .unwrap()
        .contains("usually transient"));
        for reason in ["stale info", "fetch first", "non-fast-forward"] {
            let hint =
                bash_failure_hint(&format!(" ! [rejected] feature -> feature ({reason})")).unwrap();
            assert!(hint.contains("remote-only commits"), "{hint}");
            assert!(hint.contains("Do not bypass"), "{hint}");
        }
        assert!(bash_failure_hint("stale info in a cache").is_none());
        assert!(bash_failure_hint("error: test failed").is_none());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn bash_treats_a_reader_that_closed_as_success() {
        let cwd = std::env::current_dir().unwrap();
        let mcp = McpRegistry::default();
        let out = execute(
            &call("bash", json!({ "command": "yes row | head -2" })),
            &cwd,
            &mcp,
            &Progress::default(),
        )
        .await;
        assert!(!out.is_error, "{}", out.text);
        assert_eq!(out.text.matches("row").count(), 2, "{}", out.text);
        assert!(
            out.text.ends_with(
                "[exit: 0] (a command in the pipeline stopped because its reader closed)"
            ),
            "{}",
            out.text
        );
        // A command that fails on its own is still a failure, and so is 141 from
        // something that never used a pipe.
        let failed = execute(
            &call(
                "bash",
                json!({ "command": "printf 'x\\n%.0s' {1..10} | head -2; exit 3" }),
            ),
            &cwd,
            &mcp,
            &Progress::default(),
        )
        .await;
        assert!(failed.is_error, "{}", failed.text);
        let unpiped = execute(
            &call("bash", json!({ "command": "exit 141" })),
            &cwd,
            &mcp,
            &Progress::default(),
        )
        .await;
        assert!(unpiped.is_error, "{}", unpiped.text);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn bash_marks_a_transport_failure_as_worth_retrying() {
        // The shape `gh` prints for a TLS handshake timeout: the run should try
        // the same command again rather than looking for a repository or
        // authentication problem.
        let output = execute(
            &call(
                "bash",
                json!({
                    "command": "echo 'Get \"https://api.github.com/repos/x/y\": net/http: TLS handshake timeout' >&2; exit 1"
                }),
            ),
            Path::new("."),
            &McpRegistry::default(),
            &Progress::default(),
        )
        .await;

        assert!(output.is_error, "{}", output.text);
        assert!(
            output.text.contains("failed on a network request"),
            "{}",
            output.text
        );
        assert!(
            output.text.contains("run the same command again"),
            "{}",
            output.text
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn bash_retries_a_read_only_fetch_that_failed_on_the_network() {
        use std::os::unix::fs::PermissionsExt;

        let dir = std::env::temp_dir().join(format!("oxide_net_retry_{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        // A `git` that fails the first call on the transport and answers the
        // second, so the retry is observable as two calls.
        let script = dir.join("git");
        std::fs::write(
            &script,
            "#!/bin/sh\ndir=$(dirname \"$0\")\nprintf 'call\\n' >> \"$dir/calls\"\n\
             if [ -f \"$dir/state\" ]; then echo 'abc123 HEAD'; exit 0; fi\n\
             : > \"$dir/state\"\n\
             echo 'fatal: unable to access https://github.com/x/y.git/: net/http: TLS handshake timeout' >&2\n\
             exit 1\n",
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();

        let command = format!("PATH=\"{}\":$PATH git ls-remote origin", dir.display());
        let output = execute(
            &call("bash", json!({ "command": command })),
            &dir,
            &McpRegistry::default(),
            &Progress::default(),
        )
        .await;

        assert!(!output.is_error, "{}", output.text);
        assert!(output.text.contains("abc123 HEAD"), "{}", output.text);
        assert!(
            output.text.contains("retried once after a network failure"),
            "{}",
            output.text
        );
        let calls = std::fs::read_to_string(dir.join("calls")).unwrap();
        assert_eq!(calls.lines().count(), 2, "{calls}");

        // The shape the run actually lost a step to: a `gh api` read piped
        // through `head`, with the error merged into the output.
        let gh = dir.join("gh");
        std::fs::write(
            &gh,
            "#!/bin/sh\ndir=$(dirname \"$0\")\nprintf 'call\\n' >> \"$dir/gh-calls\"\n\
             if [ -f \"$dir/gh-state\" ]; then printf 'accounts.md\\nusage.md\\n'; exit 0; fi\n\
             : > \"$dir/gh-state\"\n\
             echo 'Get \"https://api.github.com/repos/earendil-works/pi/contents\": net/http: TLS handshake timeout' >&2\n\
             exit 1\n",
        )
        .unwrap();
        std::fs::set_permissions(&gh, std::fs::Permissions::from_mode(0o755)).unwrap();

        let command = format!(
            "PATH=\"{}\":$PATH gh api repos/earendil-works/pi/contents --jq '.[].name' 2>&1 | head -60",
            dir.display()
        );
        let output = execute(
            &call("bash", json!({ "command": command })),
            &dir,
            &McpRegistry::default(),
            &Progress::default(),
        )
        .await;
        assert!(!output.is_error, "{}", output.text);
        assert!(output.text.contains("accounts.md"), "{}", output.text);
        assert!(
            output.text.contains("retried once after a network failure"),
            "{}",
            output.text
        );
        let gh_calls = std::fs::read_to_string(dir.join("gh-calls")).unwrap();
        assert_eq!(gh_calls.lines().count(), 2, "{gh_calls}");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn bash_does_not_hide_failures_beside_a_pipe() {
        let cwd = std::env::current_dir().unwrap();
        let mcp = McpRegistry::default();
        for command in [
            "printf '|'; exit 141",
            "printf x | cat; exit 141",
            "bash -c 'exit 7' | bash -c 'exit 141' | cat",
            "printf x | bash -c 'exit 141'",
            "bash -c 'exit 141' # | just a comment",
        ] {
            let out = execute(
                &call("bash", json!({ "command": command })),
                &cwd,
                &mcp,
                &Progress::default(),
            )
            .await;
            assert!(out.is_error, "{command}: {}", out.text);
            assert!(out.text.ends_with("[exit: 141]"), "{command}: {}", out.text);
        }
    }

    #[test]
    fn differing_line_names_the_first_extra_line() {
        assert_eq!(
            first_differing_line("a\nb\nc", "a"),
            Some((2, "nothing".into(), "\"b\"".into()))
        );
        assert_eq!(
            first_differing_line("a", "a\nb\nc"),
            Some((2, "\"b\"".into(), "nothing".into()))
        );
        assert_eq!(first_differing_line("a \nb", "a\nb"), None);
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
    fn detects_an_unresolved_relative_import() {
        assert!(unresolved_relative_import(
            "Error: Cannot find module './out/core/protocol.js'"
        ));
        assert!(unresolved_relative_import(
            "error: Cannot find module \"./dist/index.js\""
        ));
        assert!(unresolved_relative_import(
            "code: 'MODULE_NOT_FOUND'\nrequireStack: [ '../lib/a.js' ]"
        ));
        assert!(unresolved_relative_import(
            "Error [ERR_MODULE_NOT_FOUND]: Cannot find module './b'"
        ));
        // An absolute specifier is a missing dependency, not a path mistake.
        assert!(!unresolved_relative_import(
            "Error: Cannot find module 'express'"
        ));
        assert!(!unresolved_relative_import(
            "Error: Cannot find module '/opt/app/node_modules/x'"
        ));
        assert!(!unresolved_relative_import("everything worked"));
    }

    #[test]
    fn picks_the_module_hint_from_the_require_stack() {
        let cwd = std::env::temp_dir().join("oxide_module_project");
        let outside = std::env::temp_dir().join("oxide_module_elsewhere/replay.js");
        let inside = cwd.join("scripts/replay.js");

        // A require-stack entry outside the working directory is a scratch
        // script run from elsewhere.
        let from_outside = format!(
            "Error: Cannot find module './x'\nRequire stack:\n- {}",
            outside.display()
        );
        assert_eq!(module_failure_hint(&from_outside, &cwd), BASH_MODULE_HINT);

        let from_inside = format!(
            "Error: Cannot find module './x'\nRequire stack:\n- {}",
            inside.display()
        );
        assert_eq!(
            module_failure_hint(&from_inside, &cwd),
            BASH_MODULE_PATH_HINT
        );

        // No require stack: assume a bad specifier, not a misplaced script.
        assert_eq!(
            module_failure_hint("Error: Cannot find module './x'", &cwd),
            BASH_MODULE_PATH_HINT
        );
    }

    // `cmd` has no `;` separator and quotes `echo` differently, so this command
    // would exit 0 on Windows. The hint path is platform-independent and covered
    // by the macOS/Linux jobs; the detector itself is tested above on all hosts.
    #[cfg(not(target_os = "windows"))]
    #[tokio::test]
    async fn bash_hints_at_a_relative_import_failure() {
        let dir = std::env::temp_dir().join(format!("oxide_bash_module_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mcp = McpRegistry::default();
        let output = execute(
            &call(
                "bash",
                json!({
                    "command": "echo \"Error: Cannot find module './out/x.js'\" >&2; exit 1"
                }),
            ),
            &dir,
            &mcp,
            &Progress::default(),
        )
        .await;
        assert!(
            output.text.contains(BASH_MODULE_PATH_HINT),
            "{}",
            output.text
        );
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

    #[test]
    fn recursive_shell_grep_is_refused_without_blocking_file_filters() {
        let root = Path::new(".");
        assert!(uses_recursive_shell_grep(
            r#"grep -rn "faqsItemId" /Users/example/Projects/falcon 2>/dev/null | head -20"#,
            root
        ));
        assert!(uses_recursive_shell_grep(
            "cd jobs && /usr/bin/grep --recursive needle data",
            root
        ));
        assert!(uses_recursive_shell_grep(
            "LC_ALL=C grep -Rl needle .",
            root
        ));
        assert!(uses_recursive_shell_grep(
            "env LC_ALL=C command /usr/bin/grep -rn needle .",
            root
        ));
        assert!(uses_recursive_shell_grep(
            "sudo -u root grep -rn needle .",
            root
        ));
        assert!(uses_recursive_shell_grep(
            "grep --regexp -retry --recursive .",
            root
        ));
        assert!(uses_recursive_shell_grep("grep -efoo -R .", root));
        assert!(uses_recursive_shell_grep(
            "grep --dereference-recursive needle .",
            root
        ));

        assert!(!uses_recursive_shell_grep("grep needle config.json", root));
        assert!(!uses_recursive_shell_grep(
            "printf '%s' value | grep value",
            root
        ));
        assert!(!uses_recursive_shell_grep("echo grep -r", root));
        assert!(!uses_recursive_shell_grep("printf '%s\\n' grep -r", root));
        assert!(!uses_recursive_shell_grep(
            "echo 'example | grep -r .'",
            root
        ));
        assert!(!uses_recursive_shell_grep("git log --grep latest", root));
        assert!(!uses_recursive_shell_grep("grep -- '-r' config.json", root));
        assert!(!uses_recursive_shell_grep(
            "grep -e -retry config.json",
            root
        ));
        assert!(!uses_recursive_shell_grep("grep -e-r config.json", root));
        assert!(!uses_recursive_shell_grep(
            "grep -f -rules.txt config.json",
            root
        ));
        assert!(!uses_recursive_shell_grep(
            "grep --regexp -retry config.json",
            root
        ));
    }

    #[test]
    fn only_a_read_only_network_fetch_is_retried_after_a_transport_failure() {
        assert!(read_only_network_pipeline(
            "gh api repos/earendil-works/pi/contents/docs --jq '.[].name' 2>&1 | head -60"
        ));
        assert!(read_only_network_pipeline(
            "gh pr view 42 --comments | tail -20"
        ));
        assert!(read_only_network_pipeline(
            "curl https://example.com/docs.md | head -5"
        ));
        assert!(read_only_network_pipeline("git ls-remote origin"));

        // Anything that names a change, writes a file, or is not a fetch is
        // left to the retry hint rather than run twice.
        assert!(!read_only_network_pipeline(
            "gh api -X POST repos/x/y/issues"
        ));
        assert!(!read_only_network_pipeline(
            "gh api repos/x/y/issues -f title=fix"
        ));
        assert!(!read_only_network_pipeline(
            "gh api --method=POST repos/x/y/issues"
        ));
        assert!(!read_only_network_pipeline("gh pr comment 42 --body hi"));
        assert!(!read_only_network_pipeline(
            "curl -X DELETE https://example.com/item"
        ));
        assert!(!read_only_network_pipeline(
            "curl -o out.html https://example.com"
        ));
        assert!(!read_only_network_pipeline(
            "curl --json '{}' https://example.com"
        ));
        assert!(!read_only_network_pipeline("git push origin main"));
        assert!(!read_only_network_pipeline("cargo test 2>&1 | tail -5"));
        assert!(!read_only_network_pipeline(
            "gh api repos/x/y && rm -rf build"
        ));
        // A redirect into a file makes the command a write.
        assert!(!read_only_network_pipeline("gh api repos/x/y > issue.json"));
        assert!(!read_only_network_pipeline(
            "gh api repos/x/y 2> err.log | head -5"
        ));
        assert!(read_only_network_pipeline(
            "gh api repos/x/y 2>/dev/null | head -5"
        ));
    }

    #[test]
    fn recursive_grep_over_named_files_is_not_a_directory_sweep() {
        let root = Path::new(".");
        // A recursive grep over files named one by one cannot reach a build
        // tree, so it runs instead of costing the run a step.
        assert!(!uses_recursive_shell_grep(
            "grep -rn \"providers\" Cargo.toml src/lib.rs | head -40",
            root
        ));
        assert!(!uses_recursive_shell_grep(
            "grep -rn -e needle Cargo.toml",
            root
        ));
        assert!(uses_recursive_shell_grep("grep -rn needle src", root));
        assert!(uses_recursive_shell_grep("grep -rn needle '*.rs'", root));
        assert!(uses_recursive_shell_grep("grep -rn needle", root));
        assert!(uses_recursive_shell_grep(
            "grep -rn needle Cargo.toml missing-dir",
            root
        ));
    }

    #[test]
    fn recursive_grep_allows_only_globs_that_expand_to_files() {
        let dir =
            std::env::temp_dir().join(format!("oxide_grep_globs_{}_RUNNER~1", std::process::id()));
        std::fs::create_dir_all(dir.join("docs")).unwrap();
        std::fs::write(dir.join("docs/guide.md"), "needle\n").unwrap();
        std::fs::write(dir.join("docs/notes.md"), "needle\n").unwrap();
        std::fs::create_dir_all(dir.join("docs/cache.md")).unwrap();
        std::fs::create_dir_all(dir.join("docs/.hidden.md")).unwrap();
        for command in [
            "grep -rn needle docs/guide.*",
            "grep -rn needle docs/notes.??",
        ] {
            assert!(!uses_recursive_shell_grep(command, &dir), "{command}");
        }
        for command in [
            "grep -rn needle docs/*.md",
            "grep -rn needle docs/.*.md",
            "grep -rn needle docs/missing*.md",
            "grep -rn needle ~/docs/*.md",
            "grep -rn needle ~user/docs/*.md",
            "grep -rn needle docs/guide.*/",
            "grep -rn needle 'docs/guide.*'",
            "grep -rn needle docs/*/guide.md",
            "grep -rn needle docs/**",
            "cd elsewhere; grep -rn needle docs/guide.*",
            "env -C elsewhere grep -rn needle docs/guide.*",
            "grep -rn needle docs/guide.* docs/cache.md",
        ] {
            assert!(uses_recursive_shell_grep(command, &dir), "{command}");
        }
        std::fs::remove_dir(dir.join("docs/cache.md")).unwrap();
        assert!(!uses_recursive_shell_grep(
            "grep -rn needle docs/*.md",
            &dir
        ));
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(".", dir.join("docs/link.md")).unwrap();
            assert!(uses_recursive_shell_grep("grep -rn needle docs/*.md", &dir));
            std::fs::remove_file(dir.join("docs/link.md")).unwrap();
        }
        let docs = dir.join("docs").to_string_lossy().replace('\\', "/");
        let command = format!("ls; grep -rn needle {docs}/*.md");
        assert!(!uses_recursive_shell_grep(&command, &dir), "{command}");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn recursive_grep_follows_the_directory_its_own_cd_reached() {
        let dir = std::env::temp_dir().join(format!(
            "oxide_grep_cd_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let crate_dir = dir.join("crate");
        std::fs::create_dir_all(crate_dir.join("src")).unwrap();
        std::fs::write(crate_dir.join("src/app.rs"), "needle\n").unwrap();
        std::fs::write(crate_dir.join("src/lib.rs"), "needle\n").unwrap();

        // The operands resolve where the command's own `cd` landed, so a glob
        // that expands to files there is a search of named files rather than a
        // sweep of the project root the tool would otherwise start in.
        for command in [
            "cd crate && grep -rn needle src/app.rs src/lib.rs",
            "cd crate && grep -rn needle src/*.rs",
        ] {
            assert!(!uses_recursive_shell_grep(command, &dir), "{command}");
        }
        // A directory operand is a sweep wherever the grep runs, and the call it
        // suggests names the directory the `cd` reached.
        let grep =
            recursive_shell_grep("cd crate && grep -rn needle src | head", &dir).expect("a sweep");
        let call = grep.equivalent_call().expect("a restated call");
        let call: Value = serde_json::from_str(&call).unwrap();
        assert_eq!(call["pattern"], json!("needle"));
        assert_eq!(
            call["path"],
            json!(std::fs::canonicalize(crate_dir.join("src"))
                .unwrap()
                .to_string_lossy()
                .into_owned())
        );
        // A `cd` this reader cannot follow leaves the conservative rule, since
        // the operation's own directory is then only a guess.
        for command in [
            "cd missing && grep -rn needle src/*.rs",
            "cd crate && cd nope && grep -rn needle src/*.rs",
            "pushd crate; grep -rn needle src/*.rs",
            "env -C crate grep -rn needle src/*.rs",
        ] {
            assert!(uses_recursive_shell_grep(command, &dir), "{command}");
        }
        // Each `cd` is followed in turn, so the second one's directory is the
        // one the glob is checked against.
        for command in [
            "cd crate && cd .. && grep -rn needle crate/src/*.rs",
            "cd crate && grep -rn needle ../crate/src/*.rs",
        ] {
            assert!(!uses_recursive_shell_grep(command, &dir), "{command}");
        }
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn bash_redirects_recursive_grep_to_the_search_tool() {
        let output = execute(
            &call(
                "bash",
                json!({ "command": "grep -rn needle . 2>/dev/null | head -20" }),
            ),
            Path::new("."),
            &McpRegistry::default(),
            &Progress::default(),
        )
        .await;

        assert!(output.is_error);
        assert!(output.text.contains("recursive shell grep refused"));
        assert!(output.text.contains("call the `grep` tool directly"));
        assert!(output.text.contains("uses ripgrep when available"));
        // The refusal restates the call, so the step is recoverable without
        // translating the shell command into tool arguments by hand.
        assert!(
            output.text.contains(r#""pattern":"needle""#),
            "{}",
            output.text
        );
        assert!(output.text.contains(r#""path":".""#), "{}", output.text);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn bash_runs_a_recursive_grep_scoped_to_named_files() {
        let dir =
            std::env::temp_dir().join(format!("oxide_grep_named_files_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("api.md"), "the providers table\n").unwrap();
        std::fs::write(dir.join("features.md"), "provider counts and providers\n").unwrap();

        let output = execute(
            &call(
                "bash",
                json!({ "command": "grep -rn \"providers\" *.md | head -40" }),
            ),
            &dir,
            &McpRegistry::default(),
            &Progress::default(),
        )
        .await;

        assert!(!output.is_error, "{}", output.text);
        assert!(output.text.contains("api.md:1:the providers table"));
        assert!(output
            .text
            .contains("features.md:1:provider counts and providers"));

        let sweep = execute(
            &call(
                "bash",
                json!({ "command": "grep -rn providers . | head -5" }),
            ),
            &dir,
            &McpRegistry::default(),
            &Progress::default(),
        )
        .await;
        assert!(sweep.is_error);
        assert!(sweep.text.contains("recursive shell grep refused"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn search_tool_descriptions_direct_repository_searches_to_ripgrep_backed_grep() {
        let specs = specs(&McpRegistry::default());
        let bash = specs
            .iter()
            .find(|spec| spec.function.name == "bash")
            .unwrap();
        let grep = specs
            .iter()
            .find(|spec| spec.function.name == "grep")
            .unwrap();

        assert!(bash
            .function
            .description
            .contains("call the `grep` tool directly"));
        assert!(bash
            .function
            .description
            .contains("uses ripgrep when available"));
        assert!(grep
            .function
            .description
            .contains("instead of calling `bash`"));
        assert!(grep
            .function
            .description
            .contains("runs ripgrep when available"));
    }

    #[test]
    fn project_nvmrc_uses_the_installed_node_without_sourcing_home_scripts() {
        let base = std::env::temp_dir().join(format!(
            "oxide_node_path_{}_{}",
            std::process::id(),
            TRUNCATION_ID.fetch_add(1, Ordering::Relaxed)
        ));
        let project = base.join("project");
        let home = base.join("home");
        let node_bin = home.join(".nvm/versions/node/v22.23.3/bin");
        std::fs::create_dir_all(&project).unwrap();
        std::fs::create_dir_all(&node_bin).unwrap();
        std::fs::write(
            node_bin.join(if cfg!(windows) { "node.exe" } else { "node" }),
            "",
        )
        .unwrap();
        std::fs::write(project.join(".nvmrc"), "22.23.3\n").unwrap();
        let inherited = std::env::join_paths([home.join(".volta/bin")]).unwrap();
        let paths: Vec<_> =
            std::env::split_paths(&project_runtime_path(&project, &home, &inherited)).collect();
        assert_eq!(paths[0], node_bin);
        for invalid in ["../../outside", "lts/*", "22.23.99"] {
            std::fs::write(project.join(".nvmrc"), invalid).unwrap();
            assert_eq!(
                project_runtime_path(&project, &home, &inherited),
                augment_path(&inherited, &home)
            );
        }
        std::fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn bash_path_adds_existing_user_tools_without_duplicates() {
        let dir = std::env::temp_dir().join(format!(
            "oxide_bash_path_{}_{}",
            std::process::id(),
            TRUNCATION_ID.fetch_add(1, Ordering::Relaxed)
        ));
        let inherited_bin = dir.join("inherited/bin");
        let local_bin = dir.join(".local/bin");
        let volta_bin = dir.join(".volta/bin");
        std::fs::create_dir_all(&inherited_bin).unwrap();
        std::fs::create_dir_all(&local_bin).unwrap();
        std::fs::create_dir_all(&volta_bin).unwrap();

        let inherited = std::env::join_paths([&inherited_bin, &local_bin]).unwrap();
        let augmented: Vec<PathBuf> =
            std::env::split_paths(&augment_path(&inherited, &dir)).collect();

        assert_eq!(augmented[0], local_bin);
        assert_eq!(augmented[1], volta_bin);
        assert_eq!(
            augmented
                .iter()
                .filter(|path| **path == dir.join(".local/bin"))
                .count(),
            1
        );
        assert!(augmented.contains(&inherited_bin));
        assert!(!augmented.contains(&dir.join(".cargo/bin")));

        // The list is rebuilt for every command, so a tool directory created
        // after a long-running app starts becomes visible without a restart.
        let cargo_bin = dir.join(".cargo/bin");
        std::fs::create_dir_all(&cargo_bin).unwrap();
        let refreshed: Vec<PathBuf> =
            std::env::split_paths(&augment_path(&inherited, &dir)).collect();
        assert!(refreshed.contains(&cargo_bin));

        std::fs::remove_dir_all(&dir).ok();
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

    // A program that asks the reader's terminal for an answer of its own (zsh's
    // compinit, a credential prompt) is reached through `bash`, so the check
    // needs a terminal on this end to mean anything.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_shell_command_cannot_reach_the_terminal() {
        let dir = std::env::temp_dir().join(format!("oxide_bash_tty_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mcp = McpRegistry::default();
        let progress = Progress::new(Arc::new(|_: &str| {}));

        let out = execute(
            &call(
                "bash",
                json!({
                    "command": "if (exec 3<>/dev/tty) 2>/dev/null; then echo HAS_TTY; else echo NO_TTY; fi"
                }),
            ),
            &dir,
            &mcp,
            &progress,
        )
        .await;
        assert!(
            out.text.contains("NO_TTY") && !out.text.contains("HAS_TTY"),
            "a shell command reached the terminal: {}",
            out.text
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

    // `cmd` has no `;` separator, so on Windows this command would finish
    // before the timeout. The partial-output path is platform-independent and
    // covered by the macOS/Linux jobs.
    #[cfg(not(target_os = "windows"))]
    #[tokio::test]
    async fn bash_timeout_keeps_the_output_produced_so_far() {
        let dir = std::env::temp_dir().join(format!("oxide_bash_partial_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mcp = McpRegistry::default();
        let progress = Progress::new(Arc::new(|_: &str| {}));

        let out = execute(
            &call(
                "bash",
                json!({ "command": "echo important-line; sleep 10", "timeout": 500 }),
            ),
            &dir,
            &mcp,
            &progress,
        )
        .await;
        assert!(out.text.contains("timed out"), "{}", out.text);
        assert!(out.text.contains("important-line"), "{}", out.text);
        assert!(out.text.contains("Retry narrower"), "{}", out.text);
        assert!(out.text.contains("`grep`/`find` tools"), "{}", out.text);

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
    fn wildcard_matching_handles_backtracking_and_long_inputs() {
        for (pattern, text, expected) in [
            ("*a", "*ba", true),
            ("*a*b", "aaac", false),
            ("a**?c*", "abcc", true),
            ("*您?", "啊您好", true),
            ("?", "", false),
            ("***", "", true),
        ] {
            assert_eq!(
                wildcard(
                    &pattern.chars().collect::<Vec<_>>(),
                    &text.chars().collect::<Vec<_>>()
                ),
                expected,
                "{pattern}: {text}"
            );
        }
        assert!(!glob_match(
            &format!("{}b", "*a".repeat(40)),
            &"a".repeat(80)
        ));
        assert!(glob_match("*", &"a".repeat(100_000)));
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
    fn find_basename_patterns_match_at_every_depth_and_stop_at_the_limit() {
        let dir = std::env::temp_dir().join(format!("oxide_find_fast_{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(dir.join("nested")).unwrap();
        std::fs::write(dir.join("root.txt"), "root").unwrap();
        std::fs::write(dir.join("nested/child.txt"), "child").unwrap();
        std::fs::write(dir.join("nested/skip.rs"), "skip").unwrap();

        let all = find(&dir, &json!({ "pattern": "*.txt" })).unwrap();
        assert!(all.contains("root.txt"), "{all}");
        assert!(all.contains("nested/child.txt"), "{all}");

        let limited = find(&dir, &json!({ "pattern": "*.txt", "limit": 1 })).unwrap();
        assert_eq!(limited.lines().count(), 2, "{limited}");
        assert!(limited.ends_with("... [truncated]"), "{limited}");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn searches_distinguish_invalid_roots_from_no_matches() {
        let dir = std::env::temp_dir().join(format!("oxide_search_roots_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("root.txt"), "root needle").unwrap();
        let missing = json!({"pattern": "needle", "path": "missing"});
        assert!(find(&dir, &missing)
            .unwrap_err()
            .to_string()
            .contains("search root"));
        assert!(grep(&dir, &missing)
            .unwrap_err()
            .to_string()
            .contains("search root"));
        assert!(find(&dir, &json!({"pattern": "*", "path": "root.txt"})).is_err());
        assert_eq!(
            find(&dir, &json!({"pattern": "*.txt"})).unwrap(),
            "root.txt"
        );
        assert!(grep(&dir, &json!({"pattern": "needle"}))
            .unwrap()
            .contains("root.txt:1:"));
        assert_eq!(
            find(&dir, &json!({"pattern": "*.absent"})).unwrap(),
            NO_FILES_FOUND
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn unknown_mcp_tool_explains_how_to_load_the_catalog() {
        let output = execute(
            &call("atlassian__createConfluencePage", json!({})),
            Path::new("."),
            &McpRegistry::default(),
            &Progress::default(),
        )
        .await;
        assert!(output.is_error);
        assert!(output.text.contains("mcp_load"));
        assert!(output.text.contains("exact tool name and schema"));
    }

    #[cfg(unix)]
    #[test]
    fn fd_find_streams_only_enough_results_to_report_truncation() {
        use std::os::unix::fs::PermissionsExt;

        let dir = std::env::temp_dir().join(format!("oxide_fd_find_{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        let fake_fd = dir.join("fd");
        std::fs::write(
            &fake_fd,
            "#!/bin/sh\ncase \" $* \" in *\" --type f \"*) ;; *) exit 3 ;; esac\n\
             printf 'a.txt\\nnested/b.txt\\nthird.txt\\n'\n",
        )
        .unwrap();
        let mut permissions = std::fs::metadata(&fake_fd).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&fake_fd, permissions).unwrap();

        let output = fd_find(&fake_fd, &dir, "*.txt", 1).unwrap();
        assert_eq!(output, "a.txt\n... [truncated]");
        assert!(fd_find(&fake_fd, &dir, "src/*.rs", 1).is_none());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[cfg(unix)]
    #[test]
    fn fd_find_rejects_partial_output_from_a_failed_process() {
        use std::os::unix::fs::PermissionsExt;

        let dir = std::env::temp_dir().join(format!("oxide_fd_fail_{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        let fake_fd = dir.join("fd");
        std::fs::write(&fake_fd, "#!/bin/sh\nprintf 'plausible.txt\\n'\nexit 2\n").unwrap();
        let mut permissions = std::fs::metadata(&fake_fd).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&fake_fd, permissions).unwrap();

        assert!(fd_find(&fake_fd, &dir, "*.txt", 10).is_none());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[cfg(unix)]
    #[test]
    fn fd_find_rejects_limit_sized_output_from_a_failed_process() {
        use std::os::unix::fs::PermissionsExt;

        let dir = std::env::temp_dir().join(format!("oxide_fd_limit_fail_{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        let fake_fd = dir.join("fd");
        // Exactly the limit, then a failure. The collector reaches EOF and reads
        // the exit status instead of racing a kill at the limit, which would
        // accept the output as "killed at the limit" depending on timing.
        std::fs::write(&fake_fd, "#!/bin/sh\nprintf 'first.txt\\n'\nexit 2\n").unwrap();
        let mut permissions = std::fs::metadata(&fake_fd).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&fake_fd, permissions).unwrap();

        assert!(fd_find(&fake_fd, &dir, "*.txt", 1).is_none());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn converts_html_to_markdown_for_webfetch() {
        let md = crate::html::to_markdown("<h1>Title</h1><p>Hello &amp; bye</p>");
        assert_eq!(md, "# Title\n\nHello & bye");
        let text = crate::html::to_text("<h1>Title</h1><p>Hello &amp; bye</p>");
        assert_eq!(text, "Title\n\nHello & bye");
    }

    #[test]
    fn write_reports_a_diff() {
        let dir = std::env::temp_dir().join(format!("oxide_write_diff_{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("f.txt"), "one\ntwo\n").unwrap();

        let out = write(&dir, &json!({ "path": "f.txt", "content": "one\nTWO\n" })).unwrap();
        let diff = out.diff.expect("write should report a diff");
        assert!(diff.text.contains("TWO"), "{}", diff.text);
        assert!(diff.text.contains("two"), "{}", diff.text);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn write_refuses_a_replacement_character_the_target_lacks() {
        let dir = std::env::temp_dir().join(format!("oxide_fffd_write_{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();

        let error = write(
            &dir,
            &json!({ "path": "article.md", "content": "先改\u{FFFD}入" }),
        )
        .expect_err("introducing U+FFFD must be refused");
        assert!(error.to_string().contains("U+FFFD"), "{error}");
        assert!(!dir.join("article.md").exists());

        // Text that already carries the character may keep it: an intentional
        // encoding fixture, or a rewrite that preserves a decoder's output.
        std::fs::write(dir.join("article.md"), "already \u{FFFD} here\n").unwrap();
        write(
            &dir,
            &json!({ "path": "article.md", "content": "already \u{FFFD} here\n" }),
        )
        .expect("preserving an existing U+FFFD is allowed");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn edit_refuses_a_replacement_character_and_leaves_the_file_unchanged() {
        let dir = std::env::temp_dir().join(format!("oxide_fffd_edit_{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.md"), "先改导入并加一个路径\n").unwrap();

        let error = edit(
            &dir,
            &json!({
                "path": "a.md",
                "edits": [{ "oldText": "先改导入", "newText": "先改\u{FFFD}入" }]
            }),
        )
        .expect_err("introducing U+FFFD must be refused");
        assert!(error.to_string().contains("U+FFFD"), "{error}");
        assert_eq!(
            std::fs::read_to_string(dir.join("a.md")).unwrap(),
            "先改导入并加一个路径\n"
        );

        // The corrected replacement lands, so the guard does not cost a retry.
        edit(
            &dir,
            &json!({
                "path": "a.md",
                "edits": [{ "oldText": "先改导入", "newText": "先改一次导入" }]
            }),
        )
        .expect("a replacement without U+FFFD is written");
        assert_eq!(
            std::fs::read_to_string(dir.join("a.md")).unwrap(),
            "先改一次导入并加一个路径\n"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn fetch_failures_explain_that_a_refusal_is_not_a_missing_page() {
        let forbidden = fetch_status_error(reqwest::StatusCode::FORBIDDEN).to_string();
        assert!(forbidden.contains("403"), "{forbidden}");
        assert!(forbidden.contains("denied access"), "{forbidden}");

        let unauthorized = fetch_status_error(reqwest::StatusCode::UNAUTHORIZED).to_string();
        assert!(
            unauthorized.contains("authentication is required"),
            "{unauthorized}"
        );
        assert!(!unauthorized.contains("automated"), "{unauthorized}");
        let limited = fetch_status_error(reqwest::StatusCode::TOO_MANY_REQUESTS).to_string();
        assert!(limited.contains("wait before retrying"), "{limited}");

        let missing = fetch_status_error(reqwest::StatusCode::NOT_FOUND).to_string();
        assert!(missing.contains("404"), "{missing}");
        assert!(!missing.contains("denied access"), "{missing}");
    }

    #[test]
    fn recursive_grep_refusal_names_the_equivalent_grep_tool_call() {
        let root = Path::new(".");
        let grep = recursive_shell_grep("grep -rn needle src", root).expect("a recursive sweep");
        let refusal = recursive_shell_grep_refusal(&grep);
        assert!(
            refusal.contains("recursive shell grep refused"),
            "{refusal}"
        );
        assert!(refusal.contains(r#""pattern":"needle""#), "{refusal}");
        assert!(refusal.contains(r#""path":"src""#), "{refusal}");

        let grep = recursive_shell_grep("grep -rn todo . --include='*.rs'", root).expect("a sweep");
        let call = grep.equivalent_call().expect("a restated call");
        assert!(call.contains(r#""glob":"*.rs""#), "{call}");

        // A quoted pattern spans shell words; the restated call carries it whole.
        let grep = recursive_shell_grep(
            r#"grep -rn "Definition of Done" crates/core/src --include='*.rs'"#,
            root,
        )
        .expect("a sweep");
        let call = grep.equivalent_call().expect("a restated call");
        assert!(call.contains(r#""pattern":"Definition of Done""#), "{call}");
        assert!(call.contains(r#""path":"crates/core/src""#), "{call}");

        // A pattern file cannot be restated, so the message keeps the guidance
        // without inventing a call.
        let grep = recursive_shell_grep("grep -rf patterns.txt src", root).expect("a sweep");
        assert!(grep.equivalent_call().is_none());
    }

    #[test]
    fn recursive_grep_suggestions_preserve_literal_arguments() {
        for (command, expected) in [
            (
                r#"grep -rn 'Definition  of Done' 'source files' --include='*.rs'"#,
                json!({"pattern": "Definition  of Done", "path": "source files", "glob": "*.rs"}),
            ),
            (
                r#"grep -rnF 'a.b' src"#,
                json!({"pattern": "a.b", "path": "src", "literal": true}),
            ),
            (
                r#"grep -rn "can't  stop" source\ files --include "*.rs""#,
                json!({"pattern": "can't  stop", "path": "source files", "glob": "*.rs"}),
            ),
            (
                r#"grep -rn '"quoted"' src"#,
                json!({"pattern": "\"quoted\"", "path": "src"}),
            ),
        ] {
            let grep = recursive_shell_grep(command, Path::new(".")).expect(command);
            let call = grep.equivalent_call().expect(command);
            assert_eq!(
                serde_json::from_str::<Value>(&call).unwrap(),
                expected,
                "{command}"
            );
        }
    }

    #[test]
    fn recursive_grep_keeps_generic_guidance_for_unrepresentable_searches() {
        for command in [
            "grep -rn needle src tests",
            "grep -rn -e first -e second src",
            "grep -rn -e first -f patterns.txt src",
            "grep -rn -f patterns.txt -e second src",
            "grep -rni needle src",
            "grep -rnv needle src",
            "grep -rn needle src --exclude-dir=generated",
            "grep -rn needle src --include='*.rs' --include='*.ts'",
            "cd elsewhere && grep -rn needle src",
            r#"grep -rn "$PATTERN" src"#,
            "grep -rn needle '$unterminated",
            "grep -rn needle src*",
            "grep -rn 'a.b' src",
        ] {
            let grep = recursive_shell_grep(command, Path::new(".")).expect(command);
            assert!(grep.equivalent_call().is_none(), "{command}");
            assert!(recursive_shell_grep_refusal(&grep).contains("call the `grep` tool directly"));
        }
    }

    #[test]
    fn replacement_character_refusal_is_atomic_for_directories_and_edit_batches() {
        let dir = std::env::temp_dir().join(format!("oxide_fffd_atomic_{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        write(
            &dir,
            &json!({"path": "new/nested/a.md", "content": "\u{FFFD}"}),
        )
        .expect_err("refuse before creating parent directories");
        assert!(!dir.join("new").exists());

        let original = "first\r\nalready \u{FFFD}\r\n";
        std::fs::write(dir.join("a.md"), original).unwrap();
        let error = edit(
            &dir,
            &json!({"path": "a.md", "edits": [
                {"oldText": "first", "newText": "changed"},
                {"oldText": "already \u{FFFD}", "newText": "already \u{FFFD}\u{FFFD}"}
            ]}),
        )
        .expect_err("one corrupt edit refuses the whole batch");
        assert!(error.to_string().contains("beyond the count"), "{error}");
        assert_eq!(std::fs::read_to_string(dir.join("a.md")).unwrap(), original);

        write(
            &dir,
            &json!({"path": "a.md", "content": "\u{FFFD}\u{FFFD}"}),
        )
        .expect_err("an existing replacement character does not permit an increase");
        assert_eq!(std::fs::read_to_string(dir.join("a.md")).unwrap(), original);
        edit(
            &dir,
            &json!({"path": "a.md", "edits": [
                {"oldText": "already \u{FFFD}", "newText": "kept \u{FFFD}"}
            ]}),
        )
        .expect("preserving a replacement character is permitted");
        assert_eq!(
            std::fs::read_to_string(dir.join("a.md")).unwrap(),
            "first\r\nkept \u{FFFD}\r\n"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn one_line_longer_than_the_budget_is_still_previewed() {
        // There is no line boundary to snap to here. Dropping the whole line
        // would leave the model with a marker and nothing to read, so the cut
        // falls back to a character boundary.
        let output = format!("{}\n[exit: 1]", "x".repeat(MAX_OUTPUT_BYTES * 2));
        let result = truncate_into("bash", output, None);
        let body = result
            .split_once("]\n")
            .expect("a truncated result carries a marker")
            .1;
        assert!(result.contains("[truncated:"), "{result}");
        assert!(result.contains("[exit: 1]"), "{result}");
        assert!(body.contains('x'), "a preview must keep readable content");
        assert!(body.len() <= MAX_OUTPUT_BYTES, "{}", body.len());
    }

    #[test]
    fn unicode_previews_never_slice_inside_a_character() {
        for text in ["中文🙂".repeat(80), "中文🙂一行\n".repeat(80)] {
            for budget in 1..text.len() {
                for tail in [false, true] {
                    let (preview, _, _) =
                        end_preview(&text.lines().collect::<Vec<_>>(), budget, 2000, tail);
                    assert!(preview.len() <= budget);
                }
                assert!(tail_preview(&text, budget, 2000).len() <= budget);
            }
        }
    }

    #[test]
    fn a_tail_preview_starts_on_a_line_boundary() {
        // A byte cut taken inside a line hands the model a fragment whose first
        // characters belong to a line it cannot see the start of. The lines
        // here are long enough that the byte budget, not the line budget, is
        // what forces the cut.
        let mut output = String::new();
        for i in 0..400 {
            output.push_str(&format!("line {i}: {}\n", "payload ".repeat(40)));
        }
        output.push_str("[exit: 7]");

        let result = truncate_into("bash", output, None);
        let body = result
            .split_once("]\n")
            .expect("a truncated result carries a marker")
            .1;
        assert!(result.contains("[exit: 7]"), "{result}");
        for line in body.lines() {
            assert!(
                line.is_empty() || line.starts_with("line ") || line.starts_with("[exit:"),
                "a tail preview must not begin mid-line: {line:?}"
            );
        }
    }

    #[test]
    fn a_head_preview_ends_on_a_line_boundary() {
        let mut output = String::new();
        for i in 0..400 {
            output.push_str(&format!("line {i}: {}\n", "payload ".repeat(40)));
        }

        let result = truncate_into("read", output, None);
        let body = result
            .split_once("]\n")
            .expect("a truncated result carries a marker")
            .1;
        for line in body.lines() {
            assert!(
                line.is_empty() || line.starts_with("line "),
                "a head preview must not begin mid-line: {line:?}"
            );
        }
        // A head cut truncates the end of the last line it keeps, so the
        // boundary that matters here is the final one.
        let last = body.lines().next_back().expect("a preview keeps lines");
        assert!(
            last.ends_with("payload ") || last.ends_with("payload"),
            "a head preview must not end mid-line: {last:?}"
        );
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

        let result = truncate_into("read", output, Some(&dir));
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
        let result = truncate_into("read", output.clone(), Some(&dir));
        assert!(result.contains("; full:"), "{result}");

        let saved: Vec<_> = std::fs::read_dir(&dir).unwrap().flatten().collect();
        assert_eq!(saved.len(), 1, "{saved:?}");
        assert_eq!(std::fs::read_to_string(saved[0].path()).unwrap(), output);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn an_mcp_result_keeps_both_ends() {
        let mut output = String::from("SUMMARY: 3 issues found\n");
        for i in 0..(MCP_OUTPUT_LINES + 200) {
            output.push_str(&format!("filler line {i}\n"));
        }
        output.push_str("TOTAL: 3 open, 1 blocked");

        let result = truncate_into("atlassian__searchJiraIssuesUsingJql", output, None);
        assert!(result.contains("SUMMARY: 3 issues found"), "{result}");
        assert!(result.contains("TOTAL: 3 open, 1 blocked"), "{result}");
        assert!(result.contains("truncated…"), "{result}");
        assert!(!result.contains("filler line 1100"), "{result}");
    }

    #[test]
    fn an_mcp_result_gets_the_shared_budget_and_keeps_both_ends() {
        assert_eq!(
            output_limits("server__tool"),
            (MCP_OUTPUT_BYTES, MCP_OUTPUT_LINES)
        );
        assert_eq!(keep_end("server__tool"), Keep::Middle);

        // Non-JSON output over the budget is kept from both ends within it.
        let output = "y".repeat(MCP_OUTPUT_BYTES + 100);
        let result = truncate_into("server__tool", output, None);
        assert!(result.starts_with("[truncated:"), "{result:.120}");
        assert!(result.len() <= MCP_OUTPUT_BYTES + 400, "{}", result.len());
    }

    /// A server's answer in the shape New Relic's NRQL tools return, which is
    /// what put the worst results into the recorded sessions: one envelope, one
    /// array of small homogeneous records.
    fn nrql_records(count: usize) -> String {
        let rows = (0..count)
            .map(|i| {
                format!(
                    "{{\"facet\":\"lightcone-6dc5566c4f-{i:04}\",\"beginTimeSeconds\":1790908800,\"\
                     endTimeSeconds\":1790910000,\"k8s.pod.name\":\"lightcone-6dc5566c4f-{i:04}\",\"\
                     p95\":{{\"95\":1504.0}},\"spans\":{i}}}"
                )
            })
            .collect::<Vec<_>>()
            .join(",");
        format!(
            "{{\"result\":{{\"data\":{{\"actor\":{{\"account\":{{\"nrql\":{{\"results\":[{rows}]}}}}}}}}}}}}"
        )
    }

    #[test]
    fn a_large_json_answer_is_elided_on_record_boundaries() {
        let dir = std::env::temp_dir().join(format!("oxide_elide_{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        let output = nrql_records(600);
        assert!(output.len() > MCP_OUTPUT_BYTES, "a payload worth eliding");

        let result = truncate_into("newrelic__execute_nrql_query", output.clone(), Some(&dir));
        assert!(result.starts_with("[truncated:"), "{result:.120}");
        assert!(result.len() <= MCP_OUTPUT_BYTES + 200, "{}", result.len());
        assert!(result.contains("; full:"), "{result:.120}");

        // What reaches the model is still the document, not a fragment of one:
        // the envelope survives and the array still parses.
        let (header, body) = result.split_once("]\n").expect("a marker line");
        let document: Value = serde_json::from_str(body).expect("the answer still parses");
        let rows = document["result"]["data"]["actor"]["account"]["nrql"]["results"]
            .as_array()
            .expect("the array survived");

        // Both ends of the list are kept, and the marker between them says how
        // many records are missing, so a reader can tell what it is not seeing.
        assert_eq!(rows[0]["facet"], json!("lightcone-6dc5566c4f-0000"));
        assert_eq!(
            rows[rows.len() - 1]["facet"],
            json!("lightcone-6dc5566c4f-0599")
        );
        let kept = rows.len() - 1;
        let marker = rows
            .iter()
            .find_map(Value::as_str)
            .expect("a marker element");
        assert_eq!(marker, format!("… {} of 600 records omitted …", 600 - kept));
        // The header repeats the same count, so a reader that never opens the
        // array still knows how much of the answer it is missing.
        assert!(
            header.contains(&format!("{} of 600 records omitted", 600 - kept)),
            "{header}"
        );

        // The whole answer is what was saved, not the elided one.
        let saved: Vec<_> = std::fs::read_dir(&dir).unwrap().flatten().collect();
        assert_eq!(saved.len(), 1, "{saved:?}");
        assert_eq!(std::fs::read_to_string(saved[0].path()).unwrap(), output);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn elision_keeps_the_envelope_and_a_few_records_when_they_are_huge() {
        // Records big enough that only a handful fit: the document is still
        // whole, so the counts and the shape reach the model.
        let output = format!(
            "{{\"rows\":[{}],\"total\":300}}",
            (0..300)
                .map(|i| format!("{{\"id\":{i},\"blob\":\"{}\"}}", "z".repeat(500)))
                .collect::<Vec<_>>()
                .join(",")
        );
        let result = truncate_into("server__tool", output, None);
        let body = result.split_once("]\n").expect("a marker line").1;
        let document: Value = serde_json::from_str(body).expect("still parses");
        assert_eq!(document["total"], json!(300));
        let rows = document["rows"].as_array().unwrap();
        assert!(rows.len() > 1, "kept a record: {}", rows.len());
        assert_eq!(rows[0]["id"], json!(0));
        assert_eq!(rows[rows.len() - 1]["id"], json!(299));
    }

    #[test]
    fn a_root_level_array_is_elided_too() {
        // Some servers answer with the list itself rather than an envelope, so
        // the array to elide is the document.
        let output = format!(
            "[{}]",
            (0..2_000)
                .map(|i| format!("{{\"id\":{i},\"route\":\"/v1/search/{i}\"}}"))
                .collect::<Vec<_>>()
                .join(",")
        );
        let result = truncate_into("server__tool", output, None);
        let (header, body) = result.split_once("]\n").expect("a marker line");
        assert!(header.contains("of 2000 records omitted"), "{header}");
        let rows: Vec<Value> = serde_json::from_str(body).expect("still parses");
        assert_eq!(rows[0]["id"], json!(0));
        assert_eq!(rows[rows.len() - 1]["id"], json!(1999));
        assert!(rows.iter().any(Value::is_string), "named the gap");
    }

    #[test]
    fn a_compact_answer_that_fits_comes_back_whole() {
        // The second record is smaller than the marker that would stand in for
        // it, so the complete (compact) document is *smaller* than the
        // `total - 1` candidate that drops a record for the marker. The
        // search has to rule the whole document out before it starts: with
        // `total` in range the predicate is not monotonic, and the search can
        // walk down to zero and hand back the byte-cut fragment for a document
        // that fits.
        let envelope = "[{\"id\":1,\"blob\":\"\"},{\"id\":2}]".len();
        let blob = "z".repeat(MCP_OUTPUT_BYTES - 10 - envelope);
        let output = format!(
            "[\n  {{\n    \"id\": 1,\n    \"blob\": \"{blob}\"\n  }},\n  {{\n    \"id\": 2\n  }}\n]"
        );
        let compact =
            serde_json::to_string(&serde_json::from_str::<Value>(&output).unwrap()).unwrap();
        assert!(
            compact.len() <= MCP_OUTPUT_BYTES,
            "the compact document fits: {}",
            compact.len()
        );
        assert!(
            output.len() > MCP_OUTPUT_BYTES,
            "the original does not: {}",
            output.len()
        );

        let value: Value = serde_json::from_str(&output).unwrap();
        let whole = elided(&value, &[], 2).expect("the whole document");
        let dropped = elided(&value, &[], 1).expect("one record dropped");
        assert_eq!(whole.len(), compact.len());
        assert!(
            dropped.len() > whole.len(),
            "the marker costs more than the record it replaces: {} vs {}",
            dropped.len(),
            whole.len()
        );

        let result = truncate_into("server__tool", output, None);
        assert_eq!(result, compact, "the whole document, compacted");
    }

    #[test]
    fn an_answer_with_no_array_still_uses_the_byte_cut() {
        // One huge record is not a listing: there is no record boundary to cut
        // on, so the middle preview is still what stands in for it.
        let output = format!("{{\"body\":\"{}\"}}", "q".repeat(MCP_OUTPUT_BYTES * 4));
        let result = truncate_into("server__tool", output, None);
        assert!(result.contains("truncated…"), "{result:.160}");
        assert!(result.len() <= MCP_OUTPUT_BYTES + 200, "{}", result.len());
    }

    #[test]
    fn a_pretty_printed_answer_that_fits_is_compacted_rather_than_cut() {
        // Whitespace is the only thing over budget, so nothing is dropped and
        // no marker is drawn: the answer is returned whole.
        let output = format!(
            "{{\n  \"rows\": [\n{}\n  ]\n}}",
            (0..3_000)
                .map(|i| format!("    {{\"id\": {i}}}"))
                .collect::<Vec<_>>()
                .join(",\n")
        );
        assert!(output.len() > MCP_OUTPUT_BYTES);
        let result = truncate_into("server__tool", output, None);
        assert!(!result.starts_with("[truncated:"), "{result:.120}");
        let document: Value = serde_json::from_str(&result).expect("still parses");
        assert_eq!(document["rows"].as_array().unwrap().len(), 3_000);
        assert!(result.len() <= MCP_OUTPUT_BYTES, "{}", result.len());
    }

    #[test]
    fn built_in_tools_keep_their_own_ends() {
        assert_eq!(keep_end("bash"), Keep::Tail);
        assert_eq!(keep_end("read"), Keep::Head);
        assert_eq!(keep_end("grep"), Keep::Head);
        assert_eq!(keep_end("webfetch"), Keep::Head);
        assert!(!is_mcp_tool("read"));
        assert!(!is_mcp_tool("ls"));
        assert!(is_mcp_tool("mock__echo"));
    }

    #[test]
    fn a_middle_truncated_result_stays_within_its_budget() {
        // Long single lines: the byte budget is what bites, not the line count.
        let output = format!("{}\n{}", "a".repeat(40_000), "b".repeat(40_000));
        let result = truncate_into("server__tool", output, None);
        assert!(result.len() < MCP_OUTPUT_BYTES + 200, "{}", result.len());
        assert!(result.starts_with("[truncated:"), "{result}");
        // Both ends survive, which neither a head-only nor a tail-only cap can
        // do: 40 KB of `a` would fill a head cap and bury every `b`.
        assert!(result.contains('a'), "kept the head");
        assert!(result.contains('b'), "kept the tail");
        assert!(
            result.contains("truncated…"),
            "named the gap: {result:.120}"
        );
    }

    #[test]
    fn a_single_line_json_result_keeps_both_ends() {
        // Most MCP servers answer with one line of JSON, so the line count is
        // never exceeded and only the byte budget bites.
        let output = format!(
            "{{\"summary\":\"start marker\",\"rows\":[{}],\"nextPageToken\":\"end marker\"}}",
            "\"x\",".repeat(8000)
        );
        let result = truncate_into("server__tool", output, None);
        assert!(
            result.contains("start marker"),
            "lost the head: {result:.120}"
        );
        assert!(result.contains("end marker"), "lost the tail");
    }

    #[test]
    fn middle_truncation_handles_awkward_shapes() {
        // A single multi-byte line: both cut points land mid-character.
        let output = "\u{4e2d}".repeat(30_000);
        let result = truncate_into("server__tool", output, None);
        assert!(result.starts_with("[truncated:"), "{result:.80}");
        assert!(result.contains('\u{4e2d}'));
        assert!(result.len() <= MCP_OUTPUT_BYTES + 200, "{}", result.len());

        // Exactly at the budget: kept whole, no marker.
        let exact = "z".repeat(MCP_OUTPUT_BYTES);
        assert_eq!(truncate_into("server__tool", exact.clone(), None), exact);

        // Many short lines: the line cap bites, the byte cap does not.
        let many = (0..MCP_OUTPUT_LINES + 50)
            .map(|i| format!("r{i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let result = truncate_into("server__tool", many, None);
        assert!(result.contains("r0\n"), "kept the head: {result:.60}");
        assert!(
            result.contains(&format!("r{}", MCP_OUTPUT_LINES + 49)),
            "kept the tail"
        );
        assert!(result.contains("lines"), "named the gap");

        // Empty and tiny results are untouched.
        assert_eq!(truncate_into("server__tool", String::new(), None), "");
        assert_eq!(truncate_into("server__tool", "ok".to_string(), None), "ok");
    }

    #[test]
    fn middle_truncation_cuts_on_character_boundaries() {
        let output = format!("{}\n{}", "é".repeat(20_000), "中".repeat(20_000));
        let result = truncate_into("server__tool", output, None);
        assert!(result.contains('é') && result.contains('中'), "{result}");
    }

    /// Drives a server's answer through `execute`, the path a real MCP call
    /// takes, rather than calling `truncate` directly.
    #[tokio::test]
    async fn a_servers_large_answer_keeps_both_ends_through_execute() {
        if !matches!(
            std::process::Command::new("python3").arg("--version").output(),
            Ok(output) if output.status.success()
        ) {
            return;
        }

        let dir = std::env::temp_dir().join(format!("oxide_mcp_trunc_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let script = dir.join("big_mcp.py");
        // Answers like a real server: a summary, a long body, then the totals
        // the next call needs. Built line by line, with newlines written as
        // `chr(10)`, so the escapes belong to Python and not to this literal.
        let python = [
            "import sys, json",
            "def send(obj):",
            "    sys.stdout.write(json.dumps(obj) + chr(10))",
            "    sys.stdout.flush()",
            "for line in sys.stdin:",
            "    line = line.strip()",
            "    if not line:",
            "        continue",
            "    msg = json.loads(line)",
            "    method = msg.get('method')",
            "    if method == 'initialize':",
            "        send({'jsonrpc': '2.0', 'id': msg['id'], 'result': {'protocolVersion': '2024-11-05', 'capabilities': {}, 'serverInfo': {'name': 'big', 'version': '0'}}})",
            "    elif method == 'tools/list':",
            "        send({'jsonrpc': '2.0', 'id': msg['id'], 'result': {'tools': [{'name': 'report', 'description': 'Big report', 'inputSchema': {'type': 'object', 'properties': {}}}]}})",
            "    elif method == 'tools/call':",
            "        rows = ''.join('  PROJ-%d open' % i + chr(10) for i in range(4000))",
            "        text = 'SUMMARY: 42 issues' + chr(10) + rows + 'TOTAL: 42 issues, nextPageToken=abc123'",
            "        send({'jsonrpc': '2.0', 'id': msg['id'], 'result': {'content': [{'type': 'text', 'text': text}]}})",
        ]
        .join("\n");
        std::fs::write(&script, python).unwrap();

        let server = crate::ecosystem::McpServer {
            name: "big".to_string(),
            enabled: true,
            kind: crate::ecosystem::McpKind::Local {
                command: vec!["python3".to_string(), script.display().to_string()],
                environment: Default::default(),
                cwd: None,
            },
            domains: vec![],
        };
        let registry = crate::mcp::McpRegistry::new(&[server]);
        registry.load("big").await.unwrap();

        let output = execute(
            &call("big__report", json!({})),
            &dir,
            &registry,
            &Progress::default(),
        )
        .await;

        assert!(
            output.text.contains("SUMMARY: 42 issues"),
            "lost the summary: {:.200}",
            output.text
        );
        assert!(
            output.text.contains("nextPageToken=abc123"),
            "lost the totals the next call needs"
        );
        assert!(
            output.text.len() <= MCP_OUTPUT_BYTES + 400,
            "not capped: {}",
            output.text.len()
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn read_pages_through_long_lines() {
        let dir = std::env::temp_dir().join(format!("oxide_read_cap_{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        let line = "a".repeat(READ_MAX_BYTES + MAX_LINE_LEN * 4);
        std::fs::write(dir.join("long.txt"), &line).unwrap();

        // The line is served in continuation chunks, not cut off, and the
        // footer points at the next display line.
        let out = read(&dir, &json!({ "path": "long.txt" })).unwrap();
        assert!(out.text.starts_with("1|"), "{}", out.text);
        assert!(out.text.contains("1+|"), "{}", out.text);
        assert!(out.text.contains("use offset="), "{}", out.text);

        // Paging through the footer's offset recovers the whole line.
        let mut offset = 1usize;
        let mut assembled = String::new();
        loop {
            let page = read(&dir, &json!({ "path": "long.txt", "offset": offset })).unwrap();
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
    fn read_keeps_its_continuation_pointer_inside_the_output_cap() {
        let dir = std::env::temp_dir().join(format!("oxide_read_ptr_{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        let mut content = String::new();
        for i in 0..(READ_MAX_LINES + 1) {
            content.push_str(&format!("line {i}\n"));
        }
        std::fs::write(dir.join("big.txt"), &content).unwrap();

        // A full page plus the `… more lines` pointer must survive the central
        // cap, otherwise the model never learns the offset to page with.
        let out = read(&dir, &json!({ "path": "big.txt" })).unwrap();
        assert!(out.text.contains("use offset="), "{}", out.text);
        let capped = truncate_into("read", out.text.clone(), None);
        assert!(!capped.starts_with("[truncated:"), "{capped}");
        assert!(capped.contains("use offset="), "{capped}");

        // An oversized explicit limit is clamped, so it cannot produce a page
        // the central cap would silently cut.
        let out = read(&dir, &json!({ "path": "big.txt", "limit": 10_000 })).unwrap();
        let capped = truncate_into("read", out.text.clone(), None);
        assert!(!capped.starts_with("[truncated:"), "{capped}");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn ls_reports_files_without_reading_their_contents() {
        let dir = std::env::temp_dir().join(format!("oxide_ls_file_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("AGENTS.md"), "private contents").unwrap();
        let out = ls(&dir, &json!({ "path": "AGENTS.md" })).unwrap();
        assert!(out.contains("AGENTS.md (file, 16 bytes"), "{out}");
        assert!(out.contains("use `read`"), "{out}");
        assert!(!out.contains("private contents"), "{out}");
        assert_eq!(ls(&dir, &json!({})).unwrap(), "AGENTS.md");
        assert!(ls(&dir, &json!({ "path": "missing" })).is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn read_lists_directories_instead_of_erroring() {
        let dir = std::env::temp_dir().join(format!("oxide_read_dir_{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(dir.join("components/nested")).unwrap();
        std::fs::write(dir.join("components/index.tsx"), "export {}").unwrap();

        let out = read(&dir, &json!({ "path": "components" })).unwrap();
        assert!(out.text.contains("is a directory"), "{}", out.text);
        assert!(out.text.contains("index.tsx"), "{}", out.text);
        assert!(out.text.contains("nested/"), "{}", out.text);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn read_reports_binary_files_without_erroring() {
        let dir = std::env::temp_dir().join(format!("oxide_read_bin_{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("blob.bin"), [0xff, 0xfe, 0x00, 0x01]).unwrap();

        let out = read(&dir, &json!({ "path": "blob.bin" })).unwrap();
        assert!(out.text.contains("binary file"), "{}", out.text);
        assert!(out.text.contains("4 bytes"), "{}", out.text);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn read_rejects_an_unparseable_offset() {
        let dir =
            std::env::temp_dir().join(format!("oxide_read_bad_offset_{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.txt"), "one\ntwo\nthree\n").unwrap();

        let err = read(&dir, &json!({ "path": "a.txt", "offset": ".2" })).unwrap_err();
        assert!(
            err.to_string().contains("`offset` must be an integer"),
            "{err}"
        );

        // An explicit `null` is a present, non-integer value, not an omission.
        let err = read(&dir, &json!({ "path": "a.txt", "limit": null })).unwrap_err();
        assert!(
            err.to_string().contains("`limit` must be an integer"),
            "{err}"
        );

        // A numeric string is still accepted.
        let out = read(
            &dir,
            &json!({ "path": "a.txt", "offset": "2", "limit": "1" }),
        )
        .unwrap();
        assert!(out.text.contains("2|two"), "{}", out.text);
        assert!(!out.text.contains("1|one"), "{}", out.text);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn edit_recovers_a_wrapped_old_text_with_a_sibling_replacement() {
        let dir = std::env::temp_dir().join(format!(
            "oxide_edit_xml_{}_{}",
            std::process::id(),
            TRUNCATION_ID.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.txt"), "before\n").unwrap();
        edit(&dir, &json!({ "path": "a.txt", "edits": "\n<parameter name=\"oldText\">before", "newText": "after" })).unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.join("a.txt")).unwrap(),
            "after\n"
        );
        assert!(edit(
            &dir,
            &json!({ "path": "a.txt", "edits": "<parameter name=\"oldText\">after" })
        )
        .is_err());
        assert_eq!(
            std::fs::read_to_string(dir.join("a.txt")).unwrap(),
            "after\n"
        );
        std::fs::remove_dir_all(dir).unwrap();
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
        let message = err.to_string();
        assert!(message.contains("not valid JSON"), "{message}");
        assert!(message.contains("oldText") && message.contains("newText"));
        assert!(!message.contains("line 1 column"), "{message}");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn edit_repairs_a_stringified_edits_array_with_an_extra_brace() {
        let dir = std::env::temp_dir().join(format!("oxide_edit_repair_{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.txt"), "hello\n").unwrap();

        // Some models double-encode `edits` as a string and close the object
        // with one brace too many; it still applies instead of erroring.
        let out = edit(
            &dir,
            &json!({
                "path": "a.txt",
                "edits": "[{\"oldText\": \"hello\", \"newText\": \"world\"}}]",
            }),
        )
        .unwrap();
        assert!(out.text.contains("Successfully replaced"), "{}", out.text);
        assert_eq!(
            std::fs::read_to_string(dir.join("a.txt")).unwrap(),
            "world\n"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn read_accepts_a_range_offset() {
        let dir = std::env::temp_dir().join(format!("oxide_read_range_{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.txt"), "one\ntwo\nthree\nfour\n").unwrap();

        let out = read(&dir, &json!({ "path": "a.txt", "offset": [2, 3] })).unwrap();
        assert!(out.text.contains("2|two"), "{}", out.text);
        assert!(out.text.contains("3|three"), "{}", out.text);
        assert!(!out.text.contains("1|one"), "{}", out.text);
        assert!(!out.text.contains("4|four"), "{}", out.text);

        for offset in [json!("[2, 3]"), json!("[2]")] {
            let out = read(
                &dir,
                &json!({ "path": "a.txt", "offset": offset,
                "limit": "[2]" }),
            )
            .unwrap();
            assert!(out.text.contains("2|two"), "{}", out.text);
            assert!(out.text.contains("3|three"), "{}", out.text);
            assert!(!out.text.contains("4|four"), "{}", out.text);
        }

        // A reversed or overflowing range must not panic; the first number is
        // still the offset and the page falls back to the default span.
        let out = read(&dir, &json!({ "path": "a.txt", "offset": [3, 2] })).unwrap();
        assert!(out.text.contains("3|three"), "{}", out.text);
        assert!(!out.text.contains("1|one"), "{}", out.text);

        let out = read(&dir, &json!({ "path": "a.txt", "offset": [0, u64::MAX] })).unwrap();
        assert!(out.text.contains("1|one"), "{}", out.text);

        std::fs::remove_dir_all(&dir).ok();
    }
}
