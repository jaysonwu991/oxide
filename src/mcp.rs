use crate::ecosystem::{McpKind, McpServer};
use crate::llm::{FunctionSpec, ToolSpec};
use crate::mcp_oauth::OAuthState;
use anyhow::{bail, Context, Result};
use reqwest::header::{HeaderMap, HeaderName, HeaderValue, ACCEPT, AUTHORIZATION, CONTENT_TYPE};
use serde_json::{json, Value};
use std::io::IsTerminal;
use std::process::Stdio;
use std::sync::{Arc, RwLock};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout};
use tokio::sync::Mutex;

const PROTOCOL_VERSION: &str = "2024-11-05";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);
const MCP_SESSION_ID: &str = "mcp-session-id";

struct RawTool {
    name: String,
    description: String,
    schema: Value,
}

struct McpTool {
    exposed: String,
    original: String,
    description: String,
    schema: Value,
}

struct McpServerHandle {
    name: String,
    source: String,
    connection: Mutex<McpConnection>,
    tools: Vec<McpTool>,
}

#[derive(Default)]
pub struct McpRegistry {
    configured: Vec<McpServer>,
    servers: RwLock<Vec<Arc<McpServerHandle>>>,
    load_guard: Mutex<()>,
}

impl McpRegistry {
    /// Creates a registry without starting any configured servers. Servers are
    /// connected only when the model selects them through `mcp_load`.
    pub fn new(servers: &[McpServer]) -> Self {
        Self {
            configured: servers
                .iter()
                .filter(|server| server.enabled)
                .cloned()
                .collect(),
            servers: RwLock::new(Vec::new()),
            load_guard: Mutex::new(()),
        }
    }

    /// Eager connection helper retained for focused tests and callers that
    /// explicitly need every server available immediately.
    pub async fn connect(servers: &[McpServer]) -> Self {
        let registry = Self::new(servers);
        for server in registry.configured.clone() {
            if let Err(err) = registry.load(&server.name).await {
                eprintln!("[mcp] `{}` failed to load: {err:#}", server.name);
            }
        }
        registry
    }

    pub fn configured_servers(&self) -> Vec<(String, String)> {
        self.configured
            .iter()
            .map(|server| (server.name.clone(), server_source(server)))
            .collect()
    }

    pub async fn load(&self, name: &str) -> Result<String> {
        let _guard = self.load_guard.lock().await;
        if self
            .servers
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .iter()
            .any(|server| server.name == name)
        {
            return Ok(format!("MCP server `{name}` is already loaded"));
        }

        let server = self
            .configured
            .iter()
            .find(|server| server.name == name)
            .with_context(|| format!("no enabled MCP server named `{name}`"))?;
        let mut connection = McpConnection::connect(server).await?;
        let raw = connection
            .list_tools()
            .await
            .with_context(|| format!("listing tools from MCP server `{name}`"))?;
        let tools: Vec<McpTool> = raw
            .into_iter()
            .map(|tool| McpTool {
                exposed: expose(&server.name, &tool.name),
                original: tool.name,
                description: tool.description,
                schema: tool.schema,
            })
            .collect();
        let count = tools.len();
        self.servers
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push(Arc::new(McpServerHandle {
                name: server.name.clone(),
                source: server_source(server),
                connection: Mutex::new(connection),
                tools,
            }));
        Ok(format!(
            "loaded MCP server `{name}` with {count} tool(s); use its `{name}__*` tools now"
        ))
    }

    pub fn tool_specs(&self) -> Vec<ToolSpec> {
        self.servers
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .iter()
            .flat_map(|server| {
                server.tools.iter().map(|tool| ToolSpec {
                    kind: "function",
                    function: FunctionSpec {
                        name: tool.exposed.clone(),
                        description: format!(
                            "[mcp:{}; {}] {}",
                            server.name, server.source, tool.description
                        ),
                        parameters: tool.schema.clone(),
                    },
                })
            })
            .collect()
    }

    pub fn is_tool(&self, name: &str) -> bool {
        self.servers
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .iter()
            .any(|server| server.tools.iter().any(|tool| tool.exposed == name))
    }

