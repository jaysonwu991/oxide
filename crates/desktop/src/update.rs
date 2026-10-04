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
//! Nothing here touches Electrobun either, so both halves — whether this
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
    /// How a front-end names this installation: `app bundle`, `installer` (the
    /// Electrobun setup that made this copy), or `source build` (a checkout's
    /// build, which the app does not replace).
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
    /// An Electrobun installation of this app, replaced by the setup archive
    /// the release publishes, which takes over from the installer that made it.
    Setup,
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
            Kind::Setup => {
                asset.name.ends_with("-Setup.zip") || asset.name.ends_with("-Setup.tar.gz")
            }
            Kind::None => false,
        }
    }

    /// Whether this copy is one the app replaces in place without being asked:
    /// a bundle the user installed, which a new release simply takes the place
    /// of. An installer is not — it starts a setup that writes over the
    /// installation and may ask for elevation, which is a thing to offer rather
    /// than to do behind the reader's back — and neither is a copy the app does
    /// not own at all.
    pub fn replaces_itself(&self) -> bool {
        self.replaceable && matches!(self.kind, Kind::Bundle(_))
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
    installation_of(&executable, std::env::consts::OS, data_root().as_deref())
}

/// Where an installer puts an application: `%LOCALAPPDATA%` on Windows and
/// `$XDG_DATA_HOME` (or `~/.local/share`) on Linux, which is the directory
/// Electrobun's install roots sit under.
fn data_root() -> Option<PathBuf> {
    dirs::data_local_dir()
}

/// The record an Electrobun install root carries: the uninstaller it was
/// installed with, and the manifest that uninstaller reads.
const INSTALL_MARKERS: [&str; 3] = [".electrobun-uninstall.json", "uninstall", "uninstall.exe"];

/// The installation an executable belongs to. `os` is the platform's name,
/// which is what decides which of a release's artifacts replaces it, and
/// `data_root` is the machine's own directory of application data, which is
/// where an Electrobun installer makes its install root — both are passed in
/// so every platform's layout is tested without either platform.
pub fn installation_of(executable: &Path, os: &str, data_root: Option<&Path>) -> Installation {
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
    match install_root_of(executable, data_root) {
        Some(root) => Installation {
            label: "installer",
            replaceable: writable(&root),
            path: Some(root),
            kind: Kind::Setup,
        },
        None => Installation::none("source build"),
    }
}

/// The install root this executable lives in, out of Electrobun's layout:
/// `<data root>/<identifier>/<install root>/…`, which the installer marks with
/// the uninstaller it leaves behind. The marker is what tells an installed copy
/// from one in a checkout, and the executable has to be inside the directory —
/// another Electrobun application's install root says nothing about this one.
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
        Kind::Setup => {
            // The setup archive holds the installer, which owns the
            // installation from here: it writes over the files and waits for
            // the running app to be closed. Starting it is all this side can
            // know — it may still be waiting for elevation or on the reader —
            // so the release is reported as pending rather than installed.
            let program = unpack_setup(download, work)?;
            Command::new(&program)
                .spawn()
                .with_context(|| format!("running {}", program.display()))?;
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

/// Unpacks a downloaded setup archive and answers with the installer inside it.
///
/// Both the zip a Windows release publishes and the gzipped tar of a Linux one
/// are read by the machine's own `tar`, which keeps the install step free of an
/// archive dependency the app would otherwise carry for one file.
fn unpack_setup(archive: &Path, work: &WorkDir) -> Result<PathBuf> {
    let unpacked = work.path().join("setup");
    fs::create_dir_all(&unpacked).with_context(|| format!("creating {}", unpacked.display()))?;
    let mut command = Command::new("tar");
    command.arg("-xf").arg(archive).arg("-C").arg(&unpacked);
    run(&mut command, "unpacking the downloaded installer")?;
    let program = find_setup(&unpacked)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&program, fs::Permissions::from_mode(0o755))
            .with_context(|| format!("making {} executable", program.display()))?;
    }
    Ok(program)
}

