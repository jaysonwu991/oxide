//! Multimodal attachments: reading any file a message carries into LLM-ready
//! content parts — an image (converted to a format a provider takes, where the
//! machine has the tools to), a PDF, or any other text — plus dependency-free
//! base64 encoding, `@path` reference extraction, and a best-effort OS
//! clipboard grab.
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

/// Decodes standard base64, ignoring whitespace, or `None` for input that is
/// not base64 at all. A front-end that hands over an attachment as a data URL
/// has nothing else to decode it from, and the payload has to be read before it
/// can be downscaled.
pub fn base64_decode(input: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(input.len() / 4 * 3);
    let mut buffer = 0u32;
    let mut bits = 0u32;
    for byte in input.bytes() {
        let value = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            b'=' => break,
            b' ' | b'\n' | b'\r' | b'\t' => continue,
            _ => return None,
        } as u32;
        buffer = (buffer << 6) | value;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buffer >> bits) as u8);
            buffer &= (1 << bits) - 1;
        }
    }
    Some(out)
}

/// The largest attachment that may travel to a provider, on disk or as a data
/// URL. The bytes exist several times over while a turn is set up — the
/// front-end's own copy, the IPC message, the session log entry and the request
/// body — so an attachment past this is refused instead of being multiplied.
pub const MAX_ATTACHMENT_BYTES: usize = 20 * 1024 * 1024;

/// The most characters a data URL may hold. Base64 spends four characters per
/// three bytes and the `data:<mime>;base64,` header spends part of that budget,
/// so a URL at this length decodes to *fewer* than [`MAX_ATTACHMENT_BYTES`] —
/// the guard is a bound on the bytes, not an approximation of one. Whitespace
/// inside the payload only makes it stricter.
const MAX_DATA_URL_CHARS: usize = MAX_ATTACHMENT_BYTES / 3 * 4;

/// The image formats a provider takes as they stand — Pi's own normalized set,
/// so a GIF keeps its animation and a PNG, a JPEG or a WebP travels untouched.
/// Anything else sniffed as an image (a BMP, a TIFF, a HEIC) is converted to
/// PNG first, the way Pi converts one.
pub fn is_supported_image_mime(mime: &str) -> bool {
    matches!(
        mime,
        "image/png" | "image/jpeg" | "image/gif" | "image/webp"
    )
}

/// How many bytes of a file are read to identify it, the window Pi sniffs with.
const SNIFF_BYTES: usize = 4100;

/// The image format in a buffer, read from the bytes rather than the file's
/// name, the way Pi identifies one. `None` is either not an image at all or an
/// image whose bytes are a shape no provider takes — an animated PNG, a
/// JPEG-LS — which is what keeps a text file named `.png` from being sent as
/// base64 and a real screenshot without an extension from being missed.
pub fn sniff_image_mime(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return (bytes.get(3) != Some(&0xF7)).then_some("image/jpeg");
    }
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        return (is_png(bytes) && !is_animated_png(bytes)).then_some("image/png");
    }
    if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        return Some("image/gif");
    }
    if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP".as_slice()) {
        return Some("image/webp");
    }
    if bytes.starts_with(b"BM") && is_bmp(bytes) {
        return Some("image/bmp");
    }
    None
}

/// A PNG whose first chunk is an IHDR of the length the format declares.
fn is_png(bytes: &[u8]) -> bool {
    bytes.len() >= 16
        && u32::from_be_bytes(bytes[8..12].try_into().unwrap()) == 13
        && &bytes[12..16] == b"IHDR"
}

/// An APNG carries an `acTL` chunk before its image data. Flattening one to a
/// single frame is not what its sender attached, so it is not an image here.
fn is_animated_png(bytes: &[u8]) -> bool {
    let mut offset = 8;
    while offset + 8 <= bytes.len() {
        let length = u32::from_be_bytes(bytes[offset..offset + 4].try_into().unwrap()) as usize;
        let kind = &bytes[offset + 4..offset + 8];
        if kind == b"acTL" {
            return true;
        }
        if kind == b"IDAT" {
            return false;
        }
        let next = offset + 8 + length + 4;
        if next <= offset || next > bytes.len() {
            return false;
        }
        offset = next;
    }
    false
}

