//! Model pricing for the footer's `$cost` segment.
//!
//! Pi reads pricing from its model catalog; oxide ships a small default table
//! for common models and lets `settings.json` override or extend it via a
//! `modelPrices` map (USD per million tokens). Unknown models cost 0, and the
//! footer omits the segment just as Pi does when a cost is 0.

use crate::llm::Usage;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// USD per million tokens.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ModelPrice {
    pub input: f64,
    pub output: f64,
    pub cache_read: f64,
    pub cache_write: f64,
}

impl ModelPrice {
    pub fn cost(&self, usage: &Usage) -> f64 {
        (usage.input as f64 * self.input
            + usage.output as f64 * self.output
            + usage.cache_read as f64 * self.cache_read
            + usage.cache_write as f64 * self.cache_write)
            / 1_000_000.0
    }
}

fn price(input: f64, output: f64, cache_read: f64, cache_write: f64) -> ModelPrice {
    ModelPrice {
        input,
        output,
        cache_read,
        cache_write,
    }
}

/// A small built-in table for widely used models, in USD per million tokens.
pub fn defaults() -> BTreeMap<String, ModelPrice> {
    let mut prices = BTreeMap::new();
    for (model, value) in [
        ("gpt-4o", price(2.5, 10.0, 1.25, 0.0)),
        ("gpt-4o-mini", price(0.15, 0.6, 0.075, 0.0)),
        ("gpt-4.1", price(2.0, 8.0, 0.5, 0.0)),
        ("gpt-4.1-mini", price(0.4, 1.6, 0.1, 0.0)),
        ("o1", price(15.0, 60.0, 7.5, 0.0)),
        ("o3", price(2.0, 8.0, 0.5, 0.0)),
        ("o4-mini", price(1.1, 4.4, 0.275, 0.0)),
        ("claude-opus-4", price(15.0, 75.0, 1.5, 18.75)),
        ("claude-sonnet-4", price(3.0, 15.0, 0.3, 3.75)),
        ("claude-3-5-sonnet", price(3.0, 15.0, 0.3, 3.75)),
        ("claude-3-5-haiku", price(0.8, 4.0, 0.08, 1.0)),
        ("deepseek-chat", price(0.27, 1.1, 0.07, 0.0)),
        ("deepseek-reasoner", price(0.55, 2.19, 0.14, 0.0)),
        ("glm-5.3-flashx", price(0.37, 1.25, 0.075, 0.0)),
        ("glm-5.3-flash", price(0.15, 0.5, 0.03, 0.0)),
        ("glm-5.3", price(1.4, 4.4, 0.26, 0.0)),
        ("glm-5.2", price(1.4, 4.4, 0.26, 0.0)),
        ("glm-5.1", price(1.4, 4.4, 0.26, 0.0)),
        ("glm-5", price(1.0, 3.2, 0.2, 0.0)),
        ("glm-4.7-flashx", price(0.07, 0.4, 0.01, 0.0)),
        ("glm-4.7", price(0.6, 2.2, 0.11, 0.0)),
        ("glm-4.6", price(0.6, 2.2, 0.11, 0.0)),
        ("glm-4.5-air", price(0.2, 1.1, 0.03, 0.0)),
    ] {
        prices.insert(model.to_string(), value);
    }
    prices
}

/// Loads prices from the built-in table overlaid with `modelPrices` entries in
/// the global `settings.json` and the project `.oxide/settings.json`.
pub fn load(cwd: &Path) -> BTreeMap<String, ModelPrice> {
    let mut prices = defaults();
    for path in config_paths(cwd) {
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(value) = serde_json::from_str::<Value>(&text) else {
            continue;
        };
        let Some(map) = value.get("modelPrices").and_then(Value::as_object) else {
            continue;
        };
        for (model, entry) in map {
            if let Ok(price) = serde_json::from_value::<ModelPrice>(entry.clone()) {
                prices.insert(model.clone(), price);
            }
        }
    }
    prices
}

fn config_paths(cwd: &Path) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Some(dir) = crate::config::config_dir() {
        paths.push(dir.join("settings.json"));
    }
    if let Some(root) = crate::ecosystem::project_root(cwd) {
        paths.push(root.join(".oxide").join("settings.json"));
    }
    paths
}

/// Matches a model id against the price table by exact key or by the longest
/// prefix, so versioned ids like `claude-sonnet-4-6` inherit family pricing.
pub fn lookup<'a>(prices: &'a BTreeMap<String, ModelPrice>, model: &str) -> Option<&'a ModelPrice> {
    if let Some(price) = prices.get(model) {
        return Some(price);
    }
    prices
        .iter()
        .filter(|(key, _)| model.starts_with(key.as_str()))
        .max_by_key(|(key, _)| key.len())
        .map(|(_, price)| price)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_cover_known_models() {
        let prices = defaults();
        assert!(prices.contains_key("gpt-4o"));
        assert!(prices.contains_key("deepseek-chat"));
        assert!(prices.contains_key("glm-5.3"));
    }

    #[test]
    fn glm_flash_models_are_not_priced_as_the_flagship() {
        let prices = defaults();
        assert_eq!(lookup(&prices, "glm-5.3").unwrap().input, 1.4);
        assert_eq!(lookup(&prices, "glm-5.3-flash").unwrap().input, 0.15);
        assert_eq!(lookup(&prices, "glm-4.7-flashx").unwrap().input, 0.07);
    }

    #[test]
    fn cost_is_per_million_tokens() {
        let prices = defaults();
        let usage = Usage {
            input: 1_000_000,
            output: 0,
            cache_read: 0,
            cache_write: 0,
            reasoning: 0,
            cost: 0.0,
        };
        let price = lookup(&prices, "gpt-4o").unwrap();
        assert!((price.cost(&usage) - 2.5).abs() < 1e-9);
    }

    #[test]
    fn lookup_matches_family_prefix() {
        let prices = defaults();
        let price = lookup(&prices, "claude-sonnet-4-6").unwrap();
        assert_eq!(price.input, 3.0);
    }

    #[test]
    fn unknown_models_have_no_price() {
        let prices = defaults();
        assert!(lookup(&prices, "mystery-model").is_none());
    }
}