    pub fn server_count(&self) -> usize {
        self.servers
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .len()
    }

    pub fn configured_count(&self) -> usize {
        self.configured.len()
    }

    pub fn tool_count(&self) -> usize {
        self.servers
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .iter()
            .map(|server| server.tools.len())
            .sum()
    }

    pub async fn call(&self, name: &str, arguments: Value) -> Result<String> {
        let (handle, original) = self
            .servers
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .iter()
            .find_map(|server| {
                server
                    .tools
                    .iter()
                    .find(|tool| tool.exposed == name)
                    .map(|tool| (Arc::clone(server), tool.original.clone()))
            })
            .with_context(|| format!("unknown MCP tool `{name}`"))?;
        let mut connection = handle.connection.lock().await;
        connection
            .call_tool(&original, arguments)
            .await
            .with_context(|| format!("MCP tool `{original}` on `{}`", handle.name))
    }
}

fn server_source(server: &McpServer) -> String {
    match &server.kind {
        McpKind::Local { command, .. } => format!("local: {}", command.join(" ")),
        McpKind::Remote { url, .. } => format!("remote: {url}"),
    }
}

struct McpConnection {
    name: String,
    transport: Transport,
    next_id: u64,
}

enum Transport {
    Local {
        stdin: ChildStdin,
        stdout: BufReader<ChildStdout>,
        _child: Child,
    },
    Remote {
        client: reqwest::Client,
        url: String,
        headers: HeaderMap,
        oauth: Option<Arc<OAuthState>>,
        session_id: Option<HeaderValue>,
    },
}

impl McpConnection {
    async fn connect(server: &McpServer) -> Result<Self> {
        let transport = match &server.kind {
            McpKind::Local {
                command,
                environment,
                cwd,
            } => {
                let (program, args) = command.split_first().with_context(|| {
                    format!("MCP server `{}` has an empty command", server.name)
                })?;
                let mut process = tokio::process::Command::new(program);
                process
                    .args(args)
                    .stdin(Stdio::piped())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::null())
                    .kill_on_drop(true);
                for (key, value) in environment {
                    process.env(key, interpolate(value));
                }
                if let Some(dir) = cwd {
                    process.current_dir(dir);
                }
                let mut child = process
                    .spawn()
                    .with_context(|| format!("spawning MCP server `{}`", server.name))?;
                let stdin = child.stdin.take().context("capturing MCP stdin")?;
                let stdout = child.stdout.take().context("capturing MCP stdout")?;
                Transport::Local {
                    stdin,
                    stdout: BufReader::new(stdout),
                    _child: child,
                }
            }
            McpKind::Remote {
                url,
                headers,
                oauth,
            } => {
                let mut map = HeaderMap::new();
                for (key, value) in headers {
                    let name = HeaderName::try_from(key.as_str())
                        .with_context(|| format!("invalid MCP header name `{key}`"))?;
                    let value = HeaderValue::from_str(&interpolate(value))
                        .with_context(|| format!("invalid MCP header value for `{key}`"))?;
                    map.insert(name, value);
                }
                let oauth = {
                    let config = oauth.clone().unwrap_or_default();
                    let state = Arc::new(OAuthState::new(&server.name, &config, url));
                    if oauth.is_some() {
                        state
                            .ensure_authorized(std::io::stdin().is_terminal())
                            .await
                            .with_context(|| format!("authorizing MCP server `{}`", server.name))?;
                    }
                    Some(state)
                };
                Transport::Remote {
                    client: reqwest::Client::new(),
                    url: url.clone(),
                    headers: map,
                    oauth,
                    session_id: None,
                }
            }
        };

