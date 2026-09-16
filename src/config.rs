use crate::auth::{canonical_provider, AuthStore};
use crate::ecosystem::{self, AgentDef, Ecosystem};
use crate::memory::{MemoryStore, Scope};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const DEFAULT_SYSTEM_PROMPT: &str = "\
You are Oxide, an AI coding agent running in the user's terminal. \
You help with software engineering tasks: writing, editing, debugging and explaining code. \
Use the provided tools to inspect and modify the user's project. \
You can see images and PDFs attached to user messages, and read returns image/PDF files as viewable attachments. \
Prefer small, focused changes and verify your work. \
Be concise. When you are done, give a short summary of what you changed.";

const PORTKEY_FALLBACK_MODELS: &[&str] = &[
    "claude-haiku-4-5",
    "claude-opus-4-8",
    "claude-opus-5",
    "claude-sonnet-4-6",
    "claude-sonnet-5",
    "glm-5.2",
    "gpt-5.4",
    "gpt-5.6-luna",
    "gpt-5.6-sol",
    "gpt-5.6-terra",
];

pub fn model_label(model: &str) -> &str {
    match model {
        "claude-haiku-4-5" => "Claude Haiku 4.5",
        "claude-opus-4-8" => "Claude Opus 4.8",
        "claude-opus-5" => "Claude Opus 5",
        "claude-sonnet-4-6" => "Claude Sonnet 4.6",
        "claude-sonnet-5" => "Claude Sonnet 5",
        "glm-5.2" => "GLM-5.2",
        "gpt-5.4" => "GPT-5.4",
        "gpt-5.6-luna" => "GPT-5.6 Luna",
        "gpt-5.6-sol" => "GPT-5.6 Sol",
        "gpt-5.6-terra" => "GPT-5.6 Terra",
        _ => model,
    }
}

