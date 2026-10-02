use crate::install::{self, detect_install_method, InstallMethod};
use anyhow::{bail, Context, Result};
use oxide_core::updates::{self, Check, Component, Release, WorkDir};
use std::fs;
use std::path::Path;
use std::process::Command;

const STAGING_PREFIX: &str = ".oxide-update-";

/// What `oxide update` was asked to do. The release rules themselves — which
/// tag belongs to which component, the artifact a platform installs, whether a
/// version is newer — live in `oxide_core::updates`, so the terminal, the
/// desktop app and the editor extension resolve a release the same way.
#[derive(Debug, Clone)]
pub struct Options {
    pub check: bool,
    pub version: Option<String>,
    pub force: bool,
    /// Print the check as JSON instead of the prose a terminal reads, for a
    /// front-end that offers the update itself (`--check` always accompanies
    /// it, since the JSON describes a check and nothing else).
    pub json: bool,
    /// Which release train to check: the CLI's own, or the one a front-end that
    /// cannot link `oxide-core` asks about (the VS Code panel).
    pub component: Component,
    /// The version the caller reports running, for that front-end: the CLI
    /// knows its own, the panel passes the extension's.
    pub current: Option<String>,
}

pub async fn run(options: Options) -> Result<()> {
    let executable = std::env::current_exe().context("resolving the oxide executable")?;
    run_for(options, &executable, &updates::repo()).await
}

/// What a check found, as a front-end reads it. The shape is
/// `oxide_core::updates::Check`, which the desktop app's own check reports too;
/// this fills in the half only the CLI can know — how this binary was
/// installed, and whether it may replace it.
fn check_of(
    options: &Options,
    release: &Release,
    executable: &Path,
    repo: &str,
    current: &str,
) -> Check {
    let mut check = Check::new(
        options.component,
        release,
        current,
        options.version.is_some(),
        repo,
    );
    if options.component != Component::Cli {
        // Nothing this command installs: the app replaces itself and the panel
        // installs its own VSIX, so the check says what to do instead.
        check.advice = check.update_available.then(|| match &check.asset {
            Some(asset) => format!(
                "Update available: {} installs this release itself (Check for Updates…), or {} is \
                 on the release page.",
                options.component.label(),
                asset.name
            ),
            None => format!(
                "Update available: {} has no build for this platform; see the release page.",
                options.component.label()
            ),
        });
        return check;
    }

    let method = detect_install_method(executable);
    check.update_available |= options.force;
    check.installation = method.label().to_string();
    check.installable = match method {
        InstallMethod::Cargo | InstallMethod::Prebuilt => true,
        InstallMethod::Unknown => options.force,
        InstallMethod::Homebrew => false,
    };
    check.path = executable.display().to_string();
    check.advice = check.update_available.then(|| advice(method, executable));
    check
}

