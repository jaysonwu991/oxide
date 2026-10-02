//! Release rules shared by every Oxide front-end.
//!
//! Each component releases from its own tag — the CLI from `v*`, the desktop
//! app from `desktop-v*`, the editor extension from `extension-v*` — so a tag
//! says which component it belongs to and a release never has to be guessed at.
//! `oxide update`, the desktop app's own updater and the VS Code panel all
//! resolve a release the same way (the tag, the artifact this platform
//! installs, whether it is newer than the one running), so those rules live
//! here rather than in three copies that drift apart.

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::cmp::Ordering;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// The repository releases live in, unless `OXIDE_REPO` names a fork.
pub const DEFAULT_REPO: &str = "jaysonwu991/oxide";
const MANIFEST_NAME: &str = "Oxide-manifest";
const LEGACY_MANIFEST_NAME: &str = "oxide-manifest";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(300);
const SUPPORTED_PLATFORMS: &str = "darwin-arm64, darwin-x64, linux-x64, linux-arm64, win32-x64";

/// Which released component a check is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Component {
    Cli,
    Desktop,
    Extension,
}

impl Component {
    pub const ALL: [Component; 3] = [Component::Cli, Component::Desktop, Component::Extension];

    /// The `--component` spelling, plus the names a front-end may call itself.
    pub fn parse(text: &str) -> Option<Self> {
        match text.trim().to_ascii_lowercase().as_str() {
            "cli" | "command-line" => Some(Self::Cli),
            "desktop" | "app" => Some(Self::Desktop),
            "extension" | "vscode" | "vs-code" | "editor" => Some(Self::Extension),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Cli => "cli",
            Self::Desktop => "desktop",
            Self::Extension => "extension",
        }
    }

    /// How the component reads in a sentence.
    pub fn label(self) -> &'static str {
        match self {
            Self::Cli => "the oxide CLI",
            Self::Desktop => "the Oxide desktop app",
            Self::Extension => "the Oxide extension for VS Code",
        }
    }

    /// Whether the component ships a build per platform. The extension is one
    /// VSIX for every machine, so a check for it needs no platform.
    pub fn platform_specific(self) -> bool {
        !matches!(self, Self::Extension)
    }

    /// Whether a tag belongs to this component. The CLI accepts both `v1.2.3`
    /// and the `cli-v1.2.3` spelling release CI also understands.
    pub fn owns_tag(self, tag: &str) -> bool {
        let version = match self {
            Self::Cli => tag.strip_prefix("cli-").unwrap_or(tag),
            Self::Desktop => match tag.strip_prefix("desktop-") {
                Some(rest) => rest,
                None => return false,
            },
            Self::Extension => match tag.strip_prefix("extension-") {
                Some(rest) => rest,
                None => return false,
            },
        };
        version
            .strip_prefix('v')
            .is_some_and(|rest| rest.starts_with(|first: char| first.is_ascii_digit()))
    }

    /// The tag a version is published under.
    pub fn tag(self, version: &str) -> String {
        let version = version.trim();
        let version = version.strip_prefix('v').unwrap_or(version);
        match self {
            Self::Cli => format!("v{version}"),
            Self::Desktop => format!("desktop-v{version}"),
            Self::Extension => format!("extension-v{version}"),
        }
    }

    /// A `--version` argument as a tag, accepting the component's own spelling
    /// (`desktop-v1.2.3`), the bare tag (`v1.2.3`) or a version on its own.
    pub fn normalize_tag(self, requested: &str) -> Result<String> {
        let trimmed = requested.trim();
        let without_component = ["cli-", "desktop-", "extension-"]
            .iter()
            .find_map(|prefix| trimmed.strip_prefix(prefix))
            .unwrap_or(trimmed);
        let version = without_component.trim_start_matches('v').trim();
        if !is_semver(version) {
            bail!(
                "`{requested}` is not a version (expected something like {})",
                self.tag("0.26.0")
            );
        }
        Ok(self.tag(version))
    }

    /// The artifacts this platform installs from, most preferred first: the one
    /// an in-place install uses, then the downloads a user would pick by hand.
    pub fn assets(self, tag: &str, platform: &str) -> Vec<String> {
        let version = release_version(tag);
        match self {
            Self::Cli => {
                let extension = if platform.starts_with("win32") {
                    "zip"
                } else {
                    "tar.gz"
                };
                vec![format!("Oxide-{tag}-{platform}.{extension}")]
            }
            Self::Desktop => match platform {
                "darwin-arm64" => vec![format!("Oxide_{version}_aarch64.dmg")],
                "darwin-x64" => vec![format!("Oxide_{version}_x64.dmg")],
                "linux-x64" => vec![
                    format!("Oxide_{version}_amd64.AppImage"),
                    format!("Oxide_{version}_amd64.deb"),
                    format!("Oxide-{version}-1.x86_64.rpm"),
                ],
                "linux-arm64" => vec![
                    format!("Oxide_{version}_aarch64.AppImage"),
                    format!("Oxide_{version}_arm64.deb"),
                    format!("Oxide-{version}-1.aarch64.rpm"),
                ],
                "win32-x64" => vec![
                    format!("Oxide_{version}_x64-setup.exe"),
                    format!("Oxide_{version}_x64_en-US.msi"),
                ],
                _ => Vec::new(),
            },
            Self::Extension => vec![format!("oxide-vscode-{version}.vsix")],
        }
    }

    /// The artifact a front-end offers: the first this platform has, or the
    /// empty string when the platform has none.
    pub fn asset(self, tag: &str, platform: &str) -> String {
        self.assets(tag, platform)
            .into_iter()
            .next()
            .unwrap_or_default()
    }
}

