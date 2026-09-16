use crate::ecosystem::project_root;
use anyhow::{bail, Context, Result};
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Configuration scope for `oxide mcp` commands. `project` writes to the
/// project's `.oxide/mcp.json`; `global` writes to `~/.oxide/mcp.json`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    Project,
    Global,
}

impl Scope {
    fn parse(value: Option<&str>) -> Result<Self> {
        match value.map(str::trim).unwrap_or("project") {
            "" | "project" | "local" => Ok(Scope::Project),
            "global" | "user" => Ok(Scope::Global),
            other => bail!("unknown scope `{other}` (expected `project` or `global`)"),
        }
    }

    fn label(self) -> &'static str {
        match self {
            Scope::Project => "project",
            Scope::Global => "global",
        }
    }
}

/// A server definition discovered in one of the config files oxide reads.
struct Source {
    label: String,
    path: PathBuf,
}

fn global_path() -> Result<PathBuf> {
    let home = dirs::home_dir().context("resolving home directory")?;
    Ok(home.join(".oxide").join("mcp.json"))
}

fn project_path(cwd: &Path) -> PathBuf {
    let root = project_root(cwd).unwrap_or_else(|| cwd.to_path_buf());
    root.join(".oxide").join("mcp.json")
}

fn path_for(scope: Scope, cwd: &Path) -> Result<PathBuf> {
    match scope {
        Scope::Project => Ok(project_path(cwd)),
        Scope::Global => global_path(),
    }
}

/// Config files visible to the `oxide mcp` management commands, in ascending
/// precedence order (later overrides earlier). The runtime additionally reads
/// `<platform-config>/oxide/mcp.json` through `ecosystem::load`.
fn sources(cwd: &Path) -> Vec<Source> {
    let mut list = Vec::new();
    if let Some(home) = dirs::home_dir() {
        list.push(Source {
            label: "claude (global)".to_string(),
            path: home.join(".claude.json"),
        });
        list.push(Source {
            label: "global".to_string(),
            path: home.join(".oxide").join("mcp.json"),
        });
    }
    let root = project_root(cwd).unwrap_or_else(|| cwd.to_path_buf());
    list.push(Source {
        label: "claude (project)".to_string(),
        path: root.join(".mcp.json"),
    });
    list.push(Source {
        label: "project".to_string(),
        path: root.join(".oxide").join("mcp.json"),
    });
    list
}

fn read_file(path: &Path) -> Result<Value> {
    if !path.exists() {
        return Ok(json!({}));
    }
    let text =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    if text.trim().is_empty() {
        return Ok(json!({}));
    }
    serde_json::from_str(&text).with_context(|| format!("parsing {}", path.display()))
}

fn servers_mut(root: &mut Value) -> &mut Map<String, Value> {
    if !root.is_object() {
        *root = json!({});
    }
    let object = root.as_object_mut().expect("object");
    if !object.get("mcpServers").is_some_and(Value::is_object) {
        object.insert("mcpServers".to_string(), json!({}));
    }
    object
        .get_mut("mcpServers")
        .and_then(Value::as_object_mut)
        .expect("mcpServers object")
}

fn write_file(path: &Path, value: &Value) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    let text = serde_json::to_string_pretty(value)?;
    std::fs::write(path, format!("{text}\n")).with_context(|| format!("writing {}", path.display()))
}

fn parse_pairs(values: &[String], flag: &str) -> Result<Map<String, Value>> {
    let mut map = Map::new();
    for raw in values {
        let (key, value) = raw
            .split_once('=')
            .with_context(|| format!("invalid {flag} `{raw}` (expected KEY=VALUE)"))?;
        let key = key.trim();
        if key.is_empty() {
            bail!("invalid {flag} `{raw}` (empty key)");
        }
        map.insert(key.to_string(), json!(value));
    }
    Ok(map)
}

#[derive(Default)]
struct OAuthArgs {
    client_id: Option<String>,
    client_secret: Option<String>,
    callback_port: Option<u16>,
    scopes: Vec<String>,
    redirect_uri: Option<String>,
}

