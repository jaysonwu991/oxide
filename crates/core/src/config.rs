use crate::auth::{canonical_provider, AuthStore};
use crate::ecosystem::{self, AgentDef, Ecosystem};
use crate::memory::{MemoryStore, Scope};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub const DEFAULT_SYSTEM_PROMPT: &str = "\
You are Oxide, an AI coding agent running in the user's terminal. \
You help with software engineering tasks — writing, editing, debugging and explaining code — \
and with the work around them: research, automation, and answering questions. \
Use the provided tools to inspect and modify the user's project. \
You can see images and PDFs attached to user messages, and read returns image/PDF files as viewable attachments. \
Prefer small, focused changes and verify your work. \
Be concise while you work, but when you are done give the user a detailed final summary: what \
changed and why, the files or areas you touched, and how you verified it. Make it detailed enough \
to review without re-reading the conversation, but do not pad it or restate unchanged code.";

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

/// The GLM models offered in the picker for Z.AI, whose API has no documented
/// model listing endpoint.
const GLM_FALLBACK_MODELS: &[&str] = &[
    "glm-4.5-air",
    "glm-4.6",
    "glm-4.7",
    "glm-4.7-flash",
    "glm-5.1",
    "glm-5.2",
    "glm-5.3",
    "glm-5.3-flash",
];

pub fn model_label(model: &str) -> &str {
    match model {
        "claude-haiku-4-5" => "Claude Haiku 4.5",
        "claude-opus-4-8" => "Claude Opus 4.8",
        "claude-opus-5" => "Claude Opus 5",
        "claude-sonnet-4-6" => "Claude Sonnet 4.6",
        "claude-sonnet-5" => "Claude Sonnet 5",
        "deepseek-flash" => "DeepSeek V4.1 Flash",
        "deepseek-v4-pro" => "DeepSeek V4 Pro",
        "glm-4.5-air" => "GLM-4.5 Air",
        "glm-4.6" => "GLM-4.6",
        "glm-4.7" => "GLM-4.7",
        "glm-4.7-flash" => "GLM-4.7 Flash",
        "glm-5.1" => "GLM-5.1",
        "glm-5.2" => "GLM-5.2",
        "glm-5.3" => "GLM-5.3",
        "glm-5.3-flash" => "GLM-5.3 Flash",
        "glm-5.3-flashx" => "GLM-5.3 FlashX",
        "gpt-5.4" => "GPT-5.4",
        "gpt-5.6-luna" => "GPT-5.6 Luna",
        "gpt-5.6-sol" => "GPT-5.6 Sol",
        "gpt-5.6-terra" => "GPT-5.6 Terra",
        _ => model,
    }
}

/// Every logged-in provider as its own `Config`, starting with the active one.
/// Used by `/models` (CLI) and the desktop model picker to query each
/// provider's catalog.
pub fn provider_configs(config: &Config) -> Vec<(String, Config)> {
    let active = canonical_provider(&config.provider);
    let mut providers: Vec<(String, Config)> = Vec::new();
    if !config.api_key.trim().is_empty() {
        providers.push((active.clone(), config.clone()));
    }
    let store = crate::auth::AuthStore::load().unwrap_or_default();
    for name in store.providers() {
        if providers.iter().any(|(known, _)| known == &name) {
            continue;
        }
        let is_preset = ProviderPreset::for_name(&name).is_some();
        if !is_preset && !config.provider_base_urls.contains_key(&name) {
            continue;
        }
        let key = store.key(&name).unwrap_or_default().to_string();
        providers.push((name.clone(), config.for_provider(&name, &key)));
    }
    providers
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
            "zai" => Self {
                kind: ProviderKind::OpenAi,
                base_url: "https://api.z.ai/api/paas/v4",
                base_url_env: "ZAI_BASE_URL",
                model: "glm-5.3",
                key_env: "ZAI_API_KEY",
            },
            _ => return None,
        };
        Some(preset)
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

/// Whether a GLM model always thinks. The GLM-5.3 series rejects
/// `thinking.type = "disabled"`, so the lowest effort is the closest oxide can
/// get to turning reasoning off there.
pub fn glm_forces_thinking(model: &str) -> bool {
    let model = model.to_ascii_lowercase();
    let Some(version) = model.strip_prefix("glm-") else {
        return false;
    };
    let version = version.split(['-', '/']).next().unwrap_or_default();
    let mut parts = version.split('.');
    let major = parts
        .next()
        .filter(|part| part.len() <= 2 && part.chars().all(|ch| ch.is_ascii_digit()))
        .and_then(|part| part.parse::<u32>().ok());
    let minor = parts
        .next()
        .and_then(|part| part.parse::<u32>().ok())
        .unwrap_or(0);
    matches!(major, Some(major) if major > 5 || (major == 5 && minor >= 3))
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
    if ProviderPreset::for_name(name).is_some() && !explicit(raw, "model") {
        // A model remembered for this provider wins over its preset.
        config.model = config.model_for_provider(name);
    }
    if !explicit(raw, "base_url") {
        if let Some(url) = config.remembered_base_url(name) {
            config.base_url = url;
        } else if let Some(preset) = ProviderPreset::for_name(name) {
            config.base_url = preset.base_url.to_string();
        }
    }
}

const CONFIG_DIR_NAME: &str = "Oxide";

/// The Oxide configuration directory under the platform config dir
/// (`<platform config>/Oxide`). The pre-branding `<platform config>/oxide`
/// directory is migrated once on first access when the new one is absent.
pub fn config_dir() -> Option<PathBuf> {
    let base = dirs::config_dir()?;
    let dir = base.join(CONFIG_DIR_NAME);
    migrate_legacy_config_dir(&base, &dir);
    Some(dir)
}

