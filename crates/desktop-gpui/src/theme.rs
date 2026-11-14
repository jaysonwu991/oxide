//! The window's palette.
//!
//! The colors come from the same theme files the terminal and the Electron
//! window read — `theme_colors` resolves a theme to semantic slots — with the
//! built-in dark palette as the fallback a window paints before the engine has
//! answered.

use gpui::{Hsla, Rgba};
use serde_json::Value;
use std::collections::BTreeMap;

const DEFAULTS: &[(&str, &str)] = &[
    ("background", "#1c1c1c"),
    ("sidebar", "#161616"),
    ("panel", "#212121"),
    ("panel_2", "#262626"),
    ("panel_3", "#2d2d2d"),
    ("border", "#2b2b2b"),
    ("text", "#ededed"),
    ("dim", "#9a9a9a"),
    ("faint", "#6f6f6f"),
    ("accent", "#a9c7ff"),
    ("user", "#a9c7ff"),
    ("assistant", "#ededed"),
    ("success", "#78d38a"),
    ("tool", "#e0b072"),
    ("error", "#f08a82"),
    ("info", "#9a9a9a"),
    ("tool_pending_bg", "#232323"),
    ("tool_success_bg", "#1f2a20"),
    ("tool_error_bg", "#2e1f1f"),
    ("usage_bar_bg", "#2c3a52"),
    ("usage_bar_fg", "#ededed"),
    ("usage_bar_label", "#a9c7ff"),
    ("thinking_off", "#9a9a9a"),
    ("thinking_low", "#a9c7ff"),
    ("thinking_medium", "#8ab4ff"),
    ("thinking_high", "#c79bff"),
    ("thinking_text", "#9a9a9a"),
    ("pull_merged", "#a371f7"),
    ("diff_add", "#1f3a24"),
    ("diff_remove", "#3a1f1f"),
];

#[derive(Clone)]
pub struct Theme {
    pub name: String,
    colors: BTreeMap<String, Hsla>,
}

impl Default for Theme {
    fn default() -> Self {
        let mut colors = BTreeMap::new();
        for (slot, hex) in DEFAULTS {
            colors.insert((*slot).to_string(), parse(hex));
        }
        Self {
            name: "dark".to_string(),
            colors,
        }
    }
}

impl Theme {
    /// Reads the `theme_colors` answer: `{name, colors: {slot: "#rrggbb"}}`.
    pub fn from_value(value: &Value) -> Self {
        let mut theme = Self::default();
        if let Some(name) = value["name"].as_str() {
            theme.name = name.to_string();
        }
        if let Some(colors) = value["colors"].as_object() {
            for (slot, hex) in colors {
                if let Some(hex) = hex.as_str() {
                    theme.colors.insert(slot.clone(), parse(hex));
                }
            }
        }
        theme
    }

    pub fn color(&self, slot: &str) -> Hsla {
        self.colors
            .get(slot)
            .copied()
            .unwrap_or_else(|| gpui::rgba(0x00000000).into())
    }

    pub fn background(&self) -> Hsla {
        self.color("background")
    }
    pub fn sidebar(&self) -> Hsla {
        self.color("sidebar")
    }
    pub fn panel(&self) -> Hsla {
        self.color("panel")
    }
    pub fn panel_2(&self) -> Hsla {
        self.color("panel_2")
    }
    pub fn panel_3(&self) -> Hsla {
        self.color("panel_3")
    }
    pub fn border(&self) -> Hsla {
        self.color("border")
    }
    pub fn text(&self) -> Hsla {
        self.color("text")
    }
    pub fn dim(&self) -> Hsla {
        self.color("dim")
    }
    pub fn faint(&self) -> Hsla {
        self.color("faint")
    }
    pub fn accent(&self) -> Hsla {
        self.color("accent")
    }
    pub fn assistant(&self) -> Hsla {
        self.color("assistant")
    }
    pub fn success(&self) -> Hsla {
        self.color("success")
    }
    pub fn tool(&self) -> Hsla {
        self.color("tool")
    }
    pub fn error(&self) -> Hsla {
        self.color("error")
    }
    pub fn thinking_text(&self) -> Hsla {
        self.color("thinking_text")
    }
    pub fn info(&self) -> Hsla {
        self.color("info")
    }

    /// A slot mixed with the window's own background, for the quiet fills the
    /// page uses behind a tool card or a code block.
    pub fn wash(&self, slot: &str, amount: f32) -> Hsla {
        let base = self.color(slot);
        let background = self.background();
        gpui::Hsla {
            h: base.h,
            s: base.s,
            l: base.l * amount + background.l * (1.0 - amount),
            a: 1.0,
        }
    }

    /// A surface sunk below the page, for the tile the brand mark sits in: the
    /// page paints `#14141a` there rather than a panel, so the mark stands on
    /// something darker than the column around it.
    pub fn well(&self) -> Hsla {
        let background = self.background();
        gpui::Hsla {
            h: background.h,
            s: background.s,
            l: background.l * 0.72,
            a: 1.0,
        }
    }
}

fn parse(hex: &str) -> Hsla {
    let hex = hex.trim_start_matches('#');
    let value = u32::from_str_radix(hex, 16).unwrap_or(0x000000);
    let (red, green, blue) = if hex.len() >= 8 {
        (
            (value >> 24) & 0xff,
            (value >> 16) & 0xff,
            (value >> 8) & 0xff,
        )
    } else {
        ((value >> 16) & 0xff, (value >> 8) & 0xff, value & 0xff)
    };
    Rgba {
        r: red as f32 / 255.0,
        g: green as f32 / 255.0,
        b: blue as f32 / 255.0,
        a: 1.0,
    }
    .into()
}