fn build_oauth(args: OAuthArgs) -> Option<Value> {
    let mut entry = Map::new();
    if let Some(client_id) = args.client_id {
        entry.insert("clientId".to_string(), json!(client_id));
    }
    if let Some(client_secret) = args.client_secret {
        entry.insert("clientSecret".to_string(), json!(client_secret));
    }
    if let Some(port) = args.callback_port {
        entry.insert("callbackPort".to_string(), json!(port));
    }
    if !args.scopes.is_empty() {
        entry.insert("scopes".to_string(), json!(args.scopes));
    }
    if let Some(redirect_uri) = args.redirect_uri {
        entry.insert("redirectUri".to_string(), json!(redirect_uri));
    }
    if entry.is_empty() {
        None
    } else {
        Some(Value::Object(entry))
    }
}

fn build_entry(
    transport: &str,
    command: &[String],
    env: &[String],
    header: &[String],
    server_cwd: Option<String>,
    oauth: OAuthArgs,
) -> Result<Value> {
    match transport {
        "" | "stdio" => {
            if command.is_empty() {
                bail!(
                    "a command is required for stdio servers: `oxide mcp add <name> <command> [args...]`"
                );
            }
            let mut entry = Map::new();
            entry.insert("command".to_string(), json!(command[0]));
            if command.len() > 1 {
                entry.insert("args".to_string(), json!(&command[1..]));
            }
            let environment = parse_pairs(env, "--env")?;
            if !environment.is_empty() {
                entry.insert("env".to_string(), Value::Object(environment));
            }
            if let Some(dir) = server_cwd {
                entry.insert("cwd".to_string(), json!(dir));
            }
            Ok(Value::Object(entry))
        }
        "http" | "sse" => {
            if command.len() != 1 {
                bail!("expected exactly one URL for `--transport {transport}`");
            }
            let mut entry = Map::new();
            entry.insert("type".to_string(), json!(transport));
            entry.insert("url".to_string(), json!(command[0]));
            let headers = parse_pairs(header, "--header")?;
            if !headers.is_empty() {
                entry.insert("headers".to_string(), Value::Object(headers));
            }
            if let Some(oauth) = build_oauth(oauth) {
                entry.insert("oauth".to_string(), oauth);
            }
            Ok(Value::Object(entry))
        }
        other => bail!("unknown transport `{other}` (expected `stdio` or `http`)"),
    }
}

fn validate_name(name: &str) -> Result<()> {
    if name.trim().is_empty() {
        bail!("server name cannot be empty");
    }
    if name.contains(char::is_whitespace) {
        bail!("server name `{name}` cannot contain whitespace");
    }
    Ok(())
}

fn upsert(cwd: &Path, scope: Scope, name: &str, entry: Value) -> Result<PathBuf> {
    validate_name(name)?;
    let path = path_for(scope, cwd)?;
    let mut root = read_file(&path)?;
    servers_mut(&mut root).insert(name.to_string(), entry);
    write_file(&path, &root)?;
    Ok(path)
}

/// Parameters collected from `oxide mcp add`.
pub struct AddRequest {
    pub scope: Option<String>,
    pub transport: Option<String>,
    pub name: String,
    pub command: Vec<String>,
    pub env: Vec<String>,
    pub header: Vec<String>,
    pub cwd: Option<String>,
    pub oauth_client_id: Option<String>,
    pub oauth_client_secret: Option<String>,
    pub callback_port: Option<u16>,
    pub oauth_scope: Vec<String>,
    pub redirect_uri: Option<String>,
}

