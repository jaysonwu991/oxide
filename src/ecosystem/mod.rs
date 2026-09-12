//! Discovery and loading of the Claude Code + OpenCode configuration
//! ecosystem: rules, memory, commands, agents, skills, MCP servers and
//! plugins. Both the OpenCode layout (`.opencode/`, `opencode.json`) and the
//! Claude Code layout (`.claude/`, `CLAUDE.md`, `.mcp.json`) are understood,
//! from the project scope and the user's global scope. Project entries
//! override global entries with the same name.

mod frontmatter;

use serde_json::Value as Json;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Default)]
pub struct Ecosystem {
    pub rules: Vec<Rule>,
    pub memory: Vec<Rule>,
    pub commands: Vec<CommandDef>,
    pub agents: Vec<AgentDef>,
    pub skills: Vec<Skill>,
    pub mcp: Vec<McpServer>,
    pub plugins: Vec<PathBuf>,
    pub permission: Option<Json>,
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
    },
}

impl Ecosystem {
    pub fn summary(&self) -> String {
        format!(
            "{} agents, {} commands, {} skills, {} MCP servers, {} rules, {} memory files, {} plugins",
            self.agents.len(),
            self.commands.len(),
            self.skills.len(),
            self.mcp.len(),
            self.rules.len(),
            self.memory.len(),
            self.plugins.len(),
        )
    }

    pub fn agent(&self, name: &str) -> Option<&AgentDef> {
        self.agents.iter().find(|agent| agent.name == name)
    }

    pub fn command(&self, name: &str) -> Option<&CommandDef> {
        self.commands.iter().find(|command| command.name == name)
    }

    pub fn expand_command(&self, input: &str) -> Option<String> {
        let trimmed = input.trim();
        let rest = trimmed.strip_prefix('/')?;
        let mut parts = rest.splitn(2, char::is_whitespace);
        let name = parts.next()?;
        let arguments = parts.next().unwrap_or("").trim();
        self.command(name).map(|command| command.expand(arguments))
    }
}

/// Loads the ecosystem visible from `cwd`, merging global scope first and
/// project scope second (project wins).
pub fn load(cwd: &Path) -> Ecosystem {
    let mut ecosystem = Ecosystem::default();

    if let Some(home) = dirs::home_dir() {
        load_opencode_dir(&mut ecosystem, &home.join(".config/opencode"));
        load_claude_dir(&mut ecosystem, &home.join(".claude"));
        load_claude_mcp(&mut ecosystem, &home.join(".claude.json"));
    }

    if let Some(root) = project_root(cwd) {
        load_opencode_dir(&mut ecosystem, &root.join(".opencode"));
        for name in ["opencode.json", "opencode.jsonc"] {
            load_opencode_config(&mut ecosystem, &root.join(name));
        }
        load_claude_dir(&mut ecosystem, &root.join(".claude"));
        for name in ["CLAUDE.md", "CLAUDE.local.md"] {
            push_memory(&mut ecosystem, &root.join(name));
        }
        load_claude_mcp(&mut ecosystem, &root.join(".mcp.json"));
    }

    ecosystem
}

pub(crate) fn project_root(cwd: &Path) -> Option<PathBuf> {
    let mut current = Some(cwd.to_path_buf());
    while let Some(dir) = current {
        if dir.join(".git").exists()
            || dir.join(".opencode").exists()
            || dir.join(".claude").exists()
        {
            return Some(dir);
        }
        current = dir.parent().map(Path::to_path_buf);
    }
    None
}

// ---------------------------------------------------------------------------
// OpenCode layout
// ---------------------------------------------------------------------------

fn load_opencode_dir(ecosystem: &mut Ecosystem, dir: &Path) {
    for sub in ["agent", "agents"] {
        for file in markdown_files(&dir.join(sub)) {
            if let Some(agent) = agent_from_markdown(&file) {
                upsert_agent(ecosystem, agent);
            }
        }
    }
    for sub in ["command", "commands"] {
        for file in markdown_files(&dir.join(sub)) {
            if let Some(command) = command_from_markdown(&file) {
                upsert_command(ecosystem, command);
            }
        }
    }
    for sub in ["skill", "skills"] {
        scan_skills(ecosystem, &dir.join(sub));
    }
    for sub in ["plugin", "plugins"] {
        for file in files_with_extension(&dir.join(sub), &["ts", "js", "mjs", "cjs"]) {
            ecosystem.plugins.push(file);
        }
    }
}

