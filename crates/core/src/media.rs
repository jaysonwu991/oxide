//! Multimodal attachments: reading any file a message carries into LLM-ready
//! content parts — an image (converted to a format a provider takes, where the
//! machine has the tools to), a PDF, or any other text — plus dependency-free
//! base64 encoding, `@path` reference extraction, and a best-effort OS
//! clipboard grab.
use crate::llm::{ContentPart, FileData, ImageUrl};
use anyhow::Result;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
#[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
use std::process::Command;
#[cfg(target_os = "macos")]
use std::sync::OnceLock;

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

/// How long a pasted picture that came with no file name of its own is kept.
/// Pi writes its pasteboard grabs to the temp directory; this keeps them beside
/// the rest of the run's scratch under the config dir, and prunes them the way
/// truncated tool output is pruned.
const CLIPBOARD_RETENTION_SECS: u64 = 7 * 24 * 60 * 60;

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

/// The image format a file holds, or `None` when its bytes are not one. The
/// bytes decide: the name is never consulted, so a text file called `notes.tif`
/// is not an image and a screenshot saved without an extension is.
pub fn image_mime_of(path: &Path) -> Option<&'static str> {
    image_mime_of_bytes(&read_head(path, SNIFF_BYTES)?)
}

/// The image format a buffer holds: Pi's sniffed set, or a format only the OS
/// image tools can convert, recognized by its own signature.
fn image_mime_of_bytes(head: &[u8]) -> Option<&'static str> {
    sniff_image_mime(head).or_else(|| convertible_image_mime(head))
}

/// A format a provider does not take but the OS image tools can convert — a
/// TIFF's byte-order mark, or an ISO base media file whose brand is a HEIC, a
/// HEIF or an AVIF. Read from the bytes like every other format, so a suffix
/// alone cannot claim a file is an image.
fn convertible_image_mime(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(b"II\x2a\x00") || bytes.starts_with(b"MM\x00\x2a") {
        return Some("image/tiff");
    }
    if bytes.get(4..8) != Some(b"ftyp".as_slice()) {
        return None;
    }
    match bytes.get(8..12)? {
        b"heic" | b"heix" | b"hevc" | b"hevx" | b"heim" | b"heis" | b"hevm" | b"hevs" => {
            Some("image/heic")
        }
        b"mif1" | b"msf1" => Some("image/heif"),
        b"avif" | b"avis" => Some("image/avif"),
        _ => None,
    }
}

pub fn is_image_path(path: &Path) -> bool {
    image_mime_of(path).is_some()
}

/// A PDF is the one attachment its header alone names, so the bytes decide it
/// too and a text file called `.pdf` is attached as its own text.
pub fn is_pdf_path(path: &Path) -> bool {
    read_head(path, PDF_MAGIC.len()).is_some_and(|head| head == PDF_MAGIC)
}

/// Whether a path is one an attachment reads from, which is what a front-end
/// asks before routing a file as media. Its bytes decide — a `.tif` holding
/// text is not an image and a `.pdf` holding text is not a document — and a file
/// whose bytes cannot be read at all is not media here, since nothing names it:
/// `references_attachment` is the door for a reference, because a reference
/// whose file cannot be read is the one that has to report the refusal rather
/// than be passed over.
pub fn is_attachment_path(path: &Path) -> bool {
    read_head(path, SNIFF_BYTES).is_some_and(|head| head_is_media(&head))
}

/// Whether a buffer's own bytes name something the app attaches: an image a
/// provider takes, one the OS image tools convert, or a PDF.
fn head_is_media(head: &[u8]) -> bool {
    image_mime_of_bytes(head).is_some() || head.starts_with(PDF_MAGIC)
}

/// Whether a reference names an attachment the app has to read: its bytes say
/// media, or — the one case with no bytes to go on — its head cannot be read at
/// all and its name claims a format the app attaches. A file whose bytes *are*
/// read and name nothing is its own text, whatever it is called.
fn references_attachment(path: &Path) -> bool {
    match read_head(path, SNIFF_BYTES) {
        Some(head) => head_is_media(&head),
        None => name_claims_media(path),
    }
}

/// Whether a path's name claims a format the app attaches, which is all there
/// is to decide a file whose head cannot be read. The set is the sniff's own —
/// a TIFF and a HEIC are media the OS image tools convert — plus the other
/// spelling of each.
fn name_claims_media(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    let Some((stem, extension)) = name.rsplit_once('.') else {
        return false;
    };
    if stem.is_empty() {
        return false;
    }
    matches!(
        extension.to_ascii_lowercase().as_str(),
        "png"
            | "jpg"
            | "jpeg"
            | "gif"
            | "webp"
            | "bmp"
            | "tif"
            | "tiff"
            | "heic"
            | "heif"
            | "avif"
            | "pdf"
    )
}

/// Long edge above which an image is downscaled before it is base64-encoded.
/// Matches Anthropic's recommended maximum, so a screenshot is not shipped far
/// larger than any provider will actually read.
const MAX_IMAGE_EDGE: u32 = 1568;
/// Images smaller than this are passed through without spawning an image tool.
const IMAGE_OPTIMIZE_MIN_BYTES: usize = 200_000;

/// The header every PDF begins with, which is what decides one — a `.pdf` name
/// says nothing about what a file holds, and neither does any other name.
const PDF_MAGIC: &[u8] = b"%PDF-";

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

/// A private temporary directory for one image conversion. The name is random,
/// the mode is owner-only on the platforms that have one, and it is removed on
/// drop, so a predictable-name symlink cannot redirect the bytes written or read
/// during a conversion.
#[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
struct TempImageDir(PathBuf);