// The `command` positional accepts hyphen values so server arguments like
// `-y` work. That means clap leaves any options placed after the server name
// inside `command`; pull them back into their fields so both
// `add --env K=V name cmd` and `add name cmd --env K=V` behave the same.
impl AddRequest {
    fn absorb_trailing_options(mut self) -> Self {
        let mut args = Vec::new();
        let mut iter = std::mem::take(&mut self.command).into_iter();
        while let Some(token) = iter.next() {
            match token.as_str() {
                "--transport" => {
                    if let Some(value) = iter.next() {
                        self.transport = Some(value);
                    }
                }
                "--env" => {
                    if let Some(value) = iter.next() {
                        self.env.push(value);
                    }
                }
                "--header" => {
                    if let Some(value) = iter.next() {
                        self.header.push(value);
                    }
                }
                "--cwd" => {
                    if let Some(value) = iter.next() {
                        self.cwd = Some(value);
                    }
                }
                "--scope" | "-s" => {
                    if let Some(value) = iter.next() {
                        self.scope = Some(value);
                    }
                }
                "--oauth-client-id" => {
                    if let Some(value) = iter.next() {
                        self.oauth_client_id = Some(value);
                    }
                }
                "--oauth-client-secret" => {
                    if let Some(value) = iter.next() {
                        self.oauth_client_secret = Some(value);
                    }
                }
                "--callback-port" => {
                    if let Some(value) = iter.next() {
                        self.callback_port = value.parse().ok();
                    }
                }
                "--oauth-scope" => {
                    if let Some(value) = iter.next() {
                        self.oauth_scope.push(value);
                    }
                }
                "--redirect-uri" => {
                    if let Some(value) = iter.next() {
                        self.redirect_uri = Some(value);
                    }
                }
                _ => args.push(token),
            }
        }
        self.command = args;
        self
    }
}

pub fn add(cwd: &Path, request: AddRequest) -> Result<()> {
    let request = request.absorb_trailing_options();
    let scope = Scope::parse(request.scope.as_deref())?;
    let transport = request.transport.as_deref().unwrap_or("stdio");
    let entry = build_entry(
        transport,
        &request.command,
        &request.env,
        &request.header,
        request.cwd,
        OAuthArgs {
            client_id: request.oauth_client_id,
            client_secret: request.oauth_client_secret,
            callback_port: request.callback_port,
            scopes: request.oauth_scope,
            redirect_uri: request.redirect_uri,
        },
    )?;
    let path = upsert(cwd, scope, &request.name, entry)?;
    println!(
        "added MCP server `{}` to {} ({})",
        request.name,
        path.display(),
        scope.label()
    );
    Ok(())
}

/// Run the OAuth authorization-code flow for a configured remote server,
/// storing the resulting token under the oxide config directory.
pub async fn auth(cwd: &Path, scope: Option<String>, name: String) -> Result<()> {
    Scope::parse(scope.as_deref())?;
    let mut found = None;
    for source in sources(cwd).into_iter().rev() {
        let root = read_file(&source.path)?;
        if let Some(config) = root
            .get("mcpServers")
            .and_then(Value::as_object)
            .and_then(|servers| servers.get(&name))
        {
            found = Some((source.label.clone(), config.clone()));
            break;
        }
    }
    let Some((label, config)) = found else {
        bail!("no MCP server named `{name}`");
    };
    let url = config
        .get("url")
        .and_then(Value::as_str)
        .with_context(|| format!("MCP server `{name}` is not a remote (http) server"))?;
    let oauth = crate::ecosystem::parse_oauth(config.get("oauth")).unwrap_or_default();
    let state = crate::mcp_oauth::OAuthState::new(&name, &oauth, url);
    state.ensure_authorized(true).await?;
    println!("authorized MCP server `{name}` ({label})");
    Ok(())
}

pub fn add_json(cwd: &Path, scope: Option<String>, name: String, raw: &str) -> Result<()> {
    let value: Value = serde_json::from_str(raw).context("parsing server JSON")?;
    if !value.is_object() {
        bail!("server JSON must be an object");
    }
    if value.get("command").is_none() && value.get("url").is_none() {
        bail!("server JSON must contain `command` (stdio) or `url` (http)");
    }
    let scope = Scope::parse(scope.as_deref())?;
    let path = upsert(cwd, scope, &name, value)?;
    println!(
        "added MCP server `{name}` to {} ({})",
        path.display(),
        scope.label()
    );
    Ok(())
}

pub fn list(cwd: &Path) -> Result<()> {
    let mut merged: BTreeMap<String, (String, Value)> = BTreeMap::new();
    for source in sources(cwd) {
        let Ok(root) = read_file(&source.path) else {
            continue;
        };
        let Some(servers) = root.get("mcpServers").and_then(Value::as_object) else {
            continue;
        };
        for (name, config) in servers {
            merged.insert(name.clone(), (source.label.clone(), config.clone()));
        }
    }
    if merged.is_empty() {
        println!("no MCP servers configured");
        return Ok(());
    }
    println!("MCP servers ({}):", merged.len());
    for (name, (source, config)) in &merged {
        let (transport, detail) = describe(config);
        println!("  {name} [{transport}] {detail}");
        println!("      source: {source}");
    }
    Ok(())
}