        let mut connection = Self {
            name: server.name.clone(),
            transport,
            next_id: 1,
        };
        connection.initialize().await?;
        Ok(connection)
    }

    async fn initialize(&mut self) -> Result<()> {
        let params = json!({
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": {},
            "clientInfo": { "name": "oxide", "version": env!("CARGO_PKG_VERSION") },
        });
        self.request("initialize", params).await?;
        self.notify("notifications/initialized", json!({})).await?;
        Ok(())
    }

    async fn list_tools(&mut self) -> Result<Vec<RawTool>> {
        let result = self.request("tools/list", json!({})).await?;
        let tools = result
            .get("tools")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        Ok(tools
            .into_iter()
            .filter_map(|tool| {
                let name = tool.get("name").and_then(Value::as_str)?.to_string();
                let description = tool
                    .get("description")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                let schema = tool
                    .get("inputSchema")
                    .cloned()
                    .unwrap_or_else(|| json!({ "type": "object" }));
                Some(RawTool {
                    name,
                    description,
                    schema,
                })
            })
            .collect())
    }

    async fn call_tool(&mut self, name: &str, arguments: Value) -> Result<String> {
        let result = self
            .request(
                "tools/call",
                json!({ "name": name, "arguments": arguments }),
            )
            .await?;
        let mut text = format_content(&result);
        if result.get("isError").and_then(Value::as_bool) == Some(true) {
            text = format!("error: {text}");
        }
        Ok(text)
    }

    async fn notify(&mut self, method: &str, params: Value) -> Result<()> {
        let payload = json!({ "jsonrpc": "2.0", "method": method, "params": params });
        let mut line = serde_json::to_string(&payload)?;
        line.push('\n');
        match &mut self.transport {
            Transport::Local { stdin, .. } => {
                stdin.write_all(line.as_bytes()).await?;
                stdin.flush().await?;
                Ok(())
            }
            Transport::Remote {
                client,
                url,
                headers,
                oauth,
                session_id,
            } => {
                let headers = remote_headers(oauth, headers, session_id.as_ref()).await?;
                let mut response = post_json(client, url, &headers, &payload)
                    .send()
                    .await
                    .context("sending MCP notification")?;
                if response.status() == reqwest::StatusCode::UNAUTHORIZED {
                    if let Some(state) = oauth {
                        state.note_unauthorized(response.headers()).await;
                        state
                            .ensure_authorized(std::io::stdin().is_terminal())
                            .await
                            .with_context(|| format!("authorizing MCP server `{}`", self.name))?;
                        let headers = remote_headers(oauth, &headers, session_id.as_ref()).await?;
                        response = post_json(client, url, &headers, &payload)
                            .send()
                            .await
                            .context("retrying MCP notification after authorization")?;
                    }
                }
                if !response.status().is_success() {
                    bail!("MCP server `{}` returned {}", self.name, response.status());
                }
                Ok(())
            }
        }
    }

    async fn request(&mut self, method: &str, params: Value) -> Result<Value> {
        let id = self.next_id;
        self.next_id += 1;
        let payload = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });

        match &mut self.transport {
            Transport::Local { stdin, stdout, .. } => {
                let mut line = serde_json::to_string(&payload)?;
                line.push('\n');
                stdin.write_all(line.as_bytes()).await?;
                stdin.flush().await?;
                loop {
                    let mut buffer = String::new();
                    let read = tokio::time::timeout(REQUEST_TIMEOUT, stdout.read_line(&mut buffer))
                        .await
                        .map_err(|_| anyhow::anyhow!("MCP server `{}` timed out", self.name))?
                        .with_context(|| format!("reading from MCP server `{}`", self.name))?;
                    if read == 0 {
                        bail!("MCP server `{}` closed the connection", self.name);
                    }
                    if let Some(result) = parse_response(buffer.trim(), id, &self.name) {
                        return result;
                    }
                }
            }
            Transport::Remote {
                client,
                url,
                headers,
                oauth,
                session_id,
            } => {
                let headers = remote_headers(oauth, headers, session_id.as_ref()).await?;
                let request = post_json(client, url, &headers, &payload);
                let mut response = tokio::time::timeout(REQUEST_TIMEOUT, request.send())
                    .await
                    .map_err(|_| anyhow::anyhow!("MCP server `{}` timed out", self.name))?
                    .with_context(|| format!("calling MCP server `{}`", self.name))?;
                if response.status() == reqwest::StatusCode::UNAUTHORIZED {
                    if let Some(state) = oauth {
                        state.note_unauthorized(response.headers()).await;
                        state
                            .ensure_authorized(std::io::stdin().is_terminal())
                            .await
                            .with_context(|| format!("authorizing MCP server `{}`", self.name))?;
                        let headers = remote_headers(oauth, &headers, session_id.as_ref()).await?;
                        let request = post_json(client, url, &headers, &payload);
                        response = tokio::time::timeout(REQUEST_TIMEOUT, request.send())
                            .await
                            .map_err(|_| anyhow::anyhow!("MCP server `{}` timed out", self.name))?
                            .with_context(|| {
                                format!("calling MCP server `{}` after authorization", self.name)
                            })?;
                    }
                }
                if method == "initialize" && response.status().is_success() {
                    *session_id = response.headers().get(MCP_SESSION_ID).cloned();
                }
                let status = response.status();
                let body = response.text().await.context("reading MCP response")?;
                if !status.is_success() {
                    bail!("MCP server `{}` returned {status}: {body}", self.name);
                }
                for line in body.lines() {
                    let line = line.trim();
                    let data = line.strip_prefix("data:").map(str::trim).unwrap_or(line);
                    if data.is_empty() {
                        continue;
                    }
                    if let Some(result) = parse_response(data, id, &self.name) {
                        return result;
                    }
                }
                bail!(
                    "MCP server `{}` returned no response for `{method}`",
                    self.name
                );
            }
        }
    }
}

