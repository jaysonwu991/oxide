//! The desktop app's own release train.
//!
//! The app releases from its own tags (`desktop-v0.34.0`) carrying its own
//! bundles, so what this layer offers is a desktop release rather than the
//! CLI's: the rules themselves — which tag belongs to the app, which artifact
//! this platform installs, whether a release is newer — live in
//! `oxide_core::updates`, shared with `oxide update` and the VS Code panel. What
//! is left here is what only the running app knows: where this copy is
//! installed, whether it may be replaced in place, and how.
//!
//! Nothing here touches Tauri, so both halves — whether this installation may be
//! replaced, and putting a downloaded release in its place — are unit tested
//! like the rest of the library.

use anyhow::{bail, Context, Result};
use oxide_core::update_notice::Notice;
use oxide_core::updates::{self, Artifact, Check, Component, Release, WorkDir};
use serde_json::{json, Value};
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};

/// The version this build reports. Release CI writes the tag version in before
/// building, so a released bundle answers with the release it came from.
pub fn current_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// Where this copy of the app is installed, and how a release replaces it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Installation {
    /// How a front-end names this installation: `app bundle`, `AppImage`,
    /// `installer`, `system package` (a distribution's own build, which only
    /// its package manager updates) or `source build` (a checkout's build).
    pub label: &'static str,
    /// What a release would replace, when this copy may be replaced at all.
    pub path: Option<PathBuf>,
    /// Whether the app may write over this copy in place. A bundle an
    /// administrator installed for every user of the machine is the case this
    /// rules out.
    replaceable: bool,
    kind: Kind,
}

/// What kind of installation this is, which is what the install step switches
/// on.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Kind {
    /// A macOS `.app` bundle, replaced from the release's disk image.
    Bundle(PathBuf),
    /// The AppImage this app is running from, replaced by writing its own file.
    AppImage(PathBuf),
    /// The Windows installer, run to put the release in place.
    Installer,
    /// A copy the app does not replace: a source build, or a system package.
    None,
}

impl Installation {
    fn none(label: &'static str) -> Self {
        Self {
            label,
            path: None,
            replaceable: false,
            kind: Kind::None,
        }
    }

    /// Whether this copy can be replaced from inside the app, by the artifact
    /// the release offers for this platform.
    pub fn installable(&self, asset: Option<&Artifact>) -> bool {
        let Some(asset) = asset else {
            return false;
        };
        if !self.replaceable {
            return false;
        }
        match &self.kind {
            Kind::Bundle(_) => asset.name.ends_with(".dmg"),
            Kind::AppImage(_) => asset.name.ends_with(".AppImage"),
            Kind::Installer => asset.name.ends_with("-setup.exe"),
            Kind::None => false,
        }
    }

    /// Whether this copy is one the app replaces in place without being asked:
    /// a bundle or an AppImage the user installed, which a new release simply
    /// takes the place of. A Windows installer is not — it asks for elevation
    /// and waits for the app to be closed, which is a thing to offer rather
    /// than to do behind the reader's back — and neither is a copy the app does
    /// not own at all.
    pub fn replaces_itself(&self) -> bool {
        self.replaceable && matches!(self.kind, Kind::Bundle(_) | Kind::AppImage(_))
    }

    /// What to do instead, for a copy the app will not replace itself.
    fn advice(&self, asset: Option<&Artifact>) -> String {
        let name = asset.map_or_else(|| "the release".to_string(), |asset| asset.name.clone());
        if !self.replaceable {
            if let Some(directory) = self.path.as_deref().and_then(Path::parent) {
                if !matches!(self.kind, Kind::None) {
                    return format!(
                        "Download {name} and install it yourself: {} cannot be written to from \
                         here.",
                        directory.display()
                    );
                }
            }
        }
        format!(
            "Download {name} from the release page and install it the way this copy was installed."
        )
    }
}

/// The installation this running app belongs to.
pub fn installation() -> Installation {
    let executable = std::env::current_exe().unwrap_or_default();
    let appimage = std::env::var_os("APPIMAGE")
        .map(PathBuf::from)
        .filter(|path| path.is_file());
    let installer_dir = dirs::data_local_dir().map(|data| data.join("Programs").join("Oxide"));
    installation_of(
        &executable,
        std::env::consts::OS,
        appimage.as_deref(),
        installer_dir.as_deref(),
    )
}