pub fn get(cwd: &Path, name: &str) -> Result<()> {
    for source in sources(cwd).into_iter().rev() {
        let root = read_file(&source.path)?;
        let config = root
            .get("mcpServers")
            .and_then(Value::as_object)
            .and_then(|servers| servers.get(name));
        if let Some(config) = config {
            println!("{name} ({})", source.label);
            println!("{}", serde_json::to_string_pretty(config)?);
            return Ok(());
        }
    }
    bail!("no MCP server named `{name}`");
}

pub fn remove(cwd: &Path, scope: Option<String>, name: String) -> Result<()> {
    let scope = Scope::parse(scope.as_deref())?;
    let path = path_for(scope, cwd)?;
    if try_remove(&path, &name)? {
        println!(
            "removed MCP server `{name}` from {} ({})",
            path.display(),
            scope.label()
        );
        return Ok(());
    }
    for source in sources(cwd).into_iter().rev() {
        if source.path == path {
            continue;
        }
        if try_remove(&source.path, &name)? {
            println!(
                "removed MCP server `{name}` from {} ({})",
                source.path.display(),
                source.label
            );
            return Ok(());
        }
    }
    println!("no MCP server named `{name}`");
    Ok(())
}

fn try_remove(path: &Path, name: &str) -> Result<bool> {
    let mut root = read_file(path)?;
    let Some(servers) = root.get_mut("mcpServers").and_then(Value::as_object_mut) else {
        return Ok(false);
    };
    if servers.remove(name).is_none() {
        return Ok(false);
    }
    write_file(path, &root)?;
    Ok(true)
}

