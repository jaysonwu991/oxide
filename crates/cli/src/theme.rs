//! Named color themes for the TUI. `dark` and `light` are built in; custom
//! themes are JSON files under `.oxide/themes/` (project) or the oxide config
//! dir (global), matching Pi's theme locations. The built-in palettes are built
//! from `oxide_core::theme_view`, so the CLI and desktop use identical colors.

use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use ratatui::style::Color;

/// Semantic colors used by the TUI. Every slot has a sensible default so a
/// theme file can override only what it needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Theme {
    pub name: String,
    pub accent: Color,
    pub user: Color,
    pub assistant: Color,
    pub success: Color,
    pub tool: Color,
    pub error: Color,
    pub info: Color,
    pub dim: Color,
    pub border: Color,
    pub tool_pending_bg: Color,
    pub tool_success_bg: Color,
    pub tool_error_bg: Color,
    pub usage_bar_bg: Color,
    pub usage_bar_fg: Color,
    pub usage_bar_label: Color,
    pub thinking_off: Color,
    pub thinking_low: Color,
    pub thinking_medium: Color,
    pub thinking_high: Color,
    /// Reasoning text and its `Thinking` label, Pi's `thinkingText`.
    pub thinking_text: Color,
}

impl Theme {
    /// Builds a theme from a slot map of `#rrggbb` values, ignoring the surface
    /// slots the terminal does not use. The built-in palettes come from
    /// `oxide_core::theme_view` so the CLI and desktop share identical colors.
    fn from_slots(name: &str, slots: &BTreeMap<String, String>) -> Self {
        let color =
            |slot: &str| parse_color(slots.get(slot).map(String::as_str)).unwrap_or(Color::Reset);
        Self {
            name: name.to_string(),
            accent: color("accent"),
            user: color("user"),
            assistant: color("assistant"),
            success: color("success"),
            tool: color("tool"),
            error: color("error"),
            info: color("info"),
            dim: color("dim"),
            border: color("border"),
            tool_pending_bg: color("tool_pending_bg"),
            tool_success_bg: color("tool_success_bg"),
            tool_error_bg: color("tool_error_bg"),
            usage_bar_bg: color("usage_bar_bg"),
            usage_bar_fg: color("usage_bar_fg"),
            usage_bar_label: color("usage_bar_label"),
            thinking_off: color("thinking_off"),
            thinking_low: color("thinking_low"),
            thinking_medium: color("thinking_medium"),
            thinking_high: color("thinking_high"),
            thinking_text: color("thinking_text"),
        }
    }

    pub fn dark() -> Self {
        Self::from_slots("dark", &oxide_core::theme_view::builtin("dark").colors)
    }

    pub fn light() -> Self {
        Self::from_slots("light", &oxide_core::theme_view::builtin("light").colors)
    }

    pub fn by_name(name: &str) -> Option<Self> {
        match name.trim().to_ascii_lowercase().as_str() {
            "" | "dark" => Some(Self::dark()),
            "light" => Some(Self::light()),
            _ => None,
        }
    }
}

impl Default for Theme {
    fn default() -> Self {
        Self::dark()
    }
}

/// The serialized form of a custom theme. Colors are names (`"cyan"`,
/// `"lightblue"`) or `#rrggbb`.
#[derive(Debug, Deserialize, Default)]
pub struct ThemeFile {
    pub accent: Option<String>,
    pub user: Option<String>,
    pub assistant: Option<String>,
    pub success: Option<String>,
    pub tool: Option<String>,
    pub error: Option<String>,
    pub info: Option<String>,
    pub dim: Option<String>,
    pub border: Option<String>,
    pub tool_pending_bg: Option<String>,
    pub tool_success_bg: Option<String>,
    pub tool_error_bg: Option<String>,
    pub usage_bar_bg: Option<String>,
    pub usage_bar_fg: Option<String>,
    pub usage_bar_label: Option<String>,
    pub thinking_off: Option<String>,
    pub thinking_low: Option<String>,
    pub thinking_medium: Option<String>,
    pub thinking_high: Option<String>,
    pub thinking_text: Option<String>,
}

