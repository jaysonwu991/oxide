//! Named color themes for the TUI. `dark` and `light` are built in; custom
//! themes are JSON files under `.oxide/themes/` (project) or the oxide config
//! dir (global), matching Pi's theme locations.

use serde::Deserialize;
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
    pub tool: Color,
    pub error: Color,
    pub info: Color,
    pub dim: Color,
    pub border: Color,
    pub thinking_off: Color,
    pub thinking_low: Color,
    pub thinking_medium: Color,
    pub thinking_high: Color,
}

impl Theme {
    pub fn dark() -> Self {
        Self {
            name: "dark".to_string(),
            accent: Color::LightCyan,
            user: Color::LightCyan,
            assistant: Color::White,
            tool: Color::LightYellow,
            error: Color::LightRed,
            info: Color::Gray,
            dim: Color::DarkGray,
            border: Color::LightCyan,
            thinking_off: Color::Gray,
            thinking_low: Color::LightCyan,
            thinking_medium: Color::LightBlue,
            thinking_high: Color::LightMagenta,
        }
    }

    pub fn light() -> Self {
        Self {
            name: "light".to_string(),
            accent: Color::Blue,
            user: Color::Blue,
            assistant: Color::Black,
            tool: Color::Magenta,
            error: Color::Red,
            info: Color::DarkGray,
            dim: Color::Gray,
            border: Color::Blue,
            thinking_off: Color::Gray,
            thinking_low: Color::Cyan,
            thinking_medium: Color::Blue,
            thinking_high: Color::Magenta,
        }
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
    pub tool: Option<String>,
    pub error: Option<String>,
    pub info: Option<String>,
    pub dim: Option<String>,
    pub border: Option<String>,
    pub thinking_off: Option<String>,
    pub thinking_low: Option<String>,
    pub thinking_medium: Option<String>,
    pub thinking_high: Option<String>,
}

impl ThemeFile {
    /// Applies the overrides on top of `base`.
    pub fn apply(&self, base: Theme) -> Theme {
        Theme {
            name: base.name.clone(),
            accent: parse_color(self.accent.as_deref()).unwrap_or(base.accent),
            user: parse_color(self.user.as_deref()).unwrap_or(base.user),
            assistant: parse_color(self.assistant.as_deref()).unwrap_or(base.assistant),
            tool: parse_color(self.tool.as_deref()).unwrap_or(base.tool),
            error: parse_color(self.error.as_deref()).unwrap_or(base.error),
            info: parse_color(self.info.as_deref()).unwrap_or(base.info),
            dim: parse_color(self.dim.as_deref()).unwrap_or(base.dim),
            border: parse_color(self.border.as_deref()).unwrap_or(base.border),
            thinking_off: parse_color(self.thinking_off.as_deref()).unwrap_or(base.thinking_off),
            thinking_low: parse_color(self.thinking_low.as_deref()).unwrap_or(base.thinking_low),
            thinking_medium: parse_color(self.thinking_medium.as_deref())
                .unwrap_or(base.thinking_medium),
            thinking_high: parse_color(self.thinking_high.as_deref()).unwrap_or(base.thinking_high),
        }
    }
}

/// Resolves a theme by name: built-ins first, then `.oxide/themes/<name>.json`
/// (project), then `<config>/oxide/themes/<name>.json` (global).
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
    if let Some(config) = dirs::config_dir() {
        dirs.push(config.join("oxide/themes"));
    }
    dirs
}

fn project_theme(cwd: &Path, name: &str) -> Option<PathBuf> {
    let root = crate::ecosystem::project_root(cwd)?;
    let path = root.join(".oxide/themes").join(format!("{name}.json"));
    path.is_file().then_some(path)
}

fn global_theme(name: &str) -> Option<PathBuf> {
    let path = dirs::config_dir()?
        .join("oxide/themes")
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
    fn custom_theme_keeps_its_name_and_overrides() {
        let dir = std::env::temp_dir().join(format!("oxide_theme_{}", std::process::id()));
        std::fs::create_dir_all(dir.join(".git")).unwrap();
        std::fs::create_dir_all(dir.join(".oxide/themes")).unwrap();
        std::fs::write(
            dir.join(".oxide/themes/ocean.json"),
            r##"{"accent":"#5fd7ff","tool":"cyan"}"##,
        )
        .unwrap();

        let theme = load(&dir, "ocean");
        assert_eq!(theme.name, "ocean");
        assert_eq!(theme.accent, Color::Rgb(0x5f, 0xd7, 0xff));
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
