use crate::ecosystem::{McpKind, McpServer};
use crate::llm::{FunctionSpec, ToolSpec};
use anyhow::{bail, Context, Result};
use reqwest::header::{HeaderMap, HeaderName, HeaderValue, ACCEPT, CONTENT_TYPE};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::process::Stdio;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout};
use tokio::sync::Mutex;

const PROTOCOL_VERSION: &str = "2024-11-05";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);

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
    connection: Mutex<McpConnection>,
    tools: Vec<McpTool>,
}

#[derive(Default)]
pub struct McpRegistry {
    servers: Vec<McpServerHandle>,
    index: HashMap<String, (usize, usize)>,
}

impl McpRegistry {
    pub async fn connect(servers: &[McpServer]) -> Self {
        let mut handles = Vec::new();
        for server in servers {
            if !server.enabled {
                continue;
            }
            match McpConnection::connect(server).await {
                Ok(mut connection) => match connection.list_tools().await {
                    Ok(raw) => {
                        let tools = raw
                            .into_iter()
                            .map(|tool| McpTool {
                                exposed: expose(&server.name, &tool.name),
                                original: tool.name,
                                description: tool.description,
                                schema: tool.schema,
                            })
                            .collect();
                        handles.push(McpServerHandle {
                            name: server.name.clone(),
                            connection: Mutex::new(connection),
                            tools,
                        });
                    }
                    Err(err) => {
                        eprintln!("[mcp] `{}` tools/list failed: {err:#}", server.name);
                    }
                },
                Err(err) => {
                    eprintln!("[mcp] `{}` failed to start: {err:#}", server.name);
                }
            }
        }

        let mut index = HashMap::new();
        for (server_idx, handle) in handles.iter().enumerate() {
            for (tool_idx, tool) in handle.tools.iter().enumerate() {
                index.insert(tool.exposed.clone(), (server_idx, tool_idx));
            }
        }

        Self {
            servers: handles,
            index,
        }
    }

    pub fn tool_specs(&self) -> Vec<ToolSpec> {
        self.servers
            .iter()
            .flat_map(|server| {
                server.tools.iter().map(|tool| ToolSpec {
                    kind: "function",
                    function: FunctionSpec {
                        name: tool.exposed.clone(),
                        description: format!("[mcp:{}] {}", server.name, tool.description),
                        parameters: tool.schema.clone(),
                    },
                })
            })
            .collect()
    }

    pub fn is_tool(&self, name: &str) -> bool {
        self.index.contains_key(name)
    }

    pub fn server_count(&self) -> usize {
        self.servers.len()
    }

    pub fn tool_count(&self) -> usize {
        self.index.len()
    }

    pub async fn call(&self, name: &str, arguments: Value) -> Result<String> {
        let (server_idx, tool_idx) = *self
            .index
            .get(name)
            .with_context(|| format!("unknown MCP tool `{name}`"))?;
        let handle = &self.servers[server_idx];
        let tool = &handle.tools[tool_idx];
        let mut connection = handle.connection.lock().await;
        connection
            .call_tool(&tool.original, arguments)
            .await
            .with_context(|| format!("MCP tool `{}` on `{}`", tool.original, handle.name))
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
            McpKind::Remote { url, headers } => {
                let mut map = HeaderMap::new();
                for (key, value) in headers {
                    let name = HeaderName::try_from(key.as_str())
                        .with_context(|| format!("invalid MCP header name `{key}`"))?;
                    let value = HeaderValue::from_str(&interpolate(value))
                        .with_context(|| format!("invalid MCP header value for `{key}`"))?;
                    map.insert(name, value);
                }
                Transport::Remote {
                    client: reqwest::Client::new(),
                    url: url.clone(),
                    headers: map,
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
            } => {
                post_json(client, url, headers, &payload)
                    .send()
                    .await
                    .context("sending MCP notification")?;
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
            } => {
                let request = post_json(client, url, headers, &payload);
                let response = tokio::time::timeout(REQUEST_TIMEOUT, request.send())
                    .await
                    .map_err(|_| anyhow::anyhow!("MCP server `{}` timed out", self.name))?
                    .with_context(|| format!("calling MCP server `{}`", self.name))?;
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
        if std::process::Command::new("python3")
            .arg("--version")
            .output()
            .is_err()
        {
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

        let registry = McpRegistry::connect(&[server]).await;
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
}
