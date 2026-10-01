use crate::install::{self, detect_install_method, InstallMethod};
use anyhow::{bail, Context, Result};
use reqwest::StatusCode;
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

/// The repository releases live in, unless `OXIDE_REPO` names a fork.
const DEFAULT_REPO: &str = "jaysonwu991/oxide";
const MANIFEST_NAME: &str = "Oxide-manifest";
const LEGACY_MANIFEST_NAME: &str = "oxide-manifest";
const STAGING_PREFIX: &str = ".oxide-update-";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(300);
const SUPPORTED_PLATFORMS: &str = "darwin-arm64, darwin-x64, linux-x64, linux-arm64, win32-x64";

#[derive(Debug, Clone)]
pub struct Options {
    pub check: bool,
    pub version: Option<String>,
    pub force: bool,
}

pub async fn run(options: Options) -> Result<()> {
    let executable = std::env::current_exe().context("resolving the oxide executable")?;
    let repo = std::env::var("OXIDE_REPO").unwrap_or_else(|_| DEFAULT_REPO.to_string());
    run_for(options, &executable, &repo).await
}

async fn run_for(options: Options, executable: &Path, repo: &str) -> Result<()> {
    let platform = platform()?;
    let method = detect_install_method(executable);
    let current = current_version();

    println!("Oxide update");
    println!(
        "Installation: {} at {}",
        method.label(),
        executable.display()
    );
    println!("Current: {current}");

    let client = client()?;
    let release = match options.version.as_deref() {
        Some(requested) => pinned_release(requested, platform)?,
        None => latest_release(&client, repo, platform).await?,
    };
    if options.version.is_some() {
        println!("Target: {}", release.tag);
    } else {
        println!("Latest: {}", release.tag);
    }

    if !options.force && !should_install(&release, current, options.version.is_some()) {
        println!(
            "Already up to date; rerun with --force to reinstall {}.",
            release.tag
        );
        return Ok(());
    }
    if options.check {
        println!("{}", advice(method, executable));
        return Ok(());
    }
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
    match fetch(&client, &format!("{url}.sha256"), None).await? {
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

fn client() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .user_agent(format!("oxide/{}", current_version()))
        .timeout(REQUEST_TIMEOUT)
        .build()
        .context("building the http client")
}

/// The platform key the release assets are named after.
fn platform() -> Result<&'static str> {
    platform_for(std::env::consts::OS, std::env::consts::ARCH)
}

