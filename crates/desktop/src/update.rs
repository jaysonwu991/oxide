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
//! Nothing here touches a window either, so both halves — whether this
//! installation may be replaced, and putting a downloaded release in its place —
//! are unit tested like the rest of the library.

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
    /// How a front-end names this installation: `app bundle`, `AppImage` (the
    /// Linux build that runs from one file), `installer` (the copy a Windows
    /// setup made), or `source build` (a checkout's build, which the app does
    /// not replace).
    pub label: &'static str,
    /// What a release would replace, when this copy may be replaced at all.
    pub path: Option<PathBuf>,
    /// Whether the app may write over this copy in place. An installation an
    /// administrator made for every user of the machine is the case this rules
    /// out, as is a distribution's own package.
    replaceable: bool,
    kind: Kind,
}

/// What kind of installation this is, which is what the install step switches
/// on.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Kind {
    /// A macOS `.app` bundle, replaced from the release's disk image.
    Bundle(PathBuf),
    /// A Linux AppImage: the release's own AppImage is written over this file,
    /// which is the whole installation.
    AppImage(PathBuf),
    /// A Windows installation a setup made, replaced by running the release's
    /// installer, which writes over the files it put there.
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
            Kind::Installer => asset.name.ends_with("-Setup.exe"),
            Kind::None => false,
        }
    }

    /// Whether this copy is one the app replaces in place without being asked:
    /// a bundle or an AppImage the user installed, which a new release simply
    /// takes the place of. An installer is not — it starts a setup that writes
    /// over the installation and may ask for elevation, which is a thing to
    /// offer rather than to do behind the reader's back — and neither is a copy
    /// the app does not own at all.
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
    let appimage = std::env::var_os("APPIMAGE").map(PathBuf::from);
    installation_of(
        &executable,
        std::env::consts::OS,
        data_root().as_deref(),
        appimage.as_deref(),
    )
}

/// Where a Windows installer puts a per-user application:
/// `%LOCALAPPDATA%\Programs\<app>`, which is the directory a setup it made
/// writes over. An AppImage is not installed at all — it is a file the user
/// keeps — and macOS answers from the bundle instead.
fn data_root() -> Option<PathBuf> {
    dirs::data_local_dir()
}

/// The record an installation of this app carries. Electron's Windows setup
/// writes the uninstaller beside the program it installed, named after the
/// product, which is what tells a copy it made from a folder of files that
/// happens to hold the program.
const INSTALL_MARKERS: [&str; 2] = ["Uninstall Oxide.exe", "uninstall.exe"];

/// The installation an executable belongs to. `os` is the platform's name,
/// which is what decides which of a release's artifacts replaces it;
/// `data_root` is the machine's own directory of application data, which is
/// where a Windows setup makes the directory it installs into; and `appimage`
/// is the AppImage this process was started from, which the AppImage runtime
/// passes in its own environment — the executable inside a mounted AppImage is
/// an unpacked copy, so its own path says nothing. All three are passed in so
/// every platform's layout is tested without either platform.
pub fn installation_of(
    executable: &Path,
    os: &str,
    data_root: Option<&Path>,
    appimage: Option<&Path>,
) -> Installation {
    if os == "macos" {
        return match bundle_of(executable) {
            Some(bundle) => Installation {
                label: "app bundle",
                replaceable: bundle.parent().is_some_and(writable),
                path: Some(bundle.clone()),
                kind: Kind::Bundle(bundle),
            },
            None => Installation::none("source build"),
        };
    }
    if let Some(image) = appimage {
        return Installation {
            label: "AppImage",
            replaceable: image.parent().is_some_and(writable),
            path: Some(image.to_path_buf()),
            kind: Kind::AppImage(image.to_path_buf()),
        };
    }
    match install_root_of(executable, data_root) {
        Some(root) => Installation {
            label: "installer",
            replaceable: writable(&root),
            path: Some(root),
            kind: Kind::Installer,
        },
        None => Installation::none("source build"),
    }
}

