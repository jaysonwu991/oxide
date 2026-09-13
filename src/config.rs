use crate::auth::AuthStore;
use crate::ecosystem::{self, AgentDef, Ecosystem, McpKind};
use crate::memory::{MemoryStore, Scope};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const DEFAULT_SYSTEM_PROMPT: &str = "\
You are Oxide, an AI coding agent running in the user's terminal. \
You help with software engineering tasks: writing, editing, debugging and explaining code. \
Use the provided tools to inspect and modify the user's project. \
You can see images and PDFs attached to user messages, and read_file returns image/PDF files as viewable attachments. \
Prefer small, focused changes and verify your work. \
Be concise. When you are done, give a short summary of what you changed.";

/// The API dialect a provider speaks. OpenAI-compatible providers (OpenAI,
/// DeepSeek, and most others) share one client; Anthropic uses its own
/// Messages API.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderKind {
    OpenAi,
    Anthropic,
}

/// Built-in defaults for a known provider name.
#[derive(Debug, Clone, Copy)]
pub struct ProviderPreset {
    pub kind: ProviderKind,
    pub base_url: &'static str,
    pub base_url_env: &'static str,
    pub model: &'static str,
    pub key_env: &'static str,
}

impl ProviderPreset {
    pub fn for_name(name: &str) -> Option<Self> {
        let preset = match name.trim().to_ascii_lowercase().as_str() {
            "openai" | "gpt" | "gpt-4" | "gpt-4o" => Self {
                kind: ProviderKind::OpenAi,
                base_url: "https://api.openai.com/v1",
                base_url_env: "OPENAI_BASE_URL",
                model: "gpt-4o-mini",
                key_env: "OPENAI_API_KEY",
            },
            "deepseek" => Self {
                kind: ProviderKind::OpenAi,
                base_url: "https://api.deepseek.com/v1",
                base_url_env: "DEEPSEEK_BASE_URL",
                model: "deepseek-chat",
                key_env: "DEEPSEEK_API_KEY",
            },
            "anthropic" => Self {
                kind: ProviderKind::Anthropic,
                base_url: "https://api.anthropic.com/v1",
                base_url_env: "ANTHROPIC_BASE_URL",
                model: "claude-3-5-sonnet-latest",
                key_env: "ANTHROPIC_API_KEY",
            },
            _ => return None,
        };
        Some(preset)
    }
}

/// The agent's permission mode, modelled on Claude Code. `Build` is the normal
/// mode and follows the active agent's permission rules; `Plan` is read-only and
/// forbids workspace mutations; `AutoEdit` auto-approves file edits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum Mode {
    #[default]
    Build,
    Plan,
    #[serde(
        alias = "auto_edit",
        alias = "autoedit",
        alias = "accept-edits",
        alias = "acceptEdits"
    )]
    AutoEdit,
}

impl Mode {
    /// Parses a user-supplied mode name, accepting common aliases.
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().replace('_', "-").as_str() {
            "build" | "default" | "normal" => Some(Mode::Build),
            "plan" => Some(Mode::Plan),
            "auto-edit" | "autoedit" | "accept-edits" | "acceptedits" | "edit" => {
                Some(Mode::AutoEdit)
            }
            _ => None,
        }
    }

    /// The next mode in the cycle used by the TUI (build → auto-edit → plan).
    pub fn next(self) -> Self {
        match self {
            Mode::Build => Mode::AutoEdit,
            Mode::AutoEdit => Mode::Plan,
            Mode::Plan => Mode::Build,
        }
    }

    /// A short lowercase label used in the UI and CLI.
    pub fn label(self) -> &'static str {
        match self {
            Mode::Build => "build",
            Mode::Plan => "plan",
            Mode::AutoEdit => "auto-edit",
        }
    }
}

/// How much reasoning effort to ask the model for. `Auto` (the default) turns
/// reasoning on for models known to support it and off otherwise; the explicit
/// levels map to OpenAI's `reasoning_effort` and Anthropic's extended-thinking
/// budget.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum Reasoning {
    #[default]
    Auto,
    Off,
    Low,
    Medium,
    High,
}

