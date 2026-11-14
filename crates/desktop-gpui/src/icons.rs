//! The window's glyphs.
//!
//! Every mark is the inline SVG the Electron window strokes — the same 24-unit
//! box, the same 1.6 weight, the same paths — kept in one table and handed to
//! GPUI as an asset. GPUI rasterises the drawing and uses its alpha as a mask,
//! tinting it with the element's text color, so one drawing serves every slot a
//! glyph is painted in. The brand mark is the exception: it is four filled
//! triangles, which a single-color mask would flatten, so it is painted from a
//! canvas instead.

use gpui::{
    canvas, point, px, svg, AssetSource, Hsla, IntoElement, PathBuilder, Pixels, Point, Rgba,
    SharedString, Styled, Svg, Window,
};
use std::borrow::Cow;

const PREFIX: &str = "icons/";

macro_rules! glyph {
    ($body:literal) => {
        concat!(
            r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="#fff" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round">"##,
            $body,
            "</svg>"
        )
    };
}

const GLYPHS: &[(&str, &str)] = &[
    ("back", glyph!(r##"<path d="M15 6l-6 6 6 6M9 12h12"/>"##)),
    ("forward", glyph!(r##"<path d="M9 6l6 6-6 6M3 12h12"/>"##)),
    ("sidebar", glyph!(r##"<path d="M4 4h16v16H4ZM9 4v16"/>"##)),
    (
        "home",
        glyph!(r##"<path d="M3 11l9-8 9 8M5 10v11h5v-7h4v7h5V10"/>"##),
    ),
    (
        "folder",
        glyph!(r##"<path d="M3 6h7l2 3h9v11H3ZM3 9h18"/>"##),
    ),
    (
        "pencil",
        glyph!(r##"<path d="M11 2.5l2.5 2.5M5 11l-2 .5.5-2L11.5 1.5 14 4ZM7 3H2v11h11V9"/>"##),
    ),
    (
        "plus",
        glyph!(r##"<path d="M12 5v14M5 12h14" stroke-width="1.8"/>"##),
    ),
    (
        "more",
        glyph!(r##"<path d="M5 12h.01M12 12h.01M19 12h.01" stroke-width="2.4"/>"##),
    ),
    (
        "search",
        glyph!(r##"<circle cx="10.5" cy="10.5" r="6.5"/><path d="m15.4 15.4 4.6 4.6"/>"##),
    ),
    (
        "settings",
        glyph!(
            r##"<circle cx="12" cy="12" r="3.8" stroke-width="1.8"/><path d="M9.93 4.79 10.26 2.15h3.48l.33 2.64 1.57.65 2.1-1.63 2.45 2.45-1.63 2.1.65 1.57 2.64.33v3.48l-2.64.33-.65 1.57 1.63 2.1-2.45 2.45-2.1-1.63-1.57.65-.33 2.64h-3.48l-.33-2.64-1.57-.65-2.1 1.63-2.45-2.45 1.63-2.1-.65-1.57-2.64-.33v-3.48l2.64-.33.65-1.57-1.63-2.1 2.45-2.45 2.1 1.63Z" stroke-width="1.8"/>"##
        ),
    ),
    (
        "customize",
        glyph!(
            r##"<path d="M4 7h16M4 12h16M4 17h16"/><circle cx="9" cy="7" r="2" fill="#fff"/><circle cx="15" cy="12" r="2" fill="#fff"/><circle cx="8" cy="17" r="2" fill="#fff"/>"##
        ),
    ),
    (
        "chat",
        glyph!(
            r##"<path d="M21 15a4 4 0 0 1-4 4H8l-5 3 1.5-5A7 7 0 0 1 3 13V8a4 4 0 0 1 4-4h10a4 4 0 0 1 4 4Z"/>"##
        ),
    ),
    (
        "globe",
        glyph!(
            r##"<circle cx="12" cy="12" r="9"/><path d="M3 12h18M12 3c5 5 5 13 0 18-5-5-5-13 0-18Z"/>"##
        ),
    ),
    (
        "bell",
        glyph!(r##"<path d="M5 17V10a7 7 0 0 1 14 0v7l2 2H3ZM10 22h4"/>"##),
    ),
    (
        "keyboard",
        glyph!(
            r##"<rect x="2.5" y="6.5" width="19" height="11" rx="2.5"/><path d="M6 10h.01M9.5 10h.01M13 10h.01M16.5 10h.01M7.5 14h9"/>"##
        ),
    ),
    (
        "pin",
        glyph!(r##"<path d="m14 4 6 6-3 1-4 4-1 5-3-6-5-3 5-1 4-4Z"/>"##),
    ),
    (
        "archive",
        glyph!(r##"<path d="M4 8h16M5 8v11h14V8M3 4h18v4H3ZM9 12h6"/>"##),
    ),
    ("chevron-right", glyph!(r##"<path d="M9.5 6l6 6-6 6"/>"##)),
    (
        "chevron-down",
        glyph!(r##"<path d="M5.5 8.6 12 14.8l6.5-6.2"/>"##),
    ),
    (
        "paperclip",
        glyph!(
            r##"<path d="M21.44 11.05l-9.19 9.19a6 6 0 0 1-8.49-8.49l9.19-9.19a4 4 0 0 1 5.66 5.66l-9.2 9.19a2 2 0 0 1-2.83-2.83l8.49-8.48" stroke-width="1.8"/>"##
        ),
    ),
    (
        "lock",
        glyph!(
            r##"<path d="M8 10V7a4 4 0 0 1 8 0v3"/><rect x="5.5" y="10" width="13" height="9.5" rx="2.4" stroke-width="1.8"/>"##
        ),
    ),
    (
        "arrow-up",
        glyph!(r##"<path d="M12 20V5M5.5 11.5 12 5l6.5 6.5" stroke-width="1.8"/>"##),
    ),
    (
        "square",
        glyph!(r##"<rect x="7" y="7" width="10" height="10" rx="2" fill="#fff" stroke="none"/>"##),
    ),
    (
        "branch",
        glyph!(
            r##"<circle cx="6.5" cy="5.5" r="2.2"/><circle cx="6.5" cy="18.5" r="2.2"/><circle cx="17.5" cy="8.5" r="2.2"/><path d="M6.5 7.7v8.6M8.7 16.4c5.2 0 8.8-2.6 8.8-5.7"/>"##
        ),
    ),
    (
        "details",
        glyph!(
            r##"<circle cx="5" cy="6" r="1.8" stroke-width="1.8"/><circle cx="5" cy="18" r="1.8" stroke-width="1.8"/><path d="M10.5 6H21M10.5 12H21M10.5 18H21M3 12h4" stroke-width="1.8"/>"##
        ),
    ),
    (
        "close",
        glyph!(r##"<path d="M18 6 6 18M6 6l12 12" stroke-width="1.8"/>"##),
    ),
    ("check", glyph!(r##"<path d="m5.5 12.5 4.2 4.2 8.8-9"/>"##)),
    (
        "changes",
        glyph!(
            r##"<rect x="4" y="3" width="16" height="18" rx="3" stroke-width="1.8"/><path d="M8.5 9.5h7M12 6v7M8.5 16.5h7" stroke-width="1.8"/>"##
        ),
    ),
    (
        "image",
        glyph!(
            r##"<rect x="3" y="4.5" width="18" height="15" rx="2.5"/><circle cx="9" cy="10" r="1.7"/><path d="m20.5 15.5-5-4-8.5 8"/>"##
        ),
    ),
    (
        "file",
        glyph!(
            r##"<path d="M14 3H7a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h10a2 2 0 0 0 2-2V8z"/><path d="M14 3v5h5"/>"##
        ),
    ),
];

/// The drawing an element paints.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Icon {
    Back,
    Forward,
    Sidebar,
    Home,
    Folder,
    Pencil,
    Plus,
    More,
    Search,
    Settings,
    Customize,
    Chat,
    Pin,
    Archive,
    ChevronRight,
    ChevronDown,
    Paperclip,
    Lock,
    ArrowUp,
    Square,
    Branch,
    Close,
    Details,
    Check,
    Changes,
    Image,
    File,
    Globe,
    Bell,
    Keyboard,
}

impl Icon {
    pub fn path(self) -> &'static str {
        match self {
            Icon::Back => "icons/back.svg",
            Icon::Forward => "icons/forward.svg",
            Icon::Sidebar => "icons/sidebar.svg",
            Icon::Home => "icons/home.svg",
            Icon::Folder => "icons/folder.svg",
            Icon::Pencil => "icons/pencil.svg",
            Icon::Plus => "icons/plus.svg",
            Icon::More => "icons/more.svg",
            Icon::Search => "icons/search.svg",
            Icon::Settings => "icons/settings.svg",
            Icon::Customize => "icons/customize.svg",
            Icon::Chat => "icons/chat.svg",
            Icon::Pin => "icons/pin.svg",
            Icon::Archive => "icons/archive.svg",
            Icon::ChevronRight => "icons/chevron-right.svg",
            Icon::ChevronDown => "icons/chevron-down.svg",
            Icon::Paperclip => "icons/paperclip.svg",
            Icon::Lock => "icons/lock.svg",
            Icon::ArrowUp => "icons/arrow-up.svg",
            Icon::Square => "icons/square.svg",
            Icon::Branch => "icons/branch.svg",
            Icon::Close => "icons/close.svg",
            Icon::Details => "icons/details.svg",
            Icon::Check => "icons/check.svg",
            Icon::Changes => "icons/changes.svg",
            Icon::Image => "icons/image.svg",
            Icon::File => "icons/file.svg",
            Icon::Globe => "icons/globe.svg",
            Icon::Bell => "icons/bell.svg",
            Icon::Keyboard => "icons/keyboard.svg",
        }
    }
}

/// One glyph, at `size` points, in `color`.
pub fn icon(mark: Icon, size: f32, color: Hsla) -> Svg {
    svg()
        .path(mark.path())
        .flex_none()
        .w(px(size))
        .h(px(size))
        .text_color(color)
}

/// The brand mark: a diamond cut into a rust half and a cyan half, each with a
/// dark rim of its own. It is the drawing `app.js` writes inline.
pub fn mark(size: f32) -> impl IntoElement {
    let extent = px(size);
    canvas(
        |bounds, _, _| bounds,
        move |bounds, _, window, _| {
            let unit = f32::from(bounds.size.width) / 24.0;
            let rim = hex(0x2d2d3a);
            for corners in [
                [[3.375, 12.0], [11.3, 4.075], [11.3, 19.925]],
                [[12.7, 4.075], [20.625, 12.0], [12.7, 19.925]],
            ] {
                triangle(window, bounds.origin, unit, corners, rim);
            }
            triangle(
                window,
                bounds.origin,
                unit,
                [[4.5, 12.0], [11.3, 5.2], [11.3, 18.8]],
                hex(0xd9643a),
            );
            triangle(
                window,
                bounds.origin,
                unit,
                [[12.7, 5.2], [19.5, 12.0], [12.7, 18.8]],
                hex(0x5fd7ff),
            );
        },
    )
    .flex_none()
    .w(extent)
    .h(extent)
}

fn triangle(
    window: &mut Window,
    origin: Point<Pixels>,
    unit: f32,
    corners: [[f32; 2]; 3],
    color: Hsla,
) {
    let at = |corner: [f32; 2]| {
        point(
            px(f32::from(origin.x) + corner[0] * unit),
            px(f32::from(origin.y) + corner[1] * unit),
        )
    };
    let mut builder = PathBuilder::fill();
    builder.move_to(at(corners[0]));
    builder.line_to(at(corners[1]));
    builder.line_to(at(corners[2]));
    builder.close();
    if let Ok(path) = builder.build() {
        window.paint_path(path, color);
    }
}

fn hex(value: u32) -> Hsla {
    Rgba {
        r: ((value >> 16) & 0xff) as f32 / 255.0,
        g: ((value >> 8) & 0xff) as f32 / 255.0,
        b: (value & 0xff) as f32 / 255.0,
        a: 1.0,
    }
    .into()
}

/// Every glyph the window can name, served to GPUI's SVG renderer.
pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> gpui::Result<Option<Cow<'static, [u8]>>> {
        let name = path
            .strip_prefix(PREFIX)
            .and_then(|rest| rest.strip_suffix(".svg"));
        let Some(name) = name else {
            return Ok(None);
        };
        Ok(GLYPHS
            .iter()
            .find(|(glyph, _)| *glyph == name)
            .map(|(_, body)| Cow::Borrowed(body.as_bytes())))
    }

    fn list(&self, path: &str) -> gpui::Result<Vec<SharedString>> {
        if !path.trim_end_matches('/').eq("icons") {
            return Ok(Vec::new());
        }
        Ok(GLYPHS
            .iter()
            .map(|(name, _)| SharedString::from(format!("{PREFIX}{name}.svg")))
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_icon_has_a_drawing() {
        for mark in [
            Icon::Back,
            Icon::Forward,
            Icon::Sidebar,
            Icon::Home,
            Icon::Folder,
            Icon::Pencil,
            Icon::Plus,
            Icon::More,
            Icon::Search,
            Icon::Settings,
            Icon::Customize,
            Icon::Chat,
            Icon::Pin,
            Icon::Archive,
            Icon::ChevronRight,
            Icon::ChevronDown,
            Icon::Paperclip,
            Icon::Lock,
            Icon::ArrowUp,
            Icon::Square,
            Icon::Branch,
            Icon::Close,
            Icon::Details,
            Icon::Check,
            Icon::Changes,
            Icon::Image,
            Icon::File,
            Icon::Globe,
            Icon::Bell,
            Icon::Keyboard,
        ] {
            let body = Assets.load(mark.path()).unwrap().unwrap();
            assert!(body.starts_with(b"<svg"));
        }
    }

    #[test]
    fn an_unknown_asset_is_absent() {
        assert!(Assets.load("icons/nothing.svg").unwrap().is_none());
        assert!(Assets.load("logo.png").unwrap().is_none());
    }

    #[test]
    fn every_glyph_draws_ink() {
        for (name, body) in GLYPHS {
            let tree = usvg::Tree::from_data(body.as_bytes(), &usvg::Options::default())
                .unwrap_or_else(|error| panic!("{name}: {error}"));
            let size = tree.size();
            assert!(size.width() > 0.0 && size.height() > 0.0, "{name}");
            let mut pixmap = resvg::tiny_skia::Pixmap::new(24, 24).expect("{name}: pixmap");
            resvg::render(
                &tree,
                resvg::tiny_skia::Transform::from_scale(1.0, 1.0),
                &mut pixmap.as_mut(),
            );
            assert!(
                pixmap.pixels().iter().any(|pixel| pixel.alpha() > 0),
                "{name} draws nothing"
            );
        }
    }
}
