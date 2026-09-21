use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

const AUTH_FILE: &str = "auth.json";
#[derive(Debug, Clone, Copy)]
pub(crate) struct ProviderOption {
    pub name: &'static str,
    pub label: &'static str,
    pub description: &'static str,
    pub key_url: &'static str,
}

pub(crate) const KNOWN_PROVIDERS: [ProviderOption; 5] = [
    ProviderOption {
        name: "openai",
        label: "OpenAI",
        description: "GPT models",
        key_url: "https://platform.openai.com/api-keys",
    },
    ProviderOption {
        name: "deepseek",
        label: "DeepSeek",
        description: "DeepSeek chat and reasoning models",
        key_url: "https://platform.deepseek.com/api_keys",
    },
    ProviderOption {
        name: "anthropic",
        label: "Anthropic",
        description: "Claude models",
        key_url: "https://console.anthropic.com/settings/keys",
    },
    ProviderOption {
        name: "portkey",
        label: "Portkey",
        description: "AI gateway and model routing",
        key_url: "https://app.portkey.ai/api-keys",
    },
    ProviderOption {
        name: "zai",
        label: "Z.AI",
        description: "GLM models",
        key_url: "https://z.ai/manage-apikey/apikey-list",
    },
];

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthEntry {
    #[serde(rename = "type", default = "default_type")]
    pub kind: String,
    pub key: String,
}

fn default_type() -> String {
    "api".to_string()
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(transparent)]
pub struct AuthStore {
    pub entries: BTreeMap<String, AuthEntry>,
}

impl AuthStore {
    pub fn path() -> PathBuf {
        dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("oxide")
            .join(AUTH_FILE)
    }

    pub fn load() -> Result<Self> {
        Self::load_from(&Self::path())
    }

    pub fn load_from(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("reading credentials at {}", path.display()))?;
        serde_json::from_str(&text)
            .with_context(|| format!("parsing credentials at {}", path.display()))
    }

    pub fn save(&self) -> Result<()> {
        self.save_to(&Self::path())
    }

    pub fn save_to(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
        }
        let text = serde_json::to_string_pretty(self)?;
        std::fs::write(path, text)
            .with_context(|| format!("writing credentials to {}", path.display()))?;
        restrict_permissions(path)?;
        Ok(())
    }

    pub fn key(&self, provider: &str) -> Option<&str> {
        self.entries
            .get(&canonical_provider(provider))
            .map(|entry| entry.key.as_str())
    }

    /// Every provider with a stored credential, sorted by name.
    pub fn providers(&self) -> Vec<String> {
        self.entries.keys().cloned().collect()
    }

    pub fn set(&mut self, provider: &str, key: &str) {
        self.entries.insert(
            canonical_provider(provider),
            AuthEntry {
                kind: default_type(),
                key: key.to_string(),
            },
        );
    }

    pub fn remove(&mut self, provider: &str) -> bool {
        self.entries.remove(&canonical_provider(provider)).is_some()
    }
}

pub fn connect(provider: &str, key: &str) -> Result<String> {
    connect_with(
        &AuthStore::path(),
        &crate::config::Config::config_path(),
        provider,
        key,
    )
}

fn connect_with(auth_path: &Path, config_path: &Path, provider: &str, key: &str) -> Result<String> {
    let name = canonical_provider(provider);
    let mut store = AuthStore::load_from(auth_path)?;
    store.set(&name, key);
    store.save_to(auth_path)?;
    crate::config::Config::set_active_provider_at(config_path, &name)?;
    Ok(name)
}

/// The providers with a stored credential, or an empty list when `auth.json`
/// is missing or unreadable.
pub fn stored_providers() -> Vec<String> {
    AuthStore::load()
        .map(|store| store.providers())
        .unwrap_or_default()
}

/// Switches to a provider that already has a stored credential, so logging in
/// to a second provider never asks for the key again. Returns the canonical
/// provider name and its key.
pub fn select_stored(provider: &str) -> Result<(String, String)> {
    select_stored_with(
        &AuthStore::path(),
        &crate::config::Config::config_path(),
        provider,
    )
}

fn select_stored_with(
    auth_path: &Path,
    config_path: &Path,
    provider: &str,
) -> Result<(String, String)> {
    let name = canonical_provider(provider);
    let store = AuthStore::load_from(auth_path)?;
    let Some(key) = store.key(&name).map(str::to_string) else {
        anyhow::bail!("no stored credentials for `{name}` — run `/login {name}` to add one");
    };
    crate::config::Config::set_active_provider_at(config_path, &name)?;
    Ok((name, key))
}

pub fn canonical_provider(name: &str) -> String {
    match name.trim().to_ascii_lowercase().as_str() {
        "gpt" | "gpt-4" | "gpt-4o" => "openai".to_string(),
        "port-key" => "portkey".to_string(),
        "glm" | "z.ai" | "z-ai" | "zhipu" | "bigmodel" => "zai".to_string(),
        other => other.to_string(),
    }
}

pub(crate) fn provider_option(name: &str) -> Option<&'static ProviderOption> {
    let name = canonical_provider(name);
    KNOWN_PROVIDERS.iter().find(|option| option.name == name)
}