/// A BMP whose header describes an uncompressed bitmap with a colour depth the
/// format actually has, so a file that merely starts `BM` is read as text.
fn is_bmp(bytes: &[u8]) -> bool {
    if bytes.len() < 26 {
        return false;
    }
    let declared = u32::from_le_bytes(bytes[2..6].try_into().unwrap());
    let pixel_offset = u32::from_le_bytes(bytes[10..14].try_into().unwrap());
    let header = u32::from_le_bytes(bytes[14..18].try_into().unwrap());
    if declared != 0 && declared < 26 {
        return false;
    }
    if pixel_offset < 14 + header {
        return false;
    }
    if declared != 0 && pixel_offset >= declared {
        return false;
    }
    let (planes, depth) = match header {
        12 => (uint16_le(bytes, 22), uint16_le(bytes, 24)),
        40..=124 => {
            if bytes.len() < 30 {
                return false;
            }
            (uint16_le(bytes, 26), uint16_le(bytes, 28))
        }
        _ => return false,
    };
    planes == 1 && matches!(depth, 1 | 4 | 8 | 16 | 24 | 32)
}

fn uint16_le(bytes: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes([bytes[offset], bytes[offset + 1]])
}

fn read_head(path: &Path, limit: usize) -> Option<Vec<u8>> {
    use std::io::Read;
    let mut file = std::fs::File::open(path).ok()?;
    let mut head = vec![0u8; limit];
    let read = file.read(&mut head).ok()?;
    head.truncate(read);
    Some(head)
}

/// The image format a file holds. Its bytes decide first; the extension answers
/// only for a format the sniff does not know but the OS image tools can still
/// turn into one a provider takes, so a TIFF or a HEIC is converted rather than
/// refused.
pub fn image_mime_of(path: &Path) -> Option<&'static str> {
    if let Some(mime) = read_head(path, SNIFF_BYTES)
        .as_deref()
        .and_then(sniff_image_mime)
    {
        return Some(mime);
    }
    convertible_image_mime(path)
}

/// A format a provider does not take but the OS image tools can convert.
fn convertible_image_mime(path: &Path) -> Option<&'static str> {
    match path.extension()?.to_str()?.to_ascii_lowercase().as_str() {
        "tif" | "tiff" => Some("image/tiff"),
        "heic" => Some("image/heic"),
        "heif" => Some("image/heif"),
        "avif" => Some("image/avif"),
        _ => None,
    }
}

pub fn is_image_path(path: &Path) -> bool {
    image_mime_of(path).is_some()
}

/// A PDF by its extension or by the header the format begins with, so one that
/// was saved without a `.pdf` still attaches as a document.
pub fn is_pdf_path(path: &Path) -> bool {
    if path
        .extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("pdf"))
    {
        return true;
    }
    read_head(path, 5).is_some_and(|head| head == b"%PDF-")
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

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn image_extension(mime: &str) -> &str {
    match mime {
        "image/jpeg" => "jpg",
        "image/webp" => "webp",
        _ => "png",
    }
}

/// A private temporary directory for one image conversion. The name is random
/// and the mode is owner-only, and it is removed on drop, so a predictable-name
/// symlink cannot redirect the bytes written or read during a conversion.
#[cfg(any(target_os = "macos", target_os = "linux"))]
struct TempImageDir(PathBuf);

#[cfg(any(target_os = "macos", target_os = "linux"))]
impl TempImageDir {
    fn new() -> Option<Self> {
        let mut random = [0u8; 16];
        getrandom::getrandom(&mut random).ok()?;
        let name: String = random.iter().map(|byte| format!("{byte:02x}")).collect();
        let dir = std::env::temp_dir().join(format!("oxide-image-{name}"));
        let mut builder = std::fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder.create(&dir).ok()?;
        Some(Self(dir))
    }

    fn path(&self) -> &Path {
        &self.0
    }