async fn run_for(options: Options, executable: &Path, repo: &str) -> Result<()> {
    let component = options.component;
    // This command replaces the installed binary. The desktop app and the
    // editor extension are replaced by the front-end that runs them, so a run
    // that is not a check says where their release comes from instead of
    // installing something the caller cannot use.
    if !options.check && component != Component::Cli {
        bail!(
            "`oxide update` replaces the installed oxide CLI; {} updates itself from its own \
             release, so check it with `oxide update --check --component {}`",
            component.label(),
            component.as_str()
        );
    }
    let platform = if component.platform_specific() {
        updates::platform()?
    } else {
        ""
    };
    let current = match options.current.clone() {
        Some(current) => current,
        None if component == Component::Cli => current_version().to_string(),
        None => String::new(),
    };

    let client = updates::client()?;
    let release = match options.version.as_deref() {
        Some(requested) => updates::pinned(component, requested, platform)?,
        None => updates::latest(&client, repo, component, platform).await?,
    };
    let check = check_of(&options, &release, executable, repo, &current);
    if options.json {
        println!("{}", serde_json::to_string_pretty(&check)?);
        return Ok(());
    }

    println!("Oxide update — {}", component.label());
    if component == Component::Cli {
        println!(
            "Installation: {} at {}",
            check.installation,
            executable.display()
        );
    }
    if !current.is_empty() {
        println!("Current: {current}");
    }
    if options.version.is_some() {
        println!("Target: {}", release.tag);
    } else {
        println!("Latest: {}", release.tag);
    }

    if !check.update_available {
        if component == Component::Cli {
            println!(
                "Already up to date; rerun with --force to reinstall {}.",
                release.tag
            );
        } else {
            println!("{} is up to date.", component.label());
        }
        return Ok(());
    }
    if let Some(asset) = &check.asset {
        println!("Asset: {}", asset.name);
    }
    if options.check {
        if let Some(advice) = check.advice.as_deref() {
            println!("{advice}");
        }
        return Ok(());
    }

    let method = detect_install_method(executable);
    match method {
        InstallMethod::Homebrew => {
            bail!("oxide was installed with Homebrew; run `brew upgrade oxide` instead")
        }
        InstallMethod::Unknown if !options.force => bail!(
            "{} is not a released binary; run `oxide update --force` to replace it, or install a \
             release with install.sh",
            executable.display()
        ),
        _ => {}
    }
    if method == InstallMethod::Unknown {
        println!(
            "Replacing {} because it was given with --force",
            executable.display()
        );
    }

    let work = WorkDir::new()?;
    let (url, bytes) = archive(&client, repo, &release).await?;
    let download = work.path().join(&release.asset);
    fs::write(&download, &bytes).with_context(|| format!("writing {}", download.display()))?;
    match updates::fetch(&client, &format!("{url}.sha256"), None).await? {
        Some(sums) => {
            verify(&download, &sums)?;
            println!("Verified SHA256 checksum");
        }
        None => println!(
            "Warning: no checksum for {}; skipping verification",
            release.asset
        ),
    }
    extract(&download, work.path())?;
    let staged = work.path().join(binary_name());
    if !staged.is_file() {
        bail!("{} does not contain {}", release.asset, binary_name());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&staged, fs::Permissions::from_mode(0o755))
            .with_context(|| format!("making {} executable", staged.display()))?;
    }
    let reported = staged_version(&staged)?;
    refuse_version_mismatch(&release.asset, &reported, &release.version)?;
    replace_binary(&staged, executable)?;
    if !matches!(method, InstallMethod::Cargo | InstallMethod::Homebrew) {
        record_install(executable, repo, &release.version);
    }

    println!("Updated {} to {}", executable.display(), release.tag);
    if matches!(method, InstallMethod::Cargo) {
        println!(
            "Note: cargo still records the version it installed; rebuild from source to update it"
        );
    }
    println!("Restart oxide to use the new version");
    Ok(())
}

/// The version this binary was built from. The workspace keeps `0.0.0` as its
/// placeholder, and release CI writes the tag version in before building.
pub fn current_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

fn binary_name() -> &'static str {
    if cfg!(windows) {
        "oxide.exe"
    } else {
        "oxide"
    }
}

/// What to do about a release this installation cannot install itself.
fn advice(method: InstallMethod, executable: &Path) -> String {
    match method {
        InstallMethod::Homebrew => {
            "Update available; Homebrew manages this install: run `brew upgrade oxide`.".to_string()
        }
        InstallMethod::Unknown => format!(
            "Update available; run `oxide update --force` to replace {}, or install a release with \
             install.sh.",
            executable.display()
        ),
        InstallMethod::Cargo | InstallMethod::Prebuilt => {
            "Update available: run `oxide update` to install it.".to_string()
        }
    }
}