pub(crate) fn provider_label(name: &str) -> &str {
    provider_option(name)
        .map(|option| option.label)
        .unwrap_or(name)
}

#[cfg(unix)]
pub(crate) fn restrict_permissions(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
        .with_context(|| format!("restricting permissions on {}", path.display()))
}

#[cfg(not(unix))]
pub(crate) fn restrict_permissions(_path: &Path) -> Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonicalizes_provider_aliases() {
        assert_eq!(canonical_provider("gpt"), "openai");
        assert_eq!(canonical_provider("GPT-4o"), "openai");
        assert_eq!(canonical_provider("Anthropic"), "anthropic");
        assert_eq!(canonical_provider(" deepseek "), "deepseek");
        assert_eq!(canonical_provider("Port-Key"), "portkey");
        assert_eq!(canonical_provider("GLM"), "zai");
        assert_eq!(canonical_provider(" z.ai "), "zai");
        assert_eq!(canonical_provider("Zhipu"), "zai");
    }

    #[test]
    fn known_providers_include_zai() {
        let names: Vec<&str> = KNOWN_PROVIDERS.iter().map(|option| option.name).collect();
        assert_eq!(
            names,
            vec!["openai", "deepseek", "anthropic", "portkey", "zai"]
        );
        let zai = provider_option("glm").expect("glm resolves to the Z.AI preset");
        assert_eq!(zai.label, "Z.AI");
        assert!(zai.key_url.contains("z.ai"));
    }

    #[test]
    fn stores_several_providers_at_once() {
        let mut store = AuthStore::default();
        store.set("openai", "sk-openai");
        store.set("Anthropic", "sk-ant-test");
        store.set("deepseek", "sk-deepseek");

        assert_eq!(store.providers(), vec!["anthropic", "deepseek", "openai"]);
        assert_eq!(store.key("openai"), Some("sk-openai"));
        assert_eq!(store.key("anthropic"), Some("sk-ant-test"));
        assert_eq!(store.key("deepseek"), Some("sk-deepseek"));
    }

    #[test]
    fn select_stored_reuses_a_saved_key() {
        let dir = temp_dir("select");
        let auth_path = dir.join("auth.json");
        let config_path = dir.join("config.json");

        connect_with(&auth_path, &config_path, "openai", "sk-openai").unwrap();
        connect_with(&auth_path, &config_path, "DeepSeek", "sk-deepseek").unwrap();

        let (name, key) = select_stored_with(&auth_path, &config_path, "OPENAI").unwrap();
        assert_eq!(name, "openai");
        assert_eq!(key, "sk-openai");

        let config: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&config_path).unwrap()).unwrap();
        assert_eq!(config["provider"], "openai");

        let error = select_stored_with(&auth_path, &config_path, "portkey").unwrap_err();
        assert!(error
            .to_string()
            .contains("no stored credentials for `portkey`"));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn set_key_remove_round_trip() {
        let mut store = AuthStore::default();
        store.set("Anthropic", "sk-ant-test");
        assert_eq!(store.key("anthropic"), Some("sk-ant-test"));
        assert!(store.remove("anthropic"));
        assert_eq!(store.key("anthropic"), None);
        assert!(!store.remove("anthropic"));
    }

    #[test]
    fn serializes_as_provider_map() {
        let mut store = AuthStore::default();
        store.set("openai", "sk-test");
        let value: serde_json::Value = serde_json::to_value(&store).unwrap();
        assert_eq!(value["openai"]["type"], "api");
        assert_eq!(value["openai"]["key"], "sk-test");
    }

    fn temp_dir(tag: &str) -> PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let unique = format!(
            "oxide-auth-test-{tag}-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        );
        let dir = std::env::temp_dir().join(unique);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn connect_stores_key_and_sets_provider() {
        let dir = temp_dir("connect");
        let auth_path = dir.join("auth.json");
        let config_path = dir.join("config.json");

        let name = connect_with(&auth_path, &config_path, "DeepSeek", "sk-test").unwrap();
        assert_eq!(name, "deepseek");
        assert_eq!(
            AuthStore::load_from(&auth_path).unwrap().key("deepseek"),
            Some("sk-test")
        );

        let config: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&config_path).unwrap()).unwrap();
        assert_eq!(config["provider"], "deepseek");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn connect_preserves_existing_config_settings() {
        let dir = temp_dir("preserve");
        let auth_path = dir.join("auth.json");
        let config_path = dir.join("config.json");
        std::fs::write(&config_path, r#"{"auto_approve":false,"mode":"plan"}"#).unwrap();

        connect_with(&auth_path, &config_path, "anthropic", "sk-ant").unwrap();

        let config: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&config_path).unwrap()).unwrap();
        assert_eq!(config["provider"], "anthropic");
        assert_eq!(config["auto_approve"], false);
        assert_eq!(config["mode"], "plan");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn connect_recovers_from_invalid_config() {
        let dir = temp_dir("recover");
        let auth_path = dir.join("auth.json");
        let config_path = dir.join("config.json");
        std::fs::write(&config_path, "not json").unwrap();

        connect_with(&auth_path, &config_path, "openai", "sk-openai").unwrap();

        let config: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&config_path).unwrap()).unwrap();
        assert_eq!(config["provider"], "openai");

        std::fs::remove_dir_all(&dir).ok();
    }
}