async fn remote_headers(
    oauth: &Option<Arc<OAuthState>>,
    base: &HeaderMap,
    session_id: Option<&HeaderValue>,
) -> Result<HeaderMap> {
    let mut map = base.clone();
    if let Some(state) = oauth {
        if let Some(token) = state.access_token_if_available().await {
            let value = HeaderValue::from_str(&format!("Bearer {token}"))
                .context("building MCP authorization header")?;
            map.insert(AUTHORIZATION, value);
        }
    }
    if let Some(session_id) = session_id {
        map.insert(HeaderName::from_static(MCP_SESSION_ID), session_id.clone());
    }
    Ok(map)
}

fn post_json(
    client: &reqwest::Client,
    url: &str,
    headers: &HeaderMap,
    payload: &Value,
) -> reqwest::RequestBuilder {
    let mut map = headers.clone();
    map.entry(CONTENT_TYPE)
        .or_insert_with(|| HeaderValue::from_static("application/json"));
    map.entry(ACCEPT)
        .or_insert_with(|| HeaderValue::from_static("application/json, text/event-stream"));
    client.post(url).headers(map).json(payload)
}

fn parse_response(raw: &str, id: u64, name: &str) -> Option<Result<Value>> {
    if raw.is_empty() {
        return None;
    }
    let message: Value = serde_json::from_str(raw).ok()?;
    if message.get("id").and_then(Value::as_u64) != Some(id) {
        return None;
    }
    if let Some(error) = message.get("error") {
        let code = error.get("code").and_then(Value::as_i64).unwrap_or(0);
        let text = error
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("unknown error");
        return Some(Err(anyhow::anyhow!(
            "MCP server `{name}` error {code}: {text}"
        )));
    }
    Some(Ok(message.get("result").cloned().unwrap_or(Value::Null)))
}

fn format_content(result: &Value) -> String {
    let Some(content) = result.get("content").and_then(Value::as_array) else {
        return result.to_string();
    };
    let parts: Vec<String> = content
        .iter()
        .filter_map(|item| match item.get("type").and_then(Value::as_str) {
            Some("text") => item.get("text").and_then(Value::as_str).map(str::to_string),
            Some(other) => Some(format!("[{other} content]")),
            None => None,
        })
        .collect();
    if parts.is_empty() {
        result.to_string()
    } else {
        parts.join("\n")
    }
}

fn expose(server: &str, tool: &str) -> String {
    format!("{}__{}", sanitize(server), sanitize(tool))
}

fn sanitize(value: &str) -> String {
    value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' {
                ch
            } else {
                '_'
            }
        })
        .collect()
}