#[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
impl TempImageDir {
    fn new() -> Option<Self> {
        let mut random = [0u8; 16];
        getrandom::getrandom(&mut random).ok()?;
        let name: String = random.iter().map(|byte| format!("{byte:02x}")).collect();
        let dir = std::env::temp_dir().join(format!("oxide-image-{name}"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            std::fs::DirBuilder::new().mode(0o700).create(&dir).ok()?;
        }
        #[cfg(not(unix))]
        std::fs::DirBuilder::new().create(&dir).ok()?;
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

#[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
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

#[cfg(any(target_os = "linux", target_os = "windows"))]
fn run_ok(command: &str, args: &[&str]) -> bool {
    let mut process = Command::new(command);
    process.args(args);
    // A console program started from the window would otherwise flash a black
    // box of its own over the app on every conversion.
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        process.creation_flags(CREATE_NO_WINDOW);
    }
    process
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
}

/// Windows has no image tool of its own, so an oversized image travels with the
/// dimensions it was sent with; the front-ends downscale a paste before it is
/// sent, which is where a Windows-sized screenshot is bounded.
#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn downscale_image(_bytes: &[u8], _mime: &str) -> Option<Vec<u8>> {
    None
}

/// The extension an input image is written with before the OS tools read it —
/// `sips` and ImageMagick both go by the name, and neither reads a file called
/// `input.img` as a HEIC.
#[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
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

/// Windows converts with the imaging stack it already ships — the same bargain
/// as `sips` on macOS — so a BMP, a TIFF or a HEIC is a format a provider takes
/// by the time it is sent, and no image decoder is carried for it. `FromFile`
/// goes by the extension the input was written with, and a codec this machine
/// does not have (a HEIC without the Store's extension installed) fails rather
/// than yielding a wrong picture, which leaves the image to travel as it stands.
#[cfg(target_os = "windows")]
fn convert_image_to_png(bytes: &[u8], mime: &str) -> Option<Vec<u8>> {
    let dir = TempImageDir::new()?;
    let input = dir.write(&format!("input.{}", source_extension(mime)), bytes)?;
    let output = dir.path().join("output.png");
    let script = format!(
        "Add-Type -AssemblyName System.Drawing; \
         $image = [System.Drawing.Image]::FromFile('{}'); \
         try {{ $image.Save('{}', [System.Drawing.Imaging.ImageFormat]::Png) }} \
         finally {{ $image.Dispose() }}",
        power_shell_path(&input),
        power_shell_path(&output)
    );
    let converted = run_ok(
        "powershell",
        &["-NoProfile", "-NonInteractive", "-Command", &script],
    ) || run_ok(
        "pwsh",
        &["-NoProfile", "-NonInteractive", "-Command", &script],
    );
    converted.then(|| std::fs::read(&output).ok()).flatten()
}

/// A path written into a PowerShell single-quoted string, where a quote is
/// doubled to escape it.
#[cfg(target_os = "windows")]
fn power_shell_path(path: &Path) -> String {
    path.to_string_lossy().replace('\'', "''")
}

#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
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
        .map_err(|err| read_error(path, &err))?
        .len();
    if size > MAX_ATTACHMENT_BYTES as u64 {
        anyhow::bail!(
            "{} is {} which is past the {} limit for an attachment",
            path.display(),
            human_bytes(size),
            human_bytes(MAX_ATTACHMENT_BYTES as u64)
        );
    }
    let bytes = std::fs::read(path).map_err(|err| read_error(path, &err))?;
    part_from_bytes(&path.display().to_string(), bytes)
}

/// The reason a file could not be read, with a way out where the file itself
/// looks innocent. A platform can withhold a file the user can plainly see —
/// macOS keeps the Desktop, Documents and Downloads folders behind a per-app
/// grant, so a read there answers `Operation not permitted` while the file is
/// readable to its owner — and an errno on its own leaves the reader with
/// nothing to act on: the app is the thing that has to be allowed, so the
/// reason names the grant rather than only the refusal. Every read of a file a
/// front-end is about to attach or inline goes through here, so no door this
/// app takes to a file reports a bare errno.
pub(crate) fn read_error(path: &Path, err: &std::io::Error) -> anyhow::Error {
    let reason = format!("reading {}: {err}", path.display());
    if err.kind() != std::io::ErrorKind::PermissionDenied {
        return anyhow::anyhow!(reason);
    }
    anyhow::anyhow!("{reason}{}", permission_hint())
}

#[cfg(target_os = "macos")]
const PERMISSION_HINT: &str = " — this app may not read that file: macOS keeps the Desktop, Documents and Downloads folders behind a per-app grant, so allow this app access under System Settings → Privacy & Security → Files and Folders, or copy the file into the project";

#[cfg(not(target_os = "macos"))]
const PERMISSION_HINT: &str =
    " — this app may not read that file: check its permissions, or copy it into the project";

/// The way out of a refused read: the platform's own wording, with the app the
/// grant belongs to named when it can be.
fn permission_hint() -> String {
    permission_hint_for(responsible_app())
}

/// [`permission_hint`] with the app already resolved, so the wording is
/// testable without a process tree in the way. macOS answers a file access as
/// the *responsible* app rather than as the process asking, so the grant is
/// that app's to give; naming it turns a hint into something to act on.
fn permission_hint_for(app: Option<String>) -> String {
    match app {
        Some(app) => format!(
            " — this app may not read that file: macOS keeps the Desktop, Documents and Downloads folders behind a per-app grant, and the grant is for the app this run was started from — {app} — so allow {app} under System Settings → Privacy & Security → Files and Folders, or copy the file into the project"
        ),
        None => PERMISSION_HINT.to_string(),
    }
}

/// How far up the process tree [`responsible_bundle`] looks for an app.
#[cfg(target_os = "macos")]
const MAX_ANCESTORS: usize = 16;

/// The application whose grant macOS consults for this run's reads.
///
/// TCC answers an access as the process' *responsible* app — the app it was
/// started from, not the binary asking, which is why the system's own log
/// records a read from `~/.local/bin/oxide` as
/// `responsible=/Applications/Visual Studio Code.app` — so the Files and
/// Folders list holds that app and never `oxide`, and a hint that says "this
/// app" sends the reader looking for a name that is not there. The nearest
/// ancestor inside an application bundle is that app, named the way its own
/// bundle names itself. Read once per run: the app that started this process is
/// not going to change.
#[cfg(target_os = "macos")]
fn responsible_app() -> Option<String> {
    static APP: OnceLock<Option<String>> = OnceLock::new();
    APP.get_or_init(|| {
        let table = Command::new("/bin/ps")
            .args(["-axo", "pid=,ppid=,comm="])
            .output()
            .ok()
            .filter(|output| output.status.success())?;
        let bundle =
            responsible_bundle(&String::from_utf8_lossy(&table.stdout), std::process::id())?;
        app_name(&bundle)
    })
    .clone()
}

/// A platform with no per-app grant behind a refusal names no app.
#[cfg(not(target_os = "macos"))]
fn responsible_app() -> Option<String> {
    None
}

/// The app above `start` in the table `/bin/ps -axo pid=,ppid=,comm=` prints:
/// the outermost bundle on the path of the nearest ancestor that is inside
/// one, since a helper lives inside the app it belongs to and it is that app
/// the platform names. A run whose ancestors are none of them an app — over
/// `ssh`, or detached — names nothing rather than guessing.
#[cfg(target_os = "macos")]
fn responsible_bundle(table: &str, start: u32) -> Option<PathBuf> {
    let mut pid = start;
    for _ in 0..MAX_ANCESTORS {
        let (path, parent) = process_row(table, pid)?;
        if let Some(app) = bundle_dir(path) {
            return Some(app);
        }
        if parent <= 1 || parent == pid {
            return None;
        }
        pid = parent;
    }
    None
}