impl Reasoning {
    /// Parses a user-supplied level, accepting common aliases.
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().replace('_', "-").as_str() {
            "" | "auto" | "default" => Some(Reasoning::Auto),
            "off" | "none" | "disabled" => Some(Reasoning::Off),
            "low" | "minimal" | "small" => Some(Reasoning::Low),
            "medium" | "med" => Some(Reasoning::Medium),
            "high" | "xhigh" | "x-high" => Some(Reasoning::High),
            _ => None,
        }
    }

    /// A short lowercase label used in the UI and CLI.
    pub fn label(self) -> &'static str {
        match self {
            Reasoning::Auto => "auto",
            Reasoning::Off => "off",
            Reasoning::Low => "low",
            Reasoning::Medium => "medium",
            Reasoning::High => "high",
        }
    }

    /// The cycle used by the TUI (auto → off → low → medium → high).
    pub fn next(self) -> Self {
        match self {
            Reasoning::Auto => Reasoning::Off,
            Reasoning::Off => Reasoning::Low,
            Reasoning::Low => Reasoning::Medium,
            Reasoning::Medium => Reasoning::High,
            Reasoning::High => Reasoning::Auto,
        }
    }

    /// Resolves `Auto` against the model name.
    pub fn resolve(self, model: &str) -> Self {
        match self {
            Reasoning::Auto => {
                if supports_reasoning(model) {
                    Reasoning::Medium
                } else {
                    Reasoning::Off
                }
            }
            other => other,
        }
    }

    /// The OpenAI-compatible `reasoning_effort` value, if any.
    pub fn effort(self) -> Option<&'static str> {
        match self {
            Reasoning::Low => Some("low"),
            Reasoning::Medium => Some("medium"),
            Reasoning::High => Some("high"),
            Reasoning::Auto | Reasoning::Off => None,
        }
    }

    /// The Anthropic extended-thinking budget in tokens, if enabled.
    pub fn budget_tokens(self, max_tokens: u32) -> Option<u32> {
        let desired = match self {
            Reasoning::Low => 2048,
            Reasoning::Medium => 6144,
            Reasoning::High => 12_288,
            Reasoning::Auto | Reasoning::Off => return None,
        };
        let budget = desired.min(max_tokens.saturating_sub(1024));
        (budget >= 1024).then_some(budget)
    }
}

/// Models known to accept reasoning controls.
fn supports_reasoning(model: &str) -> bool {
    let model = model.to_ascii_lowercase();
    model.starts_with("o1")
        || model.starts_with("o3")
        || model.starts_with("o4")
        || model.starts_with("gpt-5")
        || model.contains("claude-3-7")
        || model.contains("claude-3.7")
        || model.contains("claude-sonnet-4")
        || model.contains("claude-opus-4")
        || model.contains("claude-4")
}

fn env_nonempty(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .filter(|value| !value.trim().is_empty())
}

fn explicit(raw: &Option<serde_json::Value>, key: &str) -> bool {
    raw.as_ref().is_some_and(|value| value.get(key).is_some())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub provider: String,
    pub model: String,
    pub base_url: String,
    pub api_key: String,
    #[serde(default = "default_system_prompt")]
    pub system_prompt: String,
    #[serde(default = "default_max_tokens")]
    pub max_tokens: u32,
    #[serde(default = "default_true")]
    pub auto_approve: bool,
    #[serde(default)]
    pub mode: Mode,
    #[serde(default)]
    pub reasoning: Reasoning,
    #[serde(skip)]
    pub ecosystem: Ecosystem,
    #[serde(skip)]
    pub active_agent: Option<AgentDef>,
    #[serde(skip)]
    pub memory: MemoryStore,
    #[serde(skip)]
    pub dcp: crate::dcp::DcpConfig,
}

fn default_system_prompt() -> String {
    DEFAULT_SYSTEM_PROMPT.to_string()
}

fn default_max_tokens() -> u32 {
    8192
}

fn default_true() -> bool {
    true
}

impl Default for Config {
    fn default() -> Self {
        Self {
            provider: "openai".to_string(),
            model: "gpt-4o-mini".to_string(),
            base_url: "https://api.openai.com/v1".to_string(),
            api_key: String::new(),
            system_prompt: default_system_prompt(),
            max_tokens: default_max_tokens(),
            auto_approve: true,
            mode: Mode::default(),
            reasoning: Reasoning::default(),
            ecosystem: Ecosystem::default(),
            active_agent: None,
            memory: MemoryStore::default(),
            dcp: crate::dcp::DcpConfig::default(),
        }
    }
}

impl Config {
    pub fn config_path() -> PathBuf {
        dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("oxide")
            .join("config.json")
    }

