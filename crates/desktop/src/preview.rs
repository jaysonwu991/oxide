//! A file the conversation produced, read for the window's viewer.
//!
//! Two shapes come out: an image as a data URL the page can paint (the page's
//! own policy allows `data:` pictures and nothing else), and anything else as
//! text, capped so a minified bundle cannot be a hundred megabytes of DOM. The
//! viewer decides how to draw the text; this only says what the file is.

use anyhow::{bail, Context, Result};
use serde::Serialize;
use std::path::{Component, Path};

/// How much of a text file the viewer is handed. A file longer than this is a
/// file to open in an editor, and the viewer says so instead of drawing a page
/// nobody can scroll.
const LIMIT: usize = 512 * 1024;

#[derive(Debug, Clone, Serialize)]
pub struct FileView {
    pub path: String,
    /// `image`, `markdown`, `csv`, `html` or `text` — what the viewer draws.
    pub kind: String,
    /// The file as text, for every kind but an image.
    pub text: String,
    /// An image as a `data:` URL, so the page's own policy can paint it.
    pub data_url: String,
    /// Whether the text was cut because the file is longer than the viewer reads.
    pub truncated: bool,
}

fn checked(path: &str) -> Result<&Path> {
    let candidate = Path::new(path);
    if candidate.is_absolute() {
        bail!("`{path}` is not a path inside the project.");
    }
    for component in candidate.components() {
        match component {
            Component::Normal(_) => {}
            _ => bail!("`{path}` is not a path inside the project."),
        }
    }
    Ok(candidate)
}

/// Reads one file of a project for the viewer.
pub fn read(project: &Path, path: &str) -> Result<FileView> {
    let relative = checked(path)?;
    let full = project.join(relative);
    let bytes = std::fs::read(&full).with_context(|| format!("reading {}", full.display()))?;
    let extension = full
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or_default()
        .to_lowercase();
    let kind = match extension.as_str() {
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "avif" => "image",
        "md" | "markdown" => "markdown",
        "csv" | "tsv" => "csv",
        "html" | "htm" => "html",
        _ => "text",
    };
    if kind == "image" {
        let mime = match extension.as_str() {
            "png" => "image/png",
            "gif" => "image/gif",
            "webp" => "image/webp",
            "avif" => "image/avif",
            _ => "image/jpeg",
        };
        return Ok(FileView {
            path: path.to_string(),
            kind: kind.to_string(),
            text: String::new(),
            data_url: format!(
                "data:{mime};base64,{}",
                oxide_core::media::base64_encode(&bytes)
            ),
            truncated: false,
        });
    }
    let truncated = bytes.len() > LIMIT;
    let slice = if truncated {
        &bytes[..LIMIT]
    } else {
        &bytes[..]
    };
    // A file that is not text at all still opens: it is shown as what it is
    // rather than refused, since "binary" is a fact about the file, not an error.
    let text = String::from_utf8_lossy(slice).into_owned();
    Ok(FileView {
        path: path.to_string(),
        kind: kind.to_string(),
        text,
        data_url: String::new(),
        truncated,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("oxide_preview_{name}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_markdown_file_reads_as_markdown() {
        let dir = project("markdown");
        std::fs::write(dir.join("notes.md"), "# Title\n\nBody\n").unwrap();
        let view = read(&dir, "notes.md").unwrap();
        assert_eq!(view.kind, "markdown");
        assert!(view.text.contains("# Title"));
        assert!(!view.truncated);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_csv_reads_as_csv_and_an_unknown_file_as_text() {
        let dir = project("csv");
        std::fs::write(dir.join("data.csv"), "a,b\n1,2\n").unwrap();
        assert_eq!(read(&dir, "data.csv").unwrap().kind, "csv");
        std::fs::write(dir.join("notes.log"), "a line\n").unwrap();
        let view = read(&dir, "notes.log").unwrap();
        assert_eq!(view.kind, "text");
        assert_eq!(view.text, "a line\n");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_image_comes_back_as_a_data_url_rather_than_text() {
        let dir = project("image");
        // The smallest PNG there is: eight bytes of signature are enough for the
        // reader here, which does not decode the picture.
        let png = [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
        std::fs::write(dir.join("shot.png"), png).unwrap();
        let view = read(&dir, "shot.png").unwrap();
        assert_eq!(view.kind, "image");
        assert!(
            view.data_url.starts_with("data:image/png;base64,"),
            "{}",
            view.data_url
        );
        assert!(view.text.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_file_outside_the_project_is_refused() {
        let dir = project("escape");
        for path in ["../outside.md", "/etc/passwd", "a/../../b.md"] {
            let refused = read(&dir, path).unwrap_err().to_string();
            assert!(refused.contains("not a path inside"), "{path}: {refused}");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_long_file_is_cut_and_says_so() {
        let dir = project("long");
        let body = "x".repeat(LIMIT + 1024);
        std::fs::write(dir.join("big.txt"), &body).unwrap();
        let view = read(&dir, "big.txt").unwrap();
        assert!(view.truncated);
        assert_eq!(view.text.len(), LIMIT);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