fn interpolate(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
    let mut rest = value;
    while let Some(start) = rest.find("{env:") {
        output.push_str(&rest[..start]);
        let after = &rest[start + 5..];
        match after.find('}') {
            Some(end) => {
                let key = &after[..end];
                output.push_str(&std::env::var(key).unwrap_or_default());
                rest = &after[end + 1..];
            }
            None => {
                output.push_str(&rest[start..]);
                rest = "";
            }
        }
    }
    output.push_str(rest);
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    async fn read_http_request(socket: &mut tokio::net::TcpStream) -> (String, Value) {
        let mut bytes = Vec::new();
        let header_end = loop {
            let mut chunk = [0_u8; 1024];
            let count = socket.read(&mut chunk).await.unwrap();
            assert!(count > 0, "client closed before sending HTTP headers");
            bytes.extend_from_slice(&chunk[..count]);
            if let Some(index) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
                break index + 4;
            }
        };
        let headers = String::from_utf8(bytes[..header_end].to_vec()).unwrap();
        let content_length = headers
            .lines()
            .find_map(|line| {
                let (name, value) = line.split_once(':')?;
                name.eq_ignore_ascii_case("content-length")
                    .then(|| value.trim().parse::<usize>().unwrap())
            })
            .unwrap_or(0);
        while bytes.len() < header_end + content_length {
            let mut chunk = [0_u8; 1024];
            let count = socket.read(&mut chunk).await.unwrap();
            assert!(count > 0, "client closed before sending the HTTP body");
            bytes.extend_from_slice(&chunk[..count]);
        }
        let body = serde_json::from_slice(&bytes[header_end..header_end + content_length]).unwrap();
        (headers, body)
    }

    async fn send_http_response(
        socket: &mut tokio::net::TcpStream,
        status: &str,
        body: &str,
        session_id: Option<&str>,
    ) {
        let session_header = session_id
            .map(|id| format!("Mcp-Session-Id: {id}\r\n"))
            .unwrap_or_default();
        let response = format!(
            "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n{session_header}Connection: close\r\n\r\n{body}",
            body.len()
        );
        socket.write_all(response.as_bytes()).await.unwrap();
    }

    #[test]
    fn exposes_namespaced_tool_names() {
        assert_eq!(expose("my-server", "get thing"), "my-server__get_thing");
    }

    #[test]
    fn parses_matching_json_rpc_response() {
        let raw = r#"{"jsonrpc":"2.0","id":3,"result":{"ok":true}}"#;
        let result = parse_response(raw, 3, "test").unwrap().unwrap();
        assert_eq!(result["ok"], true);
        assert!(parse_response(raw, 4, "test").is_none());
    }

    #[test]
    fn reports_json_rpc_errors() {
        let raw = r#"{"jsonrpc":"2.0","id":1,"error":{"code":-32601,"message":"nope"}}"#;
        let err = parse_response(raw, 1, "test").unwrap().unwrap_err();
        assert!(err.to_string().contains("nope"), "{err}");
    }

    #[test]
    fn formats_text_content() {
        let result = json!({ "content": [ { "type": "text", "text": "hello" } ] });
        assert_eq!(format_content(&result), "hello");
    }

    #[test]
    fn interpolates_environment_variables() {
        std::env::set_var("OXIDE_MCP_TEST", "secret");
        assert_eq!(interpolate("Bearer {env:OXIDE_MCP_TEST}"), "Bearer secret");
        assert_eq!(interpolate("plain"), "plain");
    }

    const MOCK_SERVER: &str = r#"
import sys, json
def send(obj):
    sys.stdout.write(json.dumps(obj) + "\n")
    sys.stdout.flush()
for line in sys.stdin:
    line = line.strip()
    if not line:
        continue
    msg = json.loads(line)
    method = msg.get("method")
    if method == "initialize":
        send({"jsonrpc": "2.0", "id": msg["id"], "result": {"protocolVersion": "2024-11-05", "capabilities": {}, "serverInfo": {"name": "mock", "version": "0"}}})
    elif method == "tools/list":
        send({"jsonrpc": "2.0", "id": msg["id"], "result": {"tools": [{"name": "echo", "description": "Echo text back", "inputSchema": {"type": "object", "properties": {"text": {"type": "string"}}}}]}})
    elif method == "tools/call":
        args = msg.get("params", {}).get("arguments", {})
        send({"jsonrpc": "2.0", "id": msg["id"], "result": {"content": [{"type": "text", "text": "echo:" + str(args.get("text", ""))}]}})