/// The archive to install, trying the pre-branding lowercase name when a pinned
/// release predates the `Oxide-v*` assets. Returns the URL that answered, so
/// its checksum is fetched from beside it.
async fn archive(
    client: &reqwest::Client,
    repo: &str,
    release: &Release,
) -> Result<(String, Vec<u8>)> {
    let url = release.url(repo);
    println!("Downloading {}", release.asset);
    if let Some(bytes) = updates::fetch(client, &url, None).await? {
        return Ok((url, bytes));
    }
    if let Some(legacy) = legacy_url(&url) {
        let asset = legacy.rsplit('/').next().unwrap_or(&release.asset);
        println!("Downloading {asset}");
        if let Some(bytes) = updates::fetch(client, &legacy, None).await? {
            return Ok((legacy, bytes));
        }
    }
    bail!(
        "{} is not published for {}; see https://github.com/{repo}/releases",
        release.asset,
        release.tag
    )
}

/// The lowercase archive name the releases before the rename published, which
/// a pinned version can still need.
fn legacy_url(url: &str) -> Option<String> {
    let (head, asset) = url.rsplit_once('/')?;
    asset
        .strip_prefix("Oxide-v")
        .map(|rest| format!("{head}/oxide-v{rest}"))
}

/// Checks a downloaded archive against the `.sha256` file published beside it,
/// which holds `<digest>  <name>`.
fn verify(file: &Path, sums: &[u8]) -> Result<()> {
    let text = String::from_utf8_lossy(sums);
    let expected = text
        .split_whitespace()
        .next()
        .context("the checksum file has no digest")?;
    updates::verify_sha256(file, expected)
}
fn extract(archive: &Path, dir: &Path) -> Result<()> {
    #[cfg(windows)]
    let mut command = {
        let script = format!(
            "Expand-Archive -LiteralPath {} -DestinationPath {} -Force",
            powershell_quote(archive),
            powershell_quote(dir)
        );
        let mut command = Command::new("powershell");
        command.args(["-NoProfile", "-NonInteractive", "-Command", &script]);
        command
    };
    #[cfg(not(windows))]
    let mut command = {
        let mut command = Command::new("tar");
        command.arg("-xzf").arg(archive).arg("-C").arg(dir);
        command
    };
    let program = command.get_program().to_string_lossy().to_string();
    let status = command
        .status()
        .with_context(|| format!("running {program}; is it on PATH?"))?;
    if !status.success() {
        bail!(
            "{program} failed with {status} while unpacking {}",
            archive.display()
        );
    }
    Ok(())
}

#[cfg(windows)]
fn powershell_quote(path: &Path) -> String {
    format!("'{}'", path.to_string_lossy().replace('\'', "''"))
}

