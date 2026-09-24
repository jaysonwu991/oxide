//! Multimodal attachments: image/PDF loading into provider-ready content
//! parts, dependency-free base64 encoding, `@path` reference extraction, and a
//! best-effort OS clipboard image grab.
use crate::llm::{ContentPart, FileData, ImageUrl};
use anyhow::{Context, Result};
use sha2::{Digest, Sha256};
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

/// Long edge above which an image is downscaled before it is base64-encoded.
/// Matches Anthropic's recommended maximum, so a screenshot is not shipped far
/// larger than any provider will actually read.
const MAX_IMAGE_EDGE: u32 = 1568;
/// Images smaller than this are passed through without spawning an image tool.
const IMAGE_OPTIMIZE_MIN_BYTES: usize = 200_000;

/// Best-effort downscale so a pasted screenshot or photo is not sent (or
/// stored in the session) at full resolution. Uses the OS image tools (`sips`
/// on macOS, ImageMagick/ffmpeg on Linux) and falls back to the original when
/// none is available, the format is unknown, or the result is not smaller.
pub fn optimize_image(bytes: Vec<u8>, mime: &str) -> Vec<u8> {
    if !matches!(mime, "image/png" | "image/jpeg" | "image/webp") {
        return bytes;
    }
    let oversized = image_dimensions(&bytes, mime).is_some_and(|(w, h)| w.max(h) > MAX_IMAGE_EDGE);
    if !oversized && bytes.len() <= IMAGE_OPTIMIZE_MIN_BYTES {
        return bytes;
    }
    match downscale_image(&bytes, mime) {
        // An oversized image keeps the smaller dimensions even if the bytes did
        // not shrink much; a byte-only case keeps the result only when smaller.
        Some(smaller) if !smaller.is_empty() && (oversized || smaller.len() < bytes.len()) => {
            smaller
        }
        _ => bytes,
    }
}

/// Pixel dimensions from a PNG or JPEG header. `None` for formats we do not
/// parse, so the caller falls back to the byte-size heuristic.
fn image_dimensions(bytes: &[u8], mime: &str) -> Option<(u32, u32)> {
    match mime {
        "image/png" => png_dimensions(bytes),
        "image/jpeg" => jpeg_dimensions(bytes),
        _ => None,
    }
}

fn png_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    const SIGNATURE: &[u8; 8] = b"\x89PNG\r\n\x1a\n";
    if bytes.len() < 24 || &bytes[..8] != SIGNATURE || &bytes[12..16] != b"IHDR" {
        return None;
    }
    let width = u32::from_be_bytes(bytes[16..20].try_into().ok()?);
    let height = u32::from_be_bytes(bytes[20..24].try_into().ok()?);
    (width > 0 && height > 0).then_some((width, height))
}

fn jpeg_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    if bytes.len() < 4 || bytes[0] != 0xFF || bytes[1] != 0xD8 {
        return None;
    }
    let mut index = 2;
    while index + 9 <= bytes.len() {
        if bytes[index] != 0xFF {
            index += 1;
            continue;
        }
        let marker = bytes[index + 1];
        if matches!(marker, 0xC0..=0xC3 | 0xC5..=0xC7 | 0xC9..=0xCB | 0xCD..=0xCF) {
            let height = u16::from_be_bytes([bytes[index + 5], bytes[index + 6]]) as u32;
            let width = u16::from_be_bytes([bytes[index + 7], bytes[index + 8]]) as u32;
            return (width > 0 && height > 0).then_some((width, height));
        }
        if matches!(marker, 0xD8 | 0xD9 | 0x01) || (0xD0..=0xD7).contains(&marker) {
            index += 2;
            continue;
        }
        let length = u16::from_be_bytes([bytes[index + 2], bytes[index + 3]]) as usize;
        if length < 2 {
            return None;
        }
        index += 2 + length;
    }
    None
}

fn image_extension(mime: &str) -> &str {
    match mime {
        "image/jpeg" => "jpg",
        "image/webp" => "webp",
        _ => "png",
    }
}

fn temp_image_paths(extension: &str) -> (PathBuf, PathBuf) {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let id = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir = std::env::temp_dir();
    let stamp = format!("{}-{id}", std::process::id());
    (
        dir.join(format!("oxide-image-in-{stamp}.{extension}")),
        dir.join(format!("oxide-image-out-{stamp}.{extension}")),
    )
}

#[cfg(target_os = "macos")]
fn downscale_image(bytes: &[u8], mime: &str) -> Option<Vec<u8>> {
    let (input, output) = temp_image_paths(image_extension(mime));
    std::fs::write(&input, bytes).ok()?;
    let status = Command::new("sips")
        .args(["-Z", &MAX_IMAGE_EDGE.to_string()])
        .arg(&input)
        .arg("--out")
        .arg(&output)
        .output()
        .ok()?;
    let result = status
        .status
        .success()
        .then(|| std::fs::read(&output).ok())
        .flatten();
    let _ = std::fs::remove_file(&input);
    let _ = std::fs::remove_file(&output);
    result
}