/// The installation an executable belongs to. `os` is the platform's name,
/// which is what decides which of a release's artifacts replaces it; the two
/// paths are the machine's own — where an AppImage runs from, and the directory
/// the Windows installer installs into — and are passed in so both are tested
/// without either platform.
pub fn installation_of(
    executable: &Path,
    os: &str,
    appimage: Option<&Path>,
    installer_dir: Option<&Path>,
) -> Installation {
    match os {
        "macos" => match bundle_of(executable) {
            Some(bundle) => Installation {
                label: "app bundle",
                replaceable: bundle.parent().is_some_and(writable),
                path: Some(bundle.clone()),
                kind: Kind::Bundle(bundle),
            },
            None => Installation::none("source build"),
        },
        "linux" => match appimage {
            Some(appimage) => Installation {
                label: "AppImage",
                replaceable: appimage.parent().is_some_and(writable),
                path: Some(appimage.to_path_buf()),
                kind: Kind::AppImage(appimage.to_path_buf()),
            },
            None => Installation::none("system package"),
        },
        "windows" => {
            if installer_dir.is_some_and(|dir| executable.starts_with(dir)) {
                Installation {
                    label: "installer",
                    path: executable.parent().map(Path::to_path_buf),
                    replaceable: true,
                    kind: Kind::Installer,
                }
            } else {
                Installation::none("source build")
            }
        }
        _ => Installation::none("source build"),
    }
}

/// The `.app` bundle an executable belongs to. The app's layout puts the binary
/// at `Oxide.app/Contents/MacOS/oxide-desktop`, so the bundle is three
/// directories up — and only when that directory really is one, which is what
/// tells a released app from the binary beside it in a `target/debug` build.
fn bundle_of(executable: &Path) -> Option<PathBuf> {
    let contents = executable.parent()?.parent()?;
    let bundle = contents.parent()?;
    let is_bundle = contents.file_name() == Some(OsStr::new("Contents"))
        && bundle.extension() == Some(OsStr::new("app"))
        && contents.join("Info.plist").is_file();
    is_bundle.then(|| bundle.to_path_buf())
}