impl ThemeFile {
    /// Applies the overrides on top of `base`.
    pub fn apply(&self, base: Theme) -> Theme {
        Theme {
            name: base.name.clone(),
            accent: parse_color(self.accent.as_deref()).unwrap_or(base.accent),
            user: parse_color(self.user.as_deref()).unwrap_or(base.user),
            assistant: parse_color(self.assistant.as_deref()).unwrap_or(base.assistant),
            success: parse_color(self.success.as_deref()).unwrap_or(base.success),
            tool: parse_color(self.tool.as_deref()).unwrap_or(base.tool),
            error: parse_color(self.error.as_deref()).unwrap_or(base.error),
            info: parse_color(self.info.as_deref()).unwrap_or(base.info),
            dim: parse_color(self.dim.as_deref()).unwrap_or(base.dim),
            border: parse_color(self.border.as_deref()).unwrap_or(base.border),
            tool_pending_bg: parse_color(self.tool_pending_bg.as_deref())
                .unwrap_or(base.tool_pending_bg),
            tool_success_bg: parse_color(self.tool_success_bg.as_deref())
                .unwrap_or(base.tool_success_bg),
            tool_error_bg: parse_color(self.tool_error_bg.as_deref()).unwrap_or(base.tool_error_bg),
            usage_bar_bg: parse_color(self.usage_bar_bg.as_deref()).unwrap_or(base.usage_bar_bg),
            usage_bar_fg: parse_color(self.usage_bar_fg.as_deref()).unwrap_or(base.usage_bar_fg),
            usage_bar_label: parse_color(self.usage_bar_label.as_deref())
                .unwrap_or(base.usage_bar_label),
            thinking_off: parse_color(self.thinking_off.as_deref()).unwrap_or(base.thinking_off),
            thinking_low: parse_color(self.thinking_low.as_deref()).unwrap_or(base.thinking_low),
            thinking_medium: parse_color(self.thinking_medium.as_deref())
                .unwrap_or(base.thinking_medium),
            thinking_high: parse_color(self.thinking_high.as_deref()).unwrap_or(base.thinking_high),
            thinking_text: parse_color(self.thinking_text.as_deref()).unwrap_or(base.thinking_text),
        }
    }
}

/// Resolves a theme by name: built-ins first, then `.oxide/themes/<name>.json`
/// (project), then `<config>/Oxide/themes/<name>.json` (global).
pub fn load(cwd: &Path, name: &str) -> Theme {
    let trimmed = name.trim();
    let mut theme = Theme::by_name(trimmed).unwrap_or_default();
    theme.name = if trimmed.is_empty() {
        theme.name.clone()
    } else {
        trimmed.to_string()
    };
    let candidates = [project_theme(cwd, trimmed), global_theme(trimmed)];
    for path in candidates.into_iter().flatten() {
        if let Ok(text) = std::fs::read_to_string(&path) {
            if let Ok(file) = serde_json::from_str::<ThemeFile>(&text) {
                theme = file.apply(theme);
            }
        }
    }
    theme
}

/// Names available for a picker: built-ins plus discovered theme files.
pub fn names(cwd: &Path) -> Vec<String> {
    let mut names = vec!["dark".to_string(), "light".to_string()];
    for dir in theme_dirs(cwd) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) == Some("json") {
                if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                    if !names.iter().any(|n| n == stem) {
                        names.push(stem.to_string());
                    }
                }
            }
        }
    }
    names
}

fn theme_dirs(cwd: &Path) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(root) = crate::ecosystem::project_root(cwd) {
        dirs.push(root.join(".oxide/themes"));
    }
    if let Some(config) = oxide_core::config::config_dir() {
        dirs.push(config.join("themes"));
    }
    dirs
}

fn project_theme(cwd: &Path, name: &str) -> Option<PathBuf> {
    let root = crate::ecosystem::project_root(cwd)?;
    let path = root.join(".oxide/themes").join(format!("{name}.json"));
    path.is_file().then_some(path)
}

fn global_theme(name: &str) -> Option<PathBuf> {
    let path = oxide_core::config::config_dir()?
        .join("themes")
        .join(format!("{name}.json"));
    path.is_file().then_some(path)
}

