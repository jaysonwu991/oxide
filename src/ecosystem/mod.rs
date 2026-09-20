//! Discovery and loading of the configuration ecosystem: instructions,
//! commands, agents, skills, MCP servers and plugins. The native Oxide layout
//! (`.oxide/`, `AGENTS.md`) and the Claude Code layout (`.claude/`, `CLAUDE.md`,
//! `.mcp.json`) are read from the project scope and the user's global scope.
//! Claude-compatible entries load first so native Oxide entries override them;
//! project entries override global entries with the same name.

mod frontmatter;

use serde_json::Value as Json;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Default)]
pub struct Ecosystem {
    pub rules: Vec<Rule>,
    pub memory: Vec<Rule>,
    pub commands: Vec<CommandDef>,
    pub prompt_templates: Vec<PromptTemplate>,
    pub agents: Vec<AgentDef>,
    pub skills: Vec<Skill>,
    pub mcp: Vec<McpServer>,
    /// Single-file JS/TS hook plugins (`.oxide/plugins/*.ts`) and generated
    /// hook shims, run by the hook host.
    pub hooks: Vec<PathBuf>,
    /// Names of the enabled Claude Code-style plugin packages loaded into the
    /// ecosystem (see `plugin_registry`).
    pub plugins: Vec<String>,
    /// Context files that were loaded (`AGENTS.md`/`CLAUDE.md`/overrides),
    /// kept so the TUI can show them in the startup welcome area.
    pub context_files: Vec<PathBuf>,
    /// Replaces the default system prompt (`.oxide/SYSTEM.md`).
    pub system_prompt: Option<String>,
    /// Appended to the default system prompt (`.oxide/APPEND_SYSTEM.md`).
    pub append_system_prompt: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct Rule {
    pub name: String,
    pub content: String,
}

#[derive(Debug, Clone)]
pub struct CommandDef {
    pub name: String,
    pub description: Option<String>,
    pub template: String,
    pub agent: Option<String>,
    pub subtask: bool,
}

/// A reusable prompt snippet invoked as `/<name>`, loaded from `prompts/*.md`
/// (Pi's prompt templates). Frontmatter supplies `description` and an optional
/// `argument-hint` shown in autocomplete.
#[derive(Debug, Clone)]
pub struct PromptTemplate {
    pub name: String,
    pub description: Option<String>,
    pub argument_hint: Option<String>,
    pub body: String,
}

impl PromptTemplate {
    /// Expands positional arguments, `$@`/`$ARGUMENTS`, `${N:-default}` and
    /// `${@:N}`/`${@:N:L}` slices, matching Pi's prompt-template semantics.
    pub fn expand(&self, arguments: &str) -> String {
        crate::ecosystem::expand_prompt_template(&self.body, arguments)
    }
}

impl CommandDef {
    pub fn expand(&self, arguments: &str) -> String {
        let mut output = self.template.replace("$ARGUMENTS", arguments);
        for (index, part) in arguments.split_whitespace().enumerate() {
            output = output.replace(&format!("${}", index + 1), part);
        }
        output
    }
}

/// A leading `/command` resolved against the ecosystem: the expanded prompt
/// plus the agent routing requested by the command's frontmatter.
#[derive(Debug, Clone)]
pub struct ResolvedCommand {
    pub prompt: String,
    pub agent: Option<String>,
    pub subtask: bool,
}

#[derive(Debug, Clone)]
pub struct AgentDef {
    pub name: String,
    pub description: Option<String>,
    pub mode: AgentMode,
    pub permission: Option<Json>,
    pub prompt: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AgentMode {
    Primary,
    #[default]
    Subagent,
    All,
}

#[derive(Debug, Clone)]
pub struct Skill {
    pub name: String,
    pub description: Option<String>,
    pub content: String,
}

#[derive(Debug, Clone)]
pub struct McpServer {
    pub name: String,
    pub enabled: bool,
    pub kind: McpKind,
    /// Hostnames (optionally `*.`-prefixed) this server owns. Used to route
    /// pasted URLs to the right MCP server before falling back to webfetch.
    pub domains: Vec<String>,
}

impl McpServer {
    /// Effective routing domains: the explicit `domains` config when present,
    /// otherwise a best-effort guess from well-known server names.
    pub fn domains(&self) -> Vec<String> {
        if self.domains.is_empty() {
            default_domains(&self.name)
        } else {
            self.domains.clone()
        }
    }
}

#[derive(Debug, Clone)]
pub enum McpKind {
    Local {
        command: Vec<String>,
        environment: BTreeMap<String, String>,
        cwd: Option<String>,
    },
    Remote {
        url: String,
        headers: BTreeMap<String, String>,
        oauth: Option<McpOAuth>,
    },
}

/// OAuth settings for a remote MCP server, mirroring the Claude Code `oauth`
/// block (`clientId`, `callbackPort`) plus optional secret, scopes and redirect.
#[derive(Debug, Clone, Default)]
pub struct McpOAuth {
    pub client_id: Option<String>,
    pub client_secret: Option<String>,
    pub callback_port: Option<u16>,
    pub scopes: Vec<String>,
    pub redirect_uri: Option<String>,
    pub scope_param: Option<String>,
}

impl Ecosystem {
    pub fn summary(&self) -> String {
        [
            counted(self.agents.len(), "agent"),
            counted(self.commands.len(), "command"),
            counted(self.skills.len(), "skill"),
            counted(self.mcp.len(), "MCP server"),
            counted(self.hooks.len(), "hook"),
            counted(self.plugins.len(), "plugin"),
        ]
        .join(" · ")
    }

