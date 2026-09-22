use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout};
use tokio::sync::Mutex;

const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);
const DIAGNOSTIC_TIMEOUT: Duration = Duration::from_millis(1500);

#[derive(Debug, Clone, Copy)]
struct ServerConfig {
    language_id: &'static str,
    command: &'static str,
    args: &'static [&'static str],
}

fn server_for(path: &Path) -> Option<ServerConfig> {
    match path.extension().and_then(|extension| extension.to_str())? {
        "rs" => Some(ServerConfig {
            language_id: "rust",
            command: "rust-analyzer",
            args: &[],
        }),
        "ts" | "tsx" | "js" | "jsx" | "mjs" | "cjs" => Some(ServerConfig {
            language_id: "typescript",
            command: "typescript-language-server",
            args: &["--stdio"],
        }),
        "py" => Some(ServerConfig {
            language_id: "python",
            command: "pyright-langserver",
            args: &["--stdio"],
        }),
        "go" => Some(ServerConfig {
            language_id: "go",
            command: "gopls",
            args: &[],
        }),
        _ => None,
    }
}

struct Server {
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    _child: Child,
    next_id: u64,
    initialized: bool,
    opened: HashSet<PathBuf>,
    root: PathBuf,
}

#[derive(Default)]
pub struct LspManager {
    servers: Mutex<HashMap<String, Server>>,
}

impl LspManager {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn diagnostics(&self, cwd: &Path, path: &Path) -> Option<String> {
        match self.diagnostics_inner(cwd, path).await {
            Ok(result) => result,
            Err(err) => Some(format!("[lsp] {err:#}")),
        }
    }

    async fn diagnostics_inner(&self, cwd: &Path, path: &Path) -> Result<Option<String>> {
        let config = match server_for(path) {
            Some(config) => config,
            None => return Ok(None),
        };
        self.diagnostics_for(config, cwd, path).await
    }

    async fn diagnostics_for(
        &self,
        config: ServerConfig,
        cwd: &Path,
        path: &Path,
    ) -> Result<Option<String>> {
        if !command_exists(config.command) {
            return Ok(None);
        }
        let full = if path.is_absolute() {
            path.to_path_buf()
        } else {
            cwd.join(path)
        };
        let text = std::fs::read_to_string(&full)
            .with_context(|| format!("reading {}", full.display()))?;
        let uri = file_uri(&full);

        let mut servers = self.servers.lock().await;
        let mut last_error = None;
        for _ in 0..2 {
            if !servers.contains_key(config.language_id) {
                let server = Server::connect(config, cwd).await?;
                servers.insert(config.language_id.to_string(), server);
            }
            let server = servers
                .get_mut(config.language_id)
                .expect("server was just inserted");
            match server
                .diagnose(config.language_id, &full, &uri, &text)
                .await
            {
                Ok(report) => return Ok(Some(report)),
                Err(err) => {
                    // A cached process is unusable once it exits or its pipe
                    // breaks. Drop it so the retry starts a fresh one instead
                    // of failing forever against a dead server.
                    servers.remove(config.language_id);
                    last_error = Some(err);
                }
            }
        }
        Err(last_error.expect("the loop records the last failure"))
    }
}