fn platform_for(os: &str, arch: &str) -> Result<&'static str> {
    match (os, arch) {
        ("macos", "aarch64") => Ok("darwin-arm64"),
        ("macos", "x86_64") => Ok("darwin-x64"),
        ("linux", "x86_64") => Ok("linux-x64"),
        ("linux", "aarch64") => Ok("linux-arm64"),
        ("windows", "x86_64") => Ok("win32-x64"),
        _ => bail!("no prebuilt binary for {os}-{arch}; supported targets: {SUPPORTED_PLATFORMS}"),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Release {
    tag: String,
    version: String,
    asset: String,
}

impl Release {
    fn new(tag: &str, asset: Option<String>, platform: &str) -> Self {
        Self {
            tag: tag.to_string(),
            version: version_of(tag),
            asset: asset.unwrap_or_else(|| archive_name(tag, platform)),
        }
    }

    fn url(&self, repo: &str) -> String {
        format!(
            "https://github.com/{repo}/releases/download/{}/{}",
            self.tag, self.asset
        )
    }
}

/// The release a `--version` argument pins, which resolves without asking
/// GitHub anything.
fn pinned_release(requested: &str, platform: &str) -> Result<Release> {
    let tag = normalize_tag(requested)?;
    Ok(Release {
        version: version_of(&tag),
        asset: archive_name(&tag, platform),
        tag,
    })
}

/// The newest CLI release. The installers read the release manifest first —
/// `/releases/latest` may point at a desktop or extension release, which
/// carries none — and fall back to the API's release list.
async fn latest_release(client: &reqwest::Client, repo: &str, platform: &str) -> Result<Release> {
    let manifest_url =
        format!("https://github.com/{repo}/releases/latest/download/{MANIFEST_NAME}");
    let manifest = match fetch(client, &manifest_url, None).await? {
        Some(bytes) => Some(String::from_utf8_lossy(&bytes).to_string()),
        None => {
            let legacy = format!(
                "https://github.com/{repo}/releases/latest/download/{LEGACY_MANIFEST_NAME}"
            );
            fetch(client, &legacy, None)
                .await?
                .map(|bytes| String::from_utf8_lossy(&bytes).to_string())
        }
    };
    if let Some(version) = manifest
        .as_deref()
        .and_then(|text| manifest_value(text, "version"))
    {
        let tag = normalize_tag(&version)
            .with_context(|| format!("the release manifest names version {version}"))?;
        let asset = manifest
            .as_deref()
            .and_then(|text| manifest_value(text, platform));
        return Ok(Release::new(&tag, asset, platform));
    }

    let url = format!("https://api.github.com/repos/{repo}/releases?per_page=100");
    let token = std::env::var("GH_TOKEN")
        .or_else(|_| std::env::var("GITHUB_TOKEN"))
        .ok();
    let body = fetch(client, &url, token.as_deref())
        .await?
        .with_context(|| format!("{url} has no releases"))?;
    let releases: serde_json::Value =
        serde_json::from_slice(&body).with_context(|| format!("reading {url}"))?;
    let tag = cli_tag(&releases).with_context(|| format!("no CLI release found in {repo}"))?;
    Ok(Release::new(&tag, None, platform))
}

/// The newest release whose tag belongs to the CLI, which is what `v*` marks.
/// The desktop (`desktop-v*`) and the extension (`extension-v*`) release from
/// their own tags, so their releases are skipped rather than installed.
fn cli_tag(releases: &serde_json::Value) -> Option<String> {
    releases.as_array()?.iter().find_map(|release| {
        let tag = release.get("tag_name")?.as_str()?;
        let rest = tag.strip_prefix('v')?;
        rest.starts_with(|first: char| first.is_ascii_digit())
            .then(|| tag.to_string())
    })
}

/// One `key: value` line of the release manifest the installers read.
fn manifest_value(text: &str, key: &str) -> Option<String> {
    text.lines().find_map(|line| {
        let (name, value) = line.split_once(':')?;
        let value = value.trim();
        (name.trim() == key && !value.is_empty()).then(|| value.to_string())
    })
}

fn archive_name(tag: &str, platform: &str) -> String {
    let extension = if platform.starts_with("win32") {
        "zip"
    } else {
        "tar.gz"
    };
    format!("Oxide-{tag}-{platform}.{extension}")
}

fn binary_name() -> &'static str {
    if cfg!(windows) {
        "oxide.exe"
    } else {
        "oxide"
    }
}

/// The tag GitHub publishes for a requested version: `1.2.3` and `v1.2.3` name
/// `v1.2.3`, and the `cli-v1.2.3` spelling release CI also accepts keeps its
/// component prefix, since the tag itself carries it.
fn normalize_tag(requested: &str) -> Result<String> {
    let trimmed = requested.trim();
    let (prefix, version) = match trimmed.strip_prefix("cli-") {
        Some(rest) => ("cli-", rest.strip_prefix('v').unwrap_or(rest)),
        None => ("", trimmed.strip_prefix('v').unwrap_or(trimmed)),
    };
    if !is_semver(version) {
        bail!("`{requested}` is not a version (expected something like v0.26.0)");
    }
    Ok(format!("{prefix}v{version}"))
}

fn version_of(tag: &str) -> String {
    let tag = tag.trim();
    tag.strip_prefix("cli-")
        .unwrap_or(tag)
        .trim_start_matches('v')
        .to_string()
}

fn is_semver(text: &str) -> bool {
    let (core, suffix) = match text.find(['-', '+']) {
        Some(index) => (&text[..index], Some(&text[index + 1..])),
        None => (text, None),
    };
    let mut parts = core.split('.');
    let numbers = (0..3).all(|_| {
        parts
            .next()
            .is_some_and(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()))
    });
    numbers
        && parts.next().is_none()
        && suffix.is_none_or(|suffix| {
            !suffix.is_empty()
                && suffix
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'+'))
        })
}