    pub fn agent(&self, name: &str) -> Option<&AgentDef> {
        self.agents.iter().find(|agent| agent.name == name)
    }

    pub fn command(&self, name: &str) -> Option<&CommandDef> {
        self.commands.iter().find(|command| command.name == name)
    }

    pub fn prompt_template(&self, name: &str) -> Option<&PromptTemplate> {
        self.prompt_templates
            .iter()
            .find(|template| template.name == name)
    }

    /// Resolves a leading `/command` into its expanded prompt and the agent
    /// routing (`agent`, `subtask`) declared in the command's frontmatter.
    /// Prompt templates are expanded as plain prompts (no routing).
    pub fn resolve_command(&self, input: &str) -> Option<ResolvedCommand> {
        let trimmed = input.trim();
        let rest = trimmed.strip_prefix('/')?;
        let mut parts = rest.splitn(2, char::is_whitespace);
        let name = parts.next()?;
        let arguments = parts.next().unwrap_or("").trim();
        if let Some(command) = self.command(name) {
            return Some(ResolvedCommand {
                prompt: command.expand(arguments),
                agent: command.agent.clone(),
                subtask: command.subtask,
            });
        }
        if let Some(skill_name) = name.strip_prefix("skill:") {
            let skill = self.skills.iter().find(|skill| skill.name == skill_name)?;
            let mut prompt = skill.content.clone();
            if !arguments.is_empty() {
                prompt.push_str("\n\nUser: ");
                prompt.push_str(arguments);
            }
            return Some(ResolvedCommand {
                prompt,
                agent: None,
                subtask: false,
            });
        }
        let template = self.prompt_template(name)?;
        Some(ResolvedCommand {
            prompt: template.expand(arguments),
            agent: None,
            subtask: false,
        })
    }
}

/// Loads the ecosystem visible from `cwd`, merging global scope first and
/// project scope second (project wins). Within a scope the Oxide layout is
/// loaded after the Claude Code layout so it takes precedence. Context files
/// (`AGENTS.md`/`CLAUDE.md`, with `AGENTS.override.md` winning per directory)
/// are collected by walking from the filesystem root down to `cwd`, matching
/// Pi's layering.
pub fn load(cwd: &Path) -> Ecosystem {
    load_with(cwd, true)
}

/// Like [`load`], but `load_context` controls whether `AGENTS.md`/
/// `CLAUDE.md` context files are collected (the `--no-context-files` flag).
pub fn load_with(cwd: &Path, load_context: bool) -> Ecosystem {
    load_opts(
        cwd,
        LoadOptions {
            context_files: load_context,
            project_resources: true,
        },
    )
}

/// Which scopes to load. Global resources are always trusted; project resources
/// are gated behind the project trust decision.
#[derive(Debug, Clone, Copy)]
pub struct LoadOptions {
    pub context_files: bool,
    pub project_resources: bool,
}

impl Default for LoadOptions {
    fn default() -> Self {
        Self {
            context_files: true,
            project_resources: true,
        }
    }
}

/// Loads the ecosystem honoring explicit scope options.
pub fn load_opts(cwd: &Path, options: LoadOptions) -> Ecosystem {
    let mut ecosystem = Ecosystem::default();

    if options.context_files {
        load_context_files(&mut ecosystem, cwd);
    }

    if let Some(home) = dirs::home_dir() {
        load_claude_dir(&mut ecosystem, &home.join(".claude"));
        load_mcp(&mut ecosystem, &home.join(".claude.json"));
        load_oxide_dir(&mut ecosystem, &home.join(".oxide"));
    }
    if let Some(config) = dirs::config_dir() {
        load_oxide_dir(&mut ecosystem, &config.join("oxide"));
    }

    load_enabled_plugins(&mut ecosystem);

    if options.project_resources {
        if let Some(root) = project_root(cwd) {
            load_claude_dir(&mut ecosystem, &root.join(".claude"));
            load_mcp(&mut ecosystem, &root.join(".mcp.json"));
            load_oxide_dir(&mut ecosystem, &root.join(".oxide"));
        }
    }

    ecosystem
}

/// Walks from the filesystem root down to `cwd`, collecting the nearest
/// context file in each directory. `AGENTS.override.md` in a directory replaces
/// `AGENTS.md`/`CLAUDE.md` for that directory only; other directories still
/// layer normally. The global `~/.oxide/AGENTS.md` is loaded first (lowest
/// precedence) so project instructions can override it.
pub fn load_context_files(ecosystem: &mut Ecosystem, cwd: &Path) {
    if let Some(home) = dirs::home_dir() {
        push_context(ecosystem, &home.join(".oxide").join("AGENTS.md"));
    }

    let mut ancestors: Vec<PathBuf> = cwd.ancestors().map(Path::to_path_buf).collect::<Vec<_>>();
    ancestors.reverse();
    for dir in ancestors {
        let override_path = dir.join("AGENTS.override.md");
        if override_path.is_file() {
            push_context(ecosystem, &override_path);
            continue;
        }
        for name in ["AGENTS.md", "CLAUDE.md"] {
            push_context(ecosystem, &dir.join(name));
        }
    }
}

fn push_context(ecosystem: &mut Ecosystem, path: &Path) {
    if let Some(content) = read(path) {
        ecosystem.context_files.push(path.to_path_buf());
        ecosystem.memory.push(Rule {
            name: path.display().to_string(),
            content,
        });
    }
}

pub(crate) fn project_root(cwd: &Path) -> Option<PathBuf> {
    let mut current = Some(cwd.to_path_buf());
    while let Some(dir) = current {
        if dir.join(".git").exists() || dir.join(".oxide").exists() || dir.join(".claude").exists()
        {
            return Some(dir);
        }
        current = dir.parent().map(Path::to_path_buf);
    }
    None
}

// ---------------------------------------------------------------------------
// Oxide layout
// ---------------------------------------------------------------------------

fn load_oxide_dir(ecosystem: &mut Ecosystem, dir: &Path) {
    load_layout(ecosystem, dir, "AGENTS.md");
    load_mcp(ecosystem, &dir.join("mcp.json"));
    if let Some(content) = read(&dir.join("SYSTEM.md")) {
        ecosystem.system_prompt = Some(content);
    }
    if let Some(content) = read(&dir.join("APPEND_SYSTEM.md")) {
        ecosystem.append_system_prompt.push(content);
    }
}

// ---------------------------------------------------------------------------
// Claude Code layout
// ---------------------------------------------------------------------------

fn load_claude_dir(ecosystem: &mut Ecosystem, dir: &Path) {
    load_layout(ecosystem, dir, "CLAUDE.md");
}

/// Loads Claude Code-style plugin packages installed under the oxide config
/// directory. Each enabled plugin bundles commands, agents, skills, MCP servers
/// and manifest-declared hooks, and is loaded before project resources so
/// project-local entries still override plugins with the same name.
fn load_enabled_plugins(ecosystem: &mut Ecosystem) {
    for plugin in crate::plugin_registry::enabled_plugins() {
        ecosystem.plugins.push(plugin.name.clone());
        load_plugin_dir(ecosystem, &plugin.name, &plugin.path, &plugin.manifest);
    }
}

/// Formats a count with its noun, singular for one entry (`1 agent`) and
/// plural otherwise (`2 agents`).
fn counted(count: usize, noun: &str) -> String {
    if count == 1 {
        format!("{count} {noun}")
    } else {
        format!("{count} {noun}s")
    }
}

/// The single-file hook plugins in a `plugins` directory. The global
/// `<config>/oxide/plugins` directory doubles as the install root for plugin
/// packages, so directories and the plugin state file are skipped; only JS/TS
/// modules are run by the hook host.
fn hook_files(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut files: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.is_file()
                && matches!(
                    path.extension().and_then(|ext| ext.to_str()),
                    Some("js" | "ts" | "mjs" | "cjs" | "mts" | "cts")
                )
        })
        .collect();
    files.sort();
    files
}

