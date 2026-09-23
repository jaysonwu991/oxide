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
#[derive(Clone)]
struct Source {
    label: String,
    path: PathBuf,
    scope: Scope,
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
/// precedence order (later overrides earlier), matching `ecosystem::load`.
fn sources_for(home: Option<&Path>, config: Option<&Path>, root: &Path) -> Vec<Source> {
    let mut list = Vec::new();
    let global = home.map(|home| home.join(".oxide").join("mcp.json"));
    if let Some(home) = home {
        list.push(Source {
            label: "claude (global)".to_string(),
            path: home.join(".claude.json"),
            scope: Scope::Global,
        });
    }
    if let Some(path) = global.clone() {
        list.push(Source {
            label: "global".to_string(),
            path,
            scope: Scope::Global,
        });
    }
    if let Some(config) = config {
        list.push(Source {
            label: "platform global".to_string(),
            path: config.join("Oxide").join("mcp.json"),
            scope: Scope::Global,
        });
    }
    list.push(Source {
        label: "claude (project)".to_string(),
        path: root.join(".mcp.json"),
        scope: Scope::Project,
    });
    // Running from the home directory makes `~/.oxide` the project root as
    // well; that file is already the global source, so reporting it twice would
    // mislabel global servers as project-local (and trust-gated).
    let project = root.join(".oxide").join("mcp.json");
    if global.as_deref() != Some(project.as_path()) {
        list.push(Source {
            label: "project".to_string(),
            path: project,
            scope: Scope::Project,
        });
    }
    list
}

/// Parses an explicit `--scope` value; absent or blank means "search every
/// source", which is different from the `Scope::Project` default used by the
/// write commands.
fn explicit_scope(value: Option<&str>) -> Result<Option<Scope>> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| Scope::parse(Some(value)))
        .transpose()
}

/// The sources a lookup or removal searches, highest precedence first. A pinned
/// scope stays inside its own files, so `--scope project` covers the Claude
/// Code `.mcp.json` as well as `.oxide/mcp.json` and never reaches a global
/// server that happens to share the name.
fn candidates(sources: Vec<Source>, scope: Option<Scope>) -> Vec<Source> {
    let mut list: Vec<Source> = match scope {
        Some(scope) => sources
            .into_iter()
            .filter(|source| source.scope == scope)
            .collect(),
        None => sources,
    };
    list.reverse();
    list
}

fn find_server(sources: &[Source], name: &str) -> Result<Option<(String, Value)>> {
    for source in sources {
        let root = read_file(&source.path)?;
        if let Some(config) = root
            .get("mcpServers")
            .and_then(Value::as_object)
            .and_then(|servers| servers.get(name))
        {
            return Ok(Some((source.label.clone(), config.clone())));
        }
    }
    Ok(None)
}