impl Server {
    async fn connect(config: ServerConfig, cwd: &Path) -> Result<Self> {
        let mut child = tokio::process::Command::new(config.command)
            .args(config.args)
            .current_dir(cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .with_context(|| format!("spawning {}", config.command))?;
        let stdin = child.stdin.take().context("lsp stdin unavailable")?;
        let stdout = child.stdout.take().context("lsp stdout unavailable")?;
        Ok(Self {
            stdin,
            stdout: BufReader::new(stdout),
            _child: child,
            next_id: 1,
            initialized: false,
            opened: HashSet::new(),
            root: cwd.to_path_buf(),
        })
    }

    async fn send(&mut self, value: &Value) -> Result<()> {
        let body = serde_json::to_string(value)?;
        let message = format!("Content-Length: {}\r\n\r\n{}", body.len(), body);
        self.stdin.write_all(message.as_bytes()).await?;
        self.stdin.flush().await?;
        Ok(())
    }

    async fn notify(&mut self, method: &str, params: Value) -> Result<()> {
        self.send(&json!({ "jsonrpc": "2.0", "method": method, "params": params }))
            .await
    }

    async fn request(&mut self, method: &str, params: Value) -> Result<Value> {
        let id = self.next_id;
        self.next_id += 1;
        self.send(&json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }))
            .await?;
        loop {
            let message = tokio::time::timeout(REQUEST_TIMEOUT, self.read_message())
                .await
                .context("lsp request timed out")??;
            let message = match message {
                Some(message) => message,
                None => bail!("lsp server closed the connection"),
            };
            if message.get("method").is_none()
                && message.get("id").and_then(Value::as_u64) == Some(id)
            {
                if let Some(error) = message.get("error") {
                    bail!("lsp error: {error}");
                }
                return Ok(message.get("result").cloned().unwrap_or(Value::Null));
            }
            self.reply_to_server_request(&message).await?;
        }
    }

    async fn diagnose(
        &mut self,
        language_id: &str,
        full: &Path,
        uri: &str,
        text: &str,
    ) -> Result<String> {
        self.ensure_initialized().await?;
        if self.opened.insert(full.to_path_buf()) {
            self.notify(
                "textDocument/didOpen",
                json!({
                    "textDocument": {
                        "uri": uri,
                        "languageId": language_id,
                        "version": 1,
                        "text": text,
                    }
                }),
            )
            .await?;
        } else {
            self.notify(
                "textDocument/didChange",
                json!({
                    "textDocument": { "uri": uri, "version": 2 },
                    "contentChanges": [ { "text": text } ],
                }),
            )
            .await?;
        }
        let diagnostics = self.collect_diagnostics(uri).await?;
        Ok(format_diagnostics(full, &diagnostics))
    }

    async fn collect_diagnostics(&mut self, uri: &str) -> Result<Vec<Value>> {
        let deadline = tokio::time::Instant::now() + DIAGNOSTIC_TIMEOUT;
        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                return Ok(Vec::new());
            }
            let message = match tokio::time::timeout(remaining, self.read_message()).await {
                Ok(Ok(Some(message))) => message,
                Ok(Ok(None)) => bail!("lsp server closed the connection"),
                Err(_) => return Ok(Vec::new()),
                Ok(Err(err)) => return Err(err),
            };
            if message.get("method").and_then(Value::as_str)
                == Some("textDocument/publishDiagnostics")
            {
                let params = message.get("params").cloned().unwrap_or(Value::Null);
                if params.get("uri").and_then(Value::as_str) == Some(uri) {
                    return Ok(params
                        .get("diagnostics")
                        .and_then(Value::as_array)
                        .cloned()
                        .unwrap_or_default());
                }
            }
            self.reply_to_server_request(&message).await?;
        }
    }

    async fn reply_to_server_request(&mut self, message: &Value) -> Result<()> {
        if message.get("method").is_some() && message.get("id").is_some() {
            let id = message.get("id").cloned().unwrap_or(Value::Null);
            self.send(&json!({ "jsonrpc": "2.0", "id": id, "result": Value::Null }))
                .await?;
        }
        Ok(())
    }

    async fn read_message(&mut self) -> Result<Option<Value>> {
        let mut length: Option<usize> = None;
        loop {
            let mut line = String::new();
            if self.stdout.read_line(&mut line).await? == 0 {
                return Ok(None);
            }
            let trimmed = line.trim_end();
            if trimmed.is_empty() {
                break;
            }
            if let Some(rest) = trimmed.strip_prefix("Content-Length:") {
                length = rest.trim().parse().ok();
            }
        }
        let length = match length {
            Some(length) => length,
            None => bail!("lsp message missing Content-Length"),
        };
        let mut body = vec![0u8; length];
        self.stdout.read_exact(&mut body).await?;
        Ok(Some(serde_json::from_slice(&body)?))
    }

    async fn ensure_initialized(&mut self) -> Result<()> {
        if self.initialized {
            return Ok(());
        }
        let root_uri = file_uri(&self.root);
        self.request(
            "initialize",
            json!({
                "processId": std::process::id(),
                "rootUri": root_uri,
                "capabilities": { "textDocument": { "publishDiagnostics": {} } },
                "workspaceFolders": [ { "uri": root_uri, "name": "workspace" } ],
            }),
        )
        .await?;
        self.notify("initialized", json!({})).await?;
        self.initialized = true;
        Ok(())
    }
}

fn format_diagnostics(path: &Path, diagnostics: &[Value]) -> String {
    if diagnostics.is_empty() {
        return format!("[lsp] no diagnostics for {}", path.display());
    }
    let mut lines = vec![format!(
        "[lsp] {} diagnostic(s) for {}:",
        diagnostics.len(),
        path.display()
    )];
    for diagnostic in diagnostics {
        let severity = match diagnostic.get("severity").and_then(Value::as_u64) {
            Some(1) => "error",
            Some(2) => "warning",
            Some(3) => "info",
            _ => "hint",
        };
        let message = diagnostic
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("");
        let line = diagnostic
            .get("range")
            .and_then(|range| range.get("start"))
            .and_then(|start| start.get("line"))
            .and_then(Value::as_u64)
            .map(|line| line + 1)
            .unwrap_or(0);
        lines.push(format!("  {line}: {severity}: {message}"));
    }
    lines.join("\n")
}