/// Parses a color name or `#rrggbb` value.
pub fn parse_color(value: Option<&str>) -> Option<Color> {
    let value = value?.trim();
    if let Some(hex) = value.strip_prefix('#') {
        if hex.len() == 6 {
            let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
            let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
            let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
            return Some(Color::Rgb(r, g, b));
        }
    }
    let normalized = value.to_ascii_lowercase().replace(['-', '_'], "");
    Some(match normalized.as_str() {
        "black" => Color::Black,
        "red" => Color::Red,
        "green" => Color::Green,
        "yellow" => Color::Yellow,
        "blue" => Color::Blue,
        "magenta" | "purple" => Color::Magenta,
        "cyan" => Color::Cyan,
        "gray" | "grey" => Color::Gray,
        "darkgray" | "darkgrey" => Color::DarkGray,
        "lightred" => Color::LightRed,
        "lightgreen" => Color::LightGreen,
        "lightyellow" => Color::LightYellow,
        "lightblue" => Color::LightBlue,
        "lightmagenta" | "lightpurple" => Color::LightMagenta,
        "lightcyan" => Color::LightCyan,
        "white" => Color::White,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_themes_resolve() {
        assert_eq!(Theme::by_name("dark").unwrap().name, "dark");
        assert_eq!(Theme::by_name("light").unwrap().name, "light");
        assert_eq!(Theme::by_name("").unwrap().name, "dark");
        assert!(Theme::by_name("missing").is_none());
    }

    #[test]
    fn builtin_palettes_match_theme_view() {
        // The CLI built-ins are built from the shared `theme_view` palettes, so
        // the CLI and desktop use identical dark/light colors.
        let dark = Theme::dark();
        assert_eq!(dark.accent, Color::Rgb(0xa9, 0xc7, 0xff));
        assert_eq!(dark.user, dark.accent);
        assert_eq!(dark.assistant, Color::Rgb(0xed, 0xed, 0xed));
        assert_eq!(dark.tool_pending_bg, Color::Rgb(0x23, 0x23, 0x23));
        let light = Theme::light();
        assert_eq!(light.accent, Color::Rgb(0x1a, 0x6f, 0xd4));
        assert_eq!(light.assistant, Color::Rgb(0x1b, 0x1b, 0x1f));
        assert_ne!(dark.accent, light.accent);
        // And they agree with the shared source of truth.
        let slots = oxide_core::theme_view::builtin("dark").colors;
        assert_eq!(slots.get("accent").map(String::as_str), Some("#a9c7ff"));
    }

    #[test]
    fn custom_theme_keeps_its_name_and_overrides() {
        let dir = std::env::temp_dir().join(format!("oxide_theme_{}", std::process::id()));
        std::fs::create_dir_all(dir.join(".git")).unwrap();
        std::fs::create_dir_all(dir.join(".oxide/themes")).unwrap();
        std::fs::write(
            dir.join(".oxide/themes/ocean.json"),
            r##"{"accent":"#5fd7ff","success":"green","tool":"cyan"}"##,
        )
        .unwrap();

        let theme = load(&dir, "ocean");
        assert_eq!(theme.name, "ocean");
        assert_eq!(theme.accent, Color::Rgb(0x5f, 0xd7, 0xff));
        assert_eq!(theme.success, Color::Green);
        assert_eq!(theme.tool, Color::Cyan);
        assert!(names(&dir).iter().any(|n| n == "ocean"));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn parses_named_and_hex_colors() {
        assert_eq!(parse_color(Some("cyan")), Some(Color::Cyan));
        assert_eq!(parse_color(Some("LightBlue")), Some(Color::LightBlue));
        assert_eq!(parse_color(Some("#ff8800")), Some(Color::Rgb(255, 136, 0)));
        assert_eq!(parse_color(Some("nonsense")), None);
        assert_eq!(parse_color(None), None);
    }

    #[test]
    fn theme_file_overrides_only_set_slots() {
        let file: ThemeFile =
            serde_json::from_str(r##"{"accent":"#123456","tool":"magenta"}"##).unwrap();
        let theme = file.apply(Theme::dark());
        assert_eq!(theme.accent, Color::Rgb(0x12, 0x34, 0x56));
        assert_eq!(theme.tool, Color::Magenta);
        // Untouched slots keep the base value.
        assert_eq!(theme.error, Theme::dark().error);
    }
}