/// Whether `latest` names a later release than `current`. A version that does
/// not parse (the `0.0.0` a source build reports, or a shim) is treated as
/// older so the update is offered rather than silently skipped.
fn is_newer(latest: &str, current: &str) -> bool {
    match (parse_version(latest), parse_version(current)) {
        (Some(latest), Some(current)) => compare(&latest, &current).is_gt(),
        _ => true,
    }
}

#[derive(Debug, PartialEq, Eq)]
struct Version {
    major: u64,
    minor: u64,
    patch: u64,
    pre: Vec<Identifier>,
}

/// One dot-separated pre-release identifier, ordered the way SemVer orders
/// them: a numeric identifier compares as a number and always ranks below an
/// alphanumeric one, which compares in ASCII order.
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Identifier {
    Numeric(u64),
    Text(String),
}

fn parse_version(text: &str) -> Option<Version> {
    let text = version_of(text);
    // Build metadata carries no precedence — two releases that differ only in
    // it are the same version — and a pre-release may itself hold a `-`, so the
    // metadata comes off before the pre-release is read.
    let (text, _build) = text.split_once('+').unwrap_or((text.as_str(), ""));
    let (core, pre) = match text.split_once('-') {
        Some((core, pre)) => (core, Some(pre)),
        None => (text, None),
    };
    let mut parts = core.split('.');
    let version = Version {
        major: parts.next()?.parse().ok()?,
        minor: parts.next().unwrap_or("0").parse().ok()?,
        patch: parts.next().unwrap_or("0").parse().ok()?,
        pre: match pre {
            Some(pre) => pre_identifiers(pre)?,
            None => Vec::new(),
        },
    };
    parts.next().is_none().then_some(version)
}

fn pre_identifiers(pre: &str) -> Option<Vec<Identifier>> {
    if pre.is_empty() {
        return None;
    }
    pre.split('.')
        .map(|part| {
            let alphanumeric = part
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-');
            if !alphanumeric {
                return None;
            }
            if part.bytes().all(|byte| byte.is_ascii_digit()) {
                part.parse().ok().map(Identifier::Numeric)
            } else {
                Some(Identifier::Text(part.to_string()))
            }
        })
        .collect()
}

/// A release outranks its own pre-releases, so `1.0.0` is newer than
/// `1.0.0-rc.1` and an update is offered for it.
fn compare(latest: &Version, current: &Version) -> std::cmp::Ordering {
    (latest.major, latest.minor, latest.patch)
        .cmp(&(current.major, current.minor, current.patch))
        .then_with(|| match (latest.pre.is_empty(), current.pre.is_empty()) {
            (true, true) => std::cmp::Ordering::Equal,
            (true, false) => std::cmp::Ordering::Greater,
            (false, true) => std::cmp::Ordering::Less,
            (false, false) => latest.pre.cmp(&current.pre),
        })
}