fn load_plugin_dir(
    ecosystem: &mut Ecosystem,
    name: &str,
    dir: &Path,
    manifest: &crate::plugin_registry::PluginManifest,
) {
    for file in markdown_files(&dir.join("commands")) {
        if let Some(command) = command_from_markdown(&file) {
            upsert_command(ecosystem, command);
        }
    }
    for file in markdown_files(&dir.join("agents")) {
        if let Some(agent) = agent_from_markdown(&file) {
            upsert_agent(ecosystem, agent);
        }
    }
    scan_skills(ecosystem, &dir.join("skills"));

    if let Some(servers) = manifest.mcp_servers.as_ref().and_then(Json::as_object) {
        for (server_name, config) in servers {
            if let Some(server) = mcp_from_claude(server_name, config) {
                upsert_mcp(ecosystem, server);
            }
        }
    }

    // JS/TS hook files shipped inside the plugin package.
    ecosystem.hooks.extend(hook_files(&dir.join("plugins")));
    // Claude Code command hooks declared in the plugin manifest are translated
    // into a generated JS shim run by the existing hook host.
    if let Some(path) = crate::plugin_registry::hook_shim_path(name, manifest) {
        ecosystem.hooks.push(path);
    }
}

fn load_layout(ecosystem: &mut Ecosystem, dir: &Path, memory_file: &str) {
    // `memory_file` (AGENTS.md/CLAUDE.md) placed directly in a layout directory
    // such as `.claude/` is still honored; top-level context files are collected
    // separately by `load_context_files`.
    push_memory(ecosystem, &dir.join(memory_file));

    for file in markdown_files(&dir.join("agents")) {
        if let Some(agent) = agent_from_markdown(&file) {
            upsert_agent(ecosystem, agent);
        }
    }
    for file in markdown_files(&dir.join("commands")) {
        if let Some(command) = command_from_markdown(&file) {
            upsert_command(ecosystem, command);
        }
    }
    scan_skills(ecosystem, &dir.join("skills"));
    for file in markdown_files(&dir.join("prompts")) {
        if let Some(template) = prompt_template_from_markdown(&file) {
            upsert_prompt_template(ecosystem, template);
        }
    }

    ecosystem.hooks.extend(hook_files(&dir.join("plugins")));
}

fn load_mcp(ecosystem: &mut Ecosystem, path: &Path) {
    let Some(json) = read_json(path) else { return };
    if let Some(servers) = json.get("mcpServers").and_then(Json::as_object) {
        for (name, config) in servers {
            if let Some(server) = mcp_from_claude(name, config) {
                upsert_mcp(ecosystem, server);
            }
        }
    }
}