fn sources(cwd: &Path) -> Vec<Source> {
    let home = dirs::home_dir();
    let config = crate::config::config_dir().and_then(|dir| dir.parent().map(Path::to_path_buf));
    let root = project_root(cwd).unwrap_or_else(|| cwd.to_path_buf());
    sources_for(home.as_deref(), config.as_deref(), &root)
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
    domains: &[String],
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
            if !domains.is_empty() {
                entry.insert("domains".to_string(), json!(domains));
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
            if !domains.is_empty() {
                entry.insert("domains".to_string(), json!(domains));
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
    pub domains: Vec<String>,
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
                "--domains" => {
                    if let Some(value) = iter.next() {
                        self.domains.extend(
                            value
                                .split(',')
                                .map(str::trim)
                                .filter(|d| !d.is_empty())
                                .map(str::to_string),
                        );
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
        &request.domains,
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
/// storing the resulting token under the Oxide config directory.
pub async fn auth(cwd: &Path, scope: Option<String>, name: String) -> Result<()> {
    let scope = explicit_scope(scope.as_deref())?;
    let Some((label, config)) = find_server(&candidates(sources(cwd), scope), &name)? else {
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

pub async fn list(cwd: &Path) -> Result<()> {
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

    let mut statuses = BTreeMap::new();
    let mut probes = tokio::task::JoinSet::new();
    let project_trusted = crate::trust::resolve(
        &crate::trust::TrustStore::load().unwrap_or_default(),
        cwd,
        None,
        crate::config::load_default_project_trust(),
    )
    .is_trusted();
    for (name, (source, config)) in &merged {
        if source.contains("project") && !project_trusted {
            statuses.insert(name.clone(), crate::mcp::McpStatus::NeedsTrust);
            continue;
        }
        match crate::ecosystem::mcp_from_claude(name, config) {
            Some(server) => {
                let name = name.clone();
                probes.spawn(async move { (name, crate::mcp::probe(&server).await) });
            }
            None => {
                statuses.insert(
                    name.clone(),
                    crate::mcp::McpStatus::Error("invalid configuration".to_string()),
                );
            }
        }
    }
    while let Some(result) = probes.join_next().await {
        if let Ok((name, status)) = result {
            statuses.insert(name, status);
        }
    }

    println!("MCP servers ({}):", merged.len());
    for (name, (source, config)) in &merged {
        let (transport, detail) = describe(config);
        let status = statuses
            .get(name)
            .cloned()
            .unwrap_or_else(|| crate::mcp::McpStatus::Error("status check failed".to_string()));
        println!("  {name} [{transport}] {status}");
        println!("      {detail}");
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
    let scope = explicit_scope(scope.as_deref())?;
    match remove_first(&candidates(sources(cwd), scope), &name)? {
        Some((path, label)) => {
            println!(
                "removed MCP server `{name}` from {} ({label})",
                path.display()
            )
        }
        None => println!("no MCP server named `{name}`"),
    }
    Ok(())
}

/// Removes `name` from the first source that defines it, returning where it
/// came from so the caller can report the file that actually changed.
fn remove_first(sources: &[Source], name: &str) -> Result<Option<(PathBuf, String)>> {
    for source in sources {
        if try_remove(&source.path, name)? {
            return Ok(Some((source.path.clone(), source.label.clone())));
        }
    }
    Ok(None)
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
        let suffix = if config.get("oauth").is_some() {
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
    fn home_directory_is_only_reported_as_global() {
        let home = Path::new("/home/u");
        let sources = sources_for(Some(home), None, home);
        assert_eq!(
            sources
                .iter()
                .map(|source| source.label.as_str())
                .collect::<Vec<_>>(),
            vec!["claude (global)", "global", "claude (project)"]
        );
        assert_eq!(sources[1].path, home.join(".oxide").join("mcp.json"));
    }

    #[test]
    fn project_sources_layer_above_global() {
        let home = Path::new("/home/u");
        let root = Path::new("/work/repo");
        let sources = sources_for(Some(home), Some(Path::new("/etc/xdg")), root);
        assert_eq!(
            sources
                .iter()
                .map(|source| source.label.as_str())
                .collect::<Vec<_>>(),
            vec![
                "claude (global)",
                "global",
                "platform global",
                "claude (project)",
                "project"
            ]
        );
        assert_eq!(sources[2].path, Path::new("/etc/xdg/Oxide/mcp.json"));
        assert_eq!(sources[4].path, root.join(".oxide").join("mcp.json"));
    }

    #[test]
    fn a_pinned_scope_covers_both_layouts() {
        let home = Path::new("/home/u");
        let root = Path::new("/work/repo");
        let all = || sources_for(Some(home), Some(Path::new("/etc/xdg")), root);
        let labels = |list: Vec<Source>| {
            list.into_iter()
                .map(|source| source.label)
                .collect::<Vec<_>>()
        };

        assert_eq!(
            labels(candidates(all(), Some(Scope::Project))),
            vec!["project", "claude (project)"]
        );
        assert_eq!(
            labels(candidates(all(), Some(Scope::Global))),
            vec!["platform global", "global", "claude (global)"]
        );
        assert_eq!(
            labels(candidates(all(), None)),
            vec![
                "project",
                "claude (project)",
                "platform global",
                "global",
                "claude (global)"
            ]
        );
    }

    #[test]
    fn a_pinned_scope_resolves_a_name_shared_with_the_other_scope() {
        let dir = temp_dir("scope_lookup");
        let home = dir.join("home");
        let root = dir.join("repo");
        let server = r#"{"type":"http","url":"https://mcp.newrelic.com/mcp/"}"#;
        std::fs::create_dir_all(home.join(".oxide")).unwrap();
        std::fs::create_dir_all(root.join(".oxide")).unwrap();
        std::fs::write(
            root.join(".mcp.json"),
            format!(r#"{{"mcpServers":{{"newrelic":{server}}}}}"#),
        )
        .unwrap();
        std::fs::write(
            home.join(".oxide").join("mcp.json"),
            format!(r#"{{"mcpServers":{{"newrelic":{server}}}}}"#),
        )
        .unwrap();

        let all = sources_for(Some(&home), None, &root);
        let found = find_server(&candidates(all.clone(), Some(Scope::Project)), "newrelic")
            .unwrap()
            .unwrap();
        assert_eq!(found.0, "claude (project)");
        let found = find_server(&candidates(all.clone(), Some(Scope::Global)), "newrelic")
            .unwrap()
            .unwrap();
        assert_eq!(found.0, "global");

        let removed = remove_first(&candidates(all, Some(Scope::Project)), "newrelic").unwrap();
        assert_eq!(removed.unwrap().1, "claude (project)");
        let global: Value = serde_json::from_str(
            &std::fs::read_to_string(home.join(".oxide").join("mcp.json")).unwrap(),
        )
        .unwrap();
        assert!(global["mcpServers"].get("newrelic").is_some());

        std::fs::remove_dir_all(&dir).ok();
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
    fn explicit_scope_distinguishes_absent_from_project() {
        assert_eq!(explicit_scope(None).unwrap(), None);
        assert_eq!(explicit_scope(Some("  ")).unwrap(), None);
        assert_eq!(
            explicit_scope(Some("project")).unwrap(),
            Some(Scope::Project)
        );
        assert_eq!(explicit_scope(Some("user")).unwrap(), Some(Scope::Global));
        assert!(explicit_scope(Some("bogus")).is_err());
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
            domains: Vec::new(),
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
            domains: Vec::new(),
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
            &["docs.example.com".to_string()],
            None,
            OAuthArgs::default(),
        )
        .unwrap();
        assert_eq!(stdio["command"], "npx");
        assert_eq!(stdio["args"], json!(["-y", "server-fs"]));
        assert_eq!(stdio["env"]["TOKEN"], "abc");
        assert_eq!(stdio["domains"], json!(["docs.example.com"]));

        let http = build_entry(
            "http",
            &["https://example.com/mcp".to_string()],
            &[],
            &["Authorization=Bearer x".to_string()],
            &[],
            None,
            OAuthArgs::default(),
        )
        .unwrap();
        assert_eq!(http["url"], "https://example.com/mcp");
        assert_eq!(http["headers"]["Authorization"], "Bearer x");
        assert!(http.get("oauth").is_none());

        assert!(build_entry("stdio", &[], &[], &[], &[], None, OAuthArgs::default()).is_err());
        assert!(build_entry("http", &[], &[], &[], &[], None, OAuthArgs::default()).is_err());
        assert!(build_entry(
            "carrier-pigeon",
            &[],
            &[],
            &[],
            &[],
            None,
            OAuthArgs::default()
        )
        .is_err());
    }

    #[test]
    fn builds_oauth_remote_entry() {
        let entry = build_entry(
            "http",
            &["https://example.com/mcp".to_string()],
            &[],
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
                domains: Vec::new(),
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