/// The install root this executable lives in: an installer's own directory
/// under the machine's application data, marked with the uninstaller it leaves
/// behind. The marker is what tells an installed copy from one in a checkout,
/// and the executable has to be inside the directory — another application's
/// install root says nothing about this one.
fn install_root_of(executable: &Path, data_root: Option<&Path>) -> Option<PathBuf> {
    let data_root = data_root?;
    for identifiers in fs::read_dir(data_root).ok()?.flatten() {
        if !identifiers.file_type().is_ok_and(|kind| kind.is_dir()) {
            continue;
        }
        let Ok(roots) = fs::read_dir(identifiers.path()) else {
            continue;
        };
        for root in roots.flatten() {
            let path = root.path();
            if !path.is_dir() || !executable.starts_with(&path) {
                continue;
            }
            if INSTALL_MARKERS
                .iter()
                .any(|marker| path.join(marker).is_file())
            {
                return Some(path);
            }
        }
    }
    None
}

/// The `.app` bundle an executable belongs to: the closest ancestor that is one.
///
/// How far the program sits from the bundle says nothing — Electron runs the
/// app from `Oxide.app/Contents/MacOS/<program>`, and this app's engine from
/// `Oxide.app/Contents/Resources/harness/oxide-desktop` — so what makes a
/// directory the bundle is its `Contents/` holding an `Info.plist`, which is
/// also what tells a released app from the binary beside it in a `target/debug`
/// build.
fn bundle_of(executable: &Path) -> Option<PathBuf> {
    executable
        .ancestors()
        .find(|ancestor| {
            ancestor.extension() == Some(OsStr::new("app"))
                && ancestor.join("Contents").join("Info.plist").is_file()
        })
        .map(Path::to_path_buf)
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

/// What an install is doing, reported as it goes so a front-end can paint the
/// wait rather than leave a release that is being put in place unexplained.
#[derive(Debug, Clone)]
pub struct Progress {
    /// The step in progress: `checking`, `downloading`, `verifying`,
    /// `installing`.
    pub stage: &'static str,
    /// The release being installed, known once it has been resolved.
    pub version: String,
    /// How much of the release has been written, and the size the response
    /// announced. They are what a progress bar moves on, and both are empty
    /// outside the download: `total` is `None` when no size was announced.
    pub received: u64,
    pub total: Option<u64>,
}

impl Progress {
    /// A step that carries no download: the ones before the release is
    /// fetched and the ones after it is on disk.
    fn step(stage: &'static str, version: impl Into<String>) -> Self {
        Self {
            stage,
            version: version.into(),
            received: 0,
            total: None,
        }
    }
}

/// What an install a reader cancelled reports. A cancel is not a failure — the
/// reader asked for it — so the callers that would otherwise paint one look for
/// this first.
pub const CANCELLED: &str = "the update was cancelled";

/// A cancel asked for from the window: the download in flight stops at its next
/// chunk and the install gives up before anything is put in place. Cleared as an
/// install starts, so a cancelled release can be installed again.
static CANCEL: AtomicBool = AtomicBool::new(false);

/// Whether an error an install reported is a cancel rather than a failure.
pub fn was_cancelled(error: &anyhow::Error) -> bool {
    error.to_string() == CANCELLED
}

/// Asks the install in flight to stop. Nothing happens when none is running,
/// and what has been downloaded is left in this run's scratch, which is removed
/// as the install returns.
pub fn cancel() {
    CANCEL.store(true, Ordering::SeqCst);
}

fn cancelled() -> bool {
    CANCEL.load(Ordering::SeqCst)
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
/// installation. The download reports itself as it goes and is the step a
/// reader's cancel lands on: [`cancel`] stops it where it is.
pub async fn install_reporting(report: impl Fn(Progress)) -> Result<Value> {
    let _installing = InstallGuard::take()?;
    CANCEL.store(false, Ordering::SeqCst);
    report(Progress::step("checking", ""));
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

    report(Progress::step("downloading", release.version.clone()));
    let work = WorkDir::new()?;
    let download = work.path().join(&release.asset);
    let mut reported = 0u64;
    let bytes = updates::download_reporting(
        &client,
        &release.url(&repo),
        &download,
        |received, total| {
            // A bar moves in percent, and a release that announced no size
            // moves in whole megabytes. One event per chunk would be thousands
            // of them for a bundle, each one a packet the window repaints.
            let step = total.map(|total| (total / 100).max(1)).unwrap_or(1 << 20);
            if received / step != reported / step {
                reported = received;
                report(Progress {
                    stage: "downloading",
                    version: release.version.clone(),
                    received,
                    total,
                });
            }
            !cancelled()
        },
    )
    .await;
    let bytes = match bytes {
        Ok(bytes) => bytes,
        // The reader stopped it, which is not a failure to report but is still
        // not a release to go on installing: nothing has been put in place.
        Err(_) if cancelled() => bail!(CANCELLED),
        Err(error) => return Err(error),
    };
    let mut notes = Vec::new();
    report(Progress::step("verifying", release.version.clone()));
    match release.digest.as_deref() {
        Some(digest) => updates::verify_sha256(&download, digest)?,
        None => notes.push("no checksum was published for this release".to_string()),
    }
    report(Progress::step("installing", release.version.clone()));
    let placed = install_downloaded(
        &installation,
        &release.version,
        &download,
        &work,
        &setup_staging(),
    )?;
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
    staging: &Path,
) -> Result<Placed> {
    match &installation.kind {
        Kind::Bundle(bundle) => {
            install_bundle(download, bundle, work)?;
            Ok(Placed::in_place(format!(
                "Oxide {version} is in {}. Quit Oxide and open it again to run the new version.",
                bundle.display()
            )))
        }
        Kind::AppImage(image) => {
            install_appimage(download, image)?;
            Ok(Placed::in_place(format!(
                "Oxide {version} is in {}. Quit Oxide and open it again to run the new version.",
                image.display()
            )))
        }
        Kind::Installer => {
            // The installer owns the installation from here: it writes over the
            // files and waits for the running app to be closed. Starting it is
            // all this side can know — it may still be on screen — so the
            // release is reported as pending rather than installed. It is
            // started from its own directory beside the app's state rather than
            // from this run's scratch, which is removed as the call returns and
            // would take the program out from under the installer still reading
            // it.
            let staged = stage_setup(download, staging)?;
            Command::new(&staged).spawn().with_context(|| {
                format!("starting the downloaded installer {}", staged.display())
            })?;
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

/// Copies a downloaded setup program into a directory of its own and returns
/// the copy to start.
///
/// The download lands in this run's scratch directory, which is removed as the
/// install returns — a program started from there would be reading a file an
/// unrelated cleanup is about to delete, and on Windows a program that is still
/// running is exactly what the scratch cleanup cannot remove. The directory it
/// is put in holds one installer at a time, so a second install replaces the
/// first rather than filling the config directory with old setups.
fn stage_setup(download: &Path, directory: &Path) -> Result<PathBuf> {
    remove(directory);
    fs::create_dir_all(directory).with_context(|| format!("creating {}", directory.display()))?;
    let staged = directory.join(
        download
            .file_name()
            .context("the downloaded installer has no name")?,
    );
    fs::copy(download, &staged)
        .with_context(|| format!("staging the installer in {}", staged.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&staged, fs::Permissions::from_mode(0o755))
            .with_context(|| format!("making {} executable", staged.display()))?;
    }
    Ok(staged)
}

/// Where a downloaded setup is put before it is started: the desktop app's own
/// directory in the config dir, beside the project registry it writes and
/// outside the scratch a download lands in.
pub fn setup_staging() -> PathBuf {
    oxide_core::workspaces::registry_path()
        .parent()
        .map(|parent| parent.join("installer"))
        .unwrap_or_else(|| PathBuf::from("installer"))
}

/// Writes a downloaded AppImage over the one this process was started from.
///
/// The copy that can fail happens beside the installation, and the rename that
/// puts it in place is what makes the swap one step: a Linux process running
/// from a file keeps the file it started with, so writing over the path a
/// stopped app will read is safe while this one is still going.
fn install_appimage(download: &Path, image: &Path) -> Result<()> {
    let fresh = sibling(image, "new");
    remove(&fresh);
    fs::copy(download, &fresh)
        .with_context(|| format!("copying the downloaded app to {}", fresh.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&fresh, fs::Permissions::from_mode(0o755))
            .with_context(|| format!("making {} executable", fresh.display()))?;
    }
    fs::rename(&fresh, image)
        .with_context(|| format!("putting the new app in {}", image.display()))?;
    Ok(())
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
        .unwrap_or_else(|| "Oxide".to_string());
    bundle.with_file_name(format!(".{name}.{suffix}"))
}

fn remove(path: &Path) {
    if path.is_dir() {
        let _ = fs::remove_dir_all(path);
    } else {
        let _ = fs::remove_file(path);
    }
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
            notes: None,
            released_at: None,
        }
    }

    fn bundle_fixture(root: &Path) -> PathBuf {
        let bundle = root.join("Oxide.app");
        let binary = bundle.join("Contents/MacOS/Oxide");
        fs::create_dir_all(binary.parent().unwrap()).unwrap();
        fs::write(&binary, b"binary").unwrap();
        fs::write(bundle.join("Contents/Info.plist"), b"plist").unwrap();
        bundle
    }

    /// A Windows installation: the directory the setup made under the machine's
    /// application data, the uninstaller it left there, and the app inside it.
    fn installed_copy(data_root: &Path, directory: &str, app: &str) -> (PathBuf, PathBuf) {
        let root = data_root.join(directory).join(app);
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("Uninstall Oxide.exe"), b"uninstaller").unwrap();
        let binary = root.join("oxide-desktop.exe");
        fs::write(&binary, b"binary").unwrap();
        (root, binary)
    }

    /// The AppImage a `oxide` process was started from, as the runtime passes
    /// it in the environment.
    fn appimage(root: &Path) -> PathBuf {
        let image = root.join("Oxide.AppImage");
        fs::write(&image, b"appimage").unwrap();
        image
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
        let binary = bundle.join("Contents/MacOS/Oxide");
        let installation = installation_of(&binary, "macos", None, None);
        assert_eq!(installation.label, "app bundle");
        assert_eq!(installation.path.as_deref(), Some(bundle.as_path()));

        // The disk image is what replaces it, so the app installs the release
        // itself and there is nothing to advise instead.
        let check = check_for(
            &release("macos-arm64-Oxide.dmg"),
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
            &release("macos-arm64-Oxide.dmg"),
            &installation,
            "acme/oxide",
            "0.34.0",
        );
        assert!(!fresh.update_available && !fresh.installable && fresh.advice.is_none());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn the_engines_own_path_finds_the_bundle_it_is_packaged_in() {
        // The engine is a program inside the app, not the app's own executable:
        // a copy that read the bundle as a fixed number of directories up would
        // call every packaged macOS build a checkout and never offer it an
        // update.
        let root = temp_dir("harness");
        let bundle = bundle_fixture(&root);
        let engine = bundle.join("Contents/Resources/harness/oxide-desktop");
        fs::create_dir_all(engine.parent().unwrap()).unwrap();
        fs::write(&engine, b"engine").unwrap();

        let installation = installation_of(&engine, "macos", None, None);
        assert_eq!(installation.label, "app bundle");
        assert_eq!(installation.path.as_deref(), Some(bundle.as_path()));
        assert!(installation.replaces_itself());
        assert!(matches!(installation.kind, Kind::Bundle(path) if path == bundle));
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
            &release("macos-arm64-Oxide.dmg"),
            &installation,
            "acme/oxide",
            "0.0.0",
        );
        assert!(check.update_available && !check.installable);
        assert!(check
            .advice
            .as_deref()
            .unwrap()
            .contains("macos-arm64-Oxide.dmg"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn an_installed_copy_is_replaced_by_the_release_installer() {
        let root = temp_dir("installed");
        let data_root = root.join("LocalAppData");
        let (directory, binary) = installed_copy(&data_root, "Programs", "Oxide");

        let installation = installation_of(&binary, "windows", Some(&data_root), None);
        assert_eq!(installation.label, "installer");
        assert_eq!(installation.path.as_deref(), Some(directory.as_path()));
        assert!(installation.installable(
            release("win-x64-Oxide-Setup.exe")
                .artifact("acme/oxide")
                .as_ref()
        ));
        // The AppImage is another platform's installation, so it is not what
        // this copy installs from, and the reader is told which download to
        // take instead.
        let check = check_for(
            &release("linux-x64-Oxide-Setup.AppImage"),
            &installation,
            "acme/oxide",
            "0.33.0",
        );
        assert!(check.update_available && !check.installable);
        assert!(check.advice.is_some());

        // Only a bundle or an AppImage is replaced without being asked: an
        // installer writes over the installation and asks for the app to be
        // closed.
        assert!(!installation.replaces_itself());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_copy_outside_an_install_root_names_the_download() {
        let root = temp_dir("outside");
        let data_root = root.join("data");
        // Another application's installation, beside where this one would be:
        // its uninstaller is not this app's, so an executable beside this one
        // is not a copy this app replaces.
        installed_copy(&data_root, "Programs", "Other");
        let binary = root.join("usr/lib/oxide/oxide");
        fs::create_dir_all(binary.parent().unwrap()).unwrap();
        fs::write(&binary, b"binary").unwrap();

        let installation = installation_of(&binary, "linux", Some(&data_root), None);
        assert_eq!(installation.label, "source build");

        let check = check_for(
            &release("linux-x64-Oxide-Setup.tar.gz"),
            &installation,
            "acme/oxide",
            "0.33.0",
        );
        assert!(check.update_available && !check.installable);
        assert!(
            check
                .advice
                .unwrap()
                .contains("linux-x64-Oxide-Setup.tar.gz"),
            "the download is named"
        );

        // An install root without its uninstaller record is not one either:
        // nothing says an installer made it.
        let unmarked = data_root.join("Programs").join("Oxide");
        fs::create_dir_all(&unmarked).unwrap();
        let inside = unmarked.join("oxide.exe");
        fs::write(&inside, b"binary").unwrap();
        assert_eq!(
            installation_of(&inside, "windows", Some(&data_root), None).label,
            "source build"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn the_install_root_is_found_by_the_marker_rather_than_the_directory_name() {
        // The machine's program directory is shared with every other
        // application, and the app's own directory is named after the product:
        // which root is this app's is answered by the executable being inside
        // it and the uninstaller it carries, never by the name above it.
        let root = temp_dir("marker");
        let data_root = root.join("LocalAppData");
        let (other, other_binary) = installed_copy(&data_root, "Programs", "Other");
        assert_eq!(
            installation_of(&other_binary, "windows", Some(&data_root), None).path,
            Some(other),
            "another application's root is its own"
        );
        let (directory, binary) = installed_copy(&data_root, "Programs", "Oxide");

        let installation = installation_of(&binary, "windows", Some(&data_root), None);
        assert_eq!(installation.label, "installer");
        assert_eq!(installation.path.as_deref(), Some(directory.as_path()));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_windows_installation_is_found_by_its_install_root() {
        let root = temp_dir("windows");
        let data_root = root.join("LocalAppData");
        let (directory, binary) = installed_copy(&data_root, "Programs", "Oxide");

        let installation = installation_of(&binary, "windows", Some(&data_root), None);
        assert_eq!(installation.label, "installer");
        assert_eq!(installation.path.as_deref(), Some(directory.as_path()));
        // The installer is what replaces the installation it made.
        assert!(installation.installable(
            release("win-x64-Oxide-Setup.exe")
                .artifact("acme/oxide")
                .as_ref()
        ));
        assert!(!installation.installable(
            release("macos-arm64-Oxide.dmg")
                .artifact("acme/oxide")
                .as_ref()
        ));

        // A build run from a checkout is not the installation a setup replaces.
        let checkout = root.join("target/debug/oxide-desktop.exe");
        fs::create_dir_all(checkout.parent().unwrap()).unwrap();
        fs::write(&checkout, b"binary").unwrap();
        let source = installation_of(&checkout, "windows", Some(&data_root), None);
        assert_eq!(source.label, "source build");
        assert!(!source.installable(
            release("win-x64-Oxide-Setup.exe")
                .artifact("acme/oxide")
                .as_ref()
        ));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_machine_with_no_data_directory_has_no_install_root() {
        let root = temp_dir("no-data");
        let binary = root.join("opt/oxide/oxide-desktop");
        fs::create_dir_all(binary.parent().unwrap()).unwrap();
        fs::write(&binary, b"binary").unwrap();

        assert_eq!(
            installation_of(&binary, "linux", None, None).label,
            "source build"
        );
        assert_eq!(
            installation_of(&binary, "windows", Some(&root.join("missing")), None).label,
            "source build"
        );
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
        fs::write(fresh.join("Contents/MacOS/Oxide"), b"new binary").unwrap();

        swap_in(&fresh, &bundle).unwrap();

        assert_eq!(
            fs::read(bundle.join("Contents/MacOS/Oxide")).unwrap(),
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
            fs::read(bundle.join("Contents/MacOS/Oxide")).unwrap(),
            b"binary"
        );
        assert!(
            left_behind(&root, &["Oxide.app"]).is_empty(),
            "nothing else is left"
        );
        fs::remove_dir_all(root).unwrap();
    }

    // The program a Windows install starts is put beside the app's own state —
    // the desktop directory the project registry lives in — rather than in the
    // scratch directory a download lands in, which is removed as the install
    // returns and would take the setup out from under the installer reading it.
    #[test]
    fn a_setup_is_staged_beside_the_desktops_own_state() {
        let staging = setup_staging();
        assert_eq!(
            staging.parent().unwrap(),
            oxide_core::workspaces::registry_path().parent().unwrap(),
            "the staging directory sits beside the app's own state"
        );
        assert_eq!(staging.file_name().unwrap(), "installer");
    }

    // A Windows installer is a program of its own, which this app starts and
    // cannot wait on: it writes over the installation and waits for Oxide to be
    // closed. What the install knows is that it is running, so that is what it
    // reports rather than a version it cannot claim is in place.
    #[cfg(unix)]
    #[test]
    fn a_downloaded_installer_is_started_for_the_reader() {
        let root = temp_dir("installer");
        let data_root = root.join("LocalAppData");
        let (_directory, binary) = installed_copy(&data_root, "Programs", "Oxide");
        let installation = installation_of(&binary, "windows", Some(&data_root), None);

        // The installer a release publishes: a program that leaves a mark, so
        // the test can see it ran. It is downloaded into the run's own scratch
        // the way a real install downloads it — the copy that is started has to
        // be the one the scratch cleanup cannot take away.
        let marks = root.join("installed.txt");
        let work = WorkDir::new().unwrap();
        let download = work.path().join("win-x64-Oxide-Setup.exe");
        fs::write(
            &download,
            format!("#!/bin/sh\necho installed > {}\n", marks.display()),
        )
        .unwrap();
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&download, fs::Permissions::from_mode(0o755)).unwrap();
        }

        let staging = root.join("installer");
        let placed =
            install_downloaded(&installation, "0.34.0", &download, &work, &staging).unwrap();

        assert!(placed.pending, "the installer owns what happens next");
        assert!(
            placed.note.contains("installer is running"),
            "{}",
            placed.note
        );
        let mut started = false;
        for _ in 0..100 {
            if marks.is_file() {
                started = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(started, "the downloaded installer was started");

        // The program that was started is the staged copy, which outlives the
        // call and the scratch directory the download landed in.
        let staged = staging.join("win-x64-Oxide-Setup.exe");
        assert!(
            staged.is_file(),
            "the setup is staged beside the app's state"
        );
        assert_eq!(
            fs::read(&staged).unwrap(),
            fs::read(&download).unwrap(),
            "the staged setup is the download"
        );
        let scratch = work.path().to_path_buf();
        drop(work);
        assert!(!scratch.exists(), "the run's own scratch is removed");
        assert!(staged.is_file(), "the staged setup is not the scratch's");
        fs::remove_dir_all(root).unwrap();
    }

    // A running AppImage is the file the release ships: the new one is written
    // over the path this process was started from, which a Linux process
    // running from a file does not mind.
    #[cfg(unix)]
    #[test]
    fn a_running_appimage_is_replaced_by_the_release_image() {
        let root = temp_dir("appimage");
        let image = appimage(&root);
        let installation = installation_of(&image, "linux", None, Some(&image));
        assert_eq!(installation.label, "AppImage");
        assert_eq!(installation.path.as_deref(), Some(image.as_path()));
        assert!(installation.replaces_itself(), "the app owns this file");
        assert!(installation.installable(
            release("linux-x64-Oxide-Setup.AppImage")
                .artifact("acme/oxide")
                .as_ref()
        ));
        // The archive Linux users unpack by hand is not what this installation
        // is replaced with, since replacing a file with a directory is another
        // installation altogether.
        assert!(!installation.installable(
            release("linux-x64-Oxide-Setup.tar.gz")
                .artifact("acme/oxide")
                .as_ref()
        ));

        let download = root.join("downloaded.AppImage");
        fs::write(&download, b"new appimage").unwrap();
        let work = WorkDir::new().unwrap();
        let placed = install_downloaded(
            &installation,
            "0.34.0",
            &download,
            &work,
            &root.join("installer"),
        )
        .unwrap();

        assert!(!placed.pending, "the file is in place");
        assert!(placed.note.contains("0.34.0"), "{}", placed.note);
        assert_eq!(fs::read(&image).unwrap(), b"new appimage");
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&image).unwrap().permissions().mode();
            assert!(mode & 0o111 != 0, "the image stays executable: {mode:o}");
        }
        assert!(
            left_behind(&root, &["Oxide.AppImage", "downloaded.AppImage"]).is_empty(),
            "nothing else is left"
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
        let error = install_downloaded(
            &installation,
            "0.34.0",
            &binary,
            &work,
            &root.join("installer"),
        )
        .unwrap_err();

        assert!(
            error
                .to_string()
                .contains("cannot be replaced from inside the app"),
            "{error}"
        );
        fs::remove_dir_all(root).unwrap();
    }

    // A launch installs a release on its own only where the app owns the file
    // it would write over — a bundle it can swap whole, or an AppImage — and
    // only when the release is newer than the build running: everything else is
    // the reader's to decide, through the dialog.
    #[test]
    fn a_launch_installs_only_into_a_copy_it_owns() {
        let root = temp_dir("launch");
        let bundle = bundle_fixture(&root);
        let bundle_install =
            installation_of(&bundle.join("Contents/MacOS/Oxide"), "macos", None, None);
        let notice = Notice::from_release(&release("macos-arm64-Oxide.dmg"), "acme/oxide", 1_000);

        // A bundle the user installed, with a newer release to hand: done
        // without asking, since the app is the one that put it there.
        assert!(launch_installs(&notice, "0.33.0", &bundle_install));
        // The running build is that release, so there is nothing to install.
        assert!(!launch_installs(&notice, "0.34.0", &bundle_install));
        assert!(!launch_installs(&notice, "0.35.0", &bundle_install));

        // An AppImage is the same bargain: one file the app owns outright.
        let image = appimage(&root);
        let image_install = installation_of(&image, "linux", None, Some(&image));
        let image_notice = Notice::from_release(
            &release("linux-x64-Oxide-Setup.AppImage"),
            "acme/oxide",
            1_000,
        );
        assert!(launch_installs(&image_notice, "0.33.0", &image_install));
        assert!(!launch_installs(&image_notice, "0.34.0", &image_install));

        // A checkout's build belongs to whoever is working in it.
        let checkout = installation_of(
            &root.join("target/debug/oxide-desktop"),
            "macos",
            None,
            None,
        );
        assert!(!launch_installs(&notice, "0.33.0", &checkout));

        // An installation an installer made is not a launch's to replace
        // either: that installer writes over the installation and waits for the
        // app to be closed.
        let data_root = root.join("LocalAppData");
        let (_directory, binary) = installed_copy(&data_root, "Programs", "Oxide");
        let installed = installation_of(&binary, "windows", Some(&data_root), None);
        let windows =
            Notice::from_release(&release("win-x64-Oxide-Setup.exe"), "acme/oxide", 1_000);
        assert!(!launch_installs(&windows, "0.33.0", &installed));

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

    // A cancel is not a failure, and both callers of an install tell it from one
    // by the error it reports — the desktop command, which answers the window
    // with it, and the launch, which says nothing at all about it — so the words
    // the install bails with are the contract.
    #[test]
    fn a_cancel_is_told_from_a_failure_to_install() {
        let cancelled = anyhow::anyhow!(CANCELLED);
        assert!(was_cancelled(&cancelled));
        assert!(!was_cancelled(&anyhow::anyhow!(
            "mounting the downloaded disk image: hdiutil failed with exit status: 1"
        )));
        assert!(!was_cancelled(&anyhow::anyhow!(
            "an Oxide update is already being installed"
        )));
    }

    // The steps that are not a download carry no byte counts, which is what the
    // dialog reads to decide between a determinate bar and one that works.
    #[test]
    fn a_step_that_is_not_a_download_reports_no_bytes() {
        let step = Progress::step("verifying", "0.34.0");
        assert_eq!(step.stage, "verifying");
        assert_eq!(step.version, "0.34.0");
        assert_eq!(step.received, 0);
        assert_eq!(step.total, None);
    }
}