    /// Load config from disk, then apply CLI overrides and environment, and
    /// finally discover the project + global ecosystem from `cwd`.
    pub fn load(
        cwd: &Path,
        model: Option<String>,
        provider: Option<String>,
        agent: Option<String>,
        mode: Option<String>,
        reasoning: Option<String>,
    ) -> Result<Self> {
        let path = Self::config_path();
        let raw: Option<serde_json::Value> = if path.exists() {
            let text = std::fs::read_to_string(&path)
                .with_context(|| format!("reading config at {}", path.display()))?;
            Some(
                serde_json::from_str(&text)
                    .with_context(|| format!("parsing config at {}", path.display()))?,
            )
        } else {
            None
        };

        let mut config: Config = match &raw {
            Some(value) => serde_json::from_value(value.clone())
                .with_context(|| format!("parsing config at {}", path.display()))?,
            None => Self::default(),
        };

        let provider_from_env = env_nonempty("OXIDE_PROVIDER").is_some();
        let provider_overridden = provider.is_some() || provider_from_env;
        config.provider = provider
            .or_else(|| env_nonempty("OXIDE_PROVIDER"))
            .unwrap_or_else(|| config.provider.clone());
        let preset = ProviderPreset::for_name(&config.provider);

        if let Some(value) = model {
            config.model = value;
        } else if let Some(value) = env_nonempty("OXIDE_MODEL") {
            config.model = value;
        } else if let Some(preset) = preset {
            if provider_overridden || !explicit(&raw, "model") {
                config.model = preset.model.to_string();
            }
        }

        if let Some(url) = env_nonempty("OXIDE_BASE_URL") {
            config.base_url = url;
        } else if let Some(preset) = preset {
            if let Some(url) = env_nonempty(preset.base_url_env) {
                config.base_url = url;
            } else if provider_overridden || !explicit(&raw, "base_url") {
                config.base_url = preset.base_url.to_string();
            }
        } else if let Some(url) = env_nonempty("OPENAI_BASE_URL") {
            config.base_url = url;
        }

        if let Ok(store) = AuthStore::load() {
            if let Some(key) = store.key(&config.provider) {
                config.api_key = key.to_string();
            }
        }

        if let Some(preset) = preset {
            if let Some(key) = env_nonempty(preset.key_env) {
                config.api_key = key;
            }
        }
        if let Some(key) = env_nonempty("OXIDE_API_KEY") {
            config.api_key = key;
        }
        let openai_key_fallback = match preset {
            Some(preset) => preset.kind == ProviderKind::OpenAi,
            None => true,
        };
        if config.api_key.is_empty() && openai_key_fallback {
            if let Some(key) = env_nonempty("OPENAI_API_KEY") {
                config.api_key = key;
            }
        }

        config.base_url = config.base_url.trim_end_matches('/').to_string();

        if let Some(value) = mode.or_else(|| env_nonempty("OXIDE_MODE")) {
            config.mode = Mode::parse(&value).with_context(|| {
                format!("unknown mode `{value}` (expected build, plan, or auto-edit)")
            })?;
        }

        if let Some(value) = reasoning.or_else(|| env_nonempty("OXIDE_REASONING")) {
            config.reasoning = Reasoning::parse(&value).with_context(|| {
                format!(
                    "unknown reasoning level `{value}` (expected auto, off, low, medium, or high)"
                )
            })?;
        }

        config.ecosystem = ecosystem::load(cwd);
        config.memory = MemoryStore::load(cwd);
        config.dcp = crate::dcp::load_config(cwd);
        if let Some(name) = agent {
            config.activate_agent(&name)?;
        }

        Ok(config)
    }

    /// Selects a discovered agent as the active one. Fails when the name is
    /// unknown, listing the available agents.
    pub fn activate_agent(&mut self, name: &str) -> Result<()> {
        let Some(active) = self.ecosystem.agent(name).cloned() else {
            let available: Vec<&str> = self
                .ecosystem
                .agents
                .iter()
                .map(|agent| agent.name.as_str())
                .collect();
            anyhow::bail!(
                "unknown agent `{name}` (available: {})",
                available.join(", ")
            );
        };
        self.active_agent = Some(active);
        Ok(())
    }

    pub fn require_api_key(&self) -> Result<&str> {
        if self.api_key.trim().is_empty() {
            anyhow::bail!(
                "no API key found. Run `oxide auth login`, set {}, or add \"api_key\" to {}",
                self.key_env_name(),
                Self::config_path().display()
            );
        }
        Ok(&self.api_key)
    }

    /// The API dialect this configuration targets.
    pub fn provider_kind(&self) -> ProviderKind {
        ProviderPreset::for_name(&self.provider)
            .map(|preset| preset.kind)
            .unwrap_or(ProviderKind::OpenAi)
    }