/// An artifact a release carries, as a front-end downloads it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Artifact {
    pub name: String,
    pub url: String,
    /// The `sha256:…` digest the release records for it, when it records one.
    pub digest: Option<String>,
}

/// What a check found, as the front-ends read it. `oxide update --check --json`
/// and the desktop app's own check report this shape, so a field renamed for
/// one is a field renamed for both.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Check {
    pub component: Component,
    /// The version the caller reported running — the CLI's own build version,
    /// the app's bundle version, the panel's extension version. Empty when
    /// nobody said, which leaves a front-end that reads it to compare for
    /// itself.
    pub current: String,
    /// The release the check resolved: the newest of the component, or the one
    /// `--version` asked for.
    pub latest: String,
    pub tag: String,
    /// Whether `--version` pinned the release rather than the newest being
    /// looked up.
    pub pinned: bool,
    /// Whether there is an update to install.
    pub update_available: bool,
    /// What the check found the installation to be, and whether this front-end
    /// may replace it.
    pub installation: String,
    pub installable: bool,
    pub path: String,
    /// What to do instead when the caller cannot install the release itself.
    pub advice: Option<String>,
    pub release_url: String,
    /// The artifact this platform installs, when the release carries one.
    pub asset: Option<Artifact>,
}

impl Check {
    /// The part every component answers the same way. What the check found the
    /// installation to be — and whether this front-end may replace it — is the
    /// caller's to add, since only it knows how it was installed.
    pub fn new(
        component: Component,
        release: &Release,
        current: &str,
        pinned: bool,
        repo: &str,
    ) -> Self {
        Self {
            component,
            current: current.to_string(),
            latest: release.version.clone(),
            tag: release.tag.clone(),
            pinned,
            update_available: should_install(release, current, pinned),
            installation: String::new(),
            installable: false,
            path: String::new(),
            advice: None,
            release_url: release.page_url(repo),
            asset: release.artifact(repo),
        }
    }
}

/// One release of one component, with the artifact this platform installs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Release {
    pub tag: String,
    pub version: String,
    pub asset: String,
    /// The `sha256:…` digest the release records for `asset`, when it has one.
    pub digest: Option<String>,
}

impl Release {
    pub fn new(component: Component, tag: &str, asset: Option<String>, platform: &str) -> Self {
        let asset = match asset.filter(|name| !name.is_empty()) {
            Some(asset) => asset,
            None => component.asset(tag, platform),
        };
        Self {
            tag: tag.to_string(),
            version: release_version(tag),
            asset,
            digest: None,
        }
    }

    pub fn url(&self, repo: &str) -> String {
        format!(
            "https://github.com/{repo}/releases/download/{}/{}",
            self.tag, self.asset
        )
    }

    /// The release page, which is also what a front-end opens when it cannot
    /// install the release itself.
    pub fn page_url(&self, repo: &str) -> String {
        format!("https://github.com/{repo}/releases/tag/{}", self.tag)
    }

