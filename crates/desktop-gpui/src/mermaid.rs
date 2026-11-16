//! Mermaid fences, drawn natively.
//!
//! The page asks mermaid.js to draw a diagram; here the same source goes
//! through a Rust renderer to SVG and is rasterised once, then cached by source
//! so a repaint costs a lookup. A source that does not parse yet — a diagram
//! still streaming in — has no picture, and the view shows its text instead.

use gpui::{Image, ImageFormat};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

const SCALE: f32 = 2.0;
const MAX_CACHE: usize = 64;

#[derive(Clone)]
pub struct Diagram {
    pub image: Arc<Image>,
    pub width: f32,
    pub height: f32,
}

type Cache = Mutex<HashMap<String, Option<Diagram>>>;

fn cache() -> &'static Cache {
    static CACHE: OnceLock<Cache> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn options() -> usvg::Options<'static> {
    static FONTS: OnceLock<Arc<usvg::fontdb::Database>> = OnceLock::new();
    let fonts = FONTS.get_or_init(|| {
        let mut db = usvg::fontdb::Database::new();
        db.load_system_fonts();
        Arc::new(db)
    });
    usvg::Options {
        fontdb: fonts.clone(),
        ..usvg::Options::default()
    }
}

pub fn rasterise(svg: &str) -> Option<Diagram> {
    let tree = usvg::Tree::from_str(svg, &options()).ok()?;
    let size = tree.size();
    let width = (size.width() * SCALE).ceil() as u32;
    let height = (size.height() * SCALE).ceil() as u32;
    if width == 0 || height == 0 || width > 8192 || height > 8192 {
        return None;
    }
    let mut pixmap = resvg::tiny_skia::Pixmap::new(width, height)?;
    resvg::render(
        &tree,
        resvg::tiny_skia::Transform::from_scale(SCALE, SCALE),
        &mut pixmap.as_mut(),
    );
    let png = pixmap.encode_png().ok()?;
    Some(Diagram {
        image: Arc::new(Image::from_bytes(ImageFormat::Png, png)),
        width: size.width(),
        height: size.height(),
    })
}

pub fn render(source: &str, dark: bool) -> Option<Diagram> {
    let key = format!("{dark}\n{source}");
    if let Some(found) = cache().lock().ok()?.get(&key) {
        return found.clone();
    }
    let theme = if dark {
        mermaid_rs_renderer::Theme::dark()
    } else {
        mermaid_rs_renderer::Theme::modern()
    };
    let options = mermaid_rs_renderer::RenderOptions {
        theme,
        ..Default::default()
    };
    let diagram = std::panic::catch_unwind(|| {
        mermaid_rs_renderer::render_with_options(source, options)
            .ok()
            .and_then(|svg| rasterise(&svg))
    })
    .ok()
    .flatten();
    let mut cache = cache().lock().ok()?;
    if cache.len() >= MAX_CACHE {
        cache.clear();
    }
    cache.insert(key, diagram.clone());
    diagram
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_flowchart_becomes_a_picture() {
        let diagram = render("flowchart LR\n  A[Start] --> B{Ok?}\n  B --> C[Done]", true)
            .expect("diagram renders");
        assert!(diagram.width > 20.0 && diagram.height > 20.0);
    }

    #[test]
    fn nonsense_has_no_picture() {
        assert!(render("not a diagram at all", true).is_none());
    }
}