pub(crate) fn mcp_from_claude(name: &str, config: &Json) -> Option<McpServer> {
    let enabled = config
        .get("enabled")
        .and_then(Json::as_bool)
        .unwrap_or_else(|| {
            !config
                .get("disabled")
                .and_then(Json::as_bool)
                .unwrap_or(false)
        });
    let domains = json_string_list(config.get("domains"))
        .map(|domains| domains.into_iter().map(|d| normalize_domain(&d)).collect())
        .unwrap_or_default();
    if let Some(url) = config.get("url").and_then(Json::as_str) {
        return Some(McpServer {
            name: name.to_string(),
            enabled,
            kind: McpKind::Remote {
                url: url.to_string(),
                headers: string_map(config.get("headers")),
                oauth: parse_oauth(config.get("oauth")),
            },
            domains,
        });
    }

    let mut command = vec![config.get("command").and_then(Json::as_str)?.to_string()];
    command.extend(json_string_list(config.get("args")).unwrap_or_default());
    Some(McpServer {
        name: name.to_string(),
        enabled,
        kind: McpKind::Local {
            command,
            environment: string_map(config.get("env")),
            cwd: None,
        },
        domains,
    })
}

/// Lowercases a domain and strips any scheme/path so only the host (or a
/// `*.`-prefixed wildcard) remains.
fn normalize_domain(domain: &str) -> String {
    let mut host = domain.trim().to_ascii_lowercase();
    if let Some(rest) = host.strip_prefix("http://") {
        host = rest.to_string();
    } else if let Some(rest) = host.strip_prefix("https://") {
        host = rest.to_string();
    }
    let host = host
        .split(['/', ':', '?', '#'])
        .next()
        .unwrap_or("")
        .to_string();
    host
}

/// Well-known routing domains for popular MCP servers. Users can override these
/// with the `domains` key in their server config; this map only fills the gap so
/// pasted URLs (Slack messages, Confluence pages, ...) route without setup.
pub fn default_domains(name: &str) -> Vec<String> {
    let key = name
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .collect::<String>()
        .to_ascii_lowercase();
    let domains: &[&str] = match key.as_str() {
        "atlassian" | "confluence" | "jira" => &[
            "atlassian.net",
            "*.atlassian.net",
            "jira.com",
            "*.jira.com",
            "atlassian.com",
            "*.atlassian.com",
        ],
        "slack" => &["slack.com", "*.slack.com"],
        "newrelic" | "newrelicone" | "nr" => &[
            "newrelic.com",
            "*.newrelic.com",
            "one.newrelic.com",
            "nr-assets.net",
            "*.nr-assets.net",
        ],
        "context7" => &["context7.com", "*.context7.com"],
        "contentful" => &[
            "contentful.com",
            "*.contentful.com",
            "ctfassets.net",
            "*.ctfassets.net",
        ],
        "figma" => &["figma.com", "*.figma.com"],
        "github" => &[
            "github.com",
            "*.github.com",
            "githubusercontent.com",
            "*.githubusercontent.com",
        ],
        "gitlab" => &["gitlab.com", "*.gitlab.com"],
        "notion" => &["notion.so", "*.notion.so"],
        "linear" => &["linear.app", "*.linear.app"],
        "sentry" => &["sentry.io", "*.sentry.io"],
        _ => &[],
    };
    domains.iter().map(|domain| (*domain).to_string()).collect()
}

