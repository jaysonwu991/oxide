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
const MAX_RASTER_EDGE: f32 = 8192.0;
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
    let natural_edge = size.width().max(size.height());
    if !natural_edge.is_finite() || natural_edge <= 0.0 {
        return None;
    }
    // Dense sequence diagrams routinely exceed the 4096-point edge a fixed
    // 2x raster could draw. Fit the scale to the raster ceiling instead of
    // rejecting the whole diagram.
    let scale = SCALE.min(MAX_RASTER_EDGE / natural_edge);
    let width = (size.width() * scale).ceil() as u32;
    let height = (size.height() * scale).ceil() as u32;
    if width == 0 || height == 0 {
        return None;
    }
    let mut pixmap = resvg::tiny_skia::Pixmap::new(width, height)?;
    resvg::render(
        &tree,
        resvg::tiny_skia::Transform::from_scale(scale, scale),
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
    fn the_diagram_kinds_a_transcript_uses_become_pictures() {
        for source in [
            "graph TD\n  A --> B",
            "flowchart TD\n  A[agent] -->|JSON| B[server<br/>dispatch]",
            "stateDiagram-v2\n  [*] --> Empty\n  Empty --> Bound: query\n  Bound --> Refused: stale index",
            "sequenceDiagram\n  participant C as client\n  participant D as daemon\n  C->>D: Hello\n  D-->>C: Snapshot",
        ] {
            let diagram = render(source, true).expect("diagram renders");
            assert!(diagram.width > 20.0 && diagram.height > 20.0);
        }
    }

    #[test]
    fn a_dense_diagram_is_fitted_to_the_raster_ceiling() {
        let svg = r##"<svg xmlns="http://www.w3.org/2000/svg" width="5000" height="100">
          <rect width="5000" height="100" fill="#fff"/>
        </svg>"##;
        let diagram = rasterise(svg).expect("the wide diagram rasterises");
        assert_eq!(diagram.width, 5000.0);
        assert_eq!(diagram.height, 100.0);
    }

    #[test]
    fn a_markdown_fence_reaches_the_native_renderer() {
        let blocks = crate::markdown::parse(
            "Before.\n\n```mermaid\nsequenceDiagram\n  A->>B: ping\n  B-->>A: pong\n```\n\nAfter.",
        );
        let source = blocks
            .iter()
            .find_map(|block| match block {
                crate::markdown::Block::Code { language, text }
                    if language.eq_ignore_ascii_case("mermaid") =>
                {
                    Some(text)
                }
                _ => None,
            })
            .expect("the fence parses as Mermaid");
        assert!(render(source, true).is_some());
    }

    #[test]
    fn nonsense_has_no_picture() {
        assert!(render("not a diagram at all", true).is_none());
    }
}
