use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io::{self, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

const AUTH_FILE: &str = "auth.json";
const KNOWN_PROVIDERS: [&str; 3] = ["openai", "deepseek", "anthropic"];

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
        let path = Self::path();
        if !path.exists() {
            return Ok(Self::default());
        }
        let text = std::fs::read_to_string(&path)
            .with_context(|| format!("reading credentials at {}", path.display()))?;
        serde_json::from_str(&text)
            .with_context(|| format!("parsing credentials at {}", path.display()))
    }

    pub fn save(&self) -> Result<()> {
        let path = Self::path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
        }
        let text = serde_json::to_string_pretty(self)?;
        std::fs::write(&path, text)
            .with_context(|| format!("writing credentials to {}", path.display()))?;
        restrict_permissions(&path)?;
        Ok(())
    }

    pub fn key(&self, provider: &str) -> Option<&str> {
        self.entries
            .get(&canonical_provider(provider))
            .map(|entry| entry.key.as_str())
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

pub fn login(provider: Option<String>, key: Option<String>) -> Result<()> {
    let mut store = AuthStore::load()?;
    let provider = match provider {
        Some(provider) => provider,
        None => prompt_provider()?,
    };
    let name = canonical_provider(&provider);
    let key = match key {
        Some(key) => key,
        None => read_secret(&format!("Enter API key for {name}: "))?,
    };
    let key = key.trim().to_string();
    if key.is_empty() {
        anyhow::bail!("no API key provided");
    }
    store.set(&name, &key);
    store.save()?;
    println!(
        "stored credentials for {name} in {}",
        AuthStore::path().display()
    );
    Ok(())
}

pub fn list() -> Result<()> {
    let store = AuthStore::load()?;
    if store.entries.is_empty() {
        println!("no credentials stored ({})", AuthStore::path().display());
        return Ok(());
    }
    println!("stored credentials:");
    for (name, entry) in &store.entries {
        println!("  {name} ({}) key {}", entry.kind, mask(&entry.key));
    }
    Ok(())
}

pub fn logout(provider: Option<String>) -> Result<()> {
    let mut store = AuthStore::load()?;
    let provider = match provider {
        Some(provider) => provider,
        None => prompt_provider()?,
    };
    let name = canonical_provider(&provider);
    if store.remove(&name) {
        store.save()?;
        println!("removed credentials for {name}");
    } else {
        println!("no credentials stored for {name}");
    }
    Ok(())
}

fn canonical_provider(name: &str) -> String {
    match name.trim().to_ascii_lowercase().as_str() {
        "gpt" | "gpt-4" | "gpt-4o" => "openai".to_string(),
        other => other.to_string(),
    }
}

fn prompt_provider() -> Result<String> {
    println!("select a provider:");
    for (index, name) in KNOWN_PROVIDERS.iter().enumerate() {
        println!("  {}. {name}", index + 1);
    }
    print!("provider name or number: ");
    io::stdout().flush()?;
    let mut line = String::new();
    io::stdin()
        .read_line(&mut line)
        .context("reading provider")?;
    let line = line.trim();
    if let Ok(index) = line.parse::<usize>() {
        if (1..=KNOWN_PROVIDERS.len()).contains(&index) {
            return Ok(KNOWN_PROVIDERS[index - 1].to_string());
        }
    }
    if line.is_empty() {
        anyhow::bail!("no provider provided");
    }
    Ok(line.to_string())
}

fn read_secret(prompt: &str) -> Result<String> {
    print!("{prompt}");
    io::stdout().flush()?;
    let hidden = io::stdin().is_terminal();
    if hidden {
        let _ = Command::new("stty").arg("-echo").status();
    }
    let mut line = String::new();
    let result = io::stdin().read_line(&mut line);
    if hidden {
        let _ = Command::new("stty").arg("echo").status();
        println!();
    }
    result.context("reading API key")?;
    Ok(line.trim().to_string())
}

fn mask(key: &str) -> String {
    let chars: Vec<char> = key.chars().collect();
    if chars.len() <= 8 {
        "****".to_string()
    } else {
        let tail: String = chars[chars.len() - 4..].iter().collect();
        format!("****{tail}")
    }
}

#[cfg(unix)]
fn restrict_permissions(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
        .with_context(|| format!("restricting permissions on {}", path.display()))
}

#[cfg(not(unix))]
fn restrict_permissions(_path: &Path) -> Result<()> {
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

    #[test]
    fn masks_keys() {
        assert_eq!(mask("short"), "****");
        assert_eq!(mask("sk-abcdefghijkl"), "****ijkl");
    }
}