    /// The environment variable that supplies the API key for this provider.
    pub fn key_env_name(&self) -> &'static str {
        ProviderPreset::for_name(&self.provider)
            .map(|preset| preset.key_env)
            .unwrap_or("OPENAI_API_KEY")
    }

    /// The reasoning level to use after resolving `Auto` against the model.
    pub fn effective_reasoning(&self) -> Reasoning {
        self.reasoning.resolve(&self.model)
    }

    /// Builds the effective system prompt from the base prompt plus the active
    /// agent, loaded memory, instructions, and an index of available
    /// skills/commands/subagents.
    pub fn compose_system_prompt(&self) -> String {
        let mut sections = vec![self.system_prompt.clone()];

        if self.mode == Mode::Plan {
            sections.push(
                "# Plan mode\nYou are in plan mode: do not modify files or run commands that \
                 change the workspace. Investigate the codebase with read-only tools and produce \
                 a clear, ordered implementation plan. Explain trade-offs and list the files you \
                 would change. Wait for the user to switch to build mode before making any edits."
                    .to_string(),
            );
        }

        if let Some(agent) = &self.active_agent {
            sections.push(format!(
                "You are operating as the `{}` agent.\n{}",
                agent.name, agent.prompt
            ));
        }

        for memory in &self.ecosystem.memory {
            sections.push(format!(
                "# Memory: {}\n{}",
                memory.name,
                memory.content.trim()
            ));
        }
        for rule in &self.ecosystem.rules {
            sections.push(format!(
                "# Instructions: {}\n{}",
                rule.name,
                rule.content.trim()
            ));
        }

        let remembered = self.memory.recent(8);
        if !remembered.is_empty() {
            let mut list = String::from(
                "# Persistent memory\nNotes retained across sessions. Use the `memory` tool to add, search, or forget entries.",
            );
            for entry in &remembered {
                let scope = match entry.scope {
                    Scope::Project => "project",
                    Scope::User => "user",
                };
                list.push_str(&format!("\n- [{scope}] {}", entry.content));
            }
            sections.push(list);
        }

        if !self.ecosystem.skills.is_empty() {
            let mut list = String::from(
                "# Available skills\nLoad a skill when its description matches the task.",
            );
            for skill in &self.ecosystem.skills {
                let description = skill.description.clone().unwrap_or_default();
                list.push_str(&format!("\n- {}: {}", skill.name, description));
            }
            sections.push(list);
        }

        if !self.ecosystem.commands.is_empty() {
            let mut list = String::from("# Available commands");
            for command in &self.ecosystem.commands {
                let description = command.description.clone().unwrap_or_default();
                list.push_str(&format!("\n- /{}: {}", command.name, description));
            }
            sections.push(list);
        }

        let subagents: Vec<&AgentDef> = self
            .ecosystem
            .agents
            .iter()
            .filter(|agent| agent.mode != ecosystem::AgentMode::Primary)
            .collect();
        if !subagents.is_empty() {
            let mut list = String::from("# Available subagents");
            for agent in subagents {
                let description = agent.description.clone().unwrap_or_default();
                list.push_str(&format!("\n- {}: {}", agent.name, description));
            }
            sections.push(list);
        }

        if !self.ecosystem.mcp.is_empty() {
            let mut list = String::from("# MCP servers");
            for server in &self.ecosystem.mcp {
                let transport = match &server.kind {
                    McpKind::Local { command, .. } => format!("local: {}", command.join(" ")),
                    McpKind::Remote { url, .. } => format!("remote: {url}"),
                };
                let state = if server.enabled {
                    "enabled"
                } else {
                    "disabled"
                };
                list.push_str(&format!("\n- {} ({state}, {transport})", server.name));
            }
            sections.push(list);
        }

        if self.dcp.enabled {
            sections.push(
                "# Context pruning\nUse the `compress` tool to replace closed, stale spans of the \
                 conversation with concise summaries and keep the context small. Message numbers \
                 are listed in periodic context reminders. Session history is never modified."
                    .to_string(),
            );
        }

        sections.join("\n\n")
    }

    /// Resolves a leading `/command` to its expanded prompt and any agent
    /// routing (`agent`, `subtask`) declared in the command's frontmatter.
    pub fn resolve_command(&self, input: &str) -> Option<ecosystem::ResolvedCommand> {
        self.ecosystem.resolve_command(input)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presets_cover_gpt_deepseek_and_anthropic() {
        assert_eq!(
            ProviderPreset::for_name("openai").unwrap().kind,
            ProviderKind::OpenAi
        );
        assert_eq!(
            ProviderPreset::for_name("gpt").unwrap().kind,
            ProviderKind::OpenAi
        );
        assert_eq!(
            ProviderPreset::for_name("deepseek").unwrap().kind,
            ProviderKind::OpenAi
        );
        assert_eq!(
            ProviderPreset::for_name("deepseek").unwrap().base_url,
            "https://api.deepseek.com/v1"
        );
        assert_eq!(
            ProviderPreset::for_name("anthropic").unwrap().kind,
            ProviderKind::Anthropic
        );
        assert_eq!(
            ProviderPreset::for_name("anthropic").unwrap().key_env,
            "ANTHROPIC_API_KEY"
        );
        assert!(ProviderPreset::for_name("custom-endpoint").is_none());
    }

    #[test]
    fn mode_parses_aliases_and_cycles() {
        assert_eq!(Mode::parse("build"), Some(Mode::Build));
        assert_eq!(Mode::parse("PLAN"), Some(Mode::Plan));
        assert_eq!(Mode::parse("auto_edit"), Some(Mode::AutoEdit));
        assert_eq!(Mode::parse("accept-edits"), Some(Mode::AutoEdit));
        assert_eq!(Mode::parse("nonsense"), None);

        assert_eq!(Mode::default(), Mode::Build);
        assert_eq!(Mode::Build.next(), Mode::AutoEdit);
        assert_eq!(Mode::AutoEdit.next(), Mode::Plan);
        assert_eq!(Mode::Plan.next(), Mode::Build);
    }

    #[test]
    fn reasoning_parses_aliases_and_cycles() {
        assert_eq!(Reasoning::parse("auto"), Some(Reasoning::Auto));
        assert_eq!(Reasoning::parse("OFF"), Some(Reasoning::Off));
        assert_eq!(Reasoning::parse("small"), Some(Reasoning::Low));
        assert_eq!(Reasoning::parse("medium"), Some(Reasoning::Medium));
        assert_eq!(Reasoning::parse("xhigh"), Some(Reasoning::High));
        assert_eq!(Reasoning::parse("nonsense"), None);

        assert_eq!(Reasoning::default(), Reasoning::Auto);
        assert_eq!(Reasoning::Auto.next(), Reasoning::Off);
        assert_eq!(Reasoning::Off.next(), Reasoning::Low);
        assert_eq!(Reasoning::Low.next(), Reasoning::Medium);
        assert_eq!(Reasoning::Medium.next(), Reasoning::High);
        assert_eq!(Reasoning::High.next(), Reasoning::Auto);
    }

    #[test]
    fn auto_reasoning_detects_reasoning_models() {
        assert_eq!(Reasoning::Auto.resolve("gpt-4o-mini"), Reasoning::Off);
        assert_eq!(Reasoning::Auto.resolve("o3-mini"), Reasoning::Medium);
        assert_eq!(Reasoning::Auto.resolve("gpt-5"), Reasoning::Medium);
        assert_eq!(
            Reasoning::Auto.resolve("claude-sonnet-4-20250514"),
            Reasoning::Medium
        );
        assert_eq!(Reasoning::High.resolve("gpt-4o-mini"), Reasoning::High);
    }

    #[test]
    fn reasoning_maps_to_provider_controls() {
        assert_eq!(Reasoning::Off.effort(), None);
        assert_eq!(Reasoning::Medium.effort(), Some("medium"));
        assert_eq!(Reasoning::High.budget_tokens(8192), Some(7168));
        assert_eq!(Reasoning::Low.budget_tokens(4096), Some(2048));
        assert_eq!(Reasoning::Off.budget_tokens(8192), None);
        assert_eq!(Reasoning::High.budget_tokens(1024), None);
    }

    #[test]
    fn plan_mode_adds_prompt_instructions() {
        let build = Config::default();
        assert!(!build.compose_system_prompt().contains("# Plan mode"));

        let plan = Config {
            mode: Mode::Plan,
            ..Config::default()
        };
        assert!(plan.compose_system_prompt().contains("# Plan mode"));
    }

    #[test]
    fn kind_and_key_env_resolve_from_config() {
        let anthropic = Config {
            provider: "anthropic".into(),
            ..Config::default()
        };
        assert_eq!(anthropic.provider_kind(), ProviderKind::Anthropic);
        assert_eq!(anthropic.key_env_name(), "ANTHROPIC_API_KEY");

        let custom = Config {
            provider: "my-endpoint".into(),
            ..Config::default()
        };
        assert_eq!(custom.provider_kind(), ProviderKind::OpenAi);
        assert_eq!(custom.key_env_name(), "OPENAI_API_KEY");
    }
}