fn describe(config: &Value) -> (&'static str, String) {
    if let Some(url) = config.get("url").and_then(Value::as_str) {
        let suffix =
            if config.get("oauth").is_some() || crate::ecosystem::known_oauth(url).is_some() {
                " (oauth)"
            } else {
                ""
            };
        return ("http", format!("{url}{suffix}"));
    }
    if let Some(command) = config.get("command").and_then(Value::as_str) {
        let mut parts = vec![command.to_string()];
        if let Some(args) = config.get("args").and_then(Value::as_array) {
            parts.extend(args.iter().filter_map(Value::as_str).map(str::to_string));
        }
        return ("stdio", parts.join(" "));
    }
    ("unknown", String::new())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("oxide_mcp_{tag}_{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn parses_scope_aliases() {
        assert_eq!(Scope::parse(None).unwrap(), Scope::Project);
        assert_eq!(Scope::parse(Some("project")).unwrap(), Scope::Project);
        assert_eq!(Scope::parse(Some("user")).unwrap(), Scope::Global);
        assert_eq!(Scope::parse(Some("global")).unwrap(), Scope::Global);
        assert!(Scope::parse(Some("bogus")).is_err());
    }

    #[test]
    fn absorbs_options_placed_after_the_command() {
        let request = AddRequest {
            scope: None,
            transport: None,
            name: "remote".to_string(),
            command: vec![
                "https://example.com/mcp".to_string(),
                "--transport".to_string(),
                "http".to_string(),
                "--header".to_string(),
                "Authorization=Bearer abc".to_string(),
                "--scope".to_string(),
                "global".to_string(),
            ],
            env: Vec::new(),
            header: Vec::new(),
            cwd: None,
            oauth_client_id: None,
            oauth_client_secret: None,
            callback_port: None,
            oauth_scope: Vec::new(),
            redirect_uri: None,
        }
        .absorb_trailing_options();

        assert_eq!(request.command, vec!["https://example.com/mcp"]);
        assert_eq!(request.transport.as_deref(), Some("http"));
        assert_eq!(request.header, vec!["Authorization=Bearer abc"]);
        assert_eq!(request.scope.as_deref(), Some("global"));
        assert!(request.env.is_empty());
        assert!(request.cwd.is_none());
    }

    #[test]
    fn absorbs_oauth_options_placed_after_the_command() {
        let request = AddRequest {
            scope: None,
            transport: Some("http".to_string()),
            name: "remote".to_string(),
            command: vec![
                "https://example.com/mcp".to_string(),
                "--oauth-client-id".to_string(),
                "client-123".to_string(),
                "--callback-port".to_string(),
                "3000".to_string(),
                "--oauth-scope".to_string(),
                "files:read".to_string(),
            ],
            env: Vec::new(),
            header: Vec::new(),
            cwd: None,
            oauth_client_id: None,
            oauth_client_secret: None,
            callback_port: None,
            oauth_scope: Vec::new(),
            redirect_uri: None,
        }
        .absorb_trailing_options();

        assert_eq!(request.command, vec!["https://example.com/mcp"]);
        assert_eq!(request.oauth_client_id.as_deref(), Some("client-123"));
        assert_eq!(request.callback_port, Some(3000));
        assert_eq!(request.oauth_scope, vec!["files:read"]);
    }

    #[test]
    fn builds_stdio_and_http_entries() {
        let stdio = build_entry(
            "stdio",
            &["npx".to_string(), "-y".to_string(), "server-fs".to_string()],
            &["TOKEN=abc".to_string()],
            &[],
            None,
            OAuthArgs::default(),
        )
        .unwrap();
        assert_eq!(stdio["command"], "npx");
        assert_eq!(stdio["args"], json!(["-y", "server-fs"]));
        assert_eq!(stdio["env"]["TOKEN"], "abc");

        let http = build_entry(
            "http",
            &["https://example.com/mcp".to_string()],
            &[],
            &["Authorization=Bearer x".to_string()],
            None,
            OAuthArgs::default(),
        )
        .unwrap();
        assert_eq!(http["url"], "https://example.com/mcp");
        assert_eq!(http["headers"]["Authorization"], "Bearer x");
        assert!(http.get("oauth").is_none());

        assert!(build_entry("stdio", &[], &[], &[], None, OAuthArgs::default()).is_err());
        assert!(build_entry("http", &[], &[], &[], None, OAuthArgs::default()).is_err());
        assert!(build_entry("carrier-pigeon", &[], &[], &[], None, OAuthArgs::default()).is_err());
    }

    #[test]
    fn builds_oauth_remote_entry() {
        let entry = build_entry(
            "http",
            &["https://example.com/mcp".to_string()],
            &[],
            &[],
            None,
            OAuthArgs {
                client_id: Some("client-123".to_string()),
                client_secret: None,
                callback_port: Some(3000),
                scopes: Vec::new(),
                redirect_uri: None,
            },
        )
        .unwrap();
        assert_eq!(entry["type"], "http");
        assert_eq!(entry["url"], "https://example.com/mcp");
        assert_eq!(entry["oauth"]["clientId"], "client-123");
        assert_eq!(entry["oauth"]["callbackPort"], 3000);
        assert!(entry["oauth"].get("clientSecret").is_none());
    }

    #[test]
    fn add_and_remove_round_trip() {
        let dir = temp_dir("roundtrip");
        std::fs::create_dir_all(dir.join(".git")).unwrap();

        add(
            &dir,
            AddRequest {
                scope: None,
                transport: None,
                name: "fs".to_string(),
                command: vec!["npx".to_string(), "-y".to_string(), "server-fs".to_string()],
                env: Vec::new(),
                header: Vec::new(),
                cwd: None,
                oauth_client_id: None,
                oauth_client_secret: None,
                callback_port: None,
                oauth_scope: Vec::new(),
                redirect_uri: None,
            },
        )
        .unwrap();

        let path = dir.join(".oxide").join("mcp.json");
        let root: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(root["mcpServers"]["fs"]["command"], "npx");

        remove(&dir, None, "fs".to_string()).unwrap();
        let root: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert!(root["mcpServers"].get("fs").is_none());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn add_json_requires_command_or_url() {
        let dir = temp_dir("addjson");
        std::fs::create_dir_all(dir.join(".git")).unwrap();
        assert!(add_json(&dir, None, "bad".to_string(), "{}").is_err());
        assert!(add_json(&dir, None, "good".to_string(), r#"{"command":"npx"}"#).is_ok());
        std::fs::remove_dir_all(&dir).ok();
    }
}