/// One row of that table: the pid, the parent, then the executable's path —
/// which is the whole rest of the line, spaces and all.
#[cfg(target_os = "macos")]
fn process_row(table: &str, pid: u32) -> Option<(&str, u32)> {
    table.lines().find_map(|line| {
        let (row, rest) = line.trim_start().split_once(' ')?;
        if row.parse::<u32>().ok()? != pid {
            return None;
        }
        let (parent, path) = rest.trim_start().split_once(' ')?;
        Some((path, parent.parse::<u32>().ok()?))
    })
}

/// The bundle a path is inside, as the directory it sits in: the first `.app`
/// component with the bundle's `Contents` under it. Requiring `Contents` is what
/// keeps a directory that merely ends in `.app` from being reported as the app
/// to allow.
#[cfg(target_os = "macos")]
fn bundle_dir(path: &str) -> Option<PathBuf> {
    use std::ffi::OsStr;
    use std::path::Component;
    let mut components = Path::new(path).components();
    let mut dir = PathBuf::new();
    while let Some(component) = components.next() {
        dir.push(component);
        let Component::Normal(name) = component else {
            continue;
        };
        let name = name.to_string_lossy();
        if name.len() > ".app".len()
            && name.ends_with(".app")
            && components.clone().next() == Some(Component::Normal(OsStr::new("Contents")))
        {
            return Some(dir);
        }
    }
    None
}

/// What a bundle is called, the way the platform's own listing resolves it:
/// `CFBundleDisplayName` out of the bundle's own `Info.plist`, then
/// `CFBundleName`, then the directory it was left in. TCC reads those same two
/// keys out of that same dictionary, and keys a grant on the bundle's identity
/// rather than its path — so an app whose directory does not spell its name
/// (VS Code sits in `Visual Studio Code.app` and calls itself `Code`) is named
/// by what it says it is rather than by what the folder happens to be called.
#[cfg(target_os = "macos")]
fn app_name(bundle: &Path) -> Option<String> {
    let plist = bundle.join("Contents/Info.plist");
    ["CFBundleDisplayName", "CFBundleName"]
        .into_iter()
        .find_map(|key| plist_string(&plist, key))
        .or_else(|| {
            bundle
                .file_stem()
                .map(|stem| stem.to_string_lossy().into_owned())
        })
}

/// One string out of a plist, asked of the platform's own reader so an XML
/// plist and a binary one are read the same way here.
#[cfg(target_os = "macos")]
fn plist_string(plist: &Path, key: &str) -> Option<String> {
    let output = Command::new("/usr/bin/plutil")
        .args(["-extract", key, "raw", "-o", "-"])
        .arg(plist)
        .output()
        .ok()
        .filter(|output| output.status.success())?;
    let value = String::from_utf8_lossy(&output.stdout);
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_string())
}

/// The one place bytes become a part, so a file on disk and a data URL from a
/// front-end are read the same way: an image, a PDF, or the file's own text.
/// `name` is what the part and any refusal are named by — the path of a file,
/// or the file name a front-end sent.
fn part_from_bytes(name: &str, bytes: Vec<u8>) -> Result<ContentPart> {
    if let Some(mime) = image_mime_of_bytes(&bytes[..bytes.len().min(SNIFF_BYTES)]) {
        return Ok(image_part(bytes, mime));
    }
    if bytes.starts_with(PDF_MAGIC) {
        return Ok(ContentPart::File {
            file: FileData {
                filename: Path::new(name)
                    .file_name()
                    .map(|file| file.to_string_lossy().to_string()),
                file_data: format!("data:application/pdf;base64,{}", base64_encode(&bytes)),
            },
        });
    }
    text_attachment(name, &bytes)
}

/// An image part, converted when the format is one no provider takes.
fn image_part(bytes: Vec<u8>, mime: &'static str) -> ContentPart {
    // GIF is left alone so an animation is not flattened to one frame.
    if is_supported_image_mime(mime) {
        let bytes = if mime == "image/gif" {
            bytes
        } else {
            optimize_image(bytes, mime)
        };
        return image_part_of(bytes, mime);
    }
    match convert_image_to_png(&bytes, mime) {
        Some(png) => image_part_of(optimize_image(png, "image/png"), "image/png"),
        // No image tool on this machine — a Linux box without ImageMagick, or a
        // format whose codec is not installed — so the image travels as it
        // stands rather than being refused, and the provider that cannot take
        // it says so itself.
        None => image_part_of(bytes, mime),
    }
}

fn image_part_of(bytes: Vec<u8>, mime: &str) -> ContentPart {
    ContentPart::ImageUrl {
        image_url: ImageUrl {
            url: format!("data:{mime};base64,{}", base64_encode(&bytes)),
            detail: None,
        },
    }
}

/// Everything that is neither an image nor a PDF is attached as its text,
/// wrapped the way Pi wraps one, so a `.csv`, a `.json` or a source file can be
/// sent without a format list deciding what may travel.
fn text_attachment(name: &str, bytes: &[u8]) -> Result<ContentPart> {
    if bytes.iter().take(SNIFF_BYTES).any(|byte| *byte == 0) {
        anyhow::bail!(
            "{name} is not an image, a PDF or a text file, so there is nothing to attach"
        );
    }
    Ok(ContentPart::Text {
        text: file_text(name, bytes),
    })
}

/// Pi's own wrapper around an attached file's text.
fn file_text(name: &str, bytes: &[u8]) -> String {
    format!(
        "<file name=\"{name}\">\n{}\n</file>",
        String::from_utf8_lossy(bytes)
    )
}