/// Whether the release is worth installing: a pinned `--version` is installed
/// as asked — a rollback is a request too — while the newest release is
/// installed only when it is actually newer. `--force` bypasses both.
fn should_install(release: &Release, current: &str, pinned: bool) -> bool {
    if pinned {
        return release.version != version_of(current);
    }
    is_newer(&release.version, current)
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
    if let Some(bytes) = fetch(client, &url, None).await? {
        return Ok((url, bytes));
    }
    if let Some(legacy) = legacy_url(&url) {
        let asset = legacy.rsplit('/').next().unwrap_or(&release.asset);
        println!("Downloading {asset}");
        if let Some(bytes) = fetch(client, &legacy, None).await? {
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

/// A `None` result is a 404: a file that release does not carry.
async fn fetch(
    client: &reqwest::Client,
    url: &str,
    token: Option<&str>,
) -> Result<Option<Vec<u8>>> {
    let mut request = client.get(url);
    if let Some(token) = token {
        request = request.bearer_auth(token);
    }
    let response = request
        .send()
        .await
        .with_context(|| format!("requesting {url}"))?;
    let status = response.status();
    if status == StatusCode::NOT_FOUND {
        return Ok(None);
    }
    if !status.is_success() {
        bail!("{url} returned {status}");
    }
    let bytes = response
        .bytes()
        .await
        .with_context(|| format!("reading {url}"))?;
    Ok(Some(bytes.to_vec()))
}

fn verify(file: &Path, sums: &[u8]) -> Result<()> {
    let text = String::from_utf8_lossy(sums);
    let expected = text
        .split_whitespace()
        .next()
        .context("the checksum file has no digest")?;
    let bytes = fs::read(file).with_context(|| format!("reading {}", file.display()))?;
    let actual = hex(&Sha256::digest(&bytes));
    if !expected.eq_ignore_ascii_case(&actual) {
        bail!(
            "checksum mismatch for {}: expected {expected}, got {actual}",
            file.display()
        );
    }
    Ok(())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
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

/// A private scratch directory for the download and its unpacking, removed
/// again when the update ends — successfully or not.
struct WorkDir(PathBuf);

impl WorkDir {
    fn new() -> Result<Self> {
        let mut random = [0u8; 16];
        getrandom::getrandom(&mut random)
            .ok()
            .context("generating a private name for the temporary directory")?;
        let name: String = random.iter().map(|byte| format!("{byte:02x}")).collect();
        let dir = std::env::temp_dir().join(format!("oxide-update-{name}"));
        // `mode` is a unix-only method, so `mut` is only needed there; without
        // this the Windows release build warns that the variable is never
        // mutated.
        #[cfg_attr(not(unix), allow(unused_mut))]
        let mut builder = fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder
            .create(&dir)
            .with_context(|| format!("creating {}", dir.display()))?;
        Ok(Self(dir))
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for WorkDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
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
            std::env::temp_dir().join(format!("oxide-update-{tag}-{}-{nonce}", std::process::id()));
        fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn pinned_versions_install_and_the_latest_only_when_newer() {
        let latest = pinned_release("0.26.0", "linux-x64").unwrap();
        // The newest release is installed only when it is newer than the built-in one.
        assert!(should_install(&latest, "0.25.0", false));
        assert!(!should_install(&latest, "0.26.0", false));
        assert!(!should_install(&latest, "0.27.0", false));
        // A pinned version is installed as asked, including a rollback.
        assert!(should_install(&latest, "0.27.0", true));
        assert!(!should_install(&latest, "0.26.0", true));
        // A version that does not parse is offered the update.
        assert!(should_install(&latest, "0.0.0", false));
    }

    #[test]
    fn platforms_match_the_release_matrix() {
        assert_eq!(platform_for("macos", "aarch64").unwrap(), "darwin-arm64");
        assert_eq!(platform_for("macos", "x86_64").unwrap(), "darwin-x64");
        assert_eq!(platform_for("linux", "x86_64").unwrap(), "linux-x64");
        assert_eq!(platform_for("linux", "aarch64").unwrap(), "linux-arm64");
        assert_eq!(platform_for("windows", "x86_64").unwrap(), "win32-x64");
        assert!(platform_for("freebsd", "x86_64").is_err());
        assert!(platform_for("linux", "arm").is_err());
    }

    #[test]
    fn versions_are_compared_semantically() {
        assert!(is_newer("v0.26.0", "0.25.0"));
        assert!(is_newer("0.26.0", "0.25.9"));
        assert!(is_newer("1.0.0", "0.99.99"));
        assert!(!is_newer("v0.26.0", "0.26.0"));
        assert!(!is_newer("0.25.0", "0.26.0"));
        // A release outranks its own pre-releases.
        assert!(is_newer("1.0.0", "1.0.0-rc.1"));
        assert!(!is_newer("1.0.0-rc.1", "1.0.0"));
        // A version that does not parse is offered an update.
        assert!(is_newer("0.26.0", "nightly"));
        assert_eq!(version_of("cli-v1.2.3"), "1.2.3");
    }

    #[test]
    fn pre_release_identifiers_are_ordered_the_way_semver_orders_them() {
        // Build metadata carries no precedence, so the two are one version.
        assert_eq!(parse_version("1.2.3+build.5"), parse_version("1.2.3"));
        assert!(!is_newer("1.2.3+build.5", "1.2.3"));
        assert!(is_newer("1.2.3+build.5", "1.2.3-pre"));
        // Metadata after a pre-release, and a `-` inside either one.
        assert_eq!(
            parse_version("1.2.3-rc.1+build-2"),
            parse_version("1.2.3-rc.1")
        );
        assert_eq!(parse_version("1.2.3+build-2"), parse_version("1.2.3"));
        // A numeric identifier compares as a number, not as text.
        assert!(is_newer("1.2.3-rc.10", "1.2.3-rc.2"));
        assert!(!is_newer("1.2.3-rc.2", "1.2.3-rc.10"));
        // An alphanumeric identifier outranks a numeric one.
        assert!(is_newer("1.2.3-rc.alpha", "1.2.3-rc.1"));
        assert!(!is_newer("1.2.3-rc.1", "1.2.3-rc.alpha"));
        // More identifiers outrank a version that is a prefix of them.
        assert!(is_newer("1.2.3-rc.1.1", "1.2.3-rc.1"));
        assert!(!is_newer("1.2.3-rc.1", "1.2.3-rc.1.1"));
        // Junk after the core names no version at all, so the update is offered.
        assert!(parse_version("1.2.3-").is_none());
        assert!(parse_version("1.2.3-rc..1").is_none());
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

    #[test]
    fn tags_are_normalized_from_every_spelling() {
        assert_eq!(normalize_tag("1.2.3").unwrap(), "v1.2.3");
        assert_eq!(normalize_tag("v1.2.3").unwrap(), "v1.2.3");
        assert_eq!(normalize_tag(" cli-v1.2.3 ").unwrap(), "cli-v1.2.3");
        assert_eq!(version_of("cli-v1.2.3"), "1.2.3");
        assert_eq!(
            normalize_tag("v1.2.3-rc.1+build.2").unwrap(),
            "v1.2.3-rc.1+build.2"
        );
        assert!(normalize_tag("latest").is_err());
        assert!(normalize_tag("v1.2").is_err());
    }

    #[test]
    fn manifests_name_the_platform_asset() {
        let manifest = "version: v0.26.0\ndarwin-arm64: Oxide-v0.26.0-darwin-arm64.tar.gz\n\
                        linux-x64: Oxide-v0.26.0-linux-x64.tar.gz\n";
        assert_eq!(manifest_value(manifest, "version").unwrap(), "v0.26.0");
        assert_eq!(
            manifest_value(manifest, "linux-x64").unwrap(),
            "Oxide-v0.26.0-linux-x64.tar.gz"
        );
        assert!(manifest_value(manifest, "win32-x64").is_none());
        assert!(manifest_value("not a manifest", "version").is_none());
    }

    #[test]
    fn assets_and_urls_follow_the_release_layout() {
        assert_eq!(
            archive_name("v0.26.0", "darwin-arm64"),
            "Oxide-v0.26.0-darwin-arm64.tar.gz"
        );
        assert_eq!(
            archive_name("v0.26.0", "win32-x64"),
            "Oxide-v0.26.0-win32-x64.zip"
        );
        let release = pinned_release("0.26.0", "linux-x64").unwrap();
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

    #[test]
    fn the_newest_cli_tag_skips_other_components() {
        let releases = serde_json::json!([
            { "tag_name": "extension-v0.26.0" },
            { "tag_name": "desktop-v0.26.0" },
            { "tag_name": "v0.26.0" },
            { "tag_name": "v0.25.0" },
        ]);
        assert_eq!(cli_tag(&releases).unwrap(), "v0.26.0");
        assert!(cli_tag(&serde_json::json!([])).is_none());
        assert!(cli_tag(&serde_json::json!([{ "tag_name": "nightly" }])).is_none());
    }

    #[test]
    fn checksums_are_verified_per_file() {
        let dir = temp_dir("checksum");
        let file = dir.join("artifact");
        fs::write(&file, b"oxide").unwrap();
        let digest = hex(&Sha256::digest(b"oxide"));
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

    #[test]
    fn work_directories_are_private_and_removed() {
        let dir = WorkDir::new().unwrap();
        let path = dir.path().to_path_buf();
        assert!(path.is_dir());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o700);
        }
        fs::write(path.join("archive"), b"bytes").unwrap();
        drop(dir);
        assert!(!path.exists());
    }
}