/// Runs the unpacked binary so a truncated download or a wrong-platform
/// archive is refused before it takes the place of a working one.
fn staged_version(staged: &Path) -> Result<String> {
    let output = Command::new(staged)
        .arg("--version")
        .output()
        .with_context(|| format!("running {}", staged.display()))?;
    if !output.status.success() {
        bail!(
            "the downloaded binary failed with {}; not replacing the installed one",
            output.status
        );
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let version = stdout.split_whitespace().nth(1).unwrap_or_default();
    if version.is_empty() {
        bail!("the downloaded binary reported no version; not replacing the installed one");
    }
    Ok(version.to_string())
}

/// The unpacked binary has to be the release it was unpacked for. One that
/// reports another version is a stale manifest or the wrong asset, and putting
/// it in place would leave the user on a version they did not ask for while
/// the command reported the tag they did.
fn refuse_version_mismatch(asset: &str, reported: &str, expected: &str) -> Result<()> {
    if reported != expected {
        bail!(
            "{asset} reports version {reported}, not {expected}; leaving the installed binary in \
             place"
        );
    }
    Ok(())
}

/// Records the released install beside the binary, so the next update
/// recognizes it wherever it was put: a custom `OXIDE_INSTALL_DIR`, or a path
/// `--force` allowed. A Cargo or Homebrew install keeps the location the
/// detector knows by name, so neither is marked.
fn record_install(executable: &Path, repo: &str, version: &str) {
    if let Err(error) = install::record(executable, version, repo) {
        println!(
            "Warning: could not record the installation beside {}: {error}",
            executable.display()
        );
    }
}

/// Puts `staged` in the destination's place. The file is copied next to the
/// destination first, so the last step is a rename within one filesystem, and a
/// running Windows binary is moved aside first (Windows refuses to overwrite a
/// loaded image, though it allows renaming it).
fn replace_binary(staged: &Path, destination: &Path) -> Result<()> {
    let directory = destination
        .parent()
        .filter(|dir| !dir.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    fs::create_dir_all(directory).with_context(|| format!("creating {}", directory.display()))?;
    discard_stale(directory);
    let replacement = directory.join(format!("{STAGING_PREFIX}{}.new", std::process::id()));
    fs::copy(staged, &replacement).with_context(|| format!("writing {}", replacement.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&replacement, fs::Permissions::from_mode(0o755))
            .with_context(|| format!("making {} executable", replacement.display()))?;
    }
    swap(&replacement, destination)
}

#[cfg(not(windows))]
fn swap(replacement: &Path, destination: &Path) -> Result<()> {
    fs::rename(replacement, destination)
        .with_context(|| format!("replacing {}", destination.display()))
}

#[cfg(windows)]
fn swap(replacement: &Path, destination: &Path) -> Result<()> {
    let previous =
        destination.with_file_name(format!("{STAGING_PREFIX}{}.old", std::process::id()));
    fs::rename(destination, &previous)
        .with_context(|| format!("moving {} aside", destination.display()))?;
    if let Err(error) = fs::rename(replacement, destination) {
        let _ = fs::rename(&previous, destination);
        return Err(error).with_context(|| {
            format!(
                "replacing {}; stop any other oxide process and retry",
                destination.display()
            )
        });
    }
    // The binary that is still running keeps the old file open, and Windows
    // removes it on the next run instead.
    let _ = fs::remove_file(&previous);
    Ok(())
}

/// Removes the staging files a previous run could not delete.
fn discard_stale(directory: &Path) {
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        if entry
            .file_name()
            .to_string_lossy()
            .starts_with(STAGING_PREFIX)
        {
            let _ = fs::remove_file(entry.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_dir(tag: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path =
            std::env::temp_dir().join(format!("oxide-update-{tag}-{}-{nonce}", std::process::id()));
        fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn a_stale_asset_is_refused_before_it_replaces_the_binary() {
        let asset = "Oxide-v0.27.0-linux-x64.tar.gz";
        refuse_version_mismatch(asset, "0.27.0", "0.27.0").unwrap();
        let error = refuse_version_mismatch(asset, "0.26.0", "0.27.0").unwrap_err();
        assert!(
            error
                .to_string()
                .contains("reports version 0.26.0, not 0.27.0"),
            "{error}"
        );
    }

    #[test]
    fn a_replaced_binary_is_recorded_beside_itself() {
        let dir = temp_dir("record");
        let executable = dir.join("bin/oxide");
        fs::create_dir_all(executable.parent().unwrap()).unwrap();
        assert!(!install::is_released(&executable));

        record_install(&executable, "jaysonwu991/oxide", "0.27.0");

        assert!(install::is_released(&executable));
        assert_eq!(
            fs::read_to_string(install::marker(&executable)).unwrap(),
            "source jaysonwu991/oxide\nversion 0.27.0\n"
        );
        fs::remove_dir_all(dir).unwrap();
    }

    /// A check for another component is this same resolution wearing that
    /// component's name: the desktop app and the VS Code panel cannot link
    /// `oxide-core`, so this is where they read what their own release train
    /// offers, and what to download if they install it themselves.
    #[test]
    fn a_check_for_another_component_names_its_own_release() {
        let executable = Path::new("/opt/scratch/oxide");
        let options = Options {
            check: true,
            version: None,
            force: false,
            json: true,
            component: Component::Desktop,
            current: Some("0.33.0".to_string()),
        };
        let release = Release::new(Component::Desktop, "desktop-v0.34.0", None, "darwin-arm64");
        let check = check_of(
            &options,
            &release,
            executable,
            "jaysonwu991/oxide",
            "0.33.0",
        );

        assert_eq!(check.component, Component::Desktop);
        assert!(check.update_available);
        // Nothing this command installs: the app replaces itself.
        assert!(!check.installable);
        assert!(check.path.is_empty());
        let asset = check.asset.as_ref().unwrap();
        assert_eq!(asset.name, "Oxide_0.34.0_aarch64.dmg");
        assert_eq!(
            asset.url,
            "https://github.com/jaysonwu991/oxide/releases/download/desktop-v0.34.0/Oxide_0.34.0_aarch64.dmg"
        );
        assert!(check
            .advice
            .as_deref()
            .unwrap()
            .contains("installs this release itself"));

        // A release the caller already runs leaves nothing to offer, and so no
        // advice either.
        let release = Release::new(Component::Desktop, "desktop-v0.33.0", None, "darwin-arm64");
        let check = check_of(
            &options,
            &release,
            executable,
            "jaysonwu991/oxide",
            "0.33.0",
        );
        assert!(!check.update_available);
        assert!(check.advice.is_none());
    }

    /// `oxide update` replaces the installed binary, and only that. A run asked
    /// to install the app's or the panel's release says so, rather than
    /// overwriting the CLI the caller is running with an archive it cannot use.
    #[tokio::test]
    async fn installing_another_component_is_refused() {
        let options = Options {
            check: false,
            version: None,
            force: false,
            json: false,
            component: Component::Extension,
            current: None,
        };
        let error = run_for(
            options,
            Path::new("/opt/scratch/oxide"),
            "jaysonwu991/oxide",
        )
        .await
        .unwrap_err();
        assert!(error.to_string().contains("updates itself"), "{error}");
    }

    #[test]
    fn a_pinned_cli_release_is_named_where_the_installers_read_it() {
        let release = updates::pinned(Component::Cli, "0.26.0", "linux-x64").unwrap();
        assert_eq!(release.tag, "v0.26.0");
        assert_eq!(release.version, "0.26.0");
        assert_eq!(
            release.url("jaysonwu991/oxide"),
            "https://github.com/jaysonwu991/oxide/releases/download/v0.26.0/Oxide-v0.26.0-linux-x64.tar.gz"
        );
        assert_eq!(
            legacy_url(&release.url("jaysonwu991/oxide")).unwrap(),
            "https://github.com/jaysonwu991/oxide/releases/download/v0.26.0/oxide-v0.26.0-linux-x64.tar.gz"
        );
        assert!(legacy_url("https://github.com/o/r/releases/download/v1/other.tar.gz").is_none());
    }

    /// The JSON a front-end offers the update from, and the keys it reads: a
    /// rename would otherwise leave the desktop app or the VS Code panel
    /// showing `undefined` for a version.
    #[test]
    fn the_json_check_carries_what_a_front_end_offers() {
        let executable = Path::new("/opt/scratch/oxide");
        let options = Options {
            check: true,
            version: None,
            force: false,
            json: true,
            component: Component::Cli,
            current: None,
        };
        let release = updates::pinned(Component::Cli, "0.27.0", "linux-x64").unwrap();
        let check = check_of(
            &options,
            &release,
            executable,
            "jaysonwu991/oxide",
            current_version(),
        );

        let value = serde_json::to_value(&check).unwrap();
        assert_eq!(value["component"], serde_json::json!("cli"));
        assert_eq!(value["latest"], serde_json::json!("0.27.0"));
        assert_eq!(value["tag"], serde_json::json!("v0.27.0"));
        assert_eq!(value["pinned"], serde_json::json!(false));
        assert_eq!(
            value["path"],
            serde_json::json!(executable.display().to_string())
        );
        assert_eq!(
            value["releaseUrl"],
            serde_json::json!("https://github.com/jaysonwu991/oxide/releases/tag/v0.27.0")
        );
        assert_eq!(
            value["asset"]["name"],
            serde_json::json!("Oxide-v0.27.0-linux-x64.tar.gz")
        );
        // This binary is the workspace's `0.0.0` placeholder, which is older
        // than the release and so leaves an update to offer.
        assert_eq!(value["current"], serde_json::json!(current_version()));
        assert_eq!(value["updateAvailable"], serde_json::json!(true));

        // Nothing beside a scratch path marks a release, so the check tells a
        // front-end to leave the install to the CLI, and why.
        assert_eq!(value["installation"], serde_json::json!("unknown"));
        assert_eq!(value["installable"], serde_json::json!(false));
        assert!(value["advice"].as_str().unwrap().contains("--force"));

        // A release the binary already runs is nothing to install, and so no
        // advice either.
        let release = updates::pinned(Component::Cli, current_version(), "linux-x64").unwrap();
        let check = check_of(
            &options,
            &release,
            executable,
            "jaysonwu991/oxide",
            current_version(),
        );
        let value = serde_json::to_value(&check).unwrap();
        assert_eq!(value["updateAvailable"], serde_json::json!(false));
        assert_eq!(value["advice"], serde_json::json!(null));
    }

    /// The release a `--version` pin names is offered as asked, even when the
    /// binary already runs it or is newer (a rollback is a request too), and a
    /// released install is one the front-end may update itself.
    #[test]
    fn a_pinned_check_is_offered_and_a_released_install_is_installable() {
        let dir = temp_dir("check-installable");
        let executable = dir.join("bin/oxide");
        fs::create_dir_all(executable.parent().unwrap()).unwrap();
        install::record(&executable, "0.10.0", "jaysonwu991/oxide").unwrap();
        let options = Options {
            check: true,
            version: Some("0.20.0".to_string()),
            force: false,
            json: true,
            component: Component::Cli,
            current: None,
        };
        let release = updates::pinned(Component::Cli, "0.20.0", "linux-x64").unwrap();
        let check = check_of(
            &options,
            &release,
            &executable,
            "jaysonwu991/oxide",
            current_version(),
        );

        assert!(check.pinned);
        assert!(check.update_available);
        assert!(check.installable);
        assert_eq!(check.installation, "prebuilt binary");
        assert!(check
            .advice
            .as_deref()
            .unwrap()
            .contains("run `oxide update`"));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn checksums_are_verified_per_file() {
        let dir = temp_dir("checksum");
        let file = dir.join("artifact");
        fs::write(&file, b"oxide").unwrap();
        let digest = "81a863ce5e6e98e67e81580d1ed2e11a7698d27c97e9fa1dffb69af5f940e3be";
        verify(&file, format!("{digest}  artifact\n").as_bytes()).unwrap();
        verify(
            &file,
            format!("{}  artifact\n", digest.to_uppercase()).as_bytes(),
        )
        .unwrap();
        assert!(verify(&file, b"0".repeat(64).as_slice()).is_err());
        assert!(verify(&file, b"").is_err());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn replaces_the_binary_in_place() {
        let dir = temp_dir("replace");
        let staged = dir.join("staged");
        let destination = dir.join("bin/oxide");
        fs::create_dir_all(destination.parent().unwrap()).unwrap();
        fs::write(&staged, b"new").unwrap();
        fs::write(&destination, b"old").unwrap();

        replace_binary(&staged, &destination).unwrap();

        assert_eq!(fs::read(&destination).unwrap(), b"new");
        let leftovers = fs::read_dir(destination.parent().unwrap())
            .unwrap()
            .filter_map(Result::ok)
            .map(|entry| entry.file_name().to_string_lossy().to_string())
            .filter(|name| name.starts_with(STAGING_PREFIX))
            .collect::<Vec<_>>();
        assert!(leftovers.is_empty(), "left {leftovers:?} behind");
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn stale_staging_files_are_discarded() {
        let dir = temp_dir("stale");
        let stale = dir.join(format!("{STAGING_PREFIX}1.old"));
        fs::write(&stale, b"old").unwrap();
        let kept = dir.join("oxide");
        fs::write(&kept, b"current").unwrap();

        discard_stale(&dir);

        assert!(!stale.exists());
        assert!(kept.exists());
        fs::remove_dir_all(dir).unwrap();
    }
}
