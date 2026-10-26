//! The GPUI app's own release train.
//!
//! Releases from a `gpui-v*` tag, resolved through the shared
//! [`oxide_core::updates`] rules every front-end reads, so this window is never
//! offered the Electron app's release or the CLI's. What a release carries is the
//! bundle itself — `Oxide-GPUI-<version>-macos-<arch>.app.tar.gz` — because this
//! app is one native binary rather than an Electron installer, and putting it in
//! place is unpacking that archive beside the running bundle and renaming it over
//! the old one.

use anyhow::{bail, Context, Result};
use oxide_core::updates::{self, Check, Component};
use std::path::{Path, PathBuf};

/// The version this build reports, written from the release tag by
/// `scripts/set-version.sh`.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// The `.app` bundle this process runs from, or `None` for a build started out
/// of `target/` — a checkout's binary is not something the app can replace, and
/// saying so is better than writing over a file nobody installed.
pub fn bundle() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    // Contents/MacOS/<binary> → Contents → <name>.app
    let bundle = exe.parent()?.parent()?.parent()?.to_path_buf();
    (bundle
        .extension()
        .is_some_and(|extension| extension == "app"))
    .then_some(bundle)
}

/// Whether this copy is one the app may replace from inside itself.
pub fn installable() -> bool {
    bundle().is_some()
}

/// Looks for the newest release of this app for this machine.
pub async fn check() -> Result<Check> {
    let platform = updates::platform()?;
    let repo = updates::repo();
    let client = updates::client()?;
    let release = updates::latest(&client, &repo, Component::Gpui, platform).await?;
    if release.asset.is_empty() {
        bail!("release {} carries no build for {platform}", release.tag);
    }
    let mut check = Check::new(Component::Gpui, &release, VERSION, false, &repo);
    check.installation = if installable() {
        "the app bundle".to_string()
    } else {
        "a local build".to_string()
    };
    check.installable = installable();
    check.path = bundle()
        .map(|bundle| bundle.display().to_string())
        .unwrap_or_default();
    if !check.installable {
        check.advice = Some(
            "This build is not an app bundle, so the app cannot replace it. \
             Download the release and install it by hand."
                .to_string(),
        );
    }
    Ok(check)
}

/// Downloads the release the check resolved, verifies it against the checksum
/// the release published, and puts the bundle in this copy's place. The running
/// process keeps the copy it started from, so what is left is quitting and
/// opening the app again.
pub async fn install(check: &Check) -> Result<PathBuf> {
    let target = bundle().context(
        "this build is not an app bundle, so there is nothing for the update to replace",
    )?;
    let asset = check
        .asset
        .clone()
        .context("the release carries no build for this platform")?;
    let client = updates::client()?;
    let work = updates::WorkDir::new()?;
    let archive = work.path().join(&asset.name);
    updates::download(&client, &asset.url, &archive).await?;
    match asset.digest.as_deref() {
        Some(digest) => updates::verify_sha256(&archive, digest)?,
        None => bail!(
            "release {} carries no checksum for {}",
            check.tag,
            asset.name
        ),
    }

    let staging = work.path().join("staging");
    std::fs::create_dir_all(&staging)?;
    let unpacked = std::process::Command::new("tar")
        .arg("-xzf")
        .arg(&archive)
        .arg("-C")
        .arg(&staging)
        .output()
        .context("unpacking the downloaded bundle")?;
    if !unpacked.status.success() {
        bail!(
            "unpacking {}: {}",
            asset.name,
            String::from_utf8_lossy(&unpacked.stderr).trim()
        );
    }
    let extracted =
        find_bundle(&staging).with_context(|| format!("{} holds no .app bundle", asset.name))?;

    // Staged beside the installed copy before the rename, so a failure leaves
    // the running app where it was rather than half replaced.
    let staged = target.with_extension("app.new");
    let _ = std::fs::remove_dir_all(&staged);
    copy_bundle(&extracted, &staged)?;
    let previous = target.with_extension("app.old");
    let _ = std::fs::remove_dir_all(&previous);
    std::fs::rename(&target, &previous)
        .with_context(|| format!("moving {} aside", target.display()))?;
    std::fs::rename(&staged, &target)
        .with_context(|| format!("putting the new bundle at {}", target.display()))?;
    let _ = std::fs::remove_dir_all(&previous);
    Ok(target)
}

/// The bundle an unpacked archive holds, whether it was archived from its own
/// directory or from a folder above it.
fn find_bundle(root: &Path) -> Option<PathBuf> {
    let entry = std::fs::read_dir(root).ok()?.flatten().find(|entry| {
        entry
            .path()
            .extension()
            .is_some_and(|extension| extension == "app")
    })?;
    Some(entry.path())
}

/// Copies a bundle, following symlinks inside it rather than copying them:
/// `Contents/MacOS` and `Frameworks` are the only shapes this bundle has, and a
/// copied symlink would point at an archive that is about to be removed.
fn copy_bundle(from: &Path, to: &Path) -> Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let source = entry.path();
        let destination = to.join(entry.file_name());
        if source.is_dir() {
            copy_bundle(&source, &destination)?;
        } else {
            std::fs::copy(&source, &destination)
                .with_context(|| format!("copying {}", source.display()))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_plain_directory_is_not_a_bundle() {
        assert_eq!(bundle(), None);
        assert!(!installable());
    }

    #[test]
    fn the_gpui_train_is_its_own_component() {
        assert_eq!(Component::Gpui.as_str(), "gpui");
        assert_eq!(Component::Gpui.tag("1.2.3"), "gpui-v1.2.3");
        assert!(Component::Gpui
            .assets("gpui-v1.2.3", "macos-arm64")
            .iter()
            .any(|name| name.ends_with("-macos-arm64.app.tar.gz")));
        assert!(!Component::Desktop
            .assets("desktop-v1.2.3", "macos-arm64")
            .iter()
            .any(|name| name.contains("GPUI")));
    }

    #[test]
    fn a_bundle_is_found_whatever_folder_the_archive_used() {
        let dir = std::env::temp_dir().join(format!("oxide_gpui_bundle_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("Oxide GPUI.app/Contents/MacOS")).unwrap();
        let found = find_bundle(&dir).unwrap();
        assert_eq!(found.file_name().unwrap(), "Oxide GPUI.app");
        let copied = dir.join("copy.app");
        copy_bundle(&found, &copied).unwrap();
        assert!(copied.join("Contents/MacOS").is_dir());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
