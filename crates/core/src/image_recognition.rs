//! Image-to-text recognition for models that cannot see images.
//!
//! A provider like DeepSeek has no vision input, so an attached image must be
//! turned into text before the request is built. The configured `image_script`
//! is run when one is set; otherwise a best-effort local OCR pipeline is used
//! (`tesseract`, then the macOS Vision framework through `swift`). When no
//! recognizer can read the image, the part is replaced with a marker so the
//! model still knows an image was attached instead of receiving an
//! `image_url` part it rejects.

use crate::llm::{ContentPart, ImageUrl, Message, MessageContent};
use std::path::{Path, PathBuf};
use std::process::Command;

/// The placeholder a configured command may use for the image file path.
const FILE_PLACEHOLDER: &str = "{file}";

/// Whether any message carries an image part, so the caller can skip the
/// conversion pass entirely when there is nothing to convert.
pub fn messages_have_images(messages: &[Message]) -> bool {
    messages.iter().any(|message| {
        matches!(
            message.content.as_ref(),
            Some(MessageContent::Parts(parts)) if parts.iter().any(|part| matches!(part, ContentPart::ImageUrl { .. }))
        )
    })
}

/// Replaces every image part in `messages` with its recognized text. Messages
/// without images are cloned unchanged; the stored session log is never
/// modified. Only the outgoing request copy is rewritten.
pub fn messages_with_image_text(messages: &[Message], script: Option<&str>) -> Vec<Message> {
    let mut out = Vec::with_capacity(messages.len());
    for message in messages {
        let Some(content) = message.content.as_ref() else {
            out.push(message.clone());
            continue;
        };
        let MessageContent::Parts(parts) = content else {
            out.push(message.clone());
            continue;
        };
        if !parts
            .iter()
            .any(|part| matches!(part, ContentPart::ImageUrl { .. }))
        {
            out.push(message.clone());
            continue;
        }
        let mut new_parts = Vec::with_capacity(parts.len());
        for part in parts {
            match part {
                ContentPart::ImageUrl { image_url } => {
                    let text = image_text(image_url, script)
                        .map(|text| format!("[image content]\n{text}"))
                        .unwrap_or_else(|| {
                            "[image attached, but this model cannot see images and no image \
                             recognition tool was available (install `tesseract` or set \
                             `image_script`)]"
                                .to_string()
                        });
                    new_parts.push(ContentPart::Text { text });
                }
                other => new_parts.push(other.clone()),
            }
        }
        let mut message = message.clone();
        message.content = Some(MessageContent::Parts(new_parts));
        out.push(message);
    }
    out
}

/// Recognizes one image and returns the text on its stdout (trimmed), or
/// `None` when no recognizer could read it.
pub fn image_text(image: &ImageUrl, script: Option<&str>) -> Option<String> {
    let (bytes, extension) = image_payload(&image.url)?;
    let temp = TempImageFile::new(extension)?;
    temp.write(&bytes)?;
    match script.filter(|command| !command.trim().is_empty()) {
        Some(command) => run_script(command, temp.path()),
        None => ocr_default(temp.path()),
    }
    .map(|text| text.trim().to_string())
    .filter(|text| !text.is_empty())
}

/// Decodes an image data URL into its bytes and a file extension, so the image
/// can be written to a temporary file for a recognizer that reads a path.
fn image_payload(url: &str) -> Option<(Vec<u8>, &'static str)> {
    let meta = url.strip_prefix("data:")?.split_once(',')?.0;
    let mime = meta.strip_suffix(";base64").unwrap_or(meta);
    let extension = match mime {
        "image/png" => "png",
        "image/jpeg" => "jpg",
        "image/gif" => "gif",
        "image/webp" => "webp",
        "image/bmp" => "bmp",
        _ => return None,
    };
    let payload = url.split_once(',')?.1;
    let bytes = crate::media::base64_decode(payload)?;
    (!bytes.is_empty()).then_some((bytes, extension))
}

fn ocr_default(path: &Path) -> Option<String> {
    run_stdout("tesseract", &[path_str(path)?, "stdout"]).or_else(|| {
        #[cfg(target_os = "macos")]
        {
            macos_vision(path)
        }
        #[cfg(not(target_os = "macos"))]
        {
            None
        }
    })
}

/// Runs a user-supplied `imageScript`. `{file}` is replaced with the image
/// path; a command without the placeholder gets the path appended as its last
/// argument. It runs through the platform shell so a pipeline is allowed.
fn run_script(command: &str, path: &Path) -> Option<String> {
    let path = shell_quote(path_str(path)?);
    let command = if command.contains(FILE_PLACEHOLDER) {
        command.replace(FILE_PLACEHOLDER, &path)
    } else {
        format!("{command} {path}")
    };
    run_shell(&command)
}

#[cfg(unix)]
fn shell_quote(path: &str) -> String {
    format!("'{}'", path.replace('\'', "'\\''"))
}

#[cfg(windows)]
fn shell_quote(path: &str) -> String {
    format!("\"{}\"", path.replace('"', "\\\""))
}

#[cfg(not(any(unix, windows)))]
fn shell_quote(path: &str) -> String {
    path.to_string()
}

fn run_stdout(program: &str, args: &[&str]) -> Option<String> {
    let output = Command::new(program).args(args).output().ok()?;
    if output.status.success() {
        let text = String::from_utf8(output.stdout).ok()?;
        (!text.trim().is_empty()).then_some(text)
    } else {
        None
    }
}

#[cfg(unix)]
fn run_shell(command: &str) -> Option<String> {
    let output = Command::new("sh").arg("-c").arg(command).output().ok()?;
    if output.status.success() {
        let text = String::from_utf8(output.stdout).ok()?;
        (!text.trim().is_empty()).then_some(text)
    } else {
        None
    }
}