/// Whether this process may write into `directory`. The probe is the honest
/// test — a mode bit says nothing about who is running — and it is what tells
/// an app the user installed in `/Applications` from one an administrator put
/// there for everyone.
fn writable(directory: &Path) -> bool {
    let probe = directory.join(format!(".oxide-install-probe-{}", std::process::id()));
    match fs::write(&probe, b"") {
        Ok(()) => {
            let _ = fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}

/// The newest release of the app, with the artifact this platform installs.
pub async fn resolve(client: &reqwest::Client, repo: &str) -> Result<Release> {
    let platform = updates::platform()?;
    updates::latest(client, repo, Component::Desktop, platform).await
}

/// The check a window paints, from a release already resolved: the desktop's own
/// train, and whether this installation can be replaced from here.
pub fn check_for(
    release: &Release,
    installation: &Installation,
    repo: &str,
    current: &str,
) -> Check {
    let mut check = Check::new(Component::Desktop, release, current, false, repo);
    check.installation = installation.label.to_string();
    check.path = installation
        .path
        .as_ref()
        .map(|path| path.display().to_string())
        .unwrap_or_default();
    check.installable = check.update_available && installation.installable(check.asset.as_ref());
    check.advice = (check.update_available && !check.installable)
        .then(|| installation.advice(check.asset.as_ref()));
    check
}

/// Checks the app's own release train.
pub async fn check() -> Result<Check> {
    let client = updates::client()?;
    let repo = updates::repo();
    let release = resolve(&client, &repo).await?;
    Ok(check_for(
        &release,
        &installation(),
        &repo,
        current_version(),
    ))
}

/// Whether a launch installs a release on its own: the newest release of this
/// app's train, newer than the build running, in a copy the app replaces in
/// place. Everything else — a release for a copy the user installed elsewhere,
/// a Windows installer, a checkout's build — is the window's dialog to offer
/// instead, since only the reader can decide to do it.
pub fn launch_installs(notice: &Notice, current: &str, installation: &Installation) -> bool {
    notice.is_update_for(current) && installation.replaces_itself()
}

/// Installs the newest release of the app, for a caller with no progress to
/// show — `restart` is what runs it either way. See [`install_reporting`] for
/// what the install does, step by step.
pub async fn install() -> Result<Value> {
    install_reporting(|_| {}).await
}

/// What an install is doing, reported as it goes so a front-end can paint the
/// wait rather than leave a release that is being put in place unexplained.
#[derive(Debug, Clone)]
pub struct Progress {
    /// The step in progress: `checking`, `downloading`, `verifying`,
    /// `installing`.
    pub stage: &'static str,
    /// The release being installed, known once it has been resolved.
    pub version: String,
}

/// An install in flight, process-wide. The launch installs a release without
/// being asked while the window's own dialog can be asked for the same one, and
/// two installs staging and renaming the same bundle is how an installation is
/// lost.
static INSTALLING: AtomicBool = AtomicBool::new(false);

/// The right to install, released when it goes out of scope.
#[derive(Debug)]
struct InstallGuard;

impl InstallGuard {
    fn take() -> Result<Self> {
        if INSTALLING.swap(true, Ordering::SeqCst) {
            bail!("an Oxide update is already being installed");
        }
        Ok(Self)
    }
}

impl Drop for InstallGuard {
    fn drop(&mut self) {
        INSTALLING.store(false, Ordering::SeqCst);
    }
}

/// Installs the newest release of the app, reporting each step to `report`.
///
/// The release is resolved again rather than taken from the check, so one
/// published in the meantime is the one that lands, and the download is checked
/// against the digest its release published before it goes anywhere near this
/// installation.
pub async fn install_reporting(report: impl Fn(Progress)) -> Result<Value> {
    let _installing = InstallGuard::take()?;
    report(Progress {
        stage: "checking",
        version: String::new(),
    });
    let client = updates::client()?;
    let repo = updates::repo();
    let release = resolve(&client, &repo).await?;
    let current = current_version();
    if !updates::is_newer(&release.version, current) {
        bail!("Oxide {current} is the newest release, so there is nothing to install");
    }
    let installation = installation();
    let artifact = release.artifact(&repo);
    if !installation.installable(artifact.as_ref()) {
        bail!(
            "{} cannot be replaced from inside the app; {} is on the release page",
            installation.label,
            release.asset
        );
    }

    report(Progress {
        stage: "downloading",
        version: release.version.clone(),
    });
    let work = WorkDir::new()?;
    let download = work.path().join(&release.asset);
    let bytes = updates::download(&client, &release.url(&repo), &download).await?;
    let mut notes = Vec::new();
    report(Progress {
        stage: "verifying",
        version: release.version.clone(),
    });
    match release.digest.as_deref() {
        Some(digest) => updates::verify_sha256(&download, digest)?,
        None => notes.push("no checksum was published for this release".to_string()),
    }
    report(Progress {
        stage: "installing",
        version: release.version.clone(),
    });
    let placed = install_downloaded(&installation, &release.version, &download, &work)?;
    notes.push(placed.note);

    Ok(json!({
        "ok": true,
        // A Windows installer is started rather than run to completion: it asks
        // for elevation and waits for this app to be closed, so only it knows
        // whether the release landed. The dialog reports the launch it is
        // instead of claiming the version is installed.
        "pending": placed.pending,
        "version": release.version,
        "tag": release.tag,
        "asset": release.asset,
        "bytes": bytes,
        "path": installation.path.as_ref().map(|path| path.display().to_string()).unwrap_or_default(),
        "text": notes.join(" "),
    }))
}

/// What an install left: the note a dialog shows, and whether the release is in
/// place at all. Only a Windows installer can leave that in doubt — it is
/// started as its own process, which this app cannot outlive reporting on.
#[derive(Debug)]
struct Placed {
    note: String,
    pending: bool,
}

impl Placed {
    fn in_place(note: String) -> Self {
        Self {
            note,
            pending: false,
        }
    }
}

/// Puts a downloaded release in this installation's place, answering with what
/// to tell the reader. Each kind of installation is replaced the way it was
/// installed.
fn install_downloaded(
    installation: &Installation,
    version: &str,
    download: &Path,
    work: &WorkDir,
) -> Result<Placed> {
    match &installation.kind {
        Kind::Bundle(bundle) => {
            install_bundle(download, bundle, work)?;
            Ok(Placed::in_place(format!(
                "Oxide {version} is in {}. Quit Oxide and open it again to run the new version.",
                bundle.display()
            )))
        }
        Kind::AppImage(target) => {
            replace_file(download, target)?;
            Ok(Placed::in_place(format!(
                "Oxide {version} is in place at {}. Quit Oxide and open it again to run the new \
                 version.",
                target.display()
            )))
        }
        Kind::Installer => {
            // The installer owns the installation from here: it replaces the
            // files and asks for the running app to be closed. Starting it is
            // all this side can know — it may still be waiting for elevation,
            // and it may be cancelled — so the release is reported as pending
            // rather than installed.
            Command::new(download)
                .spawn()
                .with_context(|| format!("running {}", download.display()))?;
            Ok(Placed {
                note: format!(
                    "The Oxide {version} installer is running. Finish it, then open Oxide again \
                     to run the new version."
                ),
                pending: true,
            })
        }
        Kind::None => bail!(
            "{} cannot be replaced from inside the app",
            installation.label
        ),
    }
}

/// Replaces a macOS app bundle with the one the release's disk image holds.
///
/// The image is mounted read-only and the app is copied out of it into the
/// bundle's own directory first, so both renames at the end stay within one
/// filesystem and the copy that can fail happens before anything is moved.
fn install_bundle(image: &Path, bundle: &Path, work: &WorkDir) -> Result<()> {
    let mount = work.path().join("mount");
    fs::create_dir_all(&mount).with_context(|| format!("creating {}", mount.display()))?;
    attach(image, &mount)?;
    let placed = (|| {
        let fresh = stage(&find_app(&mount)?, bundle)?;
        swap_in(&fresh, bundle)
    })();
    let detached = detach(&mount);
    placed.and(detached)
}

fn attach(image: &Path, mount: &Path) -> Result<()> {
    let mut command = Command::new("hdiutil");
    command
        .arg("attach")
        // Nothing is written to the image, and a check must not put a volume in
        // the Finder's sidebar.
        .arg("-nobrowse")
        .arg("-readonly")
        .arg("-mountpoint")
        .arg(mount)
        .arg(image);
    run(&mut command, "mounting the downloaded disk image")
}

fn detach(mount: &Path) -> Result<()> {
    let mut command = Command::new("hdiutil");
    command.arg("detach").arg(mount);
    run(&mut command, "unmounting the downloaded disk image")
}

/// The app a mounted image holds.
fn find_app(mount: &Path) -> Result<PathBuf> {
    let entries = fs::read_dir(mount).with_context(|| format!("reading {}", mount.display()))?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension() == Some(OsStr::new("app")) && path.is_dir() {
            return Ok(path);
        }
    }
    bail!("{} holds no .app bundle", mount.display())
}

/// Copies the app out of the mounted image to a hidden sibling of the bundle it
/// replaces. A copy leaves the installed app untouched until the whole new one
/// is on disk.
fn stage(source: &Path, bundle: &Path) -> Result<PathBuf> {
    let fresh = sibling(bundle, "new");
    remove(&fresh);
    let mut command = Command::new("ditto");
    command.arg(source).arg(&fresh);
    run(&mut command, "copying the downloaded app")?;
    Ok(fresh)
}

/// Moves a staged app into `bundle`'s place, keeping the copy that was there
/// until the new one is in place and putting it back if it cannot be.
fn swap_in(fresh: &Path, bundle: &Path) -> Result<()> {
    let previous = sibling(bundle, "old");
    remove(&previous);
    fs::rename(bundle, &previous).with_context(|| format!("moving {} aside", bundle.display()))?;
    if let Err(error) = fs::rename(fresh, bundle) {
        let _ = fs::rename(&previous, bundle);
        return Err(error).with_context(|| format!("putting the new app in {}", bundle.display()));
    }
    // The files just moved aside are the ones this process is running from;
    // macOS keeps them open until it quits, so the removal is best effort.
    remove(&previous);
    Ok(())
}

/// A hidden sibling of `bundle`, named so it is not mistaken for an app.
fn sibling(bundle: &Path, suffix: &str) -> PathBuf {
    let name = bundle
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| "Oxide.app".to_string());
    bundle.with_file_name(format!(".{name}.{suffix}"))
}

