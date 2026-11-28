//! Text files the window's editor opens and saves.
//!
//! The editor shares the project boundary preview.rs applies, but an editor must
//! never save a file it only read part of. Reads therefore have a larger limit
//! than the viewer's and non-UTF-8 files are refused rather than decoded lossily.

use crate::preview;
use anyhow::{bail, Context, Result};
use serde::Serialize;
use std::path::{Path, PathBuf};

const LIMIT: usize = 8 * 1024 * 1024;

#[derive(Debug, Clone, Serialize)]
pub struct EditorFile {
    pub path: String,
    pub text: String,
    /// Changes whenever the bytes on disk do, so a save never overwrites work a
    /// turn wrote after this file was opened.
    pub digest: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct SavedFile {
    pub digest: String,
    pub bytes: usize,
}

fn resolve(project: &Path, path: &str) -> Result<PathBuf> {
    let root = project
        .canonicalize()
        .with_context(|| format!("resolving {}", project.display()))?;
    let full = root.join(preview::checked(path)?);
    let resolved = full
        .canonicalize()
        .with_context(|| format!("resolving {}", full.display()))?;
    if !resolved.starts_with(&root) {
        bail!("`{path}` is not a path inside the project.");
    }
    if !resolved.is_file() {
        bail!("`{path}` is not a file.");
    }
    Ok(resolved)
}

fn digest(bytes: &[u8]) -> String {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}")
}

fn text(bytes: Vec<u8>) -> Result<String> {
    if bytes.len() > LIMIT {
        bail!("this file is larger than the editor's 8 MiB limit.");
    }
    if bytes.contains(&0) {
        bail!("this is a binary file, not editable text.");
    }
    String::from_utf8(bytes).context("this file is not UTF-8 text")
}

pub fn read(project: &Path, path: &str) -> Result<EditorFile> {
    let full = resolve(project, path)?;
    let bytes = std::fs::read(&full).with_context(|| format!("reading {}", full.display()))?;
    let digest = digest(&bytes);
    Ok(EditorFile {
        path: path.to_string(),
        text: text(bytes)?,
        digest,
    })
}

pub fn save(project: &Path, path: &str, value: &str, expected_digest: &str) -> Result<SavedFile> {
    if value.len() > LIMIT {
        bail!("this file is larger than the editor's 8 MiB limit.");
    }
    let full = resolve(project, path)?;
    let current = std::fs::read(&full).with_context(|| format!("reading {}", full.display()))?;
    if digest(&current) != expected_digest {
        bail!("`{path}` changed on disk; reopen it before saving.");
    }
    std::fs::write(&full, value.as_bytes())
        .with_context(|| format!("writing {}", full.display()))?;
    Ok(SavedFile {
        digest: digest(value.as_bytes()),
        bytes: value.len(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("oxide_editor_{name}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn reads_and_saves_text_with_a_fresh_digest() {
        let dir = project("save");
        std::fs::write(dir.join("main.rs"), "fn main() {}\n").unwrap();
        let opened = read(&dir, "main.rs").unwrap();
        assert_eq!(opened.text, "fn main() {}\n");

        let saved = save(
            &dir,
            "main.rs",
            "fn main() {\n    println!(\"hi\");\n}\n",
            &opened.digest,
        )
        .unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.join("main.rs")).unwrap(),
            "fn main() {\n    println!(\"hi\");\n}\n"
        );
        assert_eq!(
            saved.digest,
            digest(&std::fs::read(dir.join("main.rs")).unwrap())
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn refuses_a_save_after_the_file_changed() {
        let dir = project("conflict");
        std::fs::write(dir.join("main.rs"), "one\n").unwrap();
        let opened = read(&dir, "main.rs").unwrap();
        std::fs::write(dir.join("main.rs"), "two\n").unwrap();
        let error = save(&dir, "main.rs", "three\n", &opened.digest)
            .unwrap_err()
            .to_string();
        assert!(error.contains("changed on disk"), "{error}");
        assert_eq!(
            std::fs::read_to_string(dir.join("main.rs")).unwrap(),
            "two\n"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn refuses_binary_and_paths_outside_the_project() {
        let dir = project("refuse");
        std::fs::write(dir.join("image.bin"), b"a\0b").unwrap();
        assert!(read(&dir, "image.bin")
            .unwrap_err()
            .to_string()
            .contains("binary"));
        assert!(read(&dir, "../outside.rs")
            .unwrap_err()
            .to_string()
            .contains("inside the project"));
        let _ = std::fs::remove_dir_all(dir);
    }
}