#[cfg(windows)]
fn run_shell(command: &str) -> Option<String> {
    let output = Command::new("cmd").args(["/C", command]).output().ok()?;
    if output.status.success() {
        let text = String::from_utf8(output.stdout).ok()?;
        (!text.trim().is_empty()).then_some(text)
    } else {
        None
    }
}

#[cfg(not(any(unix, windows)))]
fn run_shell(_command: &str) -> Option<String> {
    None
}

fn path_str(path: &Path) -> Option<&str> {
    path.to_str()
}

/// macOS built-in Vision text recognition, run through a temporary Swift
/// script so no third-party OCR is required. `swift` ships with the Xcode
/// command line tools; when it is missing the caller falls back to a marker.
#[cfg(target_os = "macos")]
fn macos_vision(path: &Path) -> Option<String> {
    const SCRIPT: &str = r#"import Foundation
import Vision
import AppKit

let path = CommandLine.arguments[1]
guard let image = NSImage(contentsOfFile: path),
      let cgImage = image.cgImage(forProposedRect: nil, context: nil, hints: nil) else {
    FileHandle.standardError.write(Data("cannot load image\n".utf8))
    exit(1)
}
let request = VNRecognizeTextRequest()
request.recognitionLevel = .accurate
request.usesLanguageCorrection = true
let handler = VNImageRequestHandler(cgImage: cgImage, options: [:])
do {
    try handler.perform([request])
} catch {
    FileHandle.standardError.write(Data("\(error)\n".utf8))
    exit(1)
}
for case let observation as VNRecognizedTextObservation in request.results ?? [] {
    if let candidate = observation.topCandidates(1).first {
        print(candidate.string)
    }
}
"#;
    let dir = TempImageFile::new("swift")?;
    dir.write(SCRIPT.as_bytes())?;
    let script_path = dir.path().to_str()?;
    let image = path.to_str()?;
    let output = Command::new("swift")
        .args([script_path, image])
        .output()
        .ok()?;
    if output.status.success() {
        let text = String::from_utf8(output.stdout).ok()?;
        (!text.trim().is_empty()).then_some(text)
    } else {
        None
    }
}

/// A private, randomly named temporary file for one image recognition. The
/// directory is owner-only on Unix and removed on drop, so a predictable-name
/// symlink cannot redirect the image or the script.
struct TempImageFile {
    dir: PathBuf,
    path: PathBuf,
}

impl TempImageFile {
    fn new(extension: &str) -> Option<Self> {
        let mut random = [0u8; 16];
        getrandom::getrandom(&mut random).ok()?;
        let name: String = random.iter().map(|byte| format!("{byte:02x}")).collect();
        let dir = std::env::temp_dir().join(format!("oxide-image-text-{name}"));
        let mut builder = std::fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder.create(&dir).ok()?;
        let path = dir.join(format!("image.{extension}"));
        Some(Self { dir, path })
    }

    fn path(&self) -> &Path {
        &self.path
    }

    fn write(&self, bytes: &[u8]) -> Option<()> {
        use std::io::Write;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&self.path)
            .ok()?;
        file.write_all(bytes).ok()
    }
}

impl Drop for TempImageFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::ImageUrl;

    fn image(url: &str) -> ContentPart {
        ContentPart::ImageUrl {
            image_url: ImageUrl {
                url: url.to_string(),
                detail: None,
            },
        }
    }

    #[test]
    fn detects_images_in_messages() {
        assert!(!messages_have_images(&[Message::user("hi")]));
        assert!(messages_have_images(&[Message::user_parts(
            "x",
            vec![image("data:image/png;base64,AAAA")],
        )]));
        assert!(!messages_have_images(&[Message::tool_parts(
            "id",
            "x",
            vec![ContentPart::Text { text: "t".into() }],
        )]));
    }

    #[test]
    fn parses_known_image_data_urls() {
        let (_, ext) = image_payload("data:image/png;base64,AAAA").unwrap();
        assert_eq!(ext, "png");
        let (_, ext) = image_payload("data:image/jpeg;base64,AAAA").unwrap();
        assert_eq!(ext, "jpg");
        assert!(image_payload("data:application/pdf;base64,AAAA").is_none());
        assert!(image_payload("https://example.com/a.png").is_none());
    }

    #[test]
    fn leaves_messages_without_images_alone() {
        let messages = vec![Message::user("hi")];
        let converted = messages_with_image_text(&messages, None);
        assert_eq!(converted.len(), 1);
        assert_eq!(converted[0].display().as_deref(), Some("hi"));
    }

    #[test]
    fn replaces_image_parts_with_the_script_output() {
        let messages = vec![Message::user_parts(
            "look at this",
            vec![image("data:image/png;base64,AAAA")],
        )];
        let converted = messages_with_image_text(&messages, Some("printf 'ocr text'"));
        let text = converted[0].display().unwrap();
        assert!(
            !text.contains("image_url"),
            "raw image must not reach the model"
        );
        assert!(
            text.contains("ocr text"),
            "script stdout should replace the image: {text}"
        );
    }

    #[test]
    fn falls_back_to_a_marker_when_no_recognizer_outputs_text() {
        let messages = vec![Message::user_parts(
            "look at this",
            vec![image("data:image/png;base64,AAAA")],
        )];
        let converted = messages_with_image_text(&messages, Some("true"));
        let text = converted[0].display().unwrap();
        assert!(
            !text.contains("image_url"),
            "raw image must not reach the model"
        );
        assert!(
            text.contains("image attached"),
            "marker should explain the image"
        );
    }
}