    /// The artifact this release names for the platform, when it names one.
    pub fn artifact(&self, repo: &str) -> Option<Artifact> {
        (!self.asset.is_empty()).then(|| Artifact {
            name: self.asset.clone(),
            url: self.url(repo),
            digest: self.digest.clone(),
        })
    }
}

/// The repository the front-ends check, which `OXIDE_REPO` may point at a fork.
pub fn repo() -> String {
    std::env::var("OXIDE_REPO").unwrap_or_else(|_| DEFAULT_REPO.to_string())
}

pub fn client() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .user_agent(format!("oxide/{}", env!("CARGO_PKG_VERSION")))
        .timeout(REQUEST_TIMEOUT)
        .build()
        .context("building the http client")
}

/// The newest release of `component`.
///
/// The CLI reads the release manifest the installers read first —
/// `/releases/latest` may point at a desktop or extension release, which
/// carries none — and every component falls back to the API's release list.
pub async fn latest(
    client: &reqwest::Client,
    repo: &str,
    component: Component,
    platform: &str,
) -> Result<Release> {
    if component == Component::Cli {
        if let Some(release) = manifest_release(client, repo, platform).await? {
            return Ok(release);
        }
    }
    api_release(client, repo, component, platform).await
}

/// The release a `--version` argument pins, which resolves without asking
/// GitHub anything.
pub fn pinned(component: Component, requested: &str, platform: &str) -> Result<Release> {
    let tag = component.normalize_tag(requested)?;
    Ok(Release::new(component, &tag, None, platform))
}

async fn manifest_release(
    client: &reqwest::Client,
    repo: &str,
    platform: &str,
) -> Result<Option<Release>> {
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
    let Some(version) = manifest
        .as_deref()
        .and_then(|text| manifest_value(text, "version"))
    else {
        return Ok(None);
    };
    let tag = Component::Cli
        .normalize_tag(&version)
        .with_context(|| format!("the release manifest names version {version}"))?;
    let asset = manifest
        .as_deref()
        .and_then(|text| manifest_value(text, platform));
    Ok(Some(Release::new(Component::Cli, &tag, asset, platform)))
}

/// The newest release whose tag belongs to `component`. The list arrives
/// newest-created first, which is not newest-versioned — a patch cut for an
/// older line would otherwise be offered as the newest — so the tags are
/// ordered by the versions they name.
async fn api_release(
    client: &reqwest::Client,
    repo: &str,
    component: Component,
    platform: &str,
) -> Result<Release> {
    let url = format!("https://api.github.com/repos/{repo}/releases?per_page=100");
    let token = std::env::var("GH_TOKEN")
        .or_else(|_| std::env::var("GITHUB_TOKEN"))
        .ok();
    let body = fetch(client, &url, token.as_deref())
        .await?
        .with_context(|| format!("{url} has no releases"))?;
    let releases: serde_json::Value =
        serde_json::from_slice(&body).with_context(|| format!("reading {url}"))?;
    let tag = newest_tag(&releases, component)
        .with_context(|| format!("no release of {} found in {repo}", component.label()))?;
    let mut release = Release::new(component, &tag, None, platform);
    release.digest = asset_digest(&releases, &tag, &release.asset);
    Ok(release)
}

/// The newest release of one component among the releases GitHub lists, which
/// is what its own tag prefix marks.
pub fn newest_tag(releases: &serde_json::Value, component: Component) -> Option<String> {
    releases
        .as_array()?
        .iter()
        .filter_map(|release| release.get("tag_name")?.as_str())
        .filter(|tag| component.owns_tag(tag))
        .filter_map(|tag| parse_version(tag).map(|version| (version, tag)))
        .max_by(|(left, _), (right, _)| compare(left, right))
        .map(|(_, tag)| tag.to_string())
}

/// The `sha256:…` GitHub records for one asset of one release.
fn asset_digest(releases: &serde_json::Value, tag: &str, asset: &str) -> Option<String> {
    releases
        .as_array()?
        .iter()
        .find(|release| release.get("tag_name").and_then(|name| name.as_str()) == Some(tag))?
        .get("assets")?
        .as_array()?
        .iter()
        .find(|entry| entry.get("name").and_then(|name| name.as_str()) == Some(asset))?
        .get("digest")?
        .as_str()
        .filter(|digest| !digest.is_empty())
        .map(|digest| digest.to_string())
}

