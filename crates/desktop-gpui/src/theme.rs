//! The window's colors.
//!
//! Resolved from the same files as every other front-end — the built-in `dark`
//! and `light` palettes and a project's `.oxide/themes/<name>.json` — through
//! [`oxide_core::theme_view`], so the GPUI window, the terminal and the Electron
//! window are dressed the same way.

use gpui::{rgb, Rgba};
use std::collections::BTreeMap;
use std::path::Path;

pub struct Theme {
    pub name: String,
    colors: BTreeMap<String, String>,
}

impl Theme {
    /// Loads a theme by name for a project. With no project open there is no
    /// `.oxide/themes` to read, so the built-in palette is used.
    pub fn load(cwd: Option<&Path>, name: &str) -> Self {
        let colors = match cwd {
            Some(cwd) => oxide_core::theme_view::load(cwd, name),
            None => oxide_core::theme_view::builtin(name),
        };
        Self {
            name: colors.name,
            colors: colors.colors,
        }
    }

    /// The theme `config.json` selects, which is the choice `/theme` in the
    /// terminal and the Electron window's picker write.
    pub fn configured_name() -> String {
        let Ok(text) = std::fs::read_to_string(oxide_core::config::Config::config_path()) else {
            return "dark".to_string();
        };
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
            return "dark".to_string();
        };
        value["theme"]
            .as_str()
            .filter(|name| !name.is_empty())
            .unwrap_or("dark")
            .to_string()
    }

    /// The palette a toggle switches to: the two built-ins are the pair that
    /// every front-end ships, so a custom theme toggles back to `dark`.
    pub fn toggled_name(&self) -> &'static str {
        if self.name == "light" {
            "dark"
        } else {
            "light"
        }
    }

    pub fn slot(&self, slot: &str) -> Rgba {
        parse_hex(self.colors.get(slot).map(String::as_str)).unwrap_or_else(|| rgb(0xffffff))
    }

    pub fn background(&self) -> Rgba {
        self.slot("background")
    }

    pub fn sidebar(&self) -> Rgba {
        self.slot("sidebar")
    }

    pub fn panel(&self) -> Rgba {
        self.slot("panel")
    }

    pub fn panel_2(&self) -> Rgba {
        self.slot("panel_2")
    }

    pub fn panel_3(&self) -> Rgba {
        self.slot("panel_3")
    }

    pub fn border(&self) -> Rgba {
        self.slot("border")
    }

    pub fn text(&self) -> Rgba {
        self.slot("text")
    }

    pub fn dim(&self) -> Rgba {
        self.slot("dim")
    }

    pub fn faint(&self) -> Rgba {
        self.slot("faint")
    }

    pub fn accent(&self) -> Rgba {
        self.slot("accent")
    }

    pub fn user(&self) -> Rgba {
        self.slot("user")
    }

    pub fn assistant(&self) -> Rgba {
        self.slot("assistant")
    }

    pub fn success(&self) -> Rgba {
        self.slot("success")
    }

    pub fn tool(&self) -> Rgba {
        self.slot("tool")
    }

    pub fn error(&self) -> Rgba {
        self.slot("error")
    }

    pub fn tool_pending_bg(&self) -> Rgba {
        self.slot("tool_pending_bg")
    }

    pub fn tool_success_bg(&self) -> Rgba {
        self.slot("tool_success_bg")
    }

    pub fn tool_error_bg(&self) -> Rgba {
        self.slot("tool_error_bg")
    }

    pub fn thinking_text(&self) -> Rgba {
        self.slot("thinking_text")
    }
}

fn parse_hex(value: Option<&str>) -> Option<Rgba> {
    let value = value?.trim();
    let hex = value.strip_prefix('#').unwrap_or(value);
    if hex.len() != 6 {
        return None;
    }
    let value = u32::from_str_radix(hex, 16).ok()?;
    Some(rgb(value))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_theme_resolves_the_shared_slots() {
        let theme = Theme::load(None, "dark");
        assert_eq!(theme.name, "dark");
        assert_eq!(theme.background(), rgb(0x1c1c1c));
        assert_eq!(theme.text(), rgb(0xededed));
        assert_eq!(theme.slot("no-such-slot"), rgb(0xffffff));
    }

    #[test]
    fn the_light_palette_is_the_other_built_in() {
        let theme = Theme::load(None, "light");
        assert_eq!(theme.background(), rgb(0xffffff));
        assert_eq!(theme.toggled_name(), "dark");
        assert_eq!(Theme::load(None, "dark").toggled_name(), "light");
    }

    #[test]
    fn a_custom_name_with_no_project_falls_back_to_a_built_in() {
        let theme = Theme::load(None, "no-such-theme");
        assert_eq!(theme.background(), rgb(0x1c1c1c));
    }
}
