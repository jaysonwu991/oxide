//! Multimodal attachments: image/PDF loading into provider-ready content
//! parts, dependency-free base64 encoding, `@path` reference extraction, and a
//! best-effort OS clipboard image grab.
use crate::llm::{ContentPart, FileData, ImageUrl};
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
#[cfg(any(target_os = "macos", target_os = "linux"))]
use std::process::Command;

const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

pub fn base64_encode(input: &[u8]) -> String {
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(ALPHABET[((n >> 18) & 63) as usize] as char);
        out.push(ALPHABET[((n >> 12) & 63) as usize] as char);
        if chunk.len() > 1 {
            out.push(ALPHABET[((n >> 6) & 63) as usize] as char);
        } else {
            out.push('=');
        }
        if chunk.len() > 2 {
            out.push(ALPHABET[(n & 63) as usize] as char);
        } else {
            out.push('=');
        }
    }
    out
}

pub fn image_mime(path: &Path) -> Option<&'static str> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    match ext.as_str() {
        "png" => Some("image/png"),
        "jpg" | "jpeg" => Some("image/jpeg"),
        "gif" => Some("image/gif"),
        "webp" => Some("image/webp"),
        "bmp" => Some("image/bmp"),
        _ => None,
    }
}

pub fn is_image_path(path: &Path) -> bool {
    image_mime(path).is_some()
}

pub fn is_pdf_path(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("pdf"))
}

pub fn is_attachment_path(path: &Path) -> bool {
    is_image_path(path) || is_pdf_path(path)
}

/// Reads an image or PDF and turns it into a provider-ready content part.
pub fn load_attachment(path: &Path) -> Result<ContentPart> {
    let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    if is_pdf_path(path) {
        return Ok(ContentPart::File {
            file: FileData {
                filename: path
                    .file_name()
                    .map(|name| name.to_string_lossy().to_string()),
                file_data: format!("data:application/pdf;base64,{}", base64_encode(&bytes)),
            },
        });
    }
    let mime = image_mime(path).with_context(|| {
        format!(
            "unsupported attachment type: {} (expected png/jpg/gif/webp/pdf)",
            path.display()
        )
    })?;
    Ok(ContentPart::ImageUrl {
        image_url: ImageUrl {
            url: format!("data:{mime};base64,{}", base64_encode(&bytes)),
            detail: None,
        },
    })
}

/// Extracts `@path` references from free-form input that point at existing
/// image/PDF files. The input text is left untouched.
pub fn referenced_attachments(input: &str, cwd: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    for token in input.split_whitespace() {
        let Some(raw) = token.strip_prefix('@') else {
            continue;
        };
        let raw = raw.trim_end_matches([',', '.', ')', ']', '"', '\'']);
        if raw.is_empty() {
            continue;
        }
        let path = expand_path(raw, cwd);
        if path.is_file() && is_attachment_path(&path) {
            found.push(path);
        }
    }
    found
}

pub fn expand_path(raw: &str, cwd: &Path) -> PathBuf {
    if let Some(rest) = raw.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(rest);
        }
    }
    let candidate = Path::new(raw);
    if candidate.is_absolute() {
        candidate.to_path_buf()
    } else {
        cwd.join(candidate)
    }
}

/// Best-effort clipboard image grab. Requires a platform helper
/// (`pngpaste` on macOS, `wl-paste` or `xclip` on Linux) to be installed.
pub fn clipboard_image() -> Option<ContentPart> {
    let bytes = clipboard_bytes()?;
    if bytes.is_empty() {
        return None;
    }
    Some(ContentPart::ImageUrl {
        image_url: ImageUrl {
            url: format!("data:image/png;base64,{}", base64_encode(&bytes)),
            detail: None,
        },
    })
}

#[cfg(target_os = "macos")]
fn clipboard_bytes() -> Option<Vec<u8>> {
    run_stdout("pngpaste", &["-"])
}

#[cfg(target_os = "linux")]
fn clipboard_bytes() -> Option<Vec<u8>> {
    run_stdout("wl-paste", &["--type", "image/png"]).or_else(|| {
        run_stdout(
            "xclip",
            &["-selection", "clipboard", "-t", "image/png", "-o"],
        )
    })
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn clipboard_bytes() -> Option<Vec<u8>> {
    None
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn run_stdout(command: &str, args: &[&str]) -> Option<Vec<u8>> {
    let output = Command::new(command).args(args).output().ok()?;
    if output.status.success() && !output.stdout.is_empty() {
        Some(output.stdout)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_matches_rfc4648_vectors() {
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
        assert_eq!(base64_encode(b"foob"), "Zm9vYg==");
        assert_eq!(base64_encode(b"fooba"), "Zm9vYmE=");
        assert_eq!(base64_encode(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn detects_attachment_types() {
        assert_eq!(image_mime(Path::new("a.PNG")), Some("image/png"));
        assert_eq!(image_mime(Path::new("a.jpeg")), Some("image/jpeg"));
        assert!(is_image_path(Path::new("shot.webp")));
        assert!(is_pdf_path(Path::new("doc.PDF")));
        assert!(!is_attachment_path(Path::new("main.rs")));
    }

    #[test]
    fn loads_image_as_data_url() {
        let dir = std::env::temp_dir().join(format!("oxide_media_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("dot.png");
        std::fs::write(&file, b"f").unwrap();

        let part = load_attachment(&file).unwrap();
        match part {
            ContentPart::ImageUrl { image_url } => {
                assert_eq!(image_url.url, "data:image/png;base64,Zg==");
            }
            other => panic!("unexpected part: {other:?}"),
        }

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn extracts_referenced_attachments() {
        let dir = std::env::temp_dir().join(format!("oxide_media_ref_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("shot.png"), b"x").unwrap();
        std::fs::write(dir.join("notes.txt"), b"x").unwrap();

        let found = referenced_attachments("look at @shot.png and @notes.txt please", &dir);
        assert_eq!(found.len(), 1);
        assert!(found[0].ends_with("shot.png"));

        std::fs::remove_dir_all(&dir).ok();
    }
}