/// One `key: value` line of the release manifest the installers read.
pub fn manifest_value(text: &str, key: &str) -> Option<String> {
    text.lines().find_map(|line| {
        let (name, value) = line.split_once(':')?;
        let value = value.trim();
        (name.trim() == key && !value.is_empty()).then(|| value.to_string())
    })
}

/// The platform key the release assets are named after.
pub fn platform() -> Result<&'static str> {
    platform_for(std::env::consts::OS, std::env::consts::ARCH)
}

pub fn platform_for(os: &str, arch: &str) -> Result<&'static str> {
    match (os, arch) {
        ("macos", "aarch64") => Ok("darwin-arm64"),
        ("macos", "x86_64") => Ok("darwin-x64"),
        ("linux", "x86_64") => Ok("linux-x64"),
        ("linux", "aarch64") => Ok("linux-arm64"),
        ("windows", "x86_64") => Ok("win32-x64"),
        _ => bail!("no prebuilt binary for {os}-{arch}; supported targets: {SUPPORTED_PLATFORMS}"),
    }
}

/// Whether the release is worth installing: a pinned version is installed as
/// asked — a rollback is a request too — while the newest release is installed
/// only when it is actually newer.
pub fn should_install(release: &Release, current: &str, pinned: bool) -> bool {
    if pinned {
        return release.version != release_version(current);
    }
    is_newer(&release.version, current)
}

/// Whether `latest` names a later release than `current`. A version that does
/// not parse (the `0.0.0` a source build reports, or a shim) is treated as
/// older, so the update is offered rather than silently skipped.
pub fn is_newer(latest: &str, current: &str) -> bool {
    match (parse_version(latest), parse_version(current)) {
        (Some(latest), Some(current)) => compare(&latest, &current).is_gt(),
        _ => true,
    }
}

/// The version a tag names: `v1.2.3`, `cli-v1.2.3`, `desktop-v1.2.3` and
/// `extension-v1.2.3` all read as `1.2.3`. A version that carries no tag
/// prefix (a running binary's `--version`) reads as itself.
pub fn release_version(tag: &str) -> String {
    let tag = tag.trim();
    let without_component = ["cli-", "desktop-", "extension-"]
        .iter()
        .find_map(|prefix| tag.strip_prefix(prefix))
        .unwrap_or(tag);
    without_component.trim_start_matches('v').to_string()
}