"#;

    #[tokio::test]
    async fn connects_to_stdio_server_and_calls_tool() {
        if !matches!(
            std::process::Command::new("python3")
                .arg("--version")
                .output(),
            Ok(output) if output.status.success()
        ) {
            return;
        }

        let dir = std::env::temp_dir().join(format!("oxide_mcp_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let script = dir.join("mock_mcp.py");
        std::fs::write(&script, MOCK_SERVER).unwrap();

        let server = McpServer {
            name: "mock".to_string(),
            enabled: true,
            kind: McpKind::Local {
                command: vec!["python3".to_string(), script.display().to_string()],
                environment: Default::default(),
                cwd: None,
            },
        };

        let registry = McpRegistry::new(&[server]);
        assert_eq!(registry.configured_count(), 1);
        assert_eq!(registry.server_count(), 0);
        assert_eq!(registry.tool_count(), 0);
        registry.load("mock").await.unwrap();
        assert_eq!(registry.server_count(), 1);
        assert_eq!(registry.tool_count(), 1);
        assert!(registry.is_tool("mock__echo"));
        assert_eq!(registry.tool_specs()[0].function.name, "mock__echo");

        let output = registry
            .call("mock__echo", json!({ "text": "hi" }))
            .await
            .unwrap();
        assert_eq!(output, "echo:hi");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn remote_server_reuses_session_and_exposes_tools() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server_task = tokio::spawn(async move {
            for expected_method in [
                "initialize",
                "notifications/initialized",
                "tools/list",
                "tools/call",
            ] {
                let (mut socket, _) = listener.accept().await.unwrap();
                let (headers, body) = read_http_request(&mut socket).await;
                assert_eq!(body["method"], expected_method);
                if expected_method == "initialize" {
                    assert!(!headers.to_ascii_lowercase().contains(MCP_SESSION_ID));
                    let body = json!({
                        "jsonrpc": "2.0",
                        "id": body["id"],
                        "result": {
                            "protocolVersion": PROTOCOL_VERSION,
                            "capabilities": {},
                            "serverInfo": { "name": "remote-mock", "version": "0" }
                        }
                    })
                    .to_string();
                    send_http_response(&mut socket, "200 OK", &body, Some("session-123")).await;
                } else {
                    assert!(headers
                        .to_ascii_lowercase()
                        .contains("mcp-session-id: session-123"));
                    let response = match expected_method {
                        "notifications/initialized" => String::new(),
                        "tools/list" => json!({
                            "jsonrpc": "2.0",
                            "id": body["id"],
                            "result": {
                                "tools": [{
                                    "name": "lookup_document",
                                    "description": "Read a document by URL",
                                    "inputSchema": { "type": "object" }
                                }]
                            }
                        })
                        .to_string(),
                        "tools/call" => json!({
                            "jsonrpc": "2.0",
                            "id": body["id"],
                            "result": {
                                "content": [{ "type": "text", "text": "document content" }]
                            }
                        })
                        .to_string(),
                        _ => unreachable!(),
                    };
                    let status = if expected_method == "notifications/initialized" {
                        "202 Accepted"
                    } else {
                        "200 OK"
                    };
                    send_http_response(&mut socket, status, &response, None).await;
                }
            }
        });

        let url = format!("http://{address}/mcp");
        let server = McpServer {
            name: "documents".to_string(),
            enabled: true,
            kind: McpKind::Remote {
                url: url.clone(),
                headers: Default::default(),
                oauth: None,
            },
        };
        let registry = McpRegistry::connect(&[server]).await;
        assert_eq!(registry.server_count(), 1);
        assert!(registry.is_tool("documents__lookup_document"));
        assert!(registry.tool_specs()[0].function.description.contains(&url));
        let output = registry
            .call(
                "documents__lookup_document",
                json!({ "url": "https://example.com/doc" }),
            )
            .await
            .unwrap();
        assert_eq!(output, "document content");
        server_task.await.unwrap();
    }
}