/// The API dialect a provider speaks. OpenAI-compatible providers (OpenAI,
/// DeepSeek, Portkey, and most others) share one client; Anthropic uses its own
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
        let name = canonical_provider(name);
        let preset = match name.as_str() {
            "openai" => Self {
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
            "portkey" => Self {
                kind: ProviderKind::OpenAi,
                base_url: "https://api.portkey.ai/v1",
                base_url_env: "PORTKEY_BASE_URL",
                model: "claude-sonnet-5",
                key_env: "PORTKEY_API_KEY",
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

/// How much reasoning effort to ask the model for. `Auto` (the default) leaves
/// the effort to the provider or uses its native adaptive mode; explicit levels
/// are translated by the active provider client.
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

/// Newer Claude generations use adaptive thinking instead of fixed token
/// budgets. Provider-prefixed and Bedrock model ids are accepted.
pub fn supports_adaptive_thinking(model: &str) -> bool {
    let model = model.to_ascii_lowercase();
    let Some((_, suffix)) = model.split_once("claude-") else {
        return false;
    };
    let parts: Vec<&str> = suffix.split(['-', '.', '_']).collect();
    let Some(index) = parts
        .iter()
        .position(|part| part.len() <= 2 && part.chars().all(|ch| ch.is_ascii_digit()))
    else {
        return false;
    };
    let Ok(major) = parts[index].parse::<u32>() else {
        return false;
    };
    let minor = parts
        .get(index + 1)
        .filter(|part| part.len() <= 2 && part.chars().all(|ch| ch.is_ascii_digit()))
        .and_then(|part| part.parse::<u32>().ok())
        .unwrap_or(0);
    major > 4 || (major == 4 && minor >= 6)
}

fn env_nonempty(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .filter(|value| !value.trim().is_empty())
}

fn explicit(raw: &Option<serde_json::Value>, key: &str) -> bool {
    raw.as_ref().is_some_and(|value| value.get(key).is_some())
}

/// The provider and entry from `auth.json` when exactly one credential is
/// stored, so an unconfigured oxide can pick it without guessing.
fn single_stored_provider(store: &AuthStore) -> Option<(&String, &crate::auth::AuthEntry)> {
    if store.entries.len() == 1 {
        store.entries.iter().next()
    } else {
        None
    }
}

/// Adopts the sole stored credential when the config has no key yet, including
/// the provider's preset model and base URL unless the config set them.
fn apply_stored_provider_fallback(
    config: &mut Config,
    store: &AuthStore,
    raw: &Option<serde_json::Value>,
) {
    if !config.api_key.trim().is_empty() {
        return;
    }
    let Some((name, entry)) = single_stored_provider(store) else {
        return;
    };
    config.provider = name.clone();
    config.api_key = entry.key.clone();
    if let Some(preset) = ProviderPreset::for_name(name) {
        if !explicit(raw, "model") {
            config.model = preset.model.to_string();
        }
        if !explicit(raw, "base_url") {
            config.base_url = preset.base_url.to_string();
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    #[serde(default = "default_provider")]
    pub provider: String,
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub base_url: String,
    #[serde(default)]
    pub api_key: String,
    #[serde(default)]
    pub portkey_config: String,
    #[serde(default)]
    pub model_catalog: Vec<String>,
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
    #[serde(skip)]
    pub tool_filter: crate::cli::ToolFilter,
    #[serde(skip)]
    pub ephemeral: bool,
    #[serde(skip)]
    pub load_context_files: bool,
    #[serde(skip)]
    pub default_project_trust: crate::trust::DefaultTrust,
    #[serde(skip)]
    pub trusted: bool,
    #[serde(skip)]
    pub theme: crate::theme::Theme,
}

fn default_provider() -> String {
    "openai".to_string()
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

/// Reads `defaultProjectTrust` from the global `settings.json` in the oxide
/// config directory (Pi keeps the same key in `~/.pi/agent/settings.json`).
fn load_default_project_trust() -> crate::trust::DefaultTrust {
    let path = dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("oxide")
        .join("settings.json");
    let Ok(text) = std::fs::read_to_string(path) else {
        return crate::trust::DefaultTrust::default();
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
        return crate::trust::DefaultTrust::default();
    };
    value
        .get("defaultProjectTrust")
        .and_then(|value| value.as_str())
        .and_then(crate::trust::DefaultTrust::parse)
        .unwrap_or_default()
}

impl Default for Config {
    fn default() -> Self {
        Self {
            provider: "openai".to_string(),
            model: "gpt-4o-mini".to_string(),
            base_url: "https://api.openai.com/v1".to_string(),
            api_key: String::new(),
            portkey_config: String::new(),
            model_catalog: Vec::new(),
            system_prompt: default_system_prompt(),
            max_tokens: default_max_tokens(),
            auto_approve: true,
            mode: Mode::default(),
            reasoning: Reasoning::default(),
            ecosystem: Ecosystem::default(),
            active_agent: None,
            memory: MemoryStore::default(),
            dcp: crate::dcp::DcpConfig::default(),
            tool_filter: crate::cli::ToolFilter::default(),
            ephemeral: false,
            load_context_files: true,
            default_project_trust: crate::trust::DefaultTrust::default(),
            trusted: true,
            theme: crate::theme::Theme::default(),
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

        let provider_from_env = env_nonempty("OXIDE_PROVIDER");
        let provider_overridden = provider.is_some() || provider_from_env.is_some();
        config.provider = provider
            .or(provider_from_env)
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

        let store = AuthStore::load().ok();
        if let Some(store) = &store {
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
        if config.is_portkey() {
            if let Some(value) = env_nonempty("PORTKEY_CONFIG") {
                config.portkey_config = value;
            }
            if let Some(value) = env_nonempty("PORTKEY_MODELS") {
                config.model_catalog = value
                    .split(',')
                    .map(str::trim)
                    .filter(|model| !model.is_empty())
                    .map(str::to_string)
                    .collect();
            }
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

        // When no provider was chosen anywhere, fall back to the sole stored
        // credential so one saved TUI login is enough to get started.
        let provider_explicit = provider_overridden || explicit(&raw, "provider");
        if !provider_explicit {
            if let Some(store) = &store {
                apply_stored_provider_fallback(&mut config, store, &raw);
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

        config.default_project_trust = load_default_project_trust();
        config.ecosystem = ecosystem::load(cwd);
        config.memory = MemoryStore::load(cwd);
        config.dcp = crate::dcp::load_config(cwd);
        if let Some(name) = agent {
            config.activate_agent(&name)?;
        }

        Ok(config)
    }

    /// Reloads the ecosystem honoring a trust decision. Untrusted projects keep
    /// context files but drop project-local resources that can execute or
    /// reshape the agent.
    pub fn reload_ecosystem(&mut self, cwd: &Path) {
        self.ecosystem = if self.trusted {
            ecosystem::load_with(cwd, self.load_context_files)
        } else {
            ecosystem::load_opts(
                cwd,
                ecosystem::LoadOptions {
                    context_files: self.load_context_files,
                    project_resources: false,
                },
            )
        };
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
            let mut message = format!(
                "no API key found for `{}`. Start the TUI and run `/login {}`, set {}, or add \"api_key\" to {}",
                self.provider,
                self.provider,
                self.key_env_name(),
                Self::config_path().display()
            );
            if let Ok(store) = AuthStore::load() {
                let others: Vec<&str> = store
                    .entries
                    .keys()
                    .filter(|name| name.as_str() != self.provider)
                    .map(String::as_str)
                    .collect();
                if !others.is_empty() {
                    message.push_str(&format!(
                        "\nstored credentials exist for: {} (run with `--provider <name>`)",
                        others.join(", ")
                    ));
                }
            }
            anyhow::bail!(message);
        }
        Ok(&self.api_key)
    }

    /// Applies a provider credential to the running config.
    pub fn apply_provider(&mut self, provider: &str, key: &str) {
        let provider_changed = canonical_provider(&self.provider) != canonical_provider(provider);
        self.provider = provider.to_string();
        self.api_key = key.to_string();
        if provider_changed {
            let Some(preset) = ProviderPreset::for_name(provider) else {
                return;
            };
            self.model = preset.model.to_string();
            self.base_url = preset.base_url.to_string();
        }
    }

    /// Persists the active provider in `config.json` so the next launch uses it,
    /// preserving any other settings already in the file.
    pub fn set_active_provider_at(path: &Path, provider: &str) -> Result<()> {
        Self::set_active_field_at(path, "provider", provider)
    }

    /// Persists the selected model in `config.json`, preserving other settings.
    pub fn set_active_model_at(path: &Path, model: &str) -> Result<()> {
        Self::set_active_field_at(path, "model", model)
    }

    fn set_active_field_at(path: &Path, key: &str, value: &str) -> Result<()> {
        let mut root: serde_json::Value = if path.exists() {
            let text = std::fs::read_to_string(path)
                .with_context(|| format!("reading config at {}", path.display()))?;
            serde_json::from_str(&text).unwrap_or_else(|_| serde_json::json!({}))
        } else {
            serde_json::json!({})
        };
        if !root.is_object() {
            root = serde_json::json!({});
        }
        if let Some(object) = root.as_object_mut() {
            object.insert(
                key.to_string(),
                serde_json::Value::String(value.to_string()),
            );
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
        }
        let text = serde_json::to_string_pretty(&root)?;
        std::fs::write(path, text)
            .with_context(|| format!("writing config to {}", path.display()))?;
        Ok(())
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

    pub fn is_portkey(&self) -> bool {
        canonical_provider(&self.provider) == "portkey"
    }

    pub fn model_catalog(&self) -> Vec<String> {
        let mut models = if self.model_catalog.is_empty() && self.is_portkey() {
            PORTKEY_FALLBACK_MODELS
                .iter()
                .map(|model| (*model).to_string())
                .collect()
        } else {
            self.model_catalog.clone()
        };
        if self.is_portkey() && !self.model.trim().is_empty() {
            models.push(self.model.clone());
        }
        models.sort();
        models.dedup();
        models
    }

    /// Builds the effective system prompt from the base prompt plus the active
    /// agent, loaded memory, instructions, and an index of available
    /// skills/commands/subagents.
    pub fn compose_system_prompt(&self) -> String {
        // `.oxide/SYSTEM.md` replaces the default prompt; `APPEND_SYSTEM.md`
        // (and CLI `--append-system-prompt`) layer on top.
        let mut sections = vec![self
            .ecosystem
            .system_prompt
            .clone()
            .unwrap_or_else(|| self.system_prompt.clone())];
        sections.extend(
            self.ecosystem
                .append_system_prompt
                .iter()
                .map(|text| text.trim().to_string())
                .filter(|text| !text.is_empty()),
        );

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
                "# Context: {}\n{}",
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

        if !self.ecosystem.commands.is_empty() || !self.ecosystem.prompt_templates.is_empty() {
            let mut list = String::from("# Available commands");
            for command in &self.ecosystem.commands {
                let description = command.description.clone().unwrap_or_default();
                list.push_str(&format!("\n- /{}: {}", command.name, description));
            }
            for template in &self.ecosystem.prompt_templates {
                if self.ecosystem.command(&template.name).is_some() {
                    continue;
                }
                let description = template.description.clone().unwrap_or_default();
                list.push_str(&format!("\n- /{}: {}", template.name, description));
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
            sections.push(String::from(
                "# MCP servers\nUse a matching MCP server automatically when the user asks about \
                 content owned by that service or provides one of its URLs. Prefer its MCP tools \
                 over webfetch so authenticated documents, tickets, and other resources remain \
                 accessible. Call mcp_load for the matching server before using its tools.",
            ));
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
    fn presets_cover_built_in_providers() {
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
        let portkey = ProviderPreset::for_name("portkey").unwrap();
        assert_eq!(portkey.kind, ProviderKind::OpenAi);
        assert_eq!(portkey.base_url, "https://api.portkey.ai/v1");
        assert_eq!(portkey.model, "claude-sonnet-5");
        assert_eq!(portkey.key_env, "PORTKEY_API_KEY");
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
    fn detects_adaptive_claude_models() {
        assert!(supports_adaptive_thinking("claude-sonnet-5"));
        assert!(supports_adaptive_thinking("claude-opus-4-8"));
        assert!(supports_adaptive_thinking(
            "us.anthropic.claude-sonnet-4-6-20250929-v1:0"
        ));
        assert!(!supports_adaptive_thinking("claude-haiku-4-5"));
        assert!(!supports_adaptive_thinking("claude-sonnet-4"));
        assert!(!supports_adaptive_thinking("gpt-5.4"));
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
    fn mcp_prompt_routes_matching_service_content() {
        let mut config = Config::default();
        config.ecosystem.mcp.push(ecosystem::McpServer {
            name: "documents".to_string(),
            enabled: true,
            kind: ecosystem::McpKind::Remote {
                url: "https://docs.example.com/mcp".to_string(),
                headers: Default::default(),
                oauth: None,
            },
        });

        let prompt = config.compose_system_prompt();
        assert!(prompt.contains("Use a matching MCP server automatically"));
        assert!(prompt.contains("Call mcp_load for the matching server"));
        assert!(prompt.contains("Prefer its MCP tools over webfetch"));
    }

    #[test]
    fn single_stored_provider_only_when_unambiguous() {
        let mut store = AuthStore::default();
        assert!(single_stored_provider(&store).is_none());

        store.set("deepseek", "sk-deepseek");
        let (name, entry) = single_stored_provider(&store).unwrap();
        assert_eq!(name, "deepseek");
        assert_eq!(entry.key, "sk-deepseek");

        store.set("openai", "sk-openai");
        assert!(single_stored_provider(&store).is_none());
    }

    fn temp_dir(tag: &str) -> PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let unique = format!(
            "oxide-config-test-{tag}-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        );
        let dir = std::env::temp_dir().join(unique);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn apply_provider_updates_runtime_fields() {
        let mut config = Config::default();
        config.apply_provider("deepseek", "sk-test");
        assert_eq!(config.provider, "deepseek");
        assert_eq!(config.api_key, "sk-test");
        assert_eq!(config.model, "deepseek-chat");
        assert_eq!(config.base_url, "https://api.deepseek.com/v1");
    }

    #[test]
    fn apply_portkey_provider_updates_runtime_fields() {
        let mut config = Config::default();
        config.apply_provider("portkey", "pk-test");
        assert_eq!(config.provider, "portkey");
        assert_eq!(config.api_key, "pk-test");
        assert_eq!(config.model, "claude-sonnet-5");
        assert_eq!(config.base_url, "https://api.portkey.ai/v1");
        assert_eq!(config.key_env_name(), "PORTKEY_API_KEY");
        assert!(config.is_portkey());
    }

    #[test]
    fn refreshing_portkey_credentials_preserves_custom_gateway() {
        let mut config = Config {
            provider: "portkey".into(),
            model: "account-model".into(),
            base_url: "https://gateway.example.com/v1".into(),
            portkey_config: "pc-example".into(),
            ..Config::default()
        };
        config.apply_provider("port-key", "pk-new");
        assert_eq!(config.provider, "port-key");
        assert_eq!(config.api_key, "pk-new");
        assert_eq!(config.model, "account-model");
        assert_eq!(config.base_url, "https://gateway.example.com/v1");
        assert_eq!(config.portkey_config, "pc-example");
    }

    #[test]
    fn portkey_model_labels_are_friendly() {
        assert_eq!(model_label("claude-sonnet-5"), "Claude Sonnet 5");
        assert_eq!(model_label("gpt-5.6-terra"), "GPT-5.6 Terra");
        assert_eq!(model_label("custom-model"), "custom-model");
    }

    #[test]
    fn apply_provider_leaves_unknown_endpoint_untouched() {
        let mut config = Config::default();
        let model = config.model.clone();
        let base_url = config.base_url.clone();
        config.apply_provider("my-endpoint", "sk-test");
        assert_eq!(config.provider, "my-endpoint");
        assert_eq!(config.api_key, "sk-test");
        assert_eq!(config.model, model);
        assert_eq!(config.base_url, base_url);
    }

    #[test]
    fn fallback_adopts_sole_stored_credential() {
        let mut config = Config::default();
        let mut store = AuthStore::default();
        store.set("deepseek", "sk-deepseek");
        apply_stored_provider_fallback(&mut config, &store, &None);
        assert_eq!(config.provider, "deepseek");
        assert_eq!(config.api_key, "sk-deepseek");
        assert_eq!(config.model, "deepseek-chat");
        assert_eq!(config.base_url, "https://api.deepseek.com/v1");
    }

    #[test]
    fn fallback_keeps_existing_key() {
        let mut config = Config {
            api_key: "sk-openai".into(),
            ..Config::default()
        };
        let mut store = AuthStore::default();
        store.set("deepseek", "sk-deepseek");
        apply_stored_provider_fallback(&mut config, &store, &None);
        assert_eq!(config.provider, "openai");
        assert_eq!(config.api_key, "sk-openai");
    }

    #[test]
    fn fallback_ignores_multiple_credentials() {
        let mut config = Config::default();
        let mut store = AuthStore::default();
        store.set("deepseek", "sk-deepseek");
        store.set("anthropic", "sk-anthropic");
        apply_stored_provider_fallback(&mut config, &store, &None);
        assert_eq!(config.provider, "openai");
        assert!(config.api_key.is_empty());
    }

    #[test]
    fn fallback_respects_explicit_model_and_base_url() {
        let raw = Some(serde_json::json!({
            "model": "custom-model",
            "base_url": "https://custom.example/v1"
        }));
        let mut config = Config {
            model: "custom-model".into(),
            base_url: "https://custom.example/v1".into(),
            ..Config::default()
        };
        let mut store = AuthStore::default();
        store.set("deepseek", "sk-deepseek");
        apply_stored_provider_fallback(&mut config, &store, &raw);
        assert_eq!(config.provider, "deepseek");
        assert_eq!(config.api_key, "sk-deepseek");
        assert_eq!(config.model, "custom-model");
        assert_eq!(config.base_url, "https://custom.example/v1");
    }

    #[test]
    fn config_parses_provider_only_file() {
        let config: Config = serde_json::from_str(r#"{"provider":"deepseek"}"#).unwrap();
        assert_eq!(config.provider, "deepseek");
        assert!(config.model.is_empty());
        assert!(config.base_url.is_empty());
        assert!(config.api_key.is_empty());
    }

    #[test]
    fn set_active_provider_writes_and_preserves_settings() {
        let dir = temp_dir("active-provider");
        let path = dir.join("config.json");
        std::fs::write(&path, r#"{"mode":"plan"}"#).unwrap();

        Config::set_active_provider_at(&path, "anthropic").unwrap();

        let value: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(value["provider"], "anthropic");
        assert_eq!(value["mode"], "plan");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn set_active_model_writes_and_preserves_settings() {
        let dir = temp_dir("active-model");
        let path = dir.join("config.json");
        std::fs::write(&path, r#"{"provider":"deepseek","mode":"plan"}"#).unwrap();

        Config::set_active_model_at(&path, "deepseek-reasoner").unwrap();

        let value: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(value["model"], "deepseek-reasoner");
        assert_eq!(value["provider"], "deepseek");
        assert_eq!(value["mode"], "plan");
        std::fs::remove_dir_all(&dir).ok();
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