pub fn is_semver(text: &str) -> bool {
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
    let text = release_version(text);
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
fn compare(latest: &Version, current: &Version) -> Ordering {
    (latest.major, latest.minor, latest.patch)
        .cmp(&(current.major, current.minor, current.patch))
        .then_with(|| match (latest.pre.is_empty(), current.pre.is_empty()) {
            (true, true) => Ordering::Equal,
            (true, false) => Ordering::Greater,
            (false, true) => Ordering::Less,
            (false, false) => latest.pre.cmp(&current.pre),
        })
}

/// A `None` result is a 404: a file that release does not carry.
pub async fn fetch(
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
    if status == reqwest::StatusCode::NOT_FOUND {
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

/// Downloads `url` into `destination` a chunk at a time. A desktop bundle is
/// tens of megabytes, and holding one in memory to write it out again is a copy
/// nothing needs; the byte count is returned so a caller can say how big the
/// download was.
pub async fn download(client: &reqwest::Client, url: &str, destination: &Path) -> Result<u64> {
    let mut response = client
        .get(url)
        .send()
        .await
        .with_context(|| format!("requesting {url}"))?;
    let status = response.status();
    if !status.is_success() {
        bail!("{url} returned {status}");
    }
    if let Some(parent) = destination.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    let mut file = tokio::fs::File::create(destination)
        .await
        .with_context(|| format!("writing {}", destination.display()))?;
    let mut written = 0u64;
    while let Some(chunk) = response
        .chunk()
        .await
        .with_context(|| format!("reading {url}"))?
    {
        tokio::io::AsyncWriteExt::write_all(&mut file, &chunk)
            .await
            .with_context(|| format!("writing {}", destination.display()))?;
        written += chunk.len() as u64;
    }
    tokio::io::AsyncWriteExt::flush(&mut file)
        .await
        .with_context(|| format!("writing {}", destination.display()))?;
    Ok(written)
}

/// Checks a downloaded artifact against the digest its release published. A
/// digest that is not the file's is how a truncated or substituted download is
/// refused before it is installed, since the same bytes go on to replace the
/// app or open an installer.
pub fn verify_sha256(file: &Path, expected: &str) -> Result<()> {
    let expected = expected.trim();
    let expected = match expected.split_once(':') {
        Some((algorithm, digest)) if algorithm.eq_ignore_ascii_case("sha256") => digest,
        _ => expected,
    };
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

/// A private scratch directory for a download and whatever unpacking follows
/// it, removed again when the update ends — successfully or not. It is created
/// 0700 under the system temporary directory, since what lands in it is about to
/// replace a binary or an app.
pub struct WorkDir(PathBuf);

impl WorkDir {
    pub fn new() -> Result<Self> {
        let mut random = [0u8; 16];
        getrandom::getrandom(&mut random)
            .ok()
            .context("generating a private name for the temporary directory")?;
        let name = hex(&random);
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

    pub fn path(&self) -> &Path {
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

    #[test]
    fn a_work_directory_is_private_and_removed_again() {
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

    #[test]
    fn pinned_versions_install_and_the_latest_only_when_newer() {
        let latest = pinned(Component::Cli, "0.26.0", "linux-x64").unwrap();
        // The newest release is installed only when it is newer than the running one.
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
        assert_eq!(release_version("cli-v1.2.3"), "1.2.3");
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
    fn each_component_owns_its_own_tags() {
        assert!(Component::Cli.owns_tag("v0.34.0"));
        assert!(Component::Cli.owns_tag("cli-v0.34.0"));
        assert!(Component::Desktop.owns_tag("desktop-v0.34.0"));
        assert!(Component::Extension.owns_tag("extension-v0.34.0"));
        // A release of another component is not this one's, and neither is a
        // tag that names no version.
        assert!(!Component::Cli.owns_tag("desktop-v0.34.0"));
        assert!(!Component::Cli.owns_tag("extension-v0.34.0"));
        assert!(!Component::Desktop.owns_tag("v0.34.0"));
        assert!(!Component::Extension.owns_tag("desktop-v0.34.0"));
        assert!(!Component::Desktop.owns_tag("desktop-nightly"));
        assert!(!Component::Cli.owns_tag("vnext"));
    }

    #[test]
    fn tags_are_normalized_from_every_spelling() {
        assert_eq!(Component::Cli.normalize_tag("1.2.3").unwrap(), "v1.2.3");
        assert_eq!(Component::Cli.normalize_tag("v1.2.3").unwrap(), "v1.2.3");
        assert_eq!(
            Component::Cli.normalize_tag("cli-v1.2.3").unwrap(),
            "v1.2.3"
        );
        assert_eq!(release_version("cli-v1.2.3"), "1.2.3");
        assert_eq!(
            Component::Cli.normalize_tag("v1.2.3-rc.1+build.2").unwrap(),
            "v1.2.3-rc.1+build.2"
        );
        assert!(Component::Cli.normalize_tag("latest").is_err());
        assert!(Component::Cli.normalize_tag("v1.2").is_err());

        // Each component names its own train, and a version on its own is that
        // train's tag.
        assert_eq!(
            Component::Desktop.normalize_tag("0.34.0").unwrap(),
            "desktop-v0.34.0"
        );
        assert_eq!(
            Component::Desktop.normalize_tag("desktop-v0.34.0").unwrap(),
            "desktop-v0.34.0"
        );
        assert_eq!(
            Component::Extension
                .normalize_tag("extension-v0.34.0")
                .unwrap(),
            "extension-v0.34.0"
        );
        assert_eq!(release_version("desktop-v0.34.0"), "0.34.0");
        assert_eq!(release_version("extension-v0.34.0"), "0.34.0");
        assert_eq!(release_version("0.34.0"), "0.34.0");
    }

    #[test]
    fn components_are_named_the_way_a_front_end_names_itself() {
        assert_eq!(Component::parse("cli"), Some(Component::Cli));
        assert_eq!(Component::parse("desktop"), Some(Component::Desktop));
        assert_eq!(Component::parse(" Desktop "), Some(Component::Desktop));
        assert_eq!(Component::parse("extension"), Some(Component::Extension));
        assert_eq!(Component::parse("vscode"), Some(Component::Extension));
        assert_eq!(Component::parse("nightly"), None);
        // The one component that is not built per platform is the one a check
        // needs no platform for.
        assert!(Component::Cli.platform_specific());
        assert!(Component::Desktop.platform_specific());
        assert!(!Component::Extension.platform_specific());
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

    /// The artifact each component publishes for a platform, checked against
    /// the names the `desktop-v0.34.0` and `extension-v0.34.0` releases carry.
    #[test]
    fn releases_are_downloaded_by_the_artifact_their_platform_installs() {
        let cli = Release::new(Component::Cli, "v0.26.0", None, "darwin-arm64");
        assert_eq!(cli.asset, "Oxide-v0.26.0-darwin-arm64.tar.gz");
        assert_eq!(cli.version, "0.26.0");
        assert_eq!(
            cli.url("jaysonwu991/oxide"),
            "https://github.com/jaysonwu991/oxide/releases/download/v0.26.0/Oxide-v0.26.0-darwin-arm64.tar.gz"
        );
        assert_eq!(
            Release::new(Component::Cli, "v0.26.0", None, "win32-x64").asset,
            "Oxide-v0.26.0-win32-x64.zip"
        );

        let desktop = Release::new(Component::Desktop, "desktop-v0.34.0", None, "darwin-arm64");
        assert_eq!(desktop.version, "0.34.0");
        assert_eq!(desktop.asset, "Oxide_0.34.0_aarch64.dmg");
        assert_eq!(
            desktop.url("jaysonwu991/oxide"),
            "https://github.com/jaysonwu991/oxide/releases/download/desktop-v0.34.0/Oxide_0.34.0_aarch64.dmg"
        );
        assert_eq!(
            desktop.page_url("jaysonwu991/oxide"),
            "https://github.com/jaysonwu991/oxide/releases/tag/desktop-v0.34.0"
        );
        for (platform, asset) in [
            ("darwin-x64", "Oxide_0.34.0_x64.dmg"),
            ("linux-x64", "Oxide_0.34.0_amd64.AppImage"),
            ("linux-arm64", "Oxide_0.34.0_aarch64.AppImage"),
            ("win32-x64", "Oxide_0.34.0_x64-setup.exe"),
        ] {
            assert_eq!(
                Release::new(Component::Desktop, "desktop-v0.34.0", None, platform).asset,
                asset
            );
        }
        // The installers a user would pick by hand are named too, so a
        // front-end can offer a download when it cannot install in place.
        assert_eq!(
            Component::Desktop.assets("desktop-v0.34.0", "linux-x64"),
            [
                "Oxide_0.34.0_amd64.AppImage",
                "Oxide_0.34.0_amd64.deb",
                "Oxide-0.34.0-1.x86_64.rpm",
            ]
        );
        assert!(Component::Desktop
            .assets("desktop-v0.34.0", "freebsd-x64")
            .is_empty());

        // The extension is one VSIX for every machine, so its check names no
        // platform at all.
        let extension = Release::new(Component::Extension, "extension-v0.34.0", None, "");
        assert_eq!(extension.asset, "oxide-vscode-0.34.0.vsix");
        assert_eq!(
            extension.url("jaysonwu991/oxide"),
            "https://github.com/jaysonwu991/oxide/releases/download/extension-v0.34.0/oxide-vscode-0.34.0.vsix"
        );
    }

    #[test]
    fn the_newest_release_of_each_component_is_picked() {
        let releases = serde_json::json!([
            { "tag_name": "extension-v0.26.0" },
            { "tag_name": "desktop-v0.26.0" },
            { "tag_name": "v0.25.0" },
            { "tag_name": "v0.26.0" },
        ]);
        // The component's own train, whatever order the API listed them in.
        assert_eq!(newest_tag(&releases, Component::Cli).unwrap(), "v0.26.0");
        assert_eq!(
            newest_tag(&releases, Component::Desktop).unwrap(),
            "desktop-v0.26.0"
        );
        assert_eq!(
            newest_tag(&releases, Component::Extension).unwrap(),
            "extension-v0.26.0"
        );

        // A patch cut for an older line, published after a newer release, is
        // not the newest version of its component.
        let out_of_order = serde_json::json!([
            { "tag_name": "desktop-v0.33.1" },
            { "tag_name": "desktop-v0.34.0" },
        ]);
        assert_eq!(
            newest_tag(&out_of_order, Component::Desktop).unwrap(),
            "desktop-v0.34.0"
        );
        // A train that has never released, and a tag naming no version, leave
        // nothing to offer rather than something wrong.
        assert!(newest_tag(&serde_json::json!([]), Component::Desktop).is_none());
        assert!(newest_tag(
            &serde_json::json!([{ "tag_name": "nightly" }]),
            Component::Cli
        )
        .is_none());
        assert!(newest_tag(
            &serde_json::json!([{ "tag_name": "v0.26.0" }]),
            Component::Desktop
        )
        .is_none());
    }

    #[test]
    fn a_check_carries_the_artifact_a_front_end_downloads() {
        let release = Release::new(Component::Desktop, "desktop-v0.34.0", None, "darwin-arm64");
        let mut check = Check::new(Component::Desktop, &release, "0.33.0", false, DEFAULT_REPO);
        check.installation = "application bundle".to_string();
        check.installable = true;

        let value = serde_json::to_value(&check).unwrap();
        assert_eq!(value["component"], serde_json::json!("desktop"));
        assert_eq!(value["latest"], serde_json::json!("0.34.0"));
        assert_eq!(value["tag"], serde_json::json!("desktop-v0.34.0"));
        assert_eq!(value["updateAvailable"], serde_json::json!(true));
        assert_eq!(value["installable"], serde_json::json!(true));
        assert_eq!(
            value["asset"]["name"],
            serde_json::json!("Oxide_0.34.0_aarch64.dmg")
        );
        assert_eq!(
            value["asset"]["url"],
            serde_json::json!(
                "https://github.com/jaysonwu991/oxide/releases/download/desktop-v0.34.0/Oxide_0.34.0_aarch64.dmg"
            )
        );
        assert_eq!(value["asset"]["digest"], serde_json::json!(null));
        assert_eq!(
            value["releaseUrl"],
            serde_json::json!("https://github.com/jaysonwu991/oxide/releases/tag/desktop-v0.34.0")
        );
        // A round trip is how the desktop app reads its own check back.
        let parsed: Check = serde_json::from_value(value).unwrap();
        assert_eq!(parsed.component, Component::Desktop);
        assert_eq!(parsed.asset.unwrap().name, "Oxide_0.34.0_aarch64.dmg");
        assert!(parsed.update_available);

        // A release of another component is not this one's to offer.
        let release = Release::new(Component::Extension, "extension-v0.34.0", None, "");
        let check = Check::new(
            Component::Extension,
            &release,
            "0.34.0",
            false,
            DEFAULT_REPO,
        );
        assert!(!check.update_available);
        assert_eq!(check.asset.unwrap().name, "oxide-vscode-0.34.0.vsix");
    }

    #[test]
    fn the_digest_of_the_named_asset_is_read_from_the_release_list() {
        let releases = serde_json::json!([]);
        assert!(newest_tag(&releases, Component::Desktop).is_none());

        let listed = serde_json::json!([{
            "tag_name": "desktop-v0.34.0",
            "assets": [
                { "name": "Oxide_0.34.0_aarch64.dmg", "digest": "sha256:abc" },
                { "name": "Oxide_0.34.0_x64.dmg", "digest": "sha256:def" }
            ]
        }]);
        assert_eq!(
            asset_digest(&listed, "desktop-v0.34.0", "Oxide_0.34.0_aarch64.dmg").unwrap(),
            "sha256:abc"
        );
        // An asset the API records no digest for is left unverified rather
        // than refused.
        assert!(asset_digest(&listed, "desktop-v0.34.0", "Oxide_0.34.0_amd64.AppImage").is_none());
        assert!(asset_digest(&listed, "desktop-v0.33.0", "Oxide_0.34.0_aarch64.dmg").is_none());
    }

    #[test]
    fn a_download_that_is_not_the_released_bytes_is_refused() {
        let dir = std::env::temp_dir().join(format!("oxide-updates-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let file = dir.join("asset");
        fs::write(&file, b"abc").unwrap();
        // sha256("abc"), as the release publishes it, in either spelling.
        let digest = "sha256:ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
        verify_sha256(&file, digest).unwrap();
        verify_sha256(&file, &digest.to_uppercase()).unwrap();

        let error = verify_sha256(&file, "sha256:0000").unwrap_err();
        assert!(error.to_string().contains("checksum mismatch"), "{error}");
        fs::remove_dir_all(&dir).unwrap();
    }
}