fn remove(path: &Path) {
    if path.is_dir() {
        let _ = fs::remove_dir_all(path);
    } else {
        let _ = fs::remove_file(path);
    }
}

/// Writes a downloaded AppImage over the file this app runs from. The new file
/// is written beside the old one, so the last step is a rename within one
/// filesystem; the app that is running keeps reading the file it opened.
fn replace_file(download: &Path, target: &Path) -> Result<()> {
    let fresh = target.with_file_name(format!(
        ".{}.new",
        target
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_else(|| "oxide".to_string())
    ));
    fs::copy(download, &fresh).with_context(|| format!("writing {}", fresh.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&fresh, fs::Permissions::from_mode(0o755))
            .with_context(|| format!("making {} executable", fresh.display()))?;
    }
    if let Err(error) = fs::rename(&fresh, target) {
        remove(&fresh);
        return Err(error).with_context(|| format!("replacing {}", target.display()));
    }
    Ok(())
}

/// Runs a program the platform provides, reporting what it said when it fails.
fn run(command: &mut Command, what: &str) -> Result<()> {
    let program = command.get_program().to_string_lossy().to_string();
    let output = command
        .output()
        .with_context(|| format!("{what} ({program} failed to start; is it on PATH?)"))?;
    if !output.status.success() {
        let said = String::from_utf8_lossy(&output.stderr).trim().to_string();
        if said.is_empty() {
            bail!("{what}: {program} failed with {}", output.status);
        }
        bail!("{what}: {said}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_dir(tag: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path =
            std::env::temp_dir().join(format!("oxide-app-{tag}-{}-{nonce}", std::process::id()));
        fs::create_dir_all(&path).unwrap();
        path
    }

    /// A release of the app carrying `asset`, as GitHub's API reports it.
    fn release(asset: &str) -> Release {
        Release {
            tag: "desktop-v0.34.0".to_string(),
            version: "0.34.0".to_string(),
            asset: asset.to_string(),
            digest: Some(format!("sha256:{}", "0".repeat(64))),
        }
    }

    fn bundle_fixture(root: &Path) -> PathBuf {
        let bundle = root.join("Oxide.app");
        let binary = bundle.join("Contents/MacOS/oxide-desktop");
        fs::create_dir_all(binary.parent().unwrap()).unwrap();
        fs::write(&binary, b"binary").unwrap();
        fs::write(bundle.join("Contents/Info.plist"), b"plist").unwrap();
        bundle
    }

    fn left_behind(root: &Path, keep: &[&str]) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(root)
            .unwrap()
            .filter_map(Result::ok)
            .map(|entry| entry.file_name().to_string_lossy().to_string())
            .filter(|name| !keep.contains(&name.as_str()))
            .collect();
        names.sort();
        names
    }

    #[test]
    fn a_running_bundle_is_replaced_from_the_release_image() {
        let root = temp_dir("bundle");
        let bundle = bundle_fixture(&root);
        let binary = bundle.join("Contents/MacOS/oxide-desktop");
        let installation = installation_of(&binary, "macos", None, None);
        assert_eq!(installation.label, "app bundle");
        assert_eq!(installation.path.as_deref(), Some(bundle.as_path()));

        // The disk image is what replaces it, so the app installs the release
        // itself and there is nothing to advise instead.
        let check = check_for(
            &release("Oxide_0.34.0_aarch64.dmg"),
            &installation,
            "acme/oxide",
            "0.33.0",
        );
        assert!(check.update_available && check.installable);
        assert_eq!(check.component, Component::Desktop);
        assert_eq!(check.tag, "desktop-v0.34.0");
        assert_eq!(
            check.release_url,
            "https://github.com/acme/oxide/releases/tag/desktop-v0.34.0"
        );
        assert_eq!(check.path, bundle.display().to_string());
        assert_eq!(check.advice, None);

        // Running the newest release has nothing to offer, whatever the
        // installation is.
        let fresh = check_for(
            &release("Oxide_0.34.0_aarch64.dmg"),
            &installation,
            "acme/oxide",
            "0.34.0",
        );
        assert!(!fresh.update_available && !fresh.installable && fresh.advice.is_none());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_build_outside_a_bundle_is_left_to_the_reader() {
        let root = temp_dir("checkout");
        let binary = root.join("target/debug/oxide-desktop");
        fs::create_dir_all(binary.parent().unwrap()).unwrap();
        fs::write(&binary, b"binary").unwrap();

        let installation = installation_of(&binary, "macos", None, None);
        assert_eq!(installation.label, "source build");
        assert_eq!(installation.path, None);

        let check = check_for(
            &release("Oxide_0.34.0_aarch64.dmg"),
            &installation,
            "acme/oxide",
            "0.0.0",
        );
        assert!(check.update_available && !check.installable);
        assert!(check
            .advice
            .as_deref()
            .unwrap()
            .contains("Oxide_0.34.0_aarch64.dmg"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn an_appimage_is_replaced_where_it_runs_from() {
        let root = temp_dir("appimage");
        let image = root.join("Oxide.AppImage");
        fs::write(&image, b"old image").unwrap();
        // What a mounted AppImage runs from, which is not the file itself.
        let binary = root.join("squashfs-root/oxide-desktop");
        fs::create_dir_all(binary.parent().unwrap()).unwrap();
        fs::write(&binary, b"binary").unwrap();

        let installation = installation_of(&binary, "linux", Some(&image), None);
        assert_eq!(installation.label, "AppImage");
        assert_eq!(installation.path.as_deref(), Some(image.as_path()));
        assert!(
            check_for(
                &release("Oxide_0.34.0_amd64.AppImage"),
                &installation,
                "acme/oxide",
                "0.33.0"
            )
            .installable
        );

        // A release whose package is not the AppImage is not something this
        // copy installs from.
        let deb = check_for(
            &release("Oxide_0.34.0_amd64.deb"),
            &installation,
            "acme/oxide",
            "0.33.0",
        );
        assert!(deb.update_available && !deb.installable);
        assert!(deb.advice.is_some());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_linux_copy_that_is_not_an_appimage_names_the_download() {
        let root = temp_dir("system");
        let binary = root.join("usr/lib/oxide/oxide-desktop");
        fs::create_dir_all(binary.parent().unwrap()).unwrap();
        fs::write(&binary, b"binary").unwrap();

        let installation = installation_of(&binary, "linux", None, None);
        assert_eq!(installation.label, "system package");

        let check = check_for(
            &release("Oxide_0.34.0_amd64.deb"),
            &installation,
            "acme/oxide",
            "0.33.0",
        );
        assert!(check.update_available && !check.installable);
        assert!(
            check.advice.unwrap().contains("Oxide_0.34.0_amd64.deb"),
            "the download is named"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn the_windows_installer_owns_the_directory_it_installs_into() {
        let root = temp_dir("windows");
        let install_dir = root.join("Programs/Oxide");
        let binary = install_dir.join("oxide-desktop.exe");
        fs::create_dir_all(&install_dir).unwrap();
        fs::write(&binary, b"binary").unwrap();

        let installation = installation_of(&binary, "windows", None, Some(&install_dir));
        assert_eq!(installation.label, "installer");
        assert_eq!(installation.path.as_deref(), Some(install_dir.as_path()));
        // The setup is what replaces the installation it made; a release that
        // published only an MSI has nothing the app can run itself.
        assert!(installation.installable(
            release("Oxide_0.34.0_x64-setup.exe")
                .artifact("acme/oxide")
                .as_ref()
        ));
        assert!(!installation.installable(
            release("Oxide_0.34.0_x64_en-US.msi")
                .artifact("acme/oxide")
                .as_ref()
        ));

        // A build run from a checkout is not the installation a setup replaces.
        let checkout = root.join("target/debug/oxide-desktop.exe");
        fs::create_dir_all(checkout.parent().unwrap()).unwrap();
        fs::write(&checkout, b"binary").unwrap();
        let source = installation_of(&checkout, "windows", None, Some(&install_dir));
        assert_eq!(source.label, "source build");
        assert!(!source.installable(
            release("Oxide_0.34.0_x64-setup.exe")
                .artifact("acme/oxide")
                .as_ref()
        ));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn writability_is_probed_rather_than_read_off_the_mode_bits() {
        let root = temp_dir("writable");
        assert!(writable(&root));
        assert!(!writable(&root.join("missing")));
        assert!(
            left_behind(&root, &[]).is_empty(),
            "the probe was cleaned up"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn the_app_in_a_mounted_image_is_the_one_installed() {
        let root = temp_dir("mount");
        let app = root.join("Oxide.app");
        fs::create_dir_all(&app).unwrap();
        fs::write(root.join("README"), b"read me").unwrap();
        fs::write(root.join("Oxide.webloc"), b"link").unwrap();
        assert_eq!(find_app(&root).unwrap(), app);

        let plain = temp_dir("plain-mount");
        fs::write(plain.join("README"), b"read me").unwrap();
        assert!(find_app(&plain)
            .unwrap_err()
            .to_string()
            .contains("holds no .app bundle"));
        fs::remove_dir_all(root).unwrap();
        fs::remove_dir_all(plain).unwrap();
    }

    #[test]
    fn a_staged_app_replaces_the_bundle_and_the_old_one_is_thrown_away() {
        let root = temp_dir("swap");
        let bundle = bundle_fixture(&root);
        let fresh = sibling(&bundle, "new");
        fs::create_dir_all(fresh.join("Contents/MacOS")).unwrap();
        fs::write(fresh.join("Contents/MacOS/oxide-desktop"), b"new binary").unwrap();

        swap_in(&fresh, &bundle).unwrap();

        assert_eq!(
            fs::read(bundle.join("Contents/MacOS/oxide-desktop")).unwrap(),
            b"new binary"
        );
        assert!(
            left_behind(&root, &["Oxide.app"]).is_empty(),
            "nothing else is left"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_swap_that_cannot_place_the_new_app_puts_the_old_one_back() {
        let root = temp_dir("swap-failed");
        let bundle = bundle_fixture(&root);
        // A staged app that is not there: the copy that could not be made.
        let fresh = sibling(&bundle, "new");

        let error = swap_in(&fresh, &bundle).unwrap_err();

        assert!(
            error.to_string().contains("putting the new app in"),
            "{error}"
        );
        assert_eq!(
            fs::read(bundle.join("Contents/MacOS/oxide-desktop")).unwrap(),
            b"binary"
        );
        assert!(
            left_behind(&root, &["Oxide.app"]).is_empty(),
            "nothing else is left"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_downloaded_appimage_replaces_the_file_it_runs_from() {
        let root = temp_dir("appimage-install");
        let image = root.join("Oxide.AppImage");
        fs::write(&image, b"old image").unwrap();
        let download = root.join("downloaded");
        fs::write(&download, b"new image").unwrap();
        let installation = installation_of(
            &root.join("squashfs-root/oxide-desktop"),
            "linux",
            Some(&image),
            None,
        );

        let work = WorkDir::new().unwrap();
        let placed = install_downloaded(&installation, "0.34.0", &download, &work).unwrap();

        assert!(placed.note.contains("Oxide 0.34.0"), "{}", placed.note);
        assert!(!placed.pending, "the file is in place, not pending");
        assert_eq!(fs::read(&image).unwrap(), b"new image");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&image).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o755);
        }
        assert!(
            left_behind(&root, &["Oxide.AppImage", "downloaded"]).is_empty(),
            "nothing else is left"
        );
        fs::remove_dir_all(root).unwrap();
    }

    // A Windows installer is a process of its own, which this app starts and
    // cannot wait on: it asks for elevation and for Oxide to be closed. What the
    // install knows is that it is running, so that is what it reports rather
    // than a version it cannot claim is in place.
    #[cfg(unix)]
    #[test]
    fn a_windows_installer_is_reported_as_started_rather_than_installed() {
        use std::os::unix::fs::PermissionsExt;

        let root = temp_dir("installer");
        let directory = root.join("Programs/Oxide");
        fs::create_dir_all(&directory).unwrap();
        // The setup a Windows release publishes; on Unix a script stands in for
        // the installer that is started and left to finish.
        let setup = root.join("Oxide_0.34.0_x64-setup.exe");
        fs::write(&setup, b"#!/bin/sh\nexit 0\n").unwrap();
        let mut mode = fs::metadata(&setup).unwrap().permissions();
        mode.set_mode(0o755);
        fs::set_permissions(&setup, mode).unwrap();
        let installation = installation_of(
            &directory.join("oxide-desktop.exe"),
            "windows",
            None,
            Some(&directory),
        );

        let work = WorkDir::new().unwrap();
        let placed = install_downloaded(&installation, "0.34.0", &setup, &work).unwrap();

        assert!(placed.pending, "the installer owns what happens next");
        assert!(
            placed.note.contains("installer is running"),
            "{}",
            placed.note
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_copy_the_app_does_not_replace_refuses_to_install() {
        let root = temp_dir("refuse");
        let binary = root.join("target/debug/oxide-desktop");
        fs::create_dir_all(binary.parent().unwrap()).unwrap();
        let installation = installation_of(&binary, "macos", None, None);

        let work = WorkDir::new().unwrap();
        let error = install_downloaded(&installation, "0.34.0", &binary, &work).unwrap_err();

        assert!(
            error
                .to_string()
                .contains("cannot be replaced from inside the app"),
            "{error}"
        );
        fs::remove_dir_all(root).unwrap();
    }

    // A launch installs a release on its own only where the app owns the copy
    // it would write over, and only when the release is newer than the build
    // running: everything else is the reader's to decide, through the dialog.
    #[test]
    fn a_launch_installs_only_into_a_copy_it_owns() {
        let root = temp_dir("launch");
        let bundle = bundle_fixture(&root);
        let bundle_install = installation_of(
            &bundle.join("Contents/MacOS/oxide-desktop"),
            "macos",
            None,
            None,
        );
        let notice =
            Notice::from_release(&release("Oxide_0.34.0_aarch64.dmg"), "acme/oxide", 1_000);

        // A bundle the user installed, with a newer release to hand: done
        // without asking, since the app is the one that put it there.
        assert!(launch_installs(&notice, "0.33.0", &bundle_install));
        // The running build is that release, so there is nothing to install.
        assert!(!launch_installs(&notice, "0.34.0", &bundle_install));
        assert!(!launch_installs(&notice, "0.35.0", &bundle_install));

        // An AppImage is the app's own file just the same.
        let image = root.join("Oxide.AppImage");
        fs::write(&image, b"image").unwrap();
        let appimage = installation_of(
            &root.join("squashfs-root/oxide-desktop"),
            "linux",
            Some(&image),
            None,
        );
        assert!(launch_installs(&notice, "0.33.0", &appimage));

        // A checkout's build belongs to whoever is working in it, and a Windows
        // installer asks for elevation and for the app to be closed — neither is
        // a launch's to do.
        let checkout = installation_of(
            &root.join("target/debug/oxide-desktop"),
            "macos",
            None,
            None,
        );
        assert!(!launch_installs(&notice, "0.33.0", &checkout));
        let directory = root.join("Programs/Oxide");
        fs::create_dir_all(&directory).unwrap();
        let installer = installation_of(
            &directory.join("oxide-desktop.exe"),
            "windows",
            None,
            Some(&directory),
        );
        assert!(!launch_installs(&notice, "0.33.0", &installer));

        // A distribution's package is updated by its package manager.
        let system = installation_of(&root.join("usr/bin/oxide-desktop"), "linux", None, None);
        assert!(!launch_installs(&notice, "0.33.0", &system));
        fs::remove_dir_all(root).unwrap();
    }

    // The launch's own install and the dialog's are the same install, so a
    // second one is refused while the first is staging the same bundle.
    #[test]
    fn only_one_install_runs_at_a_time() {
        let held = InstallGuard::take().unwrap();
        let refused = InstallGuard::take().unwrap_err();
        assert!(
            refused.to_string().contains("already being installed"),
            "{refused}"
        );
        // The right to install goes with the guard, so the window can install
        // once a launch's own install has finished.
        drop(held);
        assert!(InstallGuard::take().is_ok());
    }
}