#[cfg(target_os = "linux")]
fn downscale_image(bytes: &[u8], mime: &str) -> Option<Vec<u8>> {
    let (input, output) = temp_image_paths(image_extension(mime));
    std::fs::write(&input, bytes).ok()?;
    let input = input.to_str()?.to_string();
    let output = output.to_str()?.to_string();
    let geometry = format!("{MAX_IMAGE_EDGE}x{MAX_IMAGE_EDGE}>");
    let resized = run_ok("magick", &[&input, "-resize", &geometry, &output])
        || run_ok("convert", &[&input, "-resize", &geometry, &output])
        || run_ok(
            "ffmpeg",
            &[
                "-y",
                "-loglevel",
                "error",
                "-i",
                &input,
                "-vf",
                &format!(
                    "scale={MAX_IMAGE_EDGE}:{MAX_IMAGE_EDGE}:force_original_aspect_ratio=decrease"
                ),
                &output,
            ],
        );
    let result = resized.then(|| std::fs::read(&output).ok()).flatten();
    let _ = std::fs::remove_file(&input);
    let _ = std::fs::remove_file(&output);
    result
}

#[cfg(target_os = "linux")]
fn run_ok(command: &str, args: &[&str]) -> bool {
    Command::new(command)
        .args(args)
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn downscale_image(_bytes: &[u8], _mime: &str) -> Option<Vec<u8>> {
    None
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
    // GIF is left alone so an animation is not flattened to one frame.
    let bytes = if mime == "image/gif" {
        bytes
    } else {
        optimize_image(bytes, mime)
    };
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

/// A short, stable id for an attachment, derived from its payload. Two copies
/// of the same image (or the same `@path` reference) hash to one id, so the
/// composer can de-duplicate a repeated paste.
pub fn attachment_id(part: &ContentPart) -> String {
    let mut hasher = Sha256::new();
    match part {
        ContentPart::Text { text } => hasher.update(text.as_bytes()),
        ContentPart::ImageUrl { image_url } => hasher.update(image_url.url.as_bytes()),
        ContentPart::File { file } => hasher.update(file.file_data.as_bytes()),
    }
    let digest = hasher.finalize();
    digest[..8]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// A short human label for an attachment, shown in the `/attach` listing.
pub fn attachment_label(part: &ContentPart) -> String {
    match part {
        ContentPart::ImageUrl { image_url } => data_url_media_type(&image_url.url)
            .and_then(|media| media.rsplit('/').next().map(str::to_string))
            .unwrap_or_else(|| "image".to_string()),
        ContentPart::File { file } => file
            .filename
            .clone()
            .unwrap_or_else(|| "document".to_string()),
        ContentPart::Text { .. } => "text".to_string(),
    }
}

fn data_url_media_type(url: &str) -> Option<&str> {
    let meta = url
        .strip_prefix("data:")?
        .split_once(',')
        .map(|(meta, _)| meta)?;
    Some(meta.strip_suffix(";base64").unwrap_or(meta))
}

/// Builds a content part from an already-encoded data URL, e.g. an image the
/// desktop read from the clipboard. Returns `None` for a type the provider
/// cannot take as an attachment.
pub fn content_part_from_data_url(
    data_url: String,
    filename: Option<String>,
) -> Option<ContentPart> {
    let media_type = data_url_media_type(&data_url)?;
    if media_type.starts_with("image/") {
        Some(ContentPart::ImageUrl {
            image_url: ImageUrl {
                url: data_url,
                detail: None,
            },
        })
    } else if media_type == "application/pdf" {
        Some(ContentPart::File {
            file: FileData {
                filename,
                file_data: data_url,
            },
        })
    } else {
        None
    }
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

/// Best-effort clipboard image grab. On macOS this uses the built-in
/// `osascript` (falling back to `pngpaste`); on Linux it needs `wl-paste` or
/// `xclip`.
pub fn clipboard_image() -> Option<ContentPart> {
    let bytes = clipboard_bytes()?;
    if bytes.is_empty() {
        return None;
    }
    let bytes = optimize_image(bytes, "image/png");
    Some(ContentPart::ImageUrl {
        image_url: ImageUrl {
            url: format!("data:image/png;base64,{}", base64_encode(&bytes)),
            detail: None,
        },
    })
}

#[cfg(target_os = "macos")]
fn clipboard_bytes() -> Option<Vec<u8>> {
    run_stdout("pngpaste", &["-"]).or_else(clipboard_bytes_osascript)
}

/// Extracts the clipboard image with the built-in `osascript`, avoiding a
/// dependency on `pngpaste`. Returns `None` when the clipboard holds no image.
#[cfg(target_os = "macos")]
fn clipboard_bytes_osascript() -> Option<Vec<u8>> {
    let path = std::env::temp_dir().join(format!("oxide-clipboard-{}.png", std::process::id()));
    let path_str = path.to_str()?;
    let script = format!(
        "set theFile to (open for access POSIX file \"{path_str}\" with write permission)\n\
         write (the clipboard as «class PNGf») to theFile\n\
         close access theFile"
    );
    let output = Command::new("osascript")
        .args(["-e", &script])
        .output()
        .ok()?;
    let bytes = if output.status.success() {
        std::fs::read(&path).ok()
    } else {
        None
    };
    let _ = std::fs::remove_file(&path);
    bytes.filter(|bytes| !bytes.is_empty())
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

    /// A real 1x1 PNG, used to build a large fixture with the system image tool.
    const ONE_PX_PNG: &[u8] = &[
        0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44,
        0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1f,
        0x15, 0xc4, 0x89, 0x00, 0x00, 0x00, 0x0b, 0x49, 0x44, 0x41, 0x54, 0x78, 0xda, 0x63, 0x64,
        0x60, 0x00, 0x00, 0x00, 0x06, 0x00, 0x02, 0x30, 0x81, 0xd0, 0x2f, 0x00, 0x00, 0x00, 0x00,
        0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
    ];

    #[test]
    fn parses_png_and_jpeg_dimensions() {
        assert_eq!(png_dimensions(ONE_PX_PNG), Some((1, 1)));
        assert_eq!(png_dimensions(b"not a png"), None);

        // SOI + a minimal SOF0 marker declaring 640x480.
        let jpeg = [
            0xFF, 0xD8, 0xFF, 0xC0, 0x00, 0x11, 0x08, 0x01, 0xE0, 0x02, 0x80, 0x03, 0x01, 0x11,
            0x00, 0x02, 0x11, 0x00, 0x03, 0x11, 0x00,
        ];
        assert_eq!(jpeg_dimensions(&jpeg), Some((640, 480)));
        assert_eq!(jpeg_dimensions(&[0xFF, 0xD9]), None);
    }

    #[test]
    fn optimize_leaves_small_and_unknown_images_alone() {
        let small = vec![0u8; 16];
        assert_eq!(optimize_image(small.clone(), "image/png"), small);
        let unknown = vec![0u8; IMAGE_OPTIMIZE_MIN_BYTES + 1];
        assert_eq!(optimize_image(unknown.clone(), "image/tiff"), unknown);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn downscales_an_oversized_image() {
        let dir = std::env::temp_dir().join(format!("oxide_media_resize_{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        let input = dir.join("in.png");
        let output = dir.join("out.png");
        std::fs::write(&input, ONE_PX_PNG).unwrap();
        let status = Command::new("sips")
            .args(["-Z", "3000"])
            .arg(&input)
            .arg("--out")
            .arg(&output)
            .output()
            .expect("sips is built into macOS");
        if !status.status.success() {
            std::fs::remove_dir_all(&dir).ok();
            return;
        }
        let large = std::fs::read(&output).unwrap();
        assert_eq!(png_dimensions(&large), Some((3000, 3000)));

        let optimized = optimize_image(large, "image/png");
        let (width, height) = png_dimensions(&optimized).expect("still a PNG");
        assert!(width <= MAX_IMAGE_EDGE && height <= MAX_IMAGE_EDGE);

        std::fs::remove_dir_all(&dir).ok();
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

    #[test]
    fn builds_parts_from_data_urls() {
        let image = content_part_from_data_url(
            "data:image/png;base64,AAAA".to_string(),
            Some("shot.png".to_string()),
        )
        .expect("image");
        match image {
            ContentPart::ImageUrl { image_url } => {
                assert_eq!(image_url.url, "data:image/png;base64,AAAA")
            }
            other => panic!("unexpected part: {other:?}"),
        }

        let pdf = content_part_from_data_url(
            "data:application/pdf;base64,AAAA".to_string(),
            Some("spec.pdf".to_string()),
        )
        .expect("pdf");
        assert!(matches!(pdf, ContentPart::File { .. }));

        assert!(
            content_part_from_data_url("data:text/plain;base64,AAAA".to_string(), None).is_none()
        );
        assert!(content_part_from_data_url("not a data url".to_string(), None).is_none());
    }

    #[test]
    fn attachment_ids_are_content_addressed() {
        let image = |url: &str| ContentPart::ImageUrl {
            image_url: ImageUrl {
                url: url.to_string(),
                detail: None,
            },
        };
        let a = image("data:image/png;base64,AAAA");
        let b = image("data:image/png;base64,AAAA");
        let c = image("data:image/jpeg;base64,BBBB");

        assert_eq!(attachment_id(&a), attachment_id(&b));
        assert_ne!(attachment_id(&a), attachment_id(&c));
        assert_eq!(attachment_id(&a).len(), 16);
        assert_eq!(attachment_label(&a), "png");
        assert_eq!(attachment_label(&c), "jpeg");
        let document = ContentPart::File {
            file: FileData {
                filename: Some("spec.pdf".into()),
                file_data: "data:application/pdf;base64,AAAA".into(),
            },
        };
        assert_eq!(attachment_label(&document), "spec.pdf");
    }
}