/// Extracts `@path` references from free-form input that point at attachment
/// files (an image or a PDF — what the CLI reads as more than text). A file
/// whose head cannot be read is taken by its name, so a reference the app may
/// not read is reported by whoever attaches it rather than being left in the
/// message as the literal `@path` it was typed as. The input text is left
/// untouched.
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
        if path.is_file() && references_attachment(&path) {
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

/// A short human label for an attachment, shown in the `/attach` listing and
/// in the composer's pending rows: an image is named by the kind of thing it
/// is and its format, since one whose front-end holds no file name for it has
/// only those to go by — a clipboard paste the pasteboard named nothing for is
/// given a file of its own by [`clipboard`] and named by that path instead,
/// so this is the fallback for a paste whose bytes could not be written.
pub fn attachment_label(part: &ContentPart) -> String {
    match part {
        ContentPart::ImageUrl { image_url } => {
            let format = data_url_media_type(&image_url.url)
                .and_then(|media| media.rsplit('/').next().map(str::to_string));
            match format {
                Some(format) => format!("image ({format})"),
                None => "image".to_string(),
            }
        }
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
/// path. The decoded bytes decide what the payload is, exactly as they do for a
/// file: the media type a front-end wrote into the URL is not trusted over them.
/// Returns `None` for a payload past [`MAX_ATTACHMENT_BYTES`] or a payload that
/// is not base64 — what a front-end reports as a refusal rather than sending.
pub fn content_part_from_data_url(
    data_url: String,
    filename: Option<String>,
) -> Option<ContentPart> {
    if !data_url.starts_with("data:") || data_url.len() > MAX_DATA_URL_CHARS {
        return None;
    }
    let payload = data_url.split_once(',')?.1;
    let bytes = base64_decode(payload)?;
    // The decoded length is what the limit is about, and it is in hand here.
    if bytes.len() > MAX_ATTACHMENT_BYTES {
        return None;
    }
    let name = filename.unwrap_or_else(|| "file".to_string());
    part_from_bytes(&name, bytes).ok()
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

/// What a paste found on the clipboard, for a front-end whose paste has no file
/// to go with it.
#[derive(Debug)]
pub enum Clipboard {
    /// The part to attach, with the file it is named by: the file a copy named,
    /// or the one a pasteboard picture was written to when the copy named none
    /// — a front-end shows a pending attachment by the file it came from.
    Attached {
        part: ContentPart,
        path: Option<PathBuf>,
    },
    /// Nothing on the pasteboard could be attached, with the types it holds when
    /// the read could name them — so a paste that finds nothing says what it did
    /// find, rather than the same thing for an empty clipboard and a copy it
    /// could not read.
    Nothing { types: String },
    /// A file the copy names and this machine has, which is not an attachment.
    Refused(String),
    /// The pasteboard could not be read at all.
    Unreadable,
}

/// A copy from the Finder is preferred over its picture: the pasteboard also
/// carries the copied file's *icon*, so grabbing the PNG-flavoured data would
/// attach a placeholder image of the file instead of the file — a copied
/// screenshot arrived as a `PNG`-document icon. The file is read the way any
/// other attachment is, so a copied text file rides along as its own text; one
/// that is here but cannot be attached — past the size limit, or a binary that
/// is neither media nor text — is refused by name rather than replaced by that
/// icon.
///
/// A paste that names no file — a picture copied straight from a preview window
/// leaves only bytes on the pasteboard — is written under the config dir's own
/// scratch and answered with that path, so a front-end shows it by a file the
/// way it shows every other attachment instead of by what it happens to be. The
/// name is the part's content id, the same one `/attach` lists it by, so the
/// two agree; the scratch holds a week's pastes and is pruned as the next one
/// arrives. Where it cannot be written the part is answered with no path rather
/// than lost, and the front-end names it by what it is.
pub fn clipboard() -> Clipboard {
    if let Some(path) = clipboard_file(clipboard_path()) {
        return attach(path);
    }
    let copy = clipboard_copy();
    if let Some(path) = copy
        .as_ref()
        .and_then(|copy| clipboard_file(copy.path.clone()))
    {
        return attach(path);
    }
    if let Some(picture) = clipboard_picture() {
        let path = clipboard_dir()
            .and_then(|dir| save_clipboard_image(&dir, &picture.part, &picture.bytes));
        return Clipboard::Attached {
            part: picture.part,
            path,
        };
    }
    match copy {
        Some(copy) => Clipboard::Nothing { types: copy.types },
        None => Clipboard::Unreadable,
    }
}

/// A copied file as the part to attach, or the reason it is not one.
fn attach(path: PathBuf) -> Clipboard {
    match load_attachment(&path) {
        Ok(part) => Clipboard::Attached {
            part,
            path: Some(path),
        },
        Err(err) => Clipboard::Refused(format!("{err:#}")),
    }
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
/// Linux it needs `wl-paste` or `xclip`. The bytes come back beside the part, so
/// a caller with somewhere to put them can give a nameless paste a file name.
struct Picture {
    part: ContentPart,
    bytes: Vec<u8>,
}

fn clipboard_picture() -> Option<Picture> {
    let bytes = clipboard_bytes()?;
    if bytes.is_empty() {
        return None;
    }
    let bytes = optimize_image(bytes, "image/png");
    let part = ContentPart::ImageUrl {
        image_url: ImageUrl {
            url: format!("data:image/png;base64,{}", base64_encode(&bytes)),
            detail: None,
        },
    };
    Some(Picture { part, bytes })
}

/// Writes a pasted picture beside the run's other scratch, named by its own
/// content id — the id the `/attach` listing shows it by — so a paste the
/// pasteboard named no file for is named like any other attachment, and a second
/// paste of the same picture lands on the file already there rather than a
/// copy. Best effort: the bytes are written where they can be, and a paste that
/// cannot be written still attaches, named by what it is.
fn save_clipboard_image(dir: &Path, part: &ContentPart, bytes: &[u8]) -> Option<PathBuf> {
    std::fs::create_dir_all(dir).ok()?;
    cleanup_clipboard(dir);
    let path = dir.join(format!("{}.png", attachment_id(part)));
    if !path.exists() {
        std::fs::write(&path, bytes).ok()?;
    }
    Some(path)
}

fn clipboard_dir() -> Option<PathBuf> {
    Some(crate::config::config_dir()?.join("clipboard"))
}

fn cleanup_clipboard(dir: &Path) {
    let cutoff = std::time::SystemTime::now()
        .checked_sub(std::time::Duration::from_secs(CLIPBOARD_RETENTION_SECS));
    let Some(cutoff) = cutoff else {
        return;
    };
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let Ok(meta) = entry.metadata() else {
            continue;
        };
        if meta.modified().map(|m| m < cutoff).unwrap_or(false) {
            let _ = std::fs::remove_file(entry.path());
        }
    }
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

/// A copied file as the read behind [`clipboard_copy`] names it, with the
/// pasteboard's own types alongside it.
#[derive(Debug)]
struct Copied {
    path: Option<PathBuf>,
    types: String,
}

/// The file a copy left on the pasteboard as an alias record.
///
/// This read is a script of its own, and it must not load AppKit: `use framework
/// "AppKit"` points AppleScript's `the clipboard` at the Cocoa pasteboard, whose
/// coercions know only the modern file URL types, so the alias record a
/// Finder-style copy writes — the one `the clipboard as alias` reads — stops
/// resolving the moment the framework is loaded, in the very script that would
/// have read it. It is asked after the AppKit read above rather than before it,
/// since a copy that also carries a picture is answered by that read.
#[cfg(target_os = "macos")]
fn clipboard_copy() -> Option<Copied> {
    let output = Command::new("osascript")
        .args(["-e", CLIPBOARD_COPY_SCRIPT])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    copied_from(&String::from_utf8_lossy(&output.stdout))
}

#[cfg(not(target_os = "macos"))]
fn clipboard_copy() -> Option<Copied> {
    // Nothing but the image grab reads the pasteboard here, so the clipboard is
    // reported as holding no file rather than as a pasteboard that could not be
    // read at all.
    Some(Copied {
        path: None,
        types: String::new(),
    })
}

/// What [`clipboard_copy`] runs: the alias record it can coerce into a path, and
/// otherwise the types the pasteboard holds, which is what a paste that found
/// nothing has to report.
#[cfg(target_os = "macos")]
const CLIPBOARD_COPY_SCRIPT: &str = r#"try
set f to (the clipboard as alias)
return "file" & tab & (POSIX path of f)
end try
set ls to {}
repeat with t in (clipboard info)
set end of ls to ((item 1 of t) as text)
end repeat
set AppleScript's text item delimiters to ", "
return "types" & tab & (ls as text)"#;

/// What that script answered: one tagged line, `file<TAB><path>` or
/// `types<TAB><name, name>`. Anything else is an answer nothing recognises,
/// which is left to read as no answer at all rather than as an empty clipboard.
#[cfg(target_os = "macos")]
fn copied_from(output: &str) -> Option<Copied> {
    // Only the line ending is taken off: the empty pasteboard answers
    // `types<TAB>`, whose tab a trim of the whole tail would eat with it.
    let (kind, value) = output.trim_end_matches(['\n', '\r']).split_once('\t')?;
    match kind {
        "file" => Some(Copied {
            path: clipboard_path_from(value),
            types: String::new(),
        }),
        "types" => Some(Copied {
            path: None,
            types: value.trim().to_string(),
        }),
        _ => None,
    }
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

    /// A 2x2 24-bit BMP, header and pixels: the smallest thing the OS image
    /// tools can be asked to decode.
    fn bmp_fixture() -> Vec<u8> {
        let mut bmp = vec![0u8; 54 + 16];
        bmp[..2].copy_from_slice(b"BM");
        bmp[2..6].copy_from_slice(&70u32.to_le_bytes());
        bmp[10..14].copy_from_slice(&54u32.to_le_bytes());
        bmp[14..18].copy_from_slice(&40u32.to_le_bytes());
        bmp[18..22].copy_from_slice(&2u32.to_le_bytes());
        bmp[22..26].copy_from_slice(&2u32.to_le_bytes());
        bmp[26..28].copy_from_slice(&1u16.to_le_bytes());
        bmp[28..30].copy_from_slice(&24u16.to_le_bytes());
        bmp[54..].copy_from_slice(&[
            0x00, 0x00, 0xff, 0x00, 0xff, 0x00, 0x00, 0x00, // bottom row, padded to 4
            0xff, 0x00, 0x00, 0xff, 0xff, 0xff, 0x00, 0x00,
        ]);
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

        // A text file named `.tif` is not an image either: the suffix does not
        // decide, so it is attached as its own text.
        let lying_tiff = dir.join("notes.tif");
        std::fs::write(&lying_tiff, b"# notes").unwrap();
        assert_eq!(image_mime_of(&lying_tiff), None);
        assert!(!is_attachment_path(&lying_tiff));
        assert!(matches!(
            load_attachment(&lying_tiff).unwrap(),
            ContentPart::Text { .. }
        ));

        // A HEIC and an AVIF are recognized by their ISO base media brand, an
        // MP4 by its own never being one.
        let heic = dir.join("photo.heic");
        std::fs::write(&heic, b"\x00\x00\x00\x18ftypheic\x00\x00\x00\x00heicmif1").unwrap();
        assert_eq!(image_mime_of(&heic), Some("image/heic"));
        let avif = dir.join("photo.avif");
        std::fs::write(&avif, b"\x00\x00\x00\x18ftypavif\x00\x00\x00\x00avifmif1").unwrap();
        assert_eq!(image_mime_of(&avif), Some("image/avif"));
        let movie = dir.join("clip.heic");
        std::fs::write(&movie, b"\x00\x00\x00\x18ftypisom\x00\x00\x00\x00isomiso2").unwrap();
        assert_eq!(image_mime_of(&movie), None);

        // A PDF is recognised by its header too.
        let spec = dir.join("spec");
        std::fs::write(&spec, b"%PDF-1.7\n...").unwrap();
        // A text file named `.pdf` is not a PDF any more than one named `.png`
        // is an image, and a `.pdf` that is one is read by its header.
        let liar_pdf = dir.join("notes.pdf");
        std::fs::write(&liar_pdf, b"# notes").unwrap();
        assert!(!is_pdf_path(&liar_pdf));
        assert!(!is_attachment_path(&liar_pdf));
        assert!(is_pdf_path(&spec));

        std::fs::remove_dir_all(&dir).ok();
    }

    /// An image no tool on this machine can decode is sent as it stands rather
    /// than refused, so a Linux box without ImageMagick — or a format whose
    /// codec is not installed — loses no attachment support to a conversion it
    /// cannot perform.
    #[test]
    fn an_image_no_tool_can_convert_travels_as_it_stands() {
        let dir = std::env::temp_dir().join(format!("oxide_media_broken_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("broken.tif");
        std::fs::write(&file, b"II*\x00not really a tiff").unwrap();

        match load_attachment(&file).unwrap() {
            ContentPart::ImageUrl { image_url } => assert!(
                image_url.url.starts_with("data:image/tiff;base64,"),
                "{}",
                image_url.url
            ),
            other => panic!("an undecodable image should still travel, got {other:?}"),
        }

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

    /// A BMP is a format no provider takes as it stands, and Windows carries no
    /// ImageMagick: the conversion is the imaging stack the platform ships,
    /// asked through PowerShell, so a BMP — or a TIFF, or a HEIC whose codec is
    /// installed — is still a PNG by the time a provider sees it. A machine
    /// without System.Drawing leaves the image to travel as it stands, which
    /// [`an_image_no_tool_can_convert_travels_as_it_stands`] covers.
    #[cfg(target_os = "windows")]
    #[test]
    fn windows_converts_an_image_with_its_own_imaging_stack() {
        let png = convert_image_to_png(&bmp_fixture(), "image/bmp")
            .expect("System.Drawing converts a BMP");
        assert_eq!(png_dimensions(&png), Some((2, 2)));
    }

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
        // A readable file is what its bytes say it is, whatever it is called.
        std::fs::write(dir.join("fake.png"), b"not an image").unwrap();
        assert!(referenced_attachments("and @fake.png", &dir).is_empty());

        std::fs::remove_dir_all(&dir).ok();
    }

    /// The one case with no bytes to go on is a file this process cannot open
    /// at all, and then its name is what decides it. The platform that withholds
    /// such a file is what makes this reachable: a reference to it is reported
    /// by whoever attaches it instead of reaching the model as literal text.
    #[cfg(unix)]
    #[test]
    fn extracts_a_reference_whose_bytes_cannot_be_read() {
        use std::os::unix::fs::PermissionsExt;

        let dir = std::env::temp_dir().join(format!("oxide_media_refused_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let shot = dir.join("shot.png");
        let paper = dir.join("paper.pdf");
        let notes = dir.join("notes.txt");
        for file in [&shot, &paper, &notes] {
            std::fs::write(file, ONE_PX_PNG).unwrap();
            std::fs::set_permissions(file, std::fs::Permissions::from_mode(0o000)).unwrap();
        }
        std::fs::write(dir.join("fake.png"), b"not an image").unwrap();

        // Root reads a file whatever its mode says, and a runner that does would
        // read the bytes of every one of these and assert nothing.
        if std::fs::read(&shot).is_err() {
            let found = referenced_attachments(
                "compare @shot.png with @paper.pdf, and @notes.txt and @fake.png too",
                &dir,
            );
            let names: Vec<String> = found
                .iter()
                .map(|path| path.file_name().unwrap().to_string_lossy().into_owned())
                .collect();
            // The name decides these, not what is in them: `notes.txt` holds the
            // same PNG bytes as `shot.png` and is left to travel as text.
            assert_eq!(names, ["shot.png", "paper.pdf"]);
        }

        for file in [&shot, &paper, &notes] {
            std::fs::set_permissions(file, std::fs::Permissions::from_mode(0o600)).ok();
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_name_claims_media_only_where_the_bytes_cannot() {
        for name in [
            "shot.png",
            "SHOT.PNG",
            "shot.jpeg",
            "shot.jpg",
            "shot.tif",
            "shot.heic",
            "photo.avif",
            "paper.pdf",
        ] {
            assert!(name_claims_media(Path::new(name)), "{name}");
        }
        for name in [
            "notes.txt",
            "notes.md",
            "shot.png.txt",
            "png",
            ".png",
            "shot.",
            "archive.tar.gz",
        ] {
            assert!(!name_claims_media(Path::new(name)), "{name}");
        }
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

    /// The read that finds the alias record a Finder-style copy writes. It is a
    /// script of its own because a framework-loaded one cannot coerce that
    /// record at all: `use framework "AppKit"` is what silently takes this read
    /// away, which is why the script is asserted to be without it.
    #[cfg(target_os = "macos")]
    #[test]
    fn reads_the_alias_record_a_framework_loaded_read_cannot_coerce() {
        assert!(!CLIPBOARD_COPY_SCRIPT.contains("use framework"));
        let copied = copied_from("file\t/Users/me/Desktop/My Shot.png\n").unwrap();
        assert_eq!(
            copied.path,
            Some(PathBuf::from("/Users/me/Desktop/My Shot.png"))
        );
        assert_eq!(copied.types, "");
        // Text coerced into a path is no path at all.
        assert_eq!(copied_from("file\thello there").unwrap().path, None);
        // A pasteboard holding no file reports what it does hold, so a paste
        // that finds nothing can say what it found.
        let copied = copied_from("types\tpublic.tiff, NeXT TIFF v4.0 pasteboard type\n").unwrap();
        assert_eq!(copied.path, None);
        assert_eq!(copied.types, "public.tiff, NeXT TIFF v4.0 pasteboard type");
        assert_eq!(copied_from("types\t\n").unwrap().types, "");
        // An answer nothing recognises is no answer at all rather than an empty
        // clipboard, so it is left to read as a clipboard that could not be
        // read.
        assert!(copied_from("").is_none());
        assert!(copied_from("hello there\n").is_none());
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
    fn a_copied_file_that_is_not_an_attachment_is_refused_by_name() {
        let dir = std::env::temp_dir().join(format!("oxide_media_attach_{}", std::process::id()));
        std::fs::create_dir_all(dir.join("folder")).unwrap();
        std::fs::write(dir.join("notes.txt"), b"hello").unwrap();

        match attach(dir.join("notes.txt")) {
            Clipboard::Attached { part, path } => {
                assert_eq!(part_kind(&part), "text");
                assert_eq!(path.as_deref(), Some(dir.join("notes.txt").as_path()));
            }
            other => panic!("a copied text file should attach: {other:?}"),
        }
        // A copy that names a folder is reported rather than passed over as an
        // empty clipboard: the paste did find something, and that is what it
        // tells the reader. The reason names the file it could not read, which
        // is the part of it that is the same on every platform — the native
        // error beside it is the platform's own words.
        let folder = dir.join("folder");
        match attach(folder.clone()) {
            Clipboard::Refused(reason) => {
                assert!(reason.contains(&folder.display().to_string()), "{reason}")
            }
            other => panic!("a folder should be refused: {other:?}"),
        }

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_refused_read_names_the_grant_to_give_back() {
        let refused = read_error(
            Path::new("/tmp/shot.png"),
            &std::io::Error::from(std::io::ErrorKind::PermissionDenied),
        )
        .to_string();
        assert!(refused.starts_with("reading /tmp/shot.png: "), "{refused}");
        assert!(refused.contains("may not read that file"), "{refused}");

        // `Operation not permitted` is what a platform that withholds a file
        // answers with, and it is the same kind to Rust as `Permission denied`.
        #[cfg(unix)]
        for errno in [1, 13] {
            let err = std::io::Error::from_raw_os_error(errno);
            assert_eq!(err.kind(), std::io::ErrorKind::PermissionDenied);
            let reason = read_error(Path::new("/tmp/shot.png"), &err).to_string();
            assert!(reason.contains("may not read that file"), "{reason}");
        }

        // Everything else is reported as it happened.
        let missing = read_error(
            Path::new("/tmp/shot.png"),
            &std::io::Error::from(std::io::ErrorKind::NotFound),
        )
        .to_string();
        assert!(!missing.contains("may not read"), "{missing}");
    }

    #[test]
    fn a_refused_read_names_the_app_that_holds_the_grant() {
        const HEAD: &str = " — this app may not read that file: macOS keeps the Desktop, Documents and Downloads folders behind a per-app grant";
        const TAIL: &str = " under System Settings → Privacy & Security → Files and Folders, or copy the file into the project";

        let named = permission_hint_for(Some("Visual Studio Code".to_string()));
        assert!(named.starts_with(HEAD), "{named}");
        assert!(named.ends_with(TAIL), "{named}");
        // The app is the one thing to allow, so it is named where the reader
        // both learns why the read was refused and what to do about it.
        assert_eq!(named.matches("Visual Studio Code").count(), 2, "{named}");

        // The hint with no app above the run is the same wording around the
        // same head and tail, so the two cannot drift apart.
        assert_eq!(permission_hint_for(None), PERMISSION_HINT);
        #[cfg(target_os = "macos")]
        {
            assert!(PERMISSION_HINT.starts_with(HEAD), "{PERMISSION_HINT}");
            assert!(PERMISSION_HINT.ends_with(TAIL), "{PERMISSION_HINT}");
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn the_app_above_a_run_is_the_outermost_bundle_on_the_path() {
        // `ps -axo pid=,ppid=,comm=` as macOS prints it: the pid, the parent,
        // then the whole executable path — spaces and all.
        let table = "  1     0 /sbin/launchd\n\
  1630     1 /Applications/Visual Studio Code.app/Contents/MacOS/Code\n\
  22956  1630 /Applications/Visual Studio Code.app/Contents/Frameworks/Code Helper.app/Contents/MacOS/Code Helper\n\
  13042 22956 oxide\n\
  17744 13042 sh";
        assert_eq!(
            responsible_bundle(table, 17744).as_deref(),
            Some(Path::new("/Applications/Visual Studio Code.app"))
        );

        // This app's own harness is inside the app that started it.
        let desktop = "  1     0 /sbin/launchd\n\
  500     1 /Applications/Oxide.app/Contents/MacOS/Oxide\n\
  501   500 /Applications/Oxide.app/Contents/Resources/harness/oxide-desktop";
        assert_eq!(
            responsible_bundle(desktop, 501).as_deref(),
            Some(Path::new("/Applications/Oxide.app"))
        );

        // A run whose ancestors are none of them an app names nothing, as does
        // a pid the table does not hold and an empty one.
        let bare = "  1     0 /sbin/launchd\n  42     1 /usr/sbin/sshd\n  77    42 -zsh";
        assert_eq!(responsible_bundle(bare, 77), None);
        assert_eq!(responsible_bundle(table, 999), None);
        assert_eq!(responsible_bundle("", 1), None);

        // A directory that merely ends in `.app` is not an app bundle, and a
        // helper inside one still belongs to the bundle around it.
        assert_eq!(bundle_dir("/Users/jayson/notes.app/readme.md"), None);
        assert_eq!(bundle_dir("/x/.app/Contents/MacOS/thing"), None);
        assert_eq!(
            bundle_dir("/System/Applications/Utilities/Terminal.app/Contents/MacOS/Terminal"),
            Some(PathBuf::from("/System/Applications/Utilities/Terminal.app"))
        );
        assert_eq!(
            bundle_dir(
                "/Applications/Visual Studio Code.app/Contents/Frameworks/Code Helper.app/Contents/MacOS/Code Helper"
            ),
            Some(PathBuf::from("/Applications/Visual Studio Code.app"))
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn a_bundle_is_named_the_way_its_own_bundle_names_itself() {
        let dir = std::env::temp_dir().join(format!("oxide_media_app_{}", std::process::id()));
        let bundle = |name: &str, plist: Option<&str>| {
            let bundle = dir.join(name);
            let contents = bundle.join("Contents");
            std::fs::create_dir_all(&contents).unwrap();
            if let Some(plist) = plist {
                std::fs::write(contents.join("Info.plist"), plist).unwrap();
            }
            bundle
        };
        let plist = |body: &str| {
            format!(
                "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<plist version=\"1.0\"><dict>{body}</dict></plist>\n"
            )
        };

        // The name the bundle gives itself outranks the directory it was left
        // in, which is the name the platform resolves and the one a grant is
        // keyed on: VS Code sits in `Visual Studio Code.app` and names itself
        // `Code`.
        let named = bundle(
            "Pretend.app",
            Some(&plist("<key>CFBundleName</key><string>Spoken For</string>")),
        );
        assert_eq!(app_name(&named).as_deref(), Some("Spoken For"));

        // The display name outranks the name, and an empty one is no name.
        let both = bundle(
            "Both.app",
            Some(&plist(
                "<key>CFBundleDisplayName</key><string>Shown</string>\
                 <key>CFBundleName</key><string>Named</string>",
            )),
        );
        assert_eq!(app_name(&both).as_deref(), Some("Shown"));
        let blank = bundle(
            "Blank.app",
            Some(&plist(
                "<key>CFBundleDisplayName</key><string></string>\
                 <key>CFBundleName</key><string>Still Named</string>",
            )),
        );
        assert_eq!(app_name(&blank).as_deref(), Some("Still Named"));

        // A bundle that says nothing about itself is named by where it sits,
        // as is one whose plist could not be read at all.
        let silent = bundle(
            "Silent.app",
            Some(&plist("<key>CFBundleVersion</key><string>1</string>")),
        );
        assert_eq!(app_name(&silent).as_deref(), Some("Silent"));
        let bare = bundle("Bare.app", None);
        assert_eq!(app_name(&bare).as_deref(), Some("Bare"));

        // A binary plist — the form many bundles ship — reads the same.
        let packed = bundle(
            "Packed.app",
            Some(&plist(
                "<key>CFBundleName</key><string>Packed Name</string>",
            )),
        );
        let packed_plist = packed.join("Contents/Info.plist");
        let status = std::process::Command::new("/usr/bin/plutil")
            .args(["-convert", "binary1"])
            .arg(&packed_plist)
            .status()
            .unwrap();
        assert!(status.success());
        assert_eq!(app_name(&packed).as_deref(), Some("Packed Name"));

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn an_attachment_the_platform_refuses_is_reported_with_the_hint() {
        use std::os::unix::fs::PermissionsExt;

        let dir = std::env::temp_dir().join(format!("oxide_media_denied_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("shot.png");
        std::fs::write(&file, b"f").unwrap();
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o000)).unwrap();
        // Root reads a file whatever its mode says, and a runner that does
        // would assert nothing.
        if std::fs::read(&file).is_err() {
            match attach(file.clone()) {
                Clipboard::Refused(reason) => {
                    assert!(reason.contains("may not read that file"), "{reason}");
                }
                other => panic!("a file this process may not read should be refused: {other:?}"),
            }
        }

        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600)).ok();
        std::fs::remove_dir_all(&dir).ok();
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
        // What the payload holds decides, and the media type a front-end wrote
        // into the URL is not trusted over it.
        let image = content_part_from_data_url(
            format!("data:image/png;base64,{}", base64_encode(ONE_PX_PNG)),
            Some("shot.png".to_string()),
        )
        .expect("image");
        match image {
            ContentPart::ImageUrl { image_url } => assert_eq!(
                image_url.url,
                format!("data:image/png;base64,{}", base64_encode(ONE_PX_PNG))
            ),
            other => panic!("unexpected part: {other:?}"),
        }

        // Text bytes labelled an image are text, the same as they are for a
        // file called `.png`.
        match content_part_from_data_url(
            format!("data:image/png;base64,{}", base64_encode(b"# notes\n")),
            Some("notes.png".to_string()),
        ) {
            Some(ContentPart::Text { text }) => {
                assert!(text.contains("<file name=\"notes.png\">"), "{text}")
            }
            other => panic!("unexpected part: {other:?}"),
        }

        let pdf = content_part_from_data_url(
            format!(
                "data:application/pdf;base64,{}",
                base64_encode(b"%PDF-1.7\n...")
            ),
            Some("spec.pdf".to_string()),
        )
        .expect("pdf");
        match pdf {
            ContentPart::File { file } => {
                assert_eq!(file.filename.as_deref(), Some("spec.pdf"));
                assert!(
                    file.file_data.starts_with("data:application/pdf;base64,"),
                    "{}",
                    file.file_data
                );
            }
            other => panic!("unexpected part: {other:?}"),
        }

        match content_part_from_data_url(
            "data:text/csv;base64,YSxiCg==".to_string(),
            Some("rows.csv".to_string()),
        ) {
            Some(ContentPart::Text { text }) => {
                assert_eq!(text, "<file name=\"rows.csv\">\na,b\n\n</file>");
            }
            other => panic!("unexpected part: {other:?}"),
        }

        // An image format no provider takes still travels — converted where a
        // tool can, as it stands where none can.
        match content_part_from_data_url(
            format!(
                "data:image/tiff;base64,{}",
                base64_encode(b"II*\x00\x00\x00")
            ),
            Some("scan.tif".to_string()),
        ) {
            Some(ContentPart::ImageUrl { image_url }) => assert!(
                image_url.url.starts_with("data:image/"),
                "{}",
                image_url.url
            ),
            other => panic!("unexpected part: {other:?}"),
        }

        // A payload that is not text is refused rather than shipped as mojibake.
        assert!(
            content_part_from_data_url("data:text/plain;base64,AAAA".to_string(), None).is_none()
        );
        // A payload that is not base64 is refused before it is decoded, a PDF
        // declared as one included.
        assert!(
            content_part_from_data_url("data:image/png;base64,!!!!".to_string(), None).is_none()
        );
        assert!(content_part_from_data_url(
            "data:application/pdf;base64,!!!!".to_string(),
            Some("spec.pdf".to_string())
        )
        .is_none());
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
        assert_eq!(attachment_label(&a), "image (png)");
        assert_eq!(attachment_label(&c), "image (jpeg)");
        let document = ContentPart::File {
            file: FileData {
                filename: Some("spec.pdf".into()),
                file_data: "data:application/pdf;base64,AAAA".into(),
            },
        };
        assert_eq!(attachment_label(&document), "spec.pdf");
    }

    /// A pasted picture the pasteboard named no file for is written beside the
    /// run's other scratch, named by the same content id `/attach` lists it by,
    /// so the composer shows a path the way it does for a file paste.
    #[test]
    fn a_pasted_picture_is_written_where_it_can_be_named() {
        let image = |url: &str| ContentPart::ImageUrl {
            image_url: ImageUrl {
                url: url.to_string(),
                detail: None,
            },
        };
        let dir = std::env::temp_dir().join(format!("oxide_media_paste_{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();

        let first = image("data:image/png;base64,AAAA");
        let path = save_clipboard_image(&dir, &first, b"the payload").expect("a scratch file");
        assert_eq!(path, dir.join(format!("{}.png", attachment_id(&first))));
        assert_eq!(std::fs::read(&path).unwrap(), b"the payload");

        // The same picture pasted again lands on the file already there rather
        // than leaving a second copy of it behind.
        assert_eq!(
            save_clipboard_image(&dir, &first, b"the payload").as_deref(),
            Some(path.as_path())
        );
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);

        // Two different pictures are named apart by their own bytes.
        let second = save_clipboard_image(
            &dir,
            &image("data:image/png;base64,BBBB"),
            b"another payload",
        )
        .expect("a scratch file");
        assert_ne!(second, path);
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 2);

        std::fs::remove_dir_all(&dir).ok();
    }

    /// Nothing is remembered forever: the scratch is pruned on the next write,
    /// the way truncated tool output is.
    #[test]
    fn a_pasted_picture_is_kept_for_a_week_and_no_longer() {
        let dir =
            std::env::temp_dir().join(format!("oxide_media_paste_age_{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        let stale = dir.join("stale.png");
        let fresh = dir.join("fresh.png");
        std::fs::write(&stale, b"stale").unwrap();
        std::fs::write(&fresh, b"fresh").unwrap();
        let past = std::time::SystemTime::now()
            - std::time::Duration::from_secs(CLIPBOARD_RETENTION_SECS + 24 * 60 * 60);
        std::fs::File::options()
            .write(true)
            .open(&stale)
            .unwrap()
            .set_modified(past)
            .unwrap();

        cleanup_clipboard(&dir);

        assert!(!stale.exists());
        assert!(fresh.exists());
        std::fs::remove_dir_all(&dir).ok();
    }

    /// A paste whose bytes cannot be written still attaches — the name is the
    /// only thing this costs — so a scratch directory the machine refuses
    /// leaves the attachment with no path rather than losing it.
    #[test]
    fn a_paste_the_scratch_refuses_still_attaches() {
        let dir = std::env::temp_dir().join(format!("oxide_media_paste_ro_{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("not-a-directory");
        std::fs::write(&file, b"x").unwrap();
        let image = ContentPart::ImageUrl {
            image_url: ImageUrl {
                url: "data:image/png;base64,AAAA".to_string(),
                detail: None,
            },
        };

        assert!(save_clipboard_image(&file.join("clipboard"), &image, b"payload").is_none());

        std::fs::remove_dir_all(&dir).ok();
    }
}