pub(crate) fn parse_oauth(value: Option<&Json>) -> Option<McpOAuth> {
    let object = value?.as_object()?;
    let callback_port = object
        .get("callbackPort")
        .or_else(|| object.get("callback_port"))
        .and_then(Json::as_u64)
        .and_then(|port| u16::try_from(port).ok());
    let scopes = json_string_list(object.get("scopes")).unwrap_or_else(|| {
        object
            .get("scope")
            .and_then(Json::as_str)
            .map(|scope| {
                scope
                    .split([',', ' '])
                    .filter(|s| !s.is_empty())
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default()
    });
    Some(McpOAuth {
        client_id: object
            .get("clientId")
            .or_else(|| object.get("client_id"))
            .and_then(Json::as_str)
            .map(str::to_string),
        client_secret: object
            .get("clientSecret")
            .or_else(|| object.get("client_secret"))
            .and_then(Json::as_str)
            .map(str::to_string),
        callback_port,
        scopes,
        redirect_uri: object
            .get("redirectUri")
            .or_else(|| object.get("redirect_uri"))
            .and_then(Json::as_str)
            .map(str::to_string),
        scope_param: object
            .get("scopeParam")
            .or_else(|| object.get("scope_param"))
            .and_then(Json::as_str)
            .map(str::to_string),
    })
}

// ---------------------------------------------------------------------------
// Markdown frontmatter loaders
// ---------------------------------------------------------------------------

fn agent_from_markdown(path: &Path) -> Option<AgentDef> {
    let raw = read(path)?;
    let front = frontmatter::parse(&raw);
    Some(AgentDef {
        name: front.get_str("name").unwrap_or_else(|| file_stem(path)),
        description: front.get_str("description"),
        mode: parse_mode(front.get_str("mode").as_deref()),
        permission: front
            .get("permission")
            .and_then(|value| serde_json::to_value(value).ok()),
        prompt: front.body,
    })
}

fn command_from_markdown(path: &Path) -> Option<CommandDef> {
    let raw = read(path)?;
    let front = frontmatter::parse(&raw);
    let agent = front.get_str("agent");
    let subtask = front.get_bool("subtask").unwrap_or(false);
    Some(CommandDef {
        name: file_stem(path),
        description: front.get_str("description"),
        template: front.body,
        agent,
        subtask,
    })
}

fn prompt_template_from_markdown(path: &Path) -> Option<PromptTemplate> {
    let raw = read(path)?;
    let front = frontmatter::parse(&raw);
    // `description` is optional; fall back to the first non-empty body line.
    let description = front.get_str("description").or_else(|| {
        front
            .body
            .lines()
            .map(str::trim)
            .find(|line| !line.is_empty())
            .map(str::to_string)
    });
    Some(PromptTemplate {
        name: file_stem(path),
        description,
        argument_hint: front.get_str("argument-hint"),
        body: front.body,
    })
}

/// Expands a prompt template body Pi-style: `$1`, `$2`, … positional args,
/// `$@`/`$ARGUMENTS`, `${N:-default}`, and `${@:N}`/`${@:N:L}` slices.
pub fn expand_prompt_template(body: &str, arguments: &str) -> String {
    let args: Vec<&str> = arguments.split_whitespace().collect();
    let joined = args.join(" ");
    let mut out = String::with_capacity(body.len());
    let chars: Vec<char> = body.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let ch = chars[i];
        if ch == '$' && i + 1 < chars.len() {
            let next = chars[i + 1];
            let (value, consumed) = if next == '{' {
                expand_braced(&chars[i..], &args, &joined)
            } else if next == '@' {
                (joined.clone(), 2)
            } else if chars[i + 1..].starts_with(&['A', 'R', 'G', 'U', 'M', 'E', 'N', 'T', 'S']) {
                (joined.clone(), 1 + "ARGUMENTS".len())
            } else if next.is_ascii_digit() {
                let mut end = i + 1;
                while end < chars.len() && chars[end].is_ascii_digit() {
                    end += 1;
                }
                let index: usize = chars[i + 1..end]
                    .iter()
                    .collect::<String>()
                    .parse()
                    .unwrap_or(0);
                (
                    index
                        .checked_sub(1)
                        .and_then(|i| args.get(i))
                        .map(|s| s.to_string())
                        .unwrap_or_default(),
                    end - i,
                )
            } else {
                (String::from("$"), 1)
            };
            out.push_str(&value);
            i += consumed;
            continue;
        }
        out.push(ch);
        i += 1;
    }
    out
}

/// Handles a `${...}` expression starting at `chars[0] == '$'`. Returns the
/// replacement and how many characters were consumed.
fn expand_braced(chars: &[char], args: &[&str], joined: &str) -> (String, usize) {
    let Some(close) = chars.iter().position(|c| *c == '}') else {
        return (String::from("$"), 1);
    };
    let inner: String = chars[2..close].iter().collect();
    let consumed = close + 1;
    let (expr, default) = match inner.split_once(":-") {
        Some((expr, default)) => (expr.trim(), Some(default)),
        None => (inner.trim(), None),
    };
    let value = if expr == "@" || expr.eq_ignore_ascii_case("ARGUMENTS") {
        Some(joined.to_string())
    } else if let Some(slice) = expr.strip_prefix('@') {
        // `${@:N}` or `${@:N:L}` — `slice` starts with `:`.
        let slice = slice.trim_start_matches(':');
        let parts: Vec<&str> = slice.split(':').collect();
        let start = parts
            .first()
            .and_then(|s| s.trim().parse::<usize>().ok())
            .unwrap_or(1);
        let count = parts.get(1).and_then(|s| s.trim().parse::<usize>().ok());
        let selected: Vec<&str> = args
            .iter()
            .skip(start.saturating_sub(1))
            .take(count.unwrap_or(usize::MAX))
            .copied()
            .collect();
        Some(selected.join(" "))
    } else if let Ok(index) = expr.parse::<usize>() {
        index
            .checked_sub(1)
            .and_then(|i| args.get(i))
            .map(|s| s.to_string())
    } else {
        None
    };
    let value = value.filter(|v| !v.is_empty());
    match (value, default) {
        (Some(value), _) => (value, consumed),
        (None, Some(default)) => (default.to_string(), consumed),
        (None, None) => (String::new(), consumed),
    }
}

fn push_skill(ecosystem: &mut Ecosystem, path: &Path) {
    let Some(raw) = read(path) else { return };
    let front = frontmatter::parse(&raw);
    let name = front
        .get_str("name")
        .unwrap_or_else(|| file_stem(path.parent().unwrap_or(path)));
    ecosystem.skills.retain(|skill| skill.name != name);
    ecosystem.skills.push(Skill {
        name,
        description: front.get_str("description"),
        content: front.body,
    });
}

fn push_memory(ecosystem: &mut Ecosystem, path: &Path) {
    if let Some(content) = read(path) {
        ecosystem.memory.push(Rule {
            name: file_stem(path),
            content,
        });
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn upsert_agent(ecosystem: &mut Ecosystem, agent: AgentDef) {
    ecosystem
        .agents
        .retain(|existing| existing.name != agent.name);
    ecosystem.agents.push(agent);
}

fn upsert_command(ecosystem: &mut Ecosystem, command: CommandDef) {
    ecosystem
        .commands
        .retain(|existing| existing.name != command.name);
    ecosystem.commands.push(command);
}

fn upsert_prompt_template(ecosystem: &mut Ecosystem, template: PromptTemplate) {
    ecosystem
        .prompt_templates
        .retain(|existing| existing.name != template.name);
    ecosystem.prompt_templates.push(template);
}

fn upsert_mcp(ecosystem: &mut Ecosystem, server: McpServer) {
    ecosystem
        .mcp
        .retain(|existing| existing.name != server.name);
    ecosystem.mcp.push(server);
}

fn scan_skills(ecosystem: &mut Ecosystem, dir: &Path) {
    push_skill(ecosystem, &dir.join("SKILL.md"));
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            push_skill(ecosystem, &path.join("SKILL.md"));
        }
    }
}

fn markdown_files(dir: &Path) -> Vec<PathBuf> {
    files_with_extension(dir, &["md"])
}

fn files_with_extension(dir: &Path, extensions: &[&str]) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut files: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .and_then(|ext| ext.to_str())
                .map(|ext| extensions.contains(&ext))
                .unwrap_or(false)
        })
        .collect();
    files.sort();
    files
}