fn load_opencode_config(ecosystem: &mut Ecosystem, path: &Path) {
    let Some(json) = read_json(path) else { return };
    let base = path.parent().unwrap_or_else(|| Path::new("."));

    if let Some(instructions) = json.get("instructions").and_then(Json::as_array) {
        for item in instructions.iter().filter_map(Json::as_str) {
            let file = base.join(item);
            if let Some(content) = read(&file) {
                ecosystem.rules.push(Rule {
                    name: file_stem(&file),
                    content,
                });
            }
        }
    }

    if let Some(paths) = json
        .get("skills")
        .and_then(|skills| skills.get("paths"))
        .and_then(Json::as_array)
    {
        for item in paths.iter().filter_map(Json::as_str) {
            scan_skills(ecosystem, &base.join(item));
        }
    }

    if let Some(agents) = json.get("agent").and_then(Json::as_object) {
        for (name, config) in agents {
            upsert_agent(ecosystem, agent_from_json(name, config));
        }
    }

    if let Some(commands) = json.get("command").and_then(Json::as_object) {
        for (name, config) in commands {
            upsert_command(ecosystem, command_from_json(name, config));
        }
    }

    if let Some(servers) = json.get("mcp").and_then(Json::as_object) {
        for (name, config) in servers {
            if let Some(server) = mcp_from_opencode(name, config) {
                upsert_mcp(ecosystem, server);
            }
        }
    }

    if let Some(plugins) = json.get("plugin").and_then(Json::as_array) {
        for item in plugins.iter().filter_map(Json::as_str) {
            ecosystem.plugins.push(base.join(item));
        }
    }

    if let Some(permission) = json.get("permission") {
        ecosystem.permission = Some(permission.clone());
    }
}

fn agent_from_json(name: &str, config: &Json) -> AgentDef {
    AgentDef {
        name: name.to_string(),
        description: config
            .get("description")
            .and_then(Json::as_str)
            .map(str::to_string),
        mode: parse_mode(config.get("mode").and_then(Json::as_str)),
        permission: config.get("permission").cloned(),
        prompt: config
            .get("prompt")
            .and_then(Json::as_str)
            .unwrap_or("")
            .to_string(),
    }
}

fn command_from_json(name: &str, config: &Json) -> CommandDef {
    CommandDef {
        name: name.to_string(),
        description: config
            .get("description")
            .and_then(Json::as_str)
            .map(str::to_string),
        template: config
            .get("template")
            .and_then(Json::as_str)
            .unwrap_or("")
            .to_string(),
    }
}

fn mcp_from_opencode(name: &str, config: &Json) -> Option<McpServer> {
    let enabled = config
        .get("enabled")
        .and_then(Json::as_bool)
        .unwrap_or(true);
    match config.get("type").and_then(Json::as_str) {
        Some("remote") => {
            let url = config.get("url").and_then(Json::as_str)?.to_string();
            Some(McpServer {
                name: name.to_string(),
                enabled,
                kind: McpKind::Remote {
                    url,
                    headers: string_map(config.get("headers")),
                },
            })
        }
        _ => {
            let command = json_string_list(config.get("command"))?;
            Some(McpServer {
                name: name.to_string(),
                enabled,
                kind: McpKind::Local {
                    command,
                    environment: string_map(config.get("environment")),
                    cwd: config.get("cwd").and_then(Json::as_str).map(str::to_string),
                },
            })
        }
    }
}

// ---------------------------------------------------------------------------
// Claude Code layout
// ---------------------------------------------------------------------------

fn load_claude_dir(ecosystem: &mut Ecosystem, dir: &Path) {
    push_memory(ecosystem, &dir.join("CLAUDE.md"));

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

    if let Ok(entries) = std::fs::read_dir(dir.join("plugins")) {
        for entry in entries.flatten() {
            ecosystem.plugins.push(entry.path());
        }
    }
}

fn load_claude_mcp(ecosystem: &mut Ecosystem, path: &Path) {
    let Some(json) = read_json(path) else { return };
    if let Some(servers) = json.get("mcpServers").and_then(Json::as_object) {
        for (name, config) in servers {
            if let Some(server) = mcp_from_claude(name, config) {
                upsert_mcp(ecosystem, server);
            }
        }
    }
}