/// Like [`config_dir`], falling back to a relative `Oxide` path when the
/// platform config directory cannot be resolved.
pub fn config_dir_or_default() -> PathBuf {
    config_dir().unwrap_or_else(|| PathBuf::from(CONFIG_DIR_NAME))
}

fn migrate_legacy_config_dir(base: &Path, dir: &Path) {
    if has_dir_entry(base, CONFIG_DIR_NAME) {
        return;
    }
    if !has_dir_entry(base, "oxide") {
        return;
    }
    // Rename through a temporary name: on case-insensitive filesystems
    // (macOS, Windows) `oxide` and `Oxide` are the same directory, so a
    // direct rename would not change the on-disk case.
    let staging = base.join(".Oxide-migrate");
    let _ = std::fs::remove_dir_all(&staging);
    if std::fs::rename(base.join("oxide"), &staging).is_ok() {
        let _ = std::fs::rename(&staging, dir);
    }
}

/// Whether `base` contains an entry whose name matches `name` exactly
/// (case-sensitively), even on a case-insensitive filesystem.
fn has_dir_entry(base: &Path, name: &str) -> bool {
    std::fs::read_dir(base)
        .map(|entries| {
            entries.flatten().any(|entry| {
                entry.file_name() == std::ffi::OsStr::new(name)
                    && entry.file_type().map(|kind| kind.is_dir()).unwrap_or(true)
            })
        })
        .unwrap_or(false)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    #[serde(default = "default_provider")]
    pub provider: String,
    #[serde(default)]
    pub model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_model: Option<String>,
    #[serde(default)]
    pub base_url: String,
    #[serde(default)]
    pub api_key: String,
    #[serde(default)]
    pub portkey_config: String,
    /// The model last used with each provider, so switching between logged-in
    /// providers restores that provider's model instead of leaving the other
    /// provider's model id behind.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub provider_models: BTreeMap<String, String>,
    /// A custom endpoint last used with each provider, so switching between
    /// providers keeps each one's gateway instead of leaking the active
    /// `base_url` into the others.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub provider_base_urls: BTreeMap<String, String>,
    #[serde(default)]
    pub model_catalog: Vec<String>,
    #[serde(default = "default_system_prompt")]
    pub system_prompt: String,
    #[serde(default = "default_max_tokens")]
    pub max_tokens: u32,
    #[serde(default = "default_true")]
    pub auto_approve: bool,
    #[serde(default)]
    pub reasoning: Reasoning,
    #[serde(skip)]
    pub ecosystem: Ecosystem,
    #[serde(skip)]
    pub active_agent: Option<AgentDef>,
    #[serde(skip)]
    pub memory: MemoryStore,
    #[serde(skip)]
    pub compaction: crate::compact::CompactionConfig,
    #[serde(skip)]
    pub prices: BTreeMap<String, crate::pricing::ModelPrice>,
    /// Whether a finished agent turn raises a desktop toast, and whether it
    /// plays the system alert sound.
    #[serde(skip)]
    pub notify: crate::notify::NotifyConfig,
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

/// Reads `hideThinkingBlock` from the global `settings.json`, Pi's key for
/// whether reasoning blocks start collapsed.
pub fn load_hide_thinking_block() -> bool {
    let path = config_dir_or_default().join("settings.json");
    let Ok(text) = std::fs::read_to_string(path) else {
        return false;
    };
    serde_json::from_str::<serde_json::Value>(&text)
        .ok()
        .and_then(|value| value.get("hideThinkingBlock")?.as_bool())
        .unwrap_or(false)
}

/// Reads `defaultProjectTrust` from the global `settings.json` in the oxide
/// config directory (Pi keeps the same key in `~/.pi/agent/settings.json`).
pub(crate) fn load_default_project_trust() -> crate::trust::DefaultTrust {
    let path = config_dir_or_default().join("settings.json");
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
            default_model: None,
            base_url: "https://api.openai.com/v1".to_string(),
            api_key: String::new(),
            portkey_config: String::new(),
            provider_models: BTreeMap::new(),
            provider_base_urls: BTreeMap::new(),
            model_catalog: Vec::new(),
            system_prompt: default_system_prompt(),
            max_tokens: default_max_tokens(),
            auto_approve: true,
            reasoning: Reasoning::default(),
            ecosystem: Ecosystem::default(),
            active_agent: None,
            memory: MemoryStore::default(),
            compaction: crate::compact::CompactionConfig::default(),
            prices: crate::pricing::defaults(),
            notify: crate::notify::NotifyConfig::default(),
            tool_filter: crate::cli::ToolFilter::default(),
            ephemeral: false,
            load_context_files: true,
            default_project_trust: crate::trust::DefaultTrust::default(),
            trusted: true,
        }
    }
}

impl Config {
    /// Cost of one usage in USD, or 0 when the model has no known price.
    pub fn usage_cost(&self, usage: &crate::llm::Usage) -> f64 {
        crate::pricing::lookup(&self.prices, &self.model)
            .map(|price| price.cost(usage))
            .unwrap_or(0.0)
    }

    /// Whether the active model is known to support a thinking/reasoning level.
    /// The footer appends ` • <level>` only for these, like Pi's `model.reasoning`.
    pub fn supports_reasoning(&self) -> bool {
        let model = self.model.to_ascii_lowercase();
        const REASONING: [&str; 12] = [
            "o1",
            "o3",
            "o4",
            "gpt-5",
            "claude",
            "deepseek-reasoner",
            "deepseek-v4",
            "deepseek-flash",
            "glm",
            "gemini-2.5",
            "qwen3",
            "sonnet",
        ];
        REASONING.iter().any(|prefix| model.starts_with(prefix))
    }

