//! Discovery and loading of the configuration ecosystem: rules, memory,
//! commands, agents, skills, MCP servers and plugins. The native Oxide layout
//! (`.oxide/`, `AGENTS.md`) is read first, then the Claude Code layout
//! (`.claude/`, `CLAUDE.md`, `.mcp.json`) for compatibility, from the project
//! scope and the user's global scope. Project entries override global entries
//! with the same name, and Oxide entries override Claude Code entries.

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

    /// Resolves a leading `/command` into its expanded prompt and the agent
    /// routing (`agent`, `subtask`) declared in the command's frontmatter.
    pub fn resolve_command(&self, input: &str) -> Option<ResolvedCommand> {
        let trimmed = input.trim();
        let rest = trimmed.strip_prefix('/')?;
        let mut parts = rest.splitn(2, char::is_whitespace);
        let name = parts.next()?;
        let arguments = parts.next().unwrap_or("").trim();
        let command = self.command(name)?;
        Some(ResolvedCommand {
            prompt: command.expand(arguments),
            agent: command.agent.clone(),
            subtask: command.subtask,
        })
    }
}

/// Loads the ecosystem visible from `cwd`, merging global scope first and
/// project scope second (project wins). Within a scope the Oxide layout is
/// loaded after the Claude Code layout so it takes precedence.
pub fn load(cwd: &Path) -> Ecosystem {
    let mut ecosystem = Ecosystem::default();

    if let Some(home) = dirs::home_dir() {
        load_claude_dir(&mut ecosystem, &home.join(".claude"));
        load_claude_mcp(&mut ecosystem, &home.join(".claude.json"));
        load_oxide_dir(&mut ecosystem, &home.join(".oxide"));
    }

    if let Some(root) = project_root(cwd) {
        load_claude_dir(&mut ecosystem, &root.join(".claude"));
        for name in ["CLAUDE.md", "CLAUDE.local.md"] {
            push_memory(&mut ecosystem, &root.join(name));
        }
        load_claude_mcp(&mut ecosystem, &root.join(".mcp.json"));
        load_oxide_dir(&mut ecosystem, &root.join(".oxide"));
        push_memory(&mut ecosystem, &root.join("AGENTS.md"));
    }

    ecosystem
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
}

// ---------------------------------------------------------------------------
// Claude Code layout
// ---------------------------------------------------------------------------

fn load_claude_dir(ecosystem: &mut Ecosystem, dir: &Path) {
    load_layout(ecosystem, dir, "CLAUDE.md");
}

fn load_layout(ecosystem: &mut Ecosystem, dir: &Path, memory_file: &str) {
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
        assert!(ecosystem.memory.iter().any(|entry| entry.name == "AGENTS"));

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
}