fn mcp_from_claude(name: &str, config: &Json) -> Option<McpServer> {
    if let Some(url) = config.get("url").and_then(Json::as_str) {
        return Some(McpServer {
            name: name.to_string(),
            enabled: true,
            kind: McpKind::Remote {
                url: url.to_string(),
                headers: string_map(config.get("headers")),
            },
        });
    }

    let mut command = vec![config.get("command").and_then(Json::as_str)?.to_string()];
    command.extend(json_string_list(config.get("args")).unwrap_or_default());
    Some(McpServer {
        name: name.to_string(),
        enabled: true,
        kind: McpKind::Local {
            command,
            environment: string_map(config.get("env")),
            cwd: None,
        },
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
    Some(CommandDef {
        name: file_stem(path),
        description: front.get_str("description"),
        template: front.body,
    })
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
    serde_json::from_str(&raw)
        .ok()
        .or_else(|| serde_json::from_str(&strip_jsonc(&raw)).ok())
}

fn file_stem(path: &Path) -> String {
    path.file_stem()
        .map(|stem| stem.to_string_lossy().to_string())
        .unwrap_or_default()
}

/// Removes `//` and `/* */` comments so JSONC configs can be parsed by
/// `serde_json`. String contents are preserved.
fn strip_jsonc(input: &str) -> String {
    let mut output = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    let mut in_string = false;
    let mut escaped = false;

    while let Some(ch) = chars.next() {
        if in_string {
            output.push(ch);
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
            }
            continue;
        }

        match ch {
            '"' => {
                in_string = true;
                output.push(ch);
            }
            '/' => match chars.peek() {
                Some('/') => {
                    for next in chars.by_ref() {
                        if next == '\n' {
                            output.push('\n');
                            break;
                        }
                    }
                }
                Some('*') => {
                    chars.next();
                    let mut previous = '\0';
                    for next in chars.by_ref() {
                        if previous == '*' && next == '/' {
                            break;
                        }
                        previous = next;
                    }
                }
                _ => output.push(ch),
            },
            _ => output.push(ch),
        }
    }
    output
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
    fn loads_both_layouts() {
        let dir = temp_dir("both");
        std::fs::create_dir_all(dir.join(".git")).unwrap();
        std::fs::create_dir_all(dir.join(".opencode/agent")).unwrap();
        std::fs::create_dir_all(dir.join(".opencode/command")).unwrap();
        std::fs::create_dir_all(dir.join(".claude/agents")).unwrap();
        std::fs::create_dir_all(dir.join(".claude/skills/audit")).unwrap();

        std::fs::write(
            dir.join(".opencode/agent/reviewer.md"),
            "---\ndescription: reviews code\nmode: subagent\n---\nReview carefully.",
        )
        .unwrap();
        std::fs::write(
            dir.join(".opencode/command/build.md"),
            "---\ndescription: build it\n---\nRun cargo build $ARGUMENTS",
        )
        .unwrap();
        std::fs::write(
            dir.join(".claude/agents/planner.md"),
            "---\nname: planner\ndescription: plans work\ntools: Read, Grep\n---\nPlan the work.",
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
            r#"{"mcpServers":{"fs":{"command":"npx","args":["-y","server-fs"]}}}"#,
        )
        .unwrap();
        std::fs::write(
            dir.join("opencode.json"),
            r#"{
                // jsonc comment
                "instructions": ["AGENTS.md"],
                "agent": {"inline": {"description": "inline agent", "prompt": "hi"}},
                "command": {"lint": {"template": "lint it"}},
                "mcp": {"remote": {"type": "remote", "url": "https://example.com/mcp"}}
            }"#,
        )
        .unwrap();
        std::fs::write(dir.join("AGENTS.md"), "Project rules.").unwrap();

        let ecosystem = load(&dir);

        assert!(ecosystem.agent("reviewer").is_some());
        assert!(ecosystem.agent("planner").is_some());
        assert!(ecosystem.agent("inline").is_some());
        assert!(ecosystem.command("build").is_some());
        assert!(ecosystem.command("lint").is_some());
        assert!(ecosystem.skills.iter().any(|skill| skill.name == "audit"));
        assert!(ecosystem.mcp.iter().any(|server| server.name == "fs"));
        assert!(ecosystem.mcp.iter().any(|server| server.name == "remote"));
        assert!(!ecosystem.memory.is_empty());
        assert!(ecosystem.rules.iter().any(|rule| rule.name == "AGENTS"));

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
        };
        assert_eq!(command.expand("world"), "Hello world from world");
    }
}