fn file_uri(path: &Path) -> String {
    let path = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    format!("file://{}", path.display())
}

fn command_exists(name: &str) -> bool {
    let Some(path) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&path)
        .any(|dir| dir.join(name).is_file() || dir.join(format!("{name}.exe")).is_file())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A tiny stdio LSP server: it answers `initialize`, sends a diagnostic on
    /// the first `didOpen` and, on its very first run, exits right after so the
    /// cached process dies; later runs stay alive and publish the diagnostic.
    const MOCK_SERVER: &str = r#"
import json
import sys

counter = sys.argv[1]


def send(obj):
    body = json.dumps(obj)
    sys.stdout.write("Content-Length: %d\r\n\r\n%s" % (len(body), body))
    sys.stdout.flush()


def read():
    length = None
    while True:
        line = sys.stdin.buffer.readline()
        if not line:
            return None
        line = line.strip()
        if not line:
            break
        if line.lower().startswith(b"content-length:"):
            length = int(line.split(b":", 1)[1].strip())
    if length is None:
        return None
    body = sys.stdin.buffer.read(length)
    if len(body) < length:
        return None
    return json.loads(body)


try:
    with open(counter, "r") as handle:
        runs = int(handle.read().strip() or "0")
except OSError:
    runs = 0
with open(counter, "w") as handle:
    handle.write(str(runs + 1))

while True:
    message = read()
    if message is None:
        break
    method = message.get("method")
    if method == "initialize":
        send({"jsonrpc": "2.0", "id": message["id"], "result": {"capabilities": {}}})
    elif method in ("textDocument/didOpen", "textDocument/didChange"):
        if runs == 0:
            sys.exit(0)
        uri = message["params"]["textDocument"]["uri"]
        diagnostics = (
            [{"severity": 1, "message": "mock diagnostic", "range": {"start": {"line": 0}}}]
            if method == "textDocument/didOpen"
            else []
        )
        send({"jsonrpc": "2.0", "method": "textDocument/publishDiagnostics", "params": {"uri": uri, "diagnostics": diagnostics}})
    elif "id" in message:
        send({"jsonrpc": "2.0", "id": message["id"], "result": None})
"#;

    #[test]
    fn maps_extensions_to_servers() {
        assert_eq!(server_for(Path::new("a.rs")).unwrap().language_id, "rust");
        assert_eq!(
            server_for(Path::new("a.tsx")).unwrap().language_id,
            "typescript"
        );
        assert!(server_for(Path::new("a.txt")).is_none());
    }

    #[test]
    fn formats_diagnostics() {
        let diagnostics = vec![json!({
            "severity": 1,
            "message": "unexpected token",
            "range": { "start": { "line": 4 } }
        })];
        let text = format_diagnostics(Path::new("src/lib.rs"), &diagnostics);
        assert!(text.contains("1 diagnostic(s)"));
        assert!(text.contains("5: error: unexpected token"));

        let clean = format_diagnostics(Path::new("src/lib.rs"), &[]);
        assert!(clean.contains("no diagnostics"));
    }

    #[tokio::test]
    async fn reconnects_after_the_cached_server_dies() {
        if !command_exists("python3") {
            return;
        }
        let dir = std::env::temp_dir().join(format!("oxide_lsp_{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        let script = dir.join("mock_lsp.py");
        std::fs::write(&script, MOCK_SERVER).unwrap();
        let counter = dir.join("count");
        let source = dir.join("a.rs");
        std::fs::write(&source, "fn main() {}\n").unwrap();

        let script_arg: &'static str =
            Box::leak(script.to_string_lossy().into_owned().into_boxed_str());
        let counter_arg: &'static str =
            Box::leak(counter.to_string_lossy().into_owned().into_boxed_str());
        let config = ServerConfig {
            language_id: "rust",
            command: "python3",
            args: Box::leak(vec![script_arg, counter_arg].into_boxed_slice()),
        };

        let manager = LspManager::new();
        let report = manager
            .diagnostics_for(config, &dir, &source)
            .await
            .expect("diagnostics should recover from a dead server")
            .expect("a report");
        assert!(report.contains("mock diagnostic"), "{report}");
        // The dead first process was evicted and replaced by the live one.
        assert_eq!(manager.servers.lock().await.len(), 1);

        std::fs::remove_dir_all(&dir).ok();
    }
}