    /// Writes a file that must not already exist, so an existing symlink is
    /// never followed.
    fn write(&self, name: &str, bytes: &[u8]) -> Option<PathBuf> {
        use std::io::Write;
        let path = self.0.join(name);
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .ok()?;
        file.write_all(bytes).ok()?;
        Some(path)
    }
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
impl Drop for TempImageDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[cfg(target_os = "macos")]
fn downscale_image(bytes: &[u8], mime: &str) -> Option<Vec<u8>> {
    let dir = TempImageDir::new()?;
    let extension = image_extension(mime);
    let input = dir.write(&format!("input.{extension}"), bytes)?;
    let output = dir.path().join(format!("output.{extension}"));
    let status = Command::new("sips")
        .args(["-Z", &MAX_IMAGE_EDGE.to_string()])
        .arg(&input)
        .arg("--out")
        .arg(&output)
        .output()
        .ok()?;
    status
        .status
        .success()
        .then(|| std::fs::read(&output).ok())
        .flatten()
}

#[cfg(target_os = "linux")]
fn downscale_image(bytes: &[u8], mime: &str) -> Option<Vec<u8>> {
    let dir = TempImageDir::new()?;
    let extension = image_extension(mime);
    let input = dir.write(&format!("input.{extension}"), bytes)?;
    let output = dir.path().join(format!("output.{extension}"));
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
    resized.then(|| std::fs::read(&output).ok()).flatten()
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

/// The extension an input image is written with before the OS tools read it —
/// `sips` and ImageMagick both go by the name, and neither reads a file called
/// `input.img` as a HEIC.
#[cfg(any(target_os = "macos", target_os = "linux"))]
fn source_extension(mime: &str) -> &str {
    match mime {
        "image/bmp" => "bmp",
        "image/tiff" => "tiff",
        "image/heic" => "heic",
        "image/heif" => "heif",
        "image/avif" => "avif",
        _ => "img",
    }
}

#[cfg(target_os = "macos")]
fn convert_image_to_png(bytes: &[u8], mime: &str) -> Option<Vec<u8>> {
    let dir = TempImageDir::new()?;
    let input = dir.write(&format!("input.{}", source_extension(mime)), bytes)?;
    let output = dir.path().join("output.png");
    let status = Command::new("sips")
        .args(["-s", "format", "png"])
        .arg(&input)
        .arg("--out")
        .arg(&output)
        .output()
        .ok()?;
    status
        .status
        .success()
        .then(|| std::fs::read(&output).ok())
        .flatten()
}

#[cfg(target_os = "linux")]
fn convert_image_to_png(bytes: &[u8], mime: &str) -> Option<Vec<u8>> {
    let dir = TempImageDir::new()?;
    let input = dir.write(&format!("input.{}", source_extension(mime)), bytes)?;
    let output = dir.path().join("output.png");
    let input = input.to_str()?.to_string();
    let output = output.to_str()?.to_string();
    let converted = run_ok("magick", &[&input, &output])
        || run_ok("convert", &[&input, &output])
        || run_ok(
            "ffmpeg",
            &["-y", "-loglevel", "error", "-i", &input, &output],
        );
    converted.then(|| std::fs::read(&output).ok()).flatten()
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn convert_image_to_png(_bytes: &[u8], _mime: &str) -> Option<Vec<u8>> {
    None
}

/// Reads a file and turns it into an LLM-ready content part, the way Pi reads
/// one: an image (converted to a format a provider takes when it is not one
/// already) becomes an image part, a PDF becomes a document part, and anything
/// else becomes its own text. Nothing is refused for being the wrong type — a
/// file past [`MAX_ATTACHMENT_BYTES`] is refused before it is read, so a stray
/// 200 MB log cannot be multiplied through the session and the request, and a
/// file whose head holds a NUL byte is refused rather than sent as mojibake.
pub fn load_attachment(path: &Path) -> Result<ContentPart> {
    let size = std::fs::metadata(path)
        .with_context(|| format!("reading {}", path.display()))?
        .len();
    if size > MAX_ATTACHMENT_BYTES as u64 {
        anyhow::bail!(
            "{} is {} which is past the {} limit for an attachment",
            path.display(),
            human_bytes(size),
            human_bytes(MAX_ATTACHMENT_BYTES as u64)
        );
    }
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
    if let Some(mime) = image_mime_of(path) {
        if !is_supported_image_mime(mime) {
            // An image no provider takes as it stands — a TIFF, a HEIC — is
            // converted to one it does, rather than refused at the door.
            let png = convert_image_to_png(&bytes, mime).with_context(|| {
                format!(
                    "{} is a {mime} image, which no provider takes and no image tool on this machine could convert",
                    path.display()
                )
            })?;
            let png = optimize_image(png, "image/png");
            return Ok(ContentPart::ImageUrl {
                image_url: ImageUrl {
                    url: format!("data:image/png;base64,{}", base64_encode(&png)),
                    detail: None,
                },
            });
        }
        // GIF is left alone so an animation is not flattened to one frame.
        let bytes = if mime == "image/gif" {
            bytes
        } else {
            optimize_image(bytes, mime)
        };
        return Ok(ContentPart::ImageUrl {
            image_url: ImageUrl {
                url: format!("data:{mime};base64,{}", base64_encode(&bytes)),
                detail: None,
            },
        });
    }
    text_attachment(path, &bytes)
}

/// Everything that is neither an image nor a PDF is attached as its text,
/// wrapped the way Pi wraps one, so a `.csv`, a `.json` or a source file can be
/// sent without a format list deciding what may travel.
fn text_attachment(path: &Path, bytes: &[u8]) -> Result<ContentPart> {
    if bytes.iter().take(SNIFF_BYTES).any(|byte| *byte == 0) {
        anyhow::bail!(
            "{} is not an image, a PDF or a text file, so there is nothing to attach",
            path.display()
        );
    }
    Ok(ContentPart::Text {
        text: file_text(&path.display().to_string(), bytes),
    })
}

/// Pi's own wrapper around an attached file's text.
fn file_text(name: &str, bytes: &[u8]) -> String {
    format!(
        "<file name=\"{name}\">\n{}\n</file>",
        String::from_utf8_lossy(bytes)
    )
}

/// Extracts `@path` references from free-form input that point at existing
/// attachment files (an image or a PDF — what the CLI reads as more than text).
/// The input text is left untouched.
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
        ContentPart::Text { text } => file_name_of(text).unwrap_or_else(|| "text".to_string()),
    }
}

/// What a part is, in the words a tool result names it with: the image's own
/// format, `pdf`, or `text`.
pub fn part_kind(part: &ContentPart) -> &str {
    match part {
        ContentPart::ImageUrl { image_url } => {
            data_url_media_type(&image_url.url).unwrap_or("image")
        }
        ContentPart::File { .. } => "pdf",
        ContentPart::Text { .. } => "text",
    }
}

/// The name inside the wrapper [`file_text`] writes.
fn file_name_of(text: &str) -> Option<String> {
    let rest = text.strip_prefix("<file name=\"")?;
    Some(rest.split('"').next()?.to_string())
}

fn data_url_media_type(url: &str) -> Option<&str> {
    let meta = url
        .strip_prefix("data:")?
        .split_once(',')
        .map(|(meta, _)| meta)?;
    Some(meta.strip_suffix(";base64").unwrap_or(meta))
}

/// A size for a message a person reads: `18 MB`, `900 KB`.
fn human_bytes(bytes: u64) -> String {
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    if bytes < 1024 * 1024 {
        return format!("{} KB", bytes / 1024);
    }
    format!("{} MB", bytes / (1024 * 1024))
}

/// Builds a content part from an already-encoded data URL, e.g. an image the
/// desktop read from the clipboard: a PDF or any other text becomes its own
/// part, and an image is downscaled (and converted, where it is not a format a
/// provider takes) here — the one place a data URL can be — so a full-resolution
/// paste is not stored or shipped at full size just because it never touched a
/// path. Returns `None` for a payload past [`MAX_ATTACHMENT_BYTES`], one that is
/// not base64, a binary payload that is not an image, or an image the OS image
/// tools could not decode.
pub fn content_part_from_data_url(
    data_url: String,
    filename: Option<String>,
) -> Option<ContentPart> {
    let media_type = data_url_media_type(&data_url)?;
    if data_url.len() > MAX_DATA_URL_CHARS {
        return None;
    }
    if media_type == "application/pdf" {
        return Some(ContentPart::File {
            file: FileData {
                filename,
                file_data: data_url,
            },
        });
    }
    let image = media_type.starts_with("image/");
    let payload = data_url.split_once(',')?.1;
    let bytes = base64_decode(payload)?;
    // The decoded length is what the limit is about, and it is in hand here for
    // nothing.
    if bytes.len() > MAX_ATTACHMENT_BYTES {
        return None;
    }
    if !image {
        // Anything that is not media travels as its text, the same shape a file
        // attaches with; a payload holding a NUL is not text.
        if bytes.iter().take(SNIFF_BYTES).any(|byte| *byte == 0) {
            return None;
        }
        let name = filename.unwrap_or_else(|| "file".to_string());
        return Some(ContentPart::Text {
            text: file_text(&name, &bytes),
        });
    }
    if !is_supported_image_mime(media_type) {
        // An image no provider takes as it stands is converted to one it does,
        // the way Pi converts one, instead of being refused.
        let png = optimize_image(convert_image_to_png(&bytes, media_type)?, "image/png");
        return Some(ContentPart::ImageUrl {
            image_url: ImageUrl {
                url: format!("data:image/png;base64,{}", base64_encode(&png)),
                detail: None,
            },
        });
    }
    // A GIF is left alone so an animation is not flattened to one frame.
    if media_type == "image/gif" {
        return Some(ContentPart::ImageUrl {
            image_url: ImageUrl {
                url: data_url,
                detail: None,
            },
        });
    }
    let bytes = optimize_image(bytes, media_type);
    Some(ContentPart::ImageUrl {
        image_url: ImageUrl {
            url: format!("data:{media_type};base64,{}", base64_encode(&bytes)),
            detail: None,
        },
    })
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

/// The attachment the clipboard holds, for a front-end whose paste has no file
/// to go with it.
///
/// A copy from the Finder is preferred over its picture: the pasteboard also
/// carries the copied file's *icon*, so grabbing the PNG-flavoured data would
/// attach a placeholder image of the file instead of the file — a copied
/// screenshot arrived as a `PNG`-document icon. The file is read the way any
/// other attachment is, so a copied text file rides along as its own text; one
/// that is here but cannot be attached — past the size limit, or a binary that
/// is neither media nor text — is reported rather than replaced by that icon.
pub fn clipboard_attachment() -> Option<ContentPart> {
    if let Some(path) = clipboard_file(clipboard_path()) {
        return load_attachment(&path).ok();
    }
    clipboard_image()
}

/// The file a clipboard copy names, when this machine can look at it.
///
/// A copy made on another machine leaves its file URL on the pasteboard while
/// the file itself stays where it was — a screenshot or a Finder copy sent over
/// by Handoff, pasted into a terminal on the machine that received it. The path
/// names nothing here, so it is not the file that was copied but a reference to
/// something this machine does not have; the pasteboard's own data is what the
/// front-end attaches instead of reporting that there is nothing to attach.
fn clipboard_file(path: Option<PathBuf>) -> Option<PathBuf> {
    path.filter(|path| path.exists())
}

/// Best-effort clipboard image grab. On macOS this asks the pasteboard through
/// AppKit for the type it advertises, then takes `pngpaste`'s answer when it is
/// installed and coerces the clipboard with `osascript` when that finds none; on
/// Linux it needs `wl-paste` or `xclip`.
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

/// The file the clipboard holds, when a copy put a file URL on it. macOS only:
/// a copy from the Finder also puts the file's own icon on the pasteboard,
/// which is the picture the grab below would take. A Linux or Windows file copy
/// carries no picture for it to mistake for the file, so the grab stays the
/// whole story there.
///
/// The URL is read out of `NSPasteboard`, not by coercing the clipboard to a
/// file URL. AppleScript's `the clipboard as «class furl»` turns *any* text on
/// the clipboard into a path (`/plain text`, `/https/::host/photo.png`) and
/// only looks at the pasteboard's first item, so an icon sitting ahead of the
/// file URL failed the coercion and the grab below attached that icon. Reading
/// the file URLs through AppKit finds the real file wherever it sits and answers
/// nothing at all when there is none, so the image grab is only reached for a
/// genuine picture.
#[cfg(target_os = "macos")]
fn clipboard_path() -> Option<PathBuf> {
    const SCRIPT: &str = r#"use framework "AppKit"
set pb to current application's NSPasteboard's generalPasteboard()
set urls to pb's readObjectsForClasses:{current application's NSURL} options:(missing value)
if urls is missing value then return ""
repeat with u in urls
if (u's isFileURL()) as boolean then return (u's |path|() as text)
end repeat
return """#;
    let output = Command::new("osascript")
        .args(["-e", SCRIPT])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    clipboard_path_from(&String::from_utf8_lossy(&output.stdout))
}

#[cfg(not(target_os = "macos"))]
fn clipboard_path() -> Option<PathBuf> {
    None
}

/// A clipboard file URL as a path. Existence is left to the caller: a file that
/// has since moved should still be reported as the file that was copied rather
/// than falling through to the pasteboard's icon of it.
#[cfg(target_os = "macos")]
fn clipboard_path_from(path: &str) -> Option<PathBuf> {
    let path = path.trim();
    if path.is_empty() || !path.starts_with('/') {
        return None;
    }
    Some(PathBuf::from(path))
}

#[cfg(target_os = "macos")]
fn clipboard_bytes() -> Option<Vec<u8>> {
    clipboard_bytes_appkit()
        .or_else(|| run_stdout("pngpaste", &["-"]))
        .or_else(clipboard_bytes_osascript)
}

/// The pasteboard's image as AppKit hands it over: the item is asked for the
/// type it advertises — `public.png`, then the `public.tiff` a copy from
/// Preview or Safari leaves — instead of coercing it the way the AppleScript
/// below does, which reads one item and guesses. Asking the item is what
/// `pngpaste` does, and it is the read that is worth making on a pasteboard
/// whose bytes are not on this machine yet: a copy that arrived from another one
/// over the network is fetched by the pasteboard server for it.
///
/// It is asked first, so the read this crate controls decides what is on the
/// pasteboard — `pngpaste` and the coercion are then answers for a machine
/// where that read found nothing, rather than gates the new types have to get
/// past.
///
/// A TIFF is turned into a PNG, which Oxide can send as rich media.
#[cfg(target_os = "macos")]
fn clipboard_bytes_appkit() -> Option<Vec<u8>> {
    let dir = TempImageDir::new()?;
    let script = format!(
        "use framework \"AppKit\"\n\
         set pb to current application's NSPasteboard's generalPasteboard()\n\
         set png to pb's dataForType:\"public.png\"\n\
         if png is not missing value then\n\
         if (png's writeToFile:\"{}/clipboard.png\" atomically:true) as boolean then return \"public.png\"\n\
         end if\n\
         set tiff to pb's dataForType:\"public.tiff\"\n\
         if tiff is not missing value then\n\
         if (tiff's writeToFile:\"{}/clipboard.tiff\" atomically:true) as boolean then return \"public.tiff\"\n\
         end if\n\
         return \"\"",
        dir.path().display(),
        dir.path().display()
    );
    let output = Command::new("osascript")
        .args(["-e", &script])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let source = dir.path().join(appkit_output_name(&String::from_utf8_lossy(
        &output.stdout,
    ))?);
    if source.extension().and_then(|ext| ext.to_str()) == Some("png") {
        return std::fs::read(&source).ok();
    }
    let converted = dir.path().join("converted.png");
    let status = Command::new("sips")
        .args(["-s", "format", "png"])
        .arg(&source)
        .arg("--out")
        .arg(&converted)
        .output()
        .ok()?;
    status
        .status
        .success()
        .then(|| std::fs::read(&converted).ok())
        .flatten()
}

/// The representation the AppKit read wrote out, from the type it reported:
/// anything else is a pasteboard with no image on it.
#[cfg(target_os = "macos")]
fn appkit_output_name(uti: &str) -> Option<&'static str> {
    match uti.trim() {
        "public.png" => Some("clipboard.png"),
        "public.tiff" => Some("clipboard.tiff"),
        _ => None,
    }
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
    fn reads_an_image_format_from_the_bytes() {
        assert_eq!(sniff_image_mime(ONE_PX_PNG), Some("image/png"));
        assert_eq!(sniff_image_mime(b"GIF89a"), Some("image/gif"));
        assert_eq!(sniff_image_mime(b"GIF87a"), Some("image/gif"));
        assert_eq!(
            sniff_image_mime(&[0xFF, 0xD8, 0xFF, 0xE0]),
            Some("image/jpeg")
        );
        // A JPEG-LS is a shape no provider takes.
        assert_eq!(sniff_image_mime(&[0xFF, 0xD8, 0xFF, 0xF7]), None);
        assert_eq!(
            sniff_image_mime(b"RIFF\x00\x00\x00\x00WEBPVP8 "),
            Some("image/webp")
        );
        assert_eq!(sniff_image_mime(&bmp_fixture()), Some("image/bmp"));
        assert_eq!(sniff_image_mime(b"# a text file named .png"), None);
    }

    /// An animated PNG is not sent as a still one: its `acTL` chunk sits
    /// between the header and the image data.
    #[test]
    fn an_animated_or_malformed_png_is_not_an_image() {
        assert_eq!(sniff_image_mime(&animated_png()), None);
        let mut truncated = ONE_PX_PNG.to_vec();
        truncated[8..12].copy_from_slice(&12u32.to_be_bytes());
        assert_eq!(sniff_image_mime(&truncated), None);
    }

    /// Pi's own normalized set: what a provider takes as it stands. Everything
    /// else sniffed as an image — a BMP included, which this sniff still
    /// recognizes — is converted to PNG rather than sent.
    #[test]
    fn a_provider_takes_pi_normalized_formats_as_they_stand() {
        for mime in ["image/png", "image/jpeg", "image/gif", "image/webp"] {
            assert!(
                is_supported_image_mime(mime),
                "{mime} should travel as it is"
            );
        }
        for mime in ["image/bmp", "image/tiff", "image/heic", "image/avif"] {
            assert!(!is_supported_image_mime(mime), "{mime} should convert");
        }
    }

    fn animated_png() -> Vec<u8> {
        let mut png = ONE_PX_PNG.to_vec();
        let ac_tl: &[u8] = b"\x00\x00\x00\x08acTL\x00\x00\x00\x01\x00\x00\x00\x02\x00\x00\x00\x00";
        png.splice(33..33, ac_tl.iter().copied());
        png
    }

    /// A 1x1 24-bit BMP, header and all.
    fn bmp_fixture() -> Vec<u8> {
        let mut bmp = vec![0u8; 54];
        bmp[..2].copy_from_slice(b"BM");
        bmp[2..6].copy_from_slice(&60u32.to_le_bytes());
        bmp[10..14].copy_from_slice(&54u32.to_le_bytes());
        bmp[14..18].copy_from_slice(&40u32.to_le_bytes());
        bmp[26..28].copy_from_slice(&1u16.to_le_bytes());
        bmp[28..30].copy_from_slice(&24u16.to_le_bytes());
        bmp
    }

    #[test]
    fn an_attachment_is_decided_by_the_bytes_rather_than_the_name() {
        let dir = std::env::temp_dir().join(format!("oxide_media_kind_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();

        // A screenshot saved without an extension is an image.
        let bare = dir.join("screenshot");
        std::fs::write(&bare, ONE_PX_PNG).unwrap();
        assert_eq!(image_mime_of(&bare), Some("image/png"));

        // A text file named `.png` is not.
        let liar = dir.join("notes.png");
        std::fs::write(&liar, b"# notes").unwrap();
        assert_eq!(image_mime_of(&liar), None);
        assert!(!is_attachment_path(&liar));

        // A format the sniff does not know is still an image here, because the
        // OS image tools can convert it.
        let scan = dir.join("scan.tif");
        std::fs::write(&scan, b"II*\x00").unwrap();
        assert_eq!(image_mime_of(&scan), Some("image/tiff"));
        assert!(is_attachment_path(&scan));

        // A PDF is recognised by its header too.
        let spec = dir.join("spec");
        std::fs::write(&spec, b"%PDF-1.7\n...").unwrap();
        assert!(is_pdf_path(&spec));
        assert!(is_pdf_path(Path::new("docs/SPEC.PDF")));

        std::fs::remove_dir_all(&dir).ok();
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
        std::fs::write(&file, ONE_PX_PNG).unwrap();

        let part = load_attachment(&file).unwrap();
        match part {
            ContentPart::ImageUrl { image_url } => {
                assert_eq!(
                    image_url.url,
                    format!("data:image/png;base64,{}", base64_encode(ONE_PX_PNG))
                );
            }
            other => panic!("unexpected part: {other:?}"),
        }

        std::fs::remove_dir_all(&dir).ok();
    }

    /// An image no provider takes as it stands is converted rather than refused:
    /// a TIFF the system tool turns into a PNG travels as that PNG. The fixture
    /// is written by the same tool, so a machine whose `sips` cannot make one is
    /// a failure rather than a silent pass.
    #[cfg(target_os = "macos")]
    #[test]
    fn converts_an_image_a_provider_will_not_take() {
        let dir = std::env::temp_dir().join(format!("oxide_media_convert_{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        let png = dir.join("in.png");
        let large = dir.join("large.png");
        let tiff = dir.join("scan.tiff");
        std::fs::write(&png, ONE_PX_PNG).unwrap();
        for args in [
            vec![
                "-Z",
                "200",
                png.to_str().unwrap(),
                "--out",
                large.to_str().unwrap(),
            ],
            vec![
                "-s",
                "format",
                "tiff",
                large.to_str().unwrap(),
                "--out",
                tiff.to_str().unwrap(),
            ],
        ] {
            let out = Command::new("sips")
                .args(&args)
                .output()
                .expect("sips is built into macOS");
            assert!(
                out.status.success(),
                "sips {args:?} failed: {}",
                String::from_utf8_lossy(&out.stderr)
            );
        }

        assert_eq!(image_mime_of(&tiff), Some("image/tiff"));
        let part = load_attachment(&tiff).expect("a TIFF is converted, not refused");
        match &part {
            ContentPart::ImageUrl { image_url } => {
                assert!(
                    image_url.url.starts_with("data:image/png;base64,"),
                    "{}",
                    image_url.url
                );
                let bytes =
                    base64_decode(image_url.url.trim_start_matches("data:image/png;base64,"))
                        .expect("the payload decodes");
                assert_eq!(png_dimensions(&bytes), Some((200, 200)));
            }
            other => panic!("a TIFF should travel as an image, got {other:?}"),
        }
        assert_eq!(part_kind(&part), "image/png");

        std::fs::remove_dir_all(&dir).ok();
    }

    /// Anything that is neither an image nor a PDF is attached as its text,
    /// wrapped the way Pi wraps one, so no format list decides what may travel.
    #[test]
    fn loads_any_other_file_as_its_text() {
        let dir = std::env::temp_dir().join(format!("oxide_media_text_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("rows.csv");
        std::fs::write(&file, b"a,b\n1,2\n").unwrap();

        match load_attachment(&file).unwrap() {
            ContentPart::Text { text } => {
                assert!(text.starts_with(&format!("<file name=\"{}\">", file.display())));
                assert!(text.contains("a,b\n1,2"));
                assert!(text.ends_with("</file>"));
            }
            other => panic!("unexpected part: {other:?}"),
        }

        // A binary file is refused rather than sent as mojibake.
        let binary = dir.join("blob.bin");
        std::fs::write(&binary, [0u8, 1, 2, 3]).unwrap();
        let error = load_attachment(&binary).unwrap_err().to_string();
        assert!(
            error.contains("nothing to attach"),
            "unexpected error: {error}"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn extracts_referenced_attachments() {
        let dir = std::env::temp_dir().join(format!("oxide_media_ref_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("shot.png"), ONE_PX_PNG).unwrap();
        std::fs::write(dir.join("notes.txt"), b"x").unwrap();

        let found = referenced_attachments("look at @shot.png and @notes.txt please", &dir);
        assert_eq!(found.len(), 1);
        assert!(found[0].ends_with("shot.png"));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn reads_a_copied_file_out_of_the_clipboard_url() {
        assert_eq!(
            clipboard_path_from("  /Users/me/Desktop/My Shot.png\n"),
            Some(PathBuf::from("/Users/me/Desktop/My Shot.png"))
        );
        // Plain text on the clipboard is not a file URL, and the folder a
        // Finder copy can hold is left to fail as an attachment rather than
        // falling through to the pasteboard's icon of it.
        assert_eq!(clipboard_path_from("hello there"), None);
        assert_eq!(clipboard_path_from("file:///Users/me/shot.png"), None);
        assert_eq!(clipboard_path_from("   "), None);
    }

    #[test]
    fn a_clipboard_file_is_only_taken_from_this_machine() {
        let dir = std::env::temp_dir().join(format!("oxide_media_clip_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("shot.png");
        std::fs::write(&file, b"f").unwrap();

        assert_eq!(clipboard_file(Some(file.clone())), Some(file));
        assert_eq!(clipboard_file(None), None);
        // A copy from another machine leaves a path that is not here behind,
        // and the pasteboard's own image is what to attach instead of giving up
        // on the paste.
        assert_eq!(
            clipboard_file(Some(PathBuf::from("/oxide/nothing/was/copied.png"))),
            None
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn takes_the_representation_appkit_wrote_out() {
        assert_eq!(appkit_output_name("public.png\n"), Some("clipboard.png"));
        assert_eq!(appkit_output_name("public.tiff"), Some("clipboard.tiff"));
        // A pasteboard with no image on it, or an answer nothing recognises, is
        // left to the coercing read below rather than read as a file.
        assert_eq!(appkit_output_name(""), None);
        assert_eq!(appkit_output_name("public.utf8-plain-text"), None);
    }

    #[test]
    fn decodes_base64_back_to_bytes() {
        for value in [
            Vec::new(),
            b"f".to_vec(),
            b"fo".to_vec(),
            b"foo".to_vec(),
            b"foob".to_vec(),
            b"fooba".to_vec(),
            b"foobar".to_vec(),
            (0..=255u8).collect(),
        ] {
            let encoded = base64_encode(&value);
            assert_eq!(base64_decode(&encoded), Some(value.clone()));
        }
        assert_eq!(base64_decode("Zm9v\nYmFy"), Some(b"foobar".to_vec()));
        assert_eq!(base64_decode("!!!!"), None);
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

        match content_part_from_data_url(
            "data:text/csv;base64,YSxiCg==".to_string(),
            Some("rows.csv".to_string()),
        ) {
            Some(ContentPart::Text { text }) => {
                assert_eq!(text, "<file name=\"rows.csv\">\na,b\n\n</file>");
            }
            other => panic!("unexpected part: {other:?}"),
        }

        // A payload that is not text is refused rather than shipped as mojibake.
        assert!(
            content_part_from_data_url("data:text/plain;base64,AAAA".to_string(), None).is_none()
        );
        // An image the OS tools cannot decode either is refused rather than
        // stored as a thumbnail that cannot be drawn.
        assert!(content_part_from_data_url(
            "data:image/tiff;base64,AAAA".to_string(),
            Some("scan.tif".to_string())
        )
        .is_none());
        assert!(
            content_part_from_data_url("data:image/png;base64,!!!!".to_string(), None).is_none()
        );
        assert!(content_part_from_data_url("not a data url".to_string(), None).is_none());
    }

    #[test]
    fn refuses_a_data_url_past_the_attachment_limit() {
        let payload = "A".repeat(MAX_DATA_URL_CHARS);
        let data_url = format!("data:application/pdf;base64,{payload}");
        assert!(content_part_from_data_url(data_url, None).is_none());
    }

    #[test]
    fn refuses_a_file_past_the_attachment_limit() {
        let dir = std::env::temp_dir().join(format!("oxide_media_limit_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("huge.pdf");
        // Sparse, so the test does not write 20 MB to disk.
        std::fs::File::create(&file)
            .unwrap()
            .set_len(MAX_ATTACHMENT_BYTES as u64 + 1)
            .unwrap();

        let error = load_attachment(&file).unwrap_err().to_string();
        assert!(error.contains("20 MB"), "unexpected error: {error}");

        std::fs::remove_dir_all(&dir).ok();
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