    /// The model's context window, used for the Pi-style context percentage
    /// and compaction threshold. `OXIDE_CONTEXT_LIMIT` overrides it.
    pub fn context_window(&self) -> u64 {
        std::env::var("OXIDE_CONTEXT_LIMIT")
            .ok()
            .and_then(|value| value.trim().parse::<u64>().ok())
            .filter(|value| *value > 0)
            .unwrap_or_else(|| (self.max_tokens as u64).max(128_000))
    }

    pub fn config_path() -> PathBuf {
        config_dir_or_default().join("config.json")
    }

    /// Load config from disk, then apply CLI overrides and environment, and
    /// finally discover the project + global ecosystem from `cwd`.
    pub fn load(
        cwd: &Path,
        model: Option<String>,
        provider: Option<String>,
        agent: Option<String>,
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
                config.base_url = config
                    .remembered_base_url(&config.provider)
                    .unwrap_or_else(|| preset.base_url.to_string());
            }
        } else if let Some(url) = env_nonempty("OPENAI_BASE_URL") {
            config.base_url = url;
        } else if provider_overridden || !explicit(&raw, "base_url") {
            // A custom provider's remembered endpoint follows it on startup.
            if let Some(url) = config.remembered_base_url(&config.provider) {
                config.base_url = url;
            }
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
        config.compaction = crate::compact::load_config(cwd);
        config.prices = crate::pricing::load(cwd);
        config.notify = crate::notify::load_config(cwd);
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

    /// Applies a provider credential to the running config. Switching providers
    /// remembers the outgoing provider's model and restores the incoming one's,
    /// so several stored logins keep separate model choices.
    pub fn apply_provider(&mut self, provider: &str, key: &str) {
        let current = canonical_provider(&self.provider);
        let target = canonical_provider(provider);
        let provider_changed = current != target;
        if provider_changed && !current.is_empty() && !self.model.trim().is_empty() {
            self.provider_models
                .insert(current.clone(), self.model.clone());
        }
        if provider_changed && !current.is_empty() {
            self.remember_base_url(&current);
        }
        self.provider = provider.to_string();
        self.api_key = key.to_string();
        if provider_changed {
            self.model = self.model_for_provider(provider);
            match ProviderPreset::for_name(provider) {
                Some(preset) => {
                    self.base_url = self
                        .provider_base_urls
                        .get(&target)
                        .cloned()
                        .unwrap_or_else(|| preset.base_url.to_string());
                }
                // A custom provider keeps whatever endpoint it was last given;
                // the previous provider's URL is used when none is remembered.
                None => {
                    if let Some(url) = self.provider_base_urls.get(&target).cloned() {
                        self.base_url = url;
                    }
                }
            }
        }
    }

    /// Records the outgoing provider's endpoint when it was customized, or
    /// clears it when it matches the preset default, so each provider only
    /// carries a `base_url` it actually needs.
    fn remember_base_url(&mut self, provider: &str) {
        let name = canonical_provider(provider);
        let default = ProviderPreset::for_name(&name).map(|preset| preset.base_url.to_string());
        if self.base_url.trim().is_empty() || default.as_deref() == Some(self.base_url.as_str()) {
            self.provider_base_urls.remove(&name);
        } else {
            self.provider_base_urls.insert(name, self.base_url.clone());
        }
    }

    /// The custom endpoint remembered for a provider, if one was saved.
    fn remembered_base_url(&self, provider: &str) -> Option<String> {
        self.provider_base_urls
            .get(&canonical_provider(provider))
            .cloned()
    }

    /// The endpoint to use with a provider: the custom one remembered for it,
    /// the active provider's current URL, otherwise the provider preset's.
    pub fn base_url_for_provider(&self, provider: &str) -> String {
        let name = canonical_provider(provider);
        if let Some(url) = self.remembered_base_url(&name) {
            return url;
        }
        if name == canonical_provider(&self.provider) && !self.base_url.trim().is_empty() {
            return self.base_url.clone();
        }
        ProviderPreset::for_name(&name)
            .map(|preset| preset.base_url.to_string())
            .unwrap_or_else(|| self.base_url.clone())
    }

    /// Applies the optional settings chosen in the login dialog. Blank values
    /// keep the provider's current/default model, endpoint, or Config ID, and
    /// a Portkey Config ID is ignored for other providers.
    pub fn apply_login_options(&mut self, model: &str, base_url: &str, portkey_config: &str) {
        let name = canonical_provider(&self.provider);
        if !model.trim().is_empty() {
            self.model = model.trim().to_string();
            self.provider_models
                .insert(name.clone(), self.model.clone());
        }
        if !base_url.trim().is_empty() {
            self.base_url = base_url.trim().trim_end_matches('/').to_string();
            self.remember_base_url(&name);
        }
        if name == "portkey" && !portkey_config.trim().is_empty() {
            self.portkey_config = portkey_config.trim().to_string();
        }
    }

    /// The model to use with a provider: the one remembered for it, otherwise
    /// the provider preset's, otherwise the model in use.
    pub fn model_for_provider(&self, provider: &str) -> String {
        let name = canonical_provider(provider);
        self.provider_models
            .get(&name)
            .cloned()
            .or_else(|| ProviderPreset::for_name(&name).map(|preset| preset.model.to_string()))
            .unwrap_or_else(|| self.model.clone())
    }

    /// A copy of this config pointed at another provider, for querying that
    /// provider's catalog without touching the running session. The Portkey
    /// model catalog only carries over to Portkey itself.
    pub fn for_provider(&self, provider: &str, key: &str) -> Self {
        let mut config = self.clone();
        config.apply_provider(provider, key);
        if !config.is_portkey() {
            config.model_catalog.clear();
        }
        config
    }

    /// Persists the active provider, its model, and the per-provider model
    /// memory so the next launch resumes this selection.
    pub fn persist_selection_at(&self, path: &Path) -> Result<()> {
        let provider = canonical_provider(&self.provider);
        let model = self.model.clone();
        let memory = self.provider_models.clone();
        let endpoints = self.provider_base_urls.clone();
        let base_url = self.base_url.clone();
        let default_url =
            ProviderPreset::for_name(&provider).map(|preset| preset.base_url.to_string());
        Self::update_at(path, move |object| {
            object.insert(
                "provider".to_string(),
                serde_json::Value::String(provider.clone()),
            );
            object.insert("model".to_string(), serde_json::Value::String(model));
            // `base_url` describes the active provider only; clear it when it
            // is the preset default so a switch cannot leave a custom gateway
            // behind for the next provider.
            if base_url.trim().is_empty() || default_url.as_deref() == Some(base_url.as_str()) {
                object.remove("base_url");
            } else {
                object.insert("base_url".to_string(), serde_json::Value::String(base_url));
            }
            if !memory.is_empty() {
                let memory_entry = object
                    .entry("provider_models")
                    .or_insert_with(|| serde_json::json!({}));
                if !memory_entry.is_object() {
                    *memory_entry = serde_json::json!({});
                }
                if let Some(object) = memory_entry.as_object_mut() {
                    for (name, model) in memory {
                        object.insert(name, serde_json::Value::String(model));
                    }
                }
            }
            if endpoints.is_empty() {
                object.remove("provider_base_urls");
            } else {
                object.insert(
                    "provider_base_urls".to_string(),
                    serde_json::to_value(&endpoints).unwrap_or_else(|_| serde_json::json!({})),
                );
            }
        })
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

    /// Persists the model used by default for new sessions (`/models` Ctrl+S).
    pub fn set_default_model_at(path: &Path, model: &str) -> Result<()> {
        Self::set_active_field_at(path, "default_model", model)
    }

    /// Persists the selected theme name in `config.json`, so the CLI and the
    /// desktop both start with it (the CLI reads the same `theme` key).
    pub fn set_theme_at(path: &Path, name: &str) -> Result<()> {
        Self::set_active_field_at(path, "theme", name)
    }

    fn set_active_field_at(path: &Path, key: &str, value: &str) -> Result<()> {
        let key = key.to_string();
        let value = value.to_string();
        Self::update_at(path, move |object| {
            object.insert(key, serde_json::Value::String(value));
        })
    }

    /// Reads, edits, and writes `config.json`, preserving every other setting
    /// and recovering from a missing or malformed file.
    fn update_at(
        path: &Path,
        update: impl FnOnce(&mut serde_json::Map<String, serde_json::Value>),
    ) -> Result<()> {
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
            update(object);
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

    /// Whether the active provider is Z.AI's GLM API, which selects thinking
    /// with `thinking.type` instead of `reasoning_effort`. A custom provider
    /// pointed at either Z.AI host is detected the same way Pi does.
    pub fn is_zai(&self) -> bool {
        if canonical_provider(&self.provider) == "zai" {
            return true;
        }
        let base_url = self.base_url.to_ascii_lowercase();
        base_url.contains("api.z.ai") || base_url.contains("open.bigmodel.cn")
    }

    /// Whether the active provider is DeepSeek's API, whose thinking mode
    /// requires an assistant turn's `reasoning_content` to be replayed on the
    /// next request. A custom provider pointed at the same host is detected
    /// too.
    pub fn is_deepseek(&self) -> bool {
        if canonical_provider(&self.provider) == "deepseek" {
            return true;
        }
        self.base_url
            .to_ascii_lowercase()
            .contains("api.deepseek.com")
    }

    /// The models bundled with a provider whose catalog cannot be listed.
    fn bundled_models(&self) -> Vec<String> {
        let models: &[&str] = if self.is_portkey() {
            PORTKEY_FALLBACK_MODELS
        } else if self.is_zai() {
            GLM_FALLBACK_MODELS
        } else {
            &[]
        };
        models.iter().map(|model| (*model).to_string()).collect()
    }

    pub fn model_catalog(&self) -> Vec<String> {
        let bundled = self.bundled_models();
        let mut models = if self.model_catalog.is_empty() {
            bundled.clone()
        } else {
            self.model_catalog.clone()
        };
        if !bundled.is_empty() && !self.model.trim().is_empty() {
            models.push(self.model.clone());
        }
        models.sort();
        models.dedup();
        models
    }

    /// Normalizes the list of models reported by the provider. A known built-in
    /// provider is authoritative, so the active model is only injected when the
    /// provider reports nothing (or the endpoint is not recognized), keeping
    /// stale or hand-typed ids out of the picker.
    pub fn merge_model_catalog(&self, mut models: Vec<String>) -> Vec<String> {
        let known = ProviderPreset::for_name(&self.provider).is_some();
        if (!known || models.is_empty()) && !self.model.trim().is_empty() {
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
                "# Available skills\nLoad a skill with the `skill` tool when its description \
                 matches the task; users can also force one with `/skill:<name>`.",
            );
            for skill in &self.ecosystem.skills {
                let description = skill.description.clone().unwrap_or_default();
                list.push_str(&format!("\n- {}: {}", skill.name, description));
            }
            sections.push(list);
        }

        if !self.ecosystem.commands.is_empty() || !self.ecosystem.prompt_templates.is_empty() {
            let mut list = String::from(
                "# Available commands\nInvoke a command with the `command` tool when the user's \
                 request matches its description; pass any focus or scope as `arguments`.",
            );
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
            let mut list = String::from(
                "# MCP servers\nRoute requests to the matching MCP server: if a pasted URL's domain \
                 matches one of the domains below (or the service is named), call mcp_load for that \
                 server first and use its tools instead of webfetch, so authenticated documents, \
                 tickets, and other resources stay accessible.",
            );
            for server in &self.ecosystem.mcp {
                if !server.enabled {
                    continue;
                }
                let domains = server.domains();
                let domains = if domains.is_empty() {
                    String::new()
                } else {
                    format!(" — {}", domains.join(", "))
                };
                list.push_str(&format!("\n- {}{}", server.name, domains));
            }
            sections.push(list);
        }

        sections.push(
            "# Tool use\nWork in larger batches to avoid extra round trips. Read a whole file (or a \
             wide `offset`/`limit`) once instead of re-reading the same path in small slices, and \
             issue several independent `read`, `grep`, `find`, or `ls` calls in the same step. Use \
             `grep` to locate a symbol or string, then read the surrounding lines. Only re-read a \
             file after you edit it. Prefer the dedicated tools over shell equivalents: `read` to \
             inspect a file, `grep` to find text, `find` to locate files, and `ls` to list a \
             directory. Keep each `bash` command focused on one task instead of chaining unrelated \
             commands with `;` or `&&`, and scope searches to the project or a specific directory \
             — never sweep the whole filesystem with `find /`. `read`, `ls`, `find`, and `grep` \
             accept absolute paths, so you do not need a shell to inspect files outside the project, \
             and `bash` already starts in the project root, so run a command directly instead of \
             prefixing `cd <root> &&`. When a command can print a large payload, select just the \
             fields you need (for example a `--jq`/`jq` filter) rather than piping it through `head`, \
             which still fetches and renders everything."
                .to_string(),
        );

        sections.push(
            "# Definition of Done\nA task is not done until you have verified the outcome, and \
             tool output is the only evidence — a command that failed, printed nothing, or was never \
             run is not proof. After each state-changing action, confirm the result before \
             reporting success, and re-read what you changed. This applies to every kind of work, \
             not just code: an edit is on disk and the build/tests/lint pass; a pull request's CI \
             and mergeability are checked (`gh pr checks <url>`, `gh pr view <url> --json \
             state,mergeable,mergeStateStatus,reviewDecision,statusCheckRollup`) and pending checks \
             are waited for; a comment, review, or ticket reply is read back to confirm it landed \
             in the right place; a release, deployment, migration, or published artifact is \
             queried for its status; and any task with a known verifier is run through it. Run a \
             verifier once and read its whole result — re-running the same build or test on \
             unchanged files is slow and adds nothing. \
             Separate what you verified (\"ran ./gradlew test — passed\", \"`grep` found the \
             annotation\") from what you assume, and say plainly when you could not confirm \
             something. Do not claim a change is applied, a fix works, a PR is ready, a reply is \
             posted, or a release is out without the output that shows it. A code change made in \
             response to a pull request or review is delivered only once it is committed and \
             pushed to the branch under review — fix and push the code before you reply in the \
             thread, and confirm the new commit before you summarize."
                .to_string(),
        );

        sections.push(
            "# Scope\nKeep every change scoped to what was asked. Before you commit, run \
             `git status` and `git diff --staged` and stage only the paths your work touched with \
             explicit `git add <path>` — never blanket-stage with `git add -A`, `git add --all`, \
             `git add .`, or `git commit -a`/`-am` in a tree that may hold unrelated edits. Leave \
             local-only and generated files out of commits and pull requests: \
             `.claude/settings.local.json`, `.idea/`, `.vscode/`, `.DS_Store`, editor state, build \
             output, and unrelated lockfile churn. Do not revert someone else's unrelated changes \
             without asking, but exclude them. Before you open or update a pull request, review its \
             file list (`git diff --name-only <base>...HEAD`, `gh pr diff --name-only`, or \
             `gh pr view <url> --json files`) and drop anything unrelated it picked up."
                .to_string(),
        );

        sections.push(
            "# GitHub and GitLab\nWork with GitHub and GitLab pull/merge requests and issues through \
             their CLIs (`gh` and `glab`) rather than `webfetch`: they authenticate, so private \
             repositories and review threads are reachable, and return structured data. Use \
             `gh pr view <url-or-number> --comments` / `glab mr view` to read an item, `gh pr diff` / \
             `glab mr diff` for its changes, `gh pr checks` for CI, and `gh pr comment` / \
             `glab mr note` to reply. Reserve `webfetch` for public pages that have no CLI \
             equivalent.\nReply to code review comments inside their existing threads instead of \
             posting one general comment: list them with `gh api repos/{owner}/{repo}/pulls/<n>/comments` \
             and answer one with \
             `gh api -X POST repos/{owner}/{repo}/pulls/<n>/comments/<comment_id>/replies -f body=<text>` \
             (`gh pr comment` is only for a new top-level comment). Select just the fields you need \
             and format API output for a person to read — one line per item, not minified JSON. Fix \
             and push the code before you reply: commit the change and `git push` it to the branch \
             under review first, so a reply never claims a comment is addressed while the branch \
             still has the old code.\nNever \
             commit to or push the repository's default branch. Determine it first with \
             `git symbolic-ref --short refs/remotes/origin/HEAD` or \
             `gh repo view --json defaultBranchRef --jq .defaultBranchRef.name` (fall back to \
             `main`/`master` only when neither is set): branch protection rejects a direct push \
             and you would have to undo the commit. When you are asked to raise a pull request \
             while on the default branch, create a feature branch first, commit there, push that \
             branch, and open the PR from it."
                .to_string(),
        );

        if self.compaction.enabled {
            sections.push(
                "# Context management\nOlder turns are summarized automatically as the context \
                 fills up; the most recent work is kept verbatim. Keep summaries of your own work \
                 close to the end of a task so nothing important is lost."
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
    fn system_prompt_enforces_scope() {
        let config = Config::default();
        let prompt = config.compose_system_prompt();
        assert!(prompt.contains("# Scope"), "{prompt}");
        assert!(prompt.contains("never blanket-stage"), "{prompt}");
        assert!(prompt.contains(".claude/settings.local.json"), "{prompt}");
    }

    #[test]
    fn system_prompt_defines_done() {
        let config = Config::default();
        let prompt = config.compose_system_prompt();
        assert!(prompt.contains("# Definition of Done"), "{prompt}");
        assert!(prompt.contains("not just code"), "{prompt}");
        assert!(prompt.contains("gh pr checks"), "{prompt}");
        assert!(prompt.contains("deployment"), "{prompt}");
    }

    #[test]
    fn system_prompt_encourages_batched_tool_use() {
        let config = Config::default();
        let prompt = config.compose_system_prompt();
        assert!(prompt.contains("# Tool use"));
        assert!(prompt.contains("batches"));
        assert!(
            prompt.contains("already starts in the project root"),
            "{prompt}"
        );
    }

    #[test]
    fn system_prompt_prefers_forge_clis() {
        let config = Config::default();
        let prompt = config.compose_system_prompt();
        assert!(prompt.contains("# GitHub and GitLab"), "{prompt}");
        assert!(prompt.contains("`gh` and `glab`"), "{prompt}");
        assert!(prompt.contains("inside their existing threads"), "{prompt}");
        assert!(prompt.contains("/replies"), "{prompt}");
    }

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
        let zai = ProviderPreset::for_name("zai").unwrap();
        assert_eq!(zai.kind, ProviderKind::OpenAi);
        assert_eq!(zai.base_url, "https://api.z.ai/api/paas/v4");
        assert_eq!(zai.model, "glm-5.3");
        assert_eq!(zai.key_env, "ZAI_API_KEY");
        assert_eq!(ProviderPreset::for_name("glm").unwrap().model, "glm-5.3");
        assert!(ProviderPreset::for_name("custom-endpoint").is_none());
    }

    #[test]
    fn zai_runtime_fields_and_fallback_catalog() {
        let mut config = Config {
            provider: "openai".into(),
            model: "gpt-4o-mini".into(),
            base_url: "https://api.openai.com/v1".into(),
            ..Config::default()
        };
        config.apply_provider("glm", "zai-key");
        assert_eq!(config.model, "glm-5.3");
        assert_eq!(config.base_url, "https://api.z.ai/api/paas/v4");
        assert!(config.is_zai());
        assert!(config.supports_reasoning());

        let catalog = config.model_catalog();
        assert!(catalog.contains(&"glm-5.3-flash".to_string()));
        assert!(catalog.contains(&"glm-5.3".to_string()));
        assert!(catalog.windows(2).all(|pair| pair[0] <= pair[1]));

        let explicit = Config {
            model: "glm-custom".into(),
            model_catalog: vec!["glm-custom".into()],
            ..config.clone()
        };
        assert_eq!(explicit.model_catalog(), vec!["glm-custom"]);
    }

    #[test]
    fn glm_forced_thinking_versions() {
        assert!(glm_forces_thinking("glm-5.3"));
        assert!(glm_forces_thinking("glm-5.3-flash"));
        assert!(glm_forces_thinking("GLM-5.4"));
        assert!(!glm_forces_thinking("glm-5.2"));
        assert!(!glm_forces_thinking("glm-4.6"));
        assert!(!glm_forces_thinking("glm-5v-turbo"));
        assert!(!glm_forces_thinking("gpt-5.4"));
    }

    #[test]
    fn custom_providers_pointed_at_zai_behave_like_zai() {
        let by_url = Config {
            provider: "my-endpoint".into(),
            base_url: "https://api.z.ai/api/paas/v4".into(),
            ..Config::default()
        };
        assert!(by_url.is_zai());
        assert!(!by_url.model_catalog().is_empty());

        let by_cn_url = Config {
            base_url: "https://open.bigmodel.cn/api/paas/v4".into(),
            ..by_url
        };
        assert!(by_cn_url.is_zai());

        let other = Config {
            base_url: "https://api.deepseek.com/v1".into(),
            ..Config::default()
        };
        assert!(!other.is_zai());
        assert!(other.model_catalog().is_empty());
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
            domains: vec!["docs.example.com".to_string()],
        });

        let prompt = config.compose_system_prompt();
        assert!(prompt.contains("Route requests to the matching MCP server"));
        assert!(prompt.contains("call mcp_load for that"));
        assert!(prompt.contains("instead of webfetch"));
        assert!(prompt.contains("documents — docs.example.com"));
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
    fn apply_provider_remembers_a_model_per_provider() {
        let mut config = Config::default();
        config.apply_provider("deepseek", "sk-deepseek");
        assert_eq!(config.model, "deepseek-chat");

        config.model = "deepseek-v4-pro".to_string();
        config.apply_provider("anthropic", "sk-ant");
        assert_eq!(config.model, "claude-3-5-sonnet-latest");
        assert_eq!(config.provider, "anthropic");

        // Switching back restores the model chosen for DeepSeek instead of
        // leaving the Anthropic model id behind.
        config.apply_provider("deepseek", "sk-deepseek");
        assert_eq!(config.model, "deepseek-v4-pro");
        assert_eq!(config.base_url, "https://api.deepseek.com/v1");
        assert_eq!(config.model_for_provider("openai"), "gpt-4o-mini");
    }

    #[test]
    fn for_provider_points_a_copy_at_another_provider() {
        let config = Config {
            provider: "portkey".into(),
            model: "claude-sonnet-5".into(),
            model_catalog: vec!["catalog-model".into()],
            base_url: "https://api.portkey.ai/v1".into(),
            ..Config::default()
        };

        let deepseek = config.for_provider("deepseek", "sk-deepseek");
        assert_eq!(deepseek.provider, "deepseek");
        assert_eq!(deepseek.api_key, "sk-deepseek");
        assert_eq!(deepseek.model, "deepseek-chat");
        assert_eq!(deepseek.base_url, "https://api.deepseek.com/v1");
        assert!(deepseek.model_catalog.is_empty());

        // The active config is untouched.
        assert_eq!(config.provider, "portkey");
        assert_eq!(config.model, "claude-sonnet-5");
        assert_eq!(config.model_catalog, vec!["catalog-model"]);

        let portkey = config.for_provider("portkey", "pk-test");
        assert_eq!(portkey.model_catalog, vec!["catalog-model"]);
    }

    #[test]
    fn persist_selection_records_the_provider_and_its_models() {
        let dir = temp_dir("persist-selection");
        let path = dir.join("config.json");
        std::fs::write(&path, r#"{"auto_approve":false}"#).unwrap();

        let mut config = Config::default();
        config.apply_provider("anthropic", "sk-ant");
        config.model = "claude-opus-5".to_string();
        config.persist_selection_at(&path).unwrap();

        let stored: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(stored["provider"], "anthropic");
        assert_eq!(stored["model"], "claude-opus-5");
        assert_eq!(stored["provider_models"]["openai"], "gpt-4o-mini");
        assert_eq!(stored["auto_approve"], false);

        let reloaded: Config = serde_json::from_value(stored).unwrap();
        assert_eq!(reloaded.provider_models["openai"], "gpt-4o-mini");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn apply_provider_remembers_a_custom_endpoint_per_provider() {
        let mut config = Config {
            provider: "portkey".into(),
            model: "gpt-5.6-sol".into(),
            base_url: "https://gateway.example.com/v1".into(),
            portkey_config: "pc-example".into(),
            ..Config::default()
        };

        config.apply_provider("deepseek", "sk-deepseek");
        assert_eq!(config.base_url, "https://api.deepseek.com/v1");
        assert_eq!(
            config.provider_base_urls["portkey"],
            "https://gateway.example.com/v1"
        );

        config.apply_provider("portkey", "pk-test");
        assert_eq!(config.base_url, "https://gateway.example.com/v1");
        assert_eq!(config.portkey_config, "pc-example");
        assert!(!config.provider_base_urls.contains_key("deepseek"));
    }

    #[test]
    fn apply_provider_restores_a_custom_provider_endpoint() {
        let mut config = Config {
            provider: "my-endpoint".into(),
            model: "custom-model".into(),
            base_url: "https://custom.example/v1".into(),
            ..Config::default()
        };

        config.apply_provider("deepseek", "sk-deepseek");
        assert_eq!(config.base_url, "https://api.deepseek.com/v1");
        assert_eq!(
            config.provider_base_urls["my-endpoint"],
            "https://custom.example/v1"
        );

        config.apply_provider("my-endpoint", "sk-custom");
        assert_eq!(config.base_url, "https://custom.example/v1");
    }

    #[test]
    fn login_options_apply_model_endpoint_and_portkey_config() {
        let mut config = Config::default();
        config.apply_provider("portkey", "pk-test");
        config.apply_login_options(
            "gpt-5.6-sol",
            "https://gateway.example.com/v1/",
            "pc-example",
        );

        assert_eq!(config.model, "gpt-5.6-sol");
        assert_eq!(config.model_for_provider("portkey"), "gpt-5.6-sol");
        assert_eq!(config.base_url, "https://gateway.example.com/v1");
        assert_eq!(
            config.base_url_for_provider("portkey"),
            "https://gateway.example.com/v1"
        );
        assert_eq!(config.portkey_config, "pc-example");

        // Blank values keep the current settings.
        config.apply_login_options("", "", "");
        assert_eq!(config.model, "gpt-5.6-sol");
        assert_eq!(config.base_url, "https://gateway.example.com/v1");
        assert_eq!(config.portkey_config, "pc-example");

        // The Config ID is ignored for other providers, and a preset endpoint
        // is not stored as a custom one.
        config.apply_provider("openai", "sk-openai");
        config.apply_login_options("gpt-4o", "https://api.openai.com/v1", "pc-ignored");
        assert_eq!(config.model, "gpt-4o");
        assert_eq!(config.portkey_config, "pc-example");
        assert!(!config.provider_base_urls.contains_key("openai"));
        assert_eq!(
            config.provider_base_urls["portkey"],
            "https://gateway.example.com/v1"
        );
    }

    #[test]
    fn persist_selection_rewrites_the_active_endpoint() {
        let dir = temp_dir("persist-endpoint");
        let path = dir.join("config.json");
        std::fs::write(
            &path,
            r#"{"provider":"portkey","base_url":"https://old.example/v1"}"#,
        )
        .unwrap();

        let mut config = Config {
            provider: "deepseek".into(),
            model: "deepseek-chat".into(),
            base_url: "https://api.deepseek.com/v1".into(),
            provider_base_urls: [(
                "portkey".to_string(),
                "https://gateway.example.com/v1".to_string(),
            )]
            .into_iter()
            .collect(),
            ..Config::default()
        };
        config.persist_selection_at(&path).unwrap();

        let stored: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(stored["provider"], "deepseek");
        assert!(stored.get("base_url").is_none());
        assert_eq!(
            stored["provider_base_urls"]["portkey"],
            "https://gateway.example.com/v1"
        );

        config.apply_provider("portkey", "pk-test");
        config.persist_selection_at(&path).unwrap();
        let stored: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(stored["base_url"], "https://gateway.example.com/v1");

        std::fs::remove_dir_all(&dir).ok();
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
        assert_eq!(model_label("deepseek-v4-pro"), "DeepSeek V4 Pro");
        assert_eq!(model_label("glm-5.3-flash"), "GLM-5.3 Flash");
        assert_eq!(model_label("custom-model"), "custom-model");
    }

    #[test]
    fn deepseek_uses_only_models_reported_by_the_provider() {
        let config = Config {
            provider: "deepseek".into(),
            model: "deepseek-v4-flash".into(),
            ..Config::default()
        };
        let merged =
            config.merge_model_catalog(vec!["deepseek-v4-pro".into(), "deepseek-flash".into()]);
        assert_eq!(merged, vec!["deepseek-flash", "deepseek-v4-pro"]);
    }

    #[test]
    fn merge_model_catalog_adds_active_model_for_unknown_providers() {
        let config = Config {
            provider: "my-endpoint".into(),
            model: "custom-model".into(),
            ..Config::default()
        };
        assert_eq!(
            config.merge_model_catalog(vec!["custom-model".into()]),
            vec!["custom-model"]
        );
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
    fn fallback_uses_the_model_remembered_for_the_stored_provider() {
        let mut config = Config {
            provider_models: [("deepseek".to_string(), "deepseek-v4-pro".to_string())]
                .into_iter()
                .collect(),
            ..Config::default()
        };
        let mut store = AuthStore::default();
        store.set("deepseek", "sk-deepseek");

        apply_stored_provider_fallback(&mut config, &store, &None);

        assert_eq!(config.provider, "deepseek");
        assert_eq!(config.model, "deepseek-v4-pro");
    }

    #[test]
    fn fallback_adopts_a_remembered_endpoint() {
        let mut config = Config {
            provider_base_urls: [(
                "deepseek".to_string(),
                "https://gateway.example/v1".to_string(),
            )]
            .into_iter()
            .collect(),
            ..Config::default()
        };
        let mut store = AuthStore::default();
        store.set("deepseek", "sk-deepseek");

        apply_stored_provider_fallback(&mut config, &store, &None);

        assert_eq!(config.provider, "deepseek");
        assert_eq!(config.base_url, "https://gateway.example/v1");
    }

    #[test]
    fn fallback_applies_a_custom_providers_remembered_endpoint() {
        let mut config = Config {
            provider_base_urls: [(
                "my-endpoint".to_string(),
                "https://custom.example/v1".to_string(),
            )]
            .into_iter()
            .collect(),
            ..Config::default()
        };
        let mut store = AuthStore::default();
        store.set("my-endpoint", "sk-custom");

        apply_stored_provider_fallback(&mut config, &store, &None);

        assert_eq!(config.provider, "my-endpoint");
        assert_eq!(config.base_url, "https://custom.example/v1");
    }

    #[test]
    fn base_url_for_provider_keeps_the_active_url() {
        let config = Config {
            provider: "portkey".into(),
            base_url: "https://gateway.example.com/v1".into(),
            ..Config::default()
        };

        assert_eq!(
            config.base_url_for_provider("portkey"),
            "https://gateway.example.com/v1"
        );
        assert_eq!(
            config.base_url_for_provider("deepseek"),
            "https://api.deepseek.com/v1"
        );
        assert_eq!(
            config.base_url_for_provider("my-endpoint"),
            "https://gateway.example.com/v1",
            "an unknown provider falls back to the active URL"
        );
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
    fn set_default_model_writes_and_preserves_settings() {
        let dir = temp_dir("default-model");
        let path = dir.join("config.json");
        std::fs::write(&path, r#"{"provider":"deepseek","mode":"plan"}"#).unwrap();

        Config::set_default_model_at(&path, "deepseek-reasoner").unwrap();

        let value: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(value["default_model"], "deepseek-reasoner");
        assert_eq!(value["provider"], "deepseek");
        assert_eq!(value["mode"], "plan");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn legacy_lowercase_config_dir_is_migrated() {
        fn temp(tag: &str) -> PathBuf {
            let dir = std::env::temp_dir()
                .join(format!("oxide_config_migrate_{tag}_{}", std::process::id()));
            std::fs::remove_dir_all(&dir).ok();
            std::fs::create_dir_all(&dir).unwrap();
            dir
        }

        let base = temp("move");
        std::fs::create_dir_all(base.join("oxide")).unwrap();
        std::fs::write(base.join("oxide/config.json"), "{}").unwrap();
        migrate_legacy_config_dir(&base, &base.join(CONFIG_DIR_NAME));
        assert!(has_dir_entry(&base, CONFIG_DIR_NAME));
        assert!(!has_dir_entry(&base, "oxide"));
        assert!(base.join(CONFIG_DIR_NAME).join("config.json").is_file());
        std::fs::remove_dir_all(&base).ok();

        // An already-branded directory wins and the legacy entry is untouched.
        let base = temp("keep");
        std::fs::create_dir_all(base.join(CONFIG_DIR_NAME)).unwrap();
        migrate_legacy_config_dir(&base, &base.join(CONFIG_DIR_NAME));
        assert!(has_dir_entry(&base, CONFIG_DIR_NAME));
        std::fs::remove_dir_all(&base).ok();
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