fn parse_mode(value: Option<&str>) -> AgentMode {
    match value {
        Some("primary") => AgentMode::Primary,
        Some("all") => AgentMode::All,
        _ => AgentMode::Subagent,
    }
}

fn json_string_list(value: Option<&Json>) -> Option<Vec<String>> {
    match value? {
        Json::Array(items) => Some(
            items
                .iter()
                .filter_map(Json::as_str)
                .map(str::to_string)
                .collect(),
        ),
        Json::String(text) => Some(
            text.split_whitespace()
                .map(str::to_string)
                .filter(|part| !part.is_empty())
                .collect(),
        ),
        _ => None,
    }
}

fn string_map(value: Option<&Json>) -> BTreeMap<String, String> {
    value
        .and_then(Json::as_object)
        .map(|object| {
            object
                .iter()
                .filter_map(|(key, value)| {
                    value.as_str().map(|value| (key.clone(), value.to_string()))
                })
                .collect()
        })
        .unwrap_or_default()
}

fn read(path: &Path) -> Option<String> {
    std::fs::read_to_string(path).ok()
}

fn read_json(path: &Path) -> Option<Json> {
    let raw = read(path)?;
    serde_json::from_str(&raw).ok()
}

fn file_stem(path: &Path) -> String {
    path.file_stem()
        .map(|stem| stem.to_string_lossy().to_string())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("oxide_eco_{tag}_{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn normalizes_and_defaults_domains() {
        assert_eq!(
            normalize_domain("https://Acme.Atlassian.Net/wiki"),
            "acme.atlassian.net"
        );
        assert_eq!(normalize_domain("*.Slack.com"), "*.slack.com");
        assert!(default_domains("slack").contains(&"slack.com".to_string()));
        assert!(default_domains("Atlassian").contains(&"*.atlassian.net".to_string()));
        assert!(default_domains("unknown-service").is_empty());
    }

    #[test]
    fn parses_explicit_mcp_domains() {
        let json: Json = serde_json::from_str(
            r#"{"url":"https://mcp.example.com","domains":["docs.example.com","*.example.com"]}"#,
        )
        .unwrap();
        let server = mcp_from_claude("docs", &json).unwrap();
        assert_eq!(server.domains, vec!["docs.example.com", "*.example.com"]);
        assert_eq!(server.domains(), vec!["docs.example.com", "*.example.com"]);
    }

    #[test]
    fn loads_claude_layout() {
        let dir = temp_dir("claude");
        std::fs::create_dir_all(dir.join(".git")).unwrap();
        std::fs::create_dir_all(dir.join(".claude/agents")).unwrap();
        std::fs::create_dir_all(dir.join(".claude/commands")).unwrap();
        std::fs::create_dir_all(dir.join(".claude/skills/audit")).unwrap();

        std::fs::write(
            dir.join(".claude/agents/planner.md"),
            "---\nname: planner\ndescription: plans work\n---\nPlan the work.",
        )
        .unwrap();
        std::fs::write(
            dir.join(".claude/commands/build.md"),
            "---\ndescription: build it\n---\nRun cargo build $ARGUMENTS",
        )
        .unwrap();
        std::fs::write(
            dir.join(".claude/skills/audit/SKILL.md"),
            "---\nname: audit\ndescription: audits deps\n---\nAudit the deps.",
        )
        .unwrap();
        std::fs::write(dir.join("CLAUDE.md"), "Project memory.").unwrap();
        std::fs::write(
            dir.join(".mcp.json"),
            r#"{"mcpServers":{
                "fs":{"command":"npx","args":["-y","server-fs"]},
                "remote":{"url":"https://example.com/mcp"}
            }}"#,
        )
        .unwrap();

        let ecosystem = load(&dir);

        assert!(ecosystem.agent("planner").is_some());
        assert!(ecosystem.command("build").is_some());
        assert!(ecosystem.skills.iter().any(|skill| skill.name == "audit"));
        assert!(ecosystem.mcp.iter().any(|server| server.name == "fs"));
        assert!(ecosystem.mcp.iter().any(|server| server.name == "remote"));
        assert!(!ecosystem.memory.is_empty());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn loads_oxide_layout_and_overrides_claude() {
        let dir = temp_dir("oxide");
        std::fs::create_dir_all(dir.join(".git")).unwrap();
        std::fs::create_dir_all(dir.join(".oxide/agents")).unwrap();
        std::fs::create_dir_all(dir.join(".oxide/commands")).unwrap();
        std::fs::create_dir_all(dir.join(".oxide/skills/audit")).unwrap();
        std::fs::create_dir_all(dir.join(".claude/agents")).unwrap();

        std::fs::write(dir.join("AGENTS.md"), "Project rules.").unwrap();
        std::fs::write(
            dir.join(".oxide/agents/planner.md"),
            "---\nname: planner\ndescription: oxide planner\n---\nPlan.",
        )
        .unwrap();
        std::fs::write(
            dir.join(".oxide/commands/build.md"),
            "---\ndescription: build it\n---\nRun cargo build $ARGUMENTS",
        )
        .unwrap();
        std::fs::write(
            dir.join(".oxide/skills/audit/SKILL.md"),
            "---\nname: audit\ndescription: audits deps\n---\nAudit.",
        )
        .unwrap();
        std::fs::write(
            dir.join(".claude/agents/planner.md"),
            "---\nname: planner\ndescription: claude planner\n---\nPlan.",
        )
        .unwrap();

        let ecosystem = load(&dir);

        assert_eq!(
            ecosystem.agent("planner").unwrap().description.as_deref(),
            Some("oxide planner")
        );
        assert!(ecosystem.command("build").is_some());
        assert!(ecosystem.skills.iter().any(|skill| skill.name == "audit"));
        assert!(ecosystem
            .context_files
            .iter()
            .any(|path| path == &dir.join("AGENTS.md")));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn override_file_replaces_context_in_its_directory() {
        let dir = temp_dir("ctx_override");
        std::fs::create_dir_all(dir.join(".git")).unwrap();
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(dir.join("AGENTS.md"), "root rules").unwrap();
        std::fs::write(dir.join("sub/AGENTS.md"), "sub rules").unwrap();
        std::fs::write(dir.join("sub/AGENTS.override.md"), "override rules").unwrap();

        let ecosystem = load(&dir.join("sub"));

        let joined: String = ecosystem
            .memory
            .iter()
            .map(|entry| entry.content.clone())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(joined.contains("root rules"));
        assert!(!joined.contains("sub rules"));
        assert!(joined.contains("override rules"));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn system_and_append_prompt_files_load() {
        let dir = temp_dir("sysprompt");
        std::fs::create_dir_all(dir.join(".git")).unwrap();
        std::fs::create_dir_all(dir.join(".oxide")).unwrap();
        std::fs::write(dir.join(".oxide/SYSTEM.md"), "replacement prompt").unwrap();
        std::fs::write(dir.join(".oxide/APPEND_SYSTEM.md"), "extra guidance").unwrap();

        let ecosystem = load(&dir);

        assert_eq!(
            ecosystem.system_prompt.as_deref(),
            Some("replacement prompt")
        );
        assert_eq!(ecosystem.append_system_prompt, vec!["extra guidance"]);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn loads_oxide_mcp_servers_and_overrides_mcp_json() {
        let dir = temp_dir("oxide_mcp");
        std::fs::create_dir_all(dir.join(".git")).unwrap();
        std::fs::create_dir_all(dir.join(".oxide")).unwrap();
        std::fs::write(
            dir.join(".mcp.json"),
            r#"{"mcpServers":{"shared":{"url":"https://example.com/mcp"},"claude":{"command":"npx"}}}"#,
        )
        .unwrap();
        std::fs::write(
            dir.join(".oxide/mcp.json"),
            r#"{"mcpServers":{"shared":{"command":"npx","args":["-y","server-fs"]}}}"#,
        )
        .unwrap();

        let ecosystem = load(&dir);

        let shared = ecosystem.mcp.iter().find(|s| s.name == "shared").unwrap();
        assert!(matches!(shared.kind, McpKind::Local { .. }));
        assert!(ecosystem.mcp.iter().any(|s| s.name == "claude"));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn project_overrides_global_name() {
        let mut ecosystem = Ecosystem::default();
        upsert_agent(
            &mut ecosystem,
            AgentDef {
                name: "x".into(),
                description: Some("global".into()),
                mode: AgentMode::Subagent,
                permission: None,
                prompt: String::new(),
            },
        );
        upsert_agent(
            &mut ecosystem,
            AgentDef {
                name: "x".into(),
                description: Some("project".into()),
                mode: AgentMode::Subagent,
                permission: None,
                prompt: String::new(),
            },
        );
        assert_eq!(ecosystem.agents.len(), 1);
        assert_eq!(
            ecosystem.agent("x").unwrap().description.as_deref(),
            Some("project")
        );
    }

    #[test]
    fn expands_command_arguments() {
        let command = CommandDef {
            name: "greet".into(),
            description: None,
            template: "Hello $1 from $ARGUMENTS".into(),
            agent: None,
            subtask: false,
        };
        assert_eq!(command.expand("world"), "Hello world from world");
    }

    #[test]
    fn resolves_command_agent_and_subtask() {
        let dir = temp_dir("command_meta");
        std::fs::create_dir_all(dir.join(".git")).unwrap();
        std::fs::create_dir_all(dir.join(".oxide/commands")).unwrap();
        std::fs::write(
            dir.join(".oxide/commands/review.md"),
            "---\ndescription: review\nagent: rust-reviewer\nsubtask: true\n---\nReview $ARGUMENTS",
        )
        .unwrap();

        let ecosystem = load(&dir);
        let resolved = ecosystem.resolve_command("/review the diff").unwrap();
        assert_eq!(resolved.prompt, "Review the diff");
        assert_eq!(resolved.agent.as_deref(), Some("rust-reviewer"));
        assert!(resolved.subtask);
        assert!(ecosystem.resolve_command("plain text").is_none());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn prompt_template_expansion_matches_pi() {
        assert_eq!(expand_prompt_template("Hi $1", "world"), "Hi world");
        assert_eq!(expand_prompt_template("$@", "a b c"), "a b c");
        assert_eq!(expand_prompt_template("$ARGUMENTS", "a b"), "a b");
        assert_eq!(expand_prompt_template("${1:-7} bullets", ""), "7 bullets");
        assert_eq!(expand_prompt_template("${1:-7} bullets", "3"), "3 bullets");
        assert_eq!(expand_prompt_template("${@:-none}", ""), "none");
        assert_eq!(expand_prompt_template("${@:2}", "a b c"), "b c");
        assert_eq!(expand_prompt_template("${@:2:1}", "a b c"), "b");
        assert_eq!(expand_prompt_template("${@:2:3}", "a b c d e"), "b c d");
        assert_eq!(expand_prompt_template("${@:2:3}", "a"), "");
        assert_eq!(expand_prompt_template("cost is $5", "a"), "cost is ");
        assert_eq!(expand_prompt_template("literal ${x}", "a"), "literal ");
    }

    #[test]
    fn loads_prompt_templates_from_prompts_dir() {
        let dir = temp_dir("prompts");
        std::fs::create_dir_all(dir.join(".git")).unwrap();
        std::fs::create_dir_all(dir.join(".oxide/prompts")).unwrap();
        std::fs::write(
            dir.join(".oxide/prompts/review.md"),
            "---\ndescription: review staged changes\nargument-hint: <focus>\n---\nReview $1 focus: ${2:-all}",
        )
        .unwrap();

        let ecosystem = load(&dir);
        let template = ecosystem.prompt_template("review").unwrap();
        assert_eq!(
            template.description.as_deref(),
            Some("review staged changes")
        );
        assert_eq!(template.argument_hint.as_deref(), Some("<focus>"));

        // Prompt templates resolve through the same path as commands.
        let resolved = ecosystem.resolve_command("/review security").unwrap();
        assert_eq!(resolved.prompt, "Review security focus: all");
        assert!(resolved.agent.is_none());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn skill_commands_load_skill_content_with_arguments() {
        let mut ecosystem = Ecosystem::default();
        ecosystem.skills.push(Skill {
            name: "audit".into(),
            description: Some("audits deps".into()),
            content: "Audit the deps.".into(),
        });

        let resolved = ecosystem
            .resolve_command("/skill:audit the lockfile")
            .unwrap();
        assert_eq!(resolved.prompt, "Audit the deps.\n\nUser: the lockfile");
        assert!(resolved.agent.is_none());
        assert!(!resolved.subtask);
        assert!(ecosystem.resolve_command("/skill:missing").is_none());
    }

    #[test]
    fn loads_plugin_package_resources() {
        let dir = temp_dir("plugin_pkg");
        let pkg = dir.join("pkg");
        std::fs::create_dir_all(pkg.join("commands")).unwrap();
        std::fs::create_dir_all(pkg.join("agents")).unwrap();
        std::fs::create_dir_all(pkg.join("skills/audit")).unwrap();
        std::fs::create_dir_all(pkg.join(".claude-plugin")).unwrap();
        std::fs::write(
            pkg.join("commands/build.md"),
            "---\ndescription: build it\n---\nRun cargo build",
        )
        .unwrap();
        std::fs::write(
            pkg.join("agents/planner.md"),
            "---\nname: planner\n---\nPlan.",
        )
        .unwrap();
        std::fs::write(
            pkg.join("skills/audit/SKILL.md"),
            "---\nname: audit\n---\nAudit.",
        )
        .unwrap();
        std::fs::write(
            pkg.join(".claude-plugin/plugin.json"),
            r#"{"name":"pkg","mcpServers":{"fs":{"command":"npx"}}}"#,
        )
        .unwrap();

        let manifest = crate::plugin_registry::plugin_manifest(&pkg).unwrap();
        let mut ecosystem = Ecosystem::default();
        load_plugin_dir(&mut ecosystem, "pkg", &pkg, &manifest);

        assert!(ecosystem.command("build").is_some());
        assert!(ecosystem.agent("planner").is_some());
        assert!(ecosystem.skills.iter().any(|skill| skill.name == "audit"));
        assert!(ecosystem.mcp.iter().any(|server| server.name == "fs"));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn summary_pluralizes_counts() {
        let mut ecosystem = Ecosystem::default();
        assert_eq!(
            ecosystem.summary(),
            "0 agents · 0 commands · 0 skills · 0 MCP servers · 0 hooks · 0 plugins"
        );

        ecosystem.hooks.push(PathBuf::from("hook.ts"));
        ecosystem.plugins.push("demo".to_string());
        assert_eq!(
            ecosystem.summary(),
            "0 agents · 0 commands · 0 skills · 0 MCP servers · 1 hook · 1 plugin"
        );
    }

    #[test]
    fn hook_files_skip_plugin_packages_and_state() {
        let dir = temp_dir("hook_files");
        std::fs::create_dir_all(dir.join("demo")).unwrap();
        std::fs::write(dir.join("hook.ts"), "export default 1").unwrap();
        std::fs::write(dir.join("extra.js"), "export default 1").unwrap();
        std::fs::write(dir.join("config.json"), "{}").unwrap();
        std::fs::write(dir.join("notes.md"), "not a hook").unwrap();

        assert_eq!(
            hook_files(&dir),
            vec![dir.join("extra.js"), dir.join("hook.ts")]
        );

        std::fs::remove_dir_all(&dir).ok();
    }
}