/// The installer an unpacked setup holds: the `installer` a Linux archive
/// carries by name, or the `<App> Setup.exe` a Windows zip holds — the payload
/// beside it is not a program to run, so a name rather than a listing order is
/// what picks one.
fn find_setup(directory: &Path) -> Result<PathBuf> {
    let mut found: Vec<PathBuf> = Vec::new();
    for entry in fs::read_dir(directory)
        .with_context(|| format!("reading {}", directory.display()))?
        .flatten()
    {
        let path = entry.path();
        if path.is_file() {
            found.push(path);
        }
    }
    found.sort();
    if let Some(named) = found
        .iter()
        .find(|path| path.file_name() == Some(OsStr::new("installer")))
    {
        return Ok(named.clone());
    }
    if let Some(setup) = found.iter().find(|path| is_setup_executable(path)) {
        return Ok(setup.clone());
    }
    let executables: Vec<&PathBuf> = found
        .iter()
        .filter(|path| path.extension() == Some(OsStr::new("exe")))
        .collect();
    match executables.as_slice() {
        [only] => Ok((*only).clone()),
        _ => bail!(
            "the downloaded installer holds no single setup program in {}",
            directory.display()
        ),
    }
}

/// A Windows setup executable keeps the app's display name, spaces and all:
/// the zip's own `Oxide Setup.exe`, beside the payload it unpacks.
fn is_setup_executable(path: &Path) -> bool {
    path.extension() == Some(OsStr::new("exe"))
        && path
            .file_stem()
            .and_then(OsStr::to_str)
            .is_some_and(|stem| stem.ends_with(" Setup"))
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

    /// An Electrobun installation: the install root under
    /// `<data root>/<identifier>/<install root>/`, the marker the installer
    /// leaves, and the app inside it.
    fn installed_copy(data_root: &Path, identifier: &str, root: &str) -> (PathBuf, PathBuf) {
        let directory = data_root.join(identifier).join(root);
        fs::create_dir_all(&directory).unwrap();
        fs::write(directory.join(".electrobun-uninstall.json"), b"{}").unwrap();
        let binary = directory.join("oxide-desktop");
        fs::write(&binary, b"binary").unwrap();
        (directory, binary)
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
        let installation = installation_of(&binary, "macos", None);
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
    fn a_build_outside_a_bundle_is_left_to_the_reader() {
        let root = temp_dir("checkout");
        let binary = root.join("target/debug/oxide-desktop");
        fs::create_dir_all(binary.parent().unwrap()).unwrap();
        fs::write(&binary, b"binary").unwrap();

        let installation = installation_of(&binary, "macos", None);
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
    fn an_installed_copy_is_replaced_by_the_release_setup() {
        let root = temp_dir("installed");
        let data_root = root.join("data");
        let (directory, binary) = installed_copy(&data_root, "com.oxide.app", "stable");

        let installation = installation_of(&binary, "linux", Some(&data_root));
        assert_eq!(installation.label, "installer");
        assert_eq!(installation.path.as_deref(), Some(directory.as_path()));
        assert!(installation.installable(
            release("linux-x64-Oxide-Setup.tar.gz")
                .artifact("acme/oxide")
                .as_ref()
        ));
        // A Windows installation is the same shape, with the zip its setup
        // ships in.
        let windows = installation_of(&binary, "windows", Some(&data_root));
        assert!(windows.installable(
            release("win-x64-Oxide-Setup.zip")
                .artifact("acme/oxide")
                .as_ref()
        ));
        // An artifact that is not the setup is not what this copy installs
        // from, and the reader is told which download to take instead.
        let check = check_for(
            &release("macos-arm64-Oxide.dmg"),
            &installation,
            "acme/oxide",
            "0.33.0",
        );
        assert!(check.update_available && !check.installable);
        assert!(check.advice.is_some());

        // Only a bundle is replaced without being asked: a setup writes over
        // the installation and may ask for elevation.
        assert!(!installation.replaces_itself());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn the_setup_program_is_found_by_name_rather_than_by_order() {
        // A Linux archive: the setup is the file the installer names itself.
        let linux = temp_dir("setup-linux");
        fs::write(linux.join("README"), b"how to install").unwrap();
        fs::write(linux.join("installer"), b"#!/bin/sh\n").unwrap();
        assert_eq!(find_setup(&linux).unwrap(), linux.join("installer"));
        fs::remove_dir_all(&linux).unwrap();

        // A Windows zip: the setup keeps the display name, and the payload
        // beside it is not the program to start — even when it sorts first.
        let windows = temp_dir("setup-windows");
        fs::write(windows.join("aaa-payload.exe"), b"payload").unwrap();
        fs::write(windows.join("Oxide Setup.exe"), b"setup").unwrap();
        assert_eq!(
            find_setup(&windows).unwrap(),
            windows.join("Oxide Setup.exe")
        );
        fs::remove_dir_all(&windows).unwrap();

        // Nothing to run is reported rather than passed over.
        let empty = temp_dir("setup-empty");
        fs::write(empty.join("README"), b"nothing here").unwrap();
        assert!(find_setup(&empty).is_err());
        fs::remove_dir_all(&empty).unwrap();
    }

    #[test]
    fn a_copy_outside_an_install_root_names_the_download() {
        let root = temp_dir("outside");
        let data_root = root.join("data");
        // Another Electrobun application's installation: its uninstaller is not
        // this app's, so an executable beside this one is not one this app
        // replaces.
        installed_copy(&data_root, "com.other.app", "stable");
        let binary = root.join("usr/lib/oxide/oxide-desktop");
        fs::create_dir_all(binary.parent().unwrap()).unwrap();
        fs::write(&binary, b"binary").unwrap();

        let installation = installation_of(&binary, "linux", Some(&data_root));
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
        let unmarked = data_root.join("com.oxide.app").join("stable");
        fs::create_dir_all(&unmarked).unwrap();
        let inside = unmarked.join("oxide-desktop");
        fs::write(&inside, b"binary").unwrap();
        assert_eq!(
            installation_of(&inside, "linux", Some(&data_root)).label,
            "source build"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_windows_installation_is_found_by_its_install_root() {
        let root = temp_dir("windows");
        let data_root = root.join("LocalAppData");
        let (directory, binary) = installed_copy(&data_root, "com.oxide.app", "stable");

        let installation = installation_of(&binary, "windows", Some(&data_root));
        assert_eq!(installation.label, "installer");
        assert_eq!(installation.path.as_deref(), Some(directory.as_path()));
        // The setup is what replaces the installation it made.
        assert!(installation.installable(
            release("win-x64-Oxide-Setup.zip")
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
        let source = installation_of(&checkout, "windows", Some(&data_root));
        assert_eq!(source.label, "source build");
        assert!(!source.installable(
            release("win-x64-Oxide-Setup.zip")
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
            installation_of(&binary, "linux", None).label,
            "source build"
        );
        assert_eq!(
            installation_of(&binary, "windows", Some(&root.join("missing"))).label,
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

    // A setup is an installer process of its own, which this app starts and
    // cannot wait on: it writes over the installation and asks for Oxide to be
    // closed, and on Windows it may ask for elevation. What the install knows
    // is that it is running, so that is what it reports rather than a version
    // it cannot claim is in place.
    #[cfg(unix)]
    #[test]
    fn a_downloaded_setup_is_unpacked_and_its_installer_started() {
        let root = temp_dir("setup-install");
        let data_root = root.join("data");
        let (_directory, binary) = installed_copy(&data_root, "com.oxide.app", "stable");
        let installation = installation_of(&binary, "linux", Some(&data_root));

        // The installer a release publishes, packed the way it is published: a
        // program that leaves a mark, so the test can see it ran.
        let held = root.join("setup-src");
        fs::create_dir_all(&held).unwrap();
        let marks = root.join("installed.txt");
        let program = held.join("installer");
        fs::write(
            &program,
            format!("#!/bin/sh\necho unpacked > {}\n", marks.display()),
        )
        .unwrap();
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&program, fs::Permissions::from_mode(0o755)).unwrap();
        }
        let download = root.join("linux-x64-Oxide-Setup.tar.gz");
        let mut pack = Command::new("tar");
        pack.arg("czf")
            .arg(&download)
            .arg("-C")
            .arg(&held)
            .arg("installer");
        run(&mut pack, "packing the fixture").unwrap();

        let work = WorkDir::new().unwrap();
        let placed = install_downloaded(&installation, "0.34.0", &download, &work).unwrap();

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
        assert!(started, "the unpacked installer was started");
        fs::remove_dir_all(root).unwrap();
    }

    // A setup archive that holds no installer is reported rather than started
    // as nothing: a release whose layout changed is the reader's to know about.
    #[cfg(unix)]
    #[test]
    fn an_archive_with_no_installer_in_it_is_refused() {
        let root = temp_dir("empty-setup");
        let data_root = root.join("data");
        let (_directory, binary) = installed_copy(&data_root, "com.oxide.app", "stable");
        let installation = installation_of(&binary, "linux", Some(&data_root));

        let held = root.join("setup-src");
        fs::create_dir_all(&held).unwrap();
        fs::write(held.join("README"), b"nothing here").unwrap();
        let download = root.join("linux-x64-Oxide-Setup.tar.gz");
        let mut pack = Command::new("tar");
        pack.arg("czf")
            .arg(&download)
            .arg("-C")
            .arg(&held)
            .arg("README");
        run(&mut pack, "packing the fixture").unwrap();

        let work = WorkDir::new().unwrap();
        let error = install_downloaded(&installation, "0.34.0", &download, &work).unwrap_err();

        assert!(
            error.to_string().contains("holds no single setup program"),
            "{error}"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_copy_the_app_does_not_replace_refuses_to_install() {
        let root = temp_dir("refuse");
        let binary = root.join("target/debug/oxide-desktop");
        fs::create_dir_all(binary.parent().unwrap()).unwrap();
        let installation = installation_of(&binary, "macos", None);

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
    // it would write over — a bundle it can swap whole — and only when the
    // release is newer than the build running: everything else is the reader's
    // to decide, through the dialog.
    #[test]
    fn a_launch_installs_only_into_a_copy_it_owns() {
        let root = temp_dir("launch");
        let bundle = bundle_fixture(&root);
        let bundle_install =
            installation_of(&bundle.join("Contents/MacOS/oxide-desktop"), "macos", None);
        let notice = Notice::from_release(&release("macos-arm64-Oxide.dmg"), "acme/oxide", 1_000);

        // A bundle the user installed, with a newer release to hand: done
        // without asking, since the app is the one that put it there.
        assert!(launch_installs(&notice, "0.33.0", &bundle_install));
        // The running build is that release, so there is nothing to install.
        assert!(!launch_installs(&notice, "0.34.0", &bundle_install));
        assert!(!launch_installs(&notice, "0.35.0", &bundle_install));

        // A checkout's build belongs to whoever is working in it.
        let checkout = installation_of(&root.join("target/debug/oxide-desktop"), "macos", None);
        assert!(!launch_installs(&notice, "0.33.0", &checkout));

        // An installation a setup made is not a launch's to replace either:
        // that setup writes over the installation and may ask for elevation or
        // for the app to be closed.
        let data_root = root.join("data");
        let (_directory, binary) = installed_copy(&data_root, "com.oxide.app", "stable");
        let installed = installation_of(&binary, "windows", Some(&data_root));
        assert!(!launch_installs(&notice, "0.33.0", &installed));

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
