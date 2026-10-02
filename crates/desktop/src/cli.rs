//! Driving the installed `oxide` CLI from the desktop app.
//!
//! The app links `oxide-core` for everything a turn needs, but a released install
//! belongs to the CLI: `oxide update` is the one place the release list, the
//! manifest's checksums and the install itself are resolved, so the app runs that
//! binary instead of keeping a second copy of those rules in step. Nothing here
//! touches Tauri, so which binary is chosen and how its answer is read are unit
//! tested like the rest of the library.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// How long a CLI call may take. A check is one release lookup; an install
/// downloads an archive and unpacks it, so it gets the longer budget.
pub const CHECK_TIMEOUT: Duration = Duration::from_secs(60);
pub const INSTALL_TIMEOUT: Duration = Duration::from_secs(600);

/// The exit of a CLI call: what it printed, and how it ended.
#[derive(Debug, Clone)]
pub struct CliOutput {
    pub code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

impl CliOutput {
    pub fn succeeded(&self) -> bool {
        self.code == Some(0)
    }

    /// Everything the call said, on whichever stream it said it: `oxide update`
    /// writes its progress on stdout and a failure on stderr, and a window
    /// showing one text should not have to pick between them.
    pub fn text(&self) -> String {
        [self.stdout.trim(), self.stderr.trim()]
            .into_iter()
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join("\n")
    }
}

/// Why a call is reported as failed: what the CLI printed, else how it ended, so
/// a call that said nothing at all is not read as having succeeded.
pub fn failure(output: &CliOutput, what: &str) -> String {
    let text = output.text();
    if !text.is_empty() {
        return text;
    }
    match output.code {
        Some(code) => format!("{what} exited with code {code}"),
        None => format!("{what} was killed"),
    }
}

/// The name a released `oxide` has on this platform.
pub fn binary_name() -> &'static str {
    if cfg!(windows) {
        "oxide.exe"
    } else {
        "oxide"
    }
}

/// The directories a released `oxide` lands in, in the order they are searched.
///
/// `PATH` comes first, then the directories the installers use. The latter are
/// here because a GUI app is not started from a shell: macOS hands a launch from
/// the Finder the bare `/usr/bin:/bin:/usr/sbin:/sbin`, so a Homebrew install —
/// exactly the one `oxide update` hands to `brew upgrade` — is on no `PATH` this
/// process can see.
pub fn search_paths(path_var: &str, home: Option<&Path>, os: &str) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = std::env::split_paths(path_var)
        .filter(|dir| !dir.as_os_str().is_empty())
        .collect();
    if let Some(home) = home {
        dirs.push(home.join(".local/bin"));
        dirs.push(home.join(".cargo/bin"));
        if os == "windows" {
            // Where the released Windows archive is unpacked, and the shim
            // directory a package manager copies it through.
            dirs.push(home.join("AppData/Local/Programs/oxide"));
            dirs.push(home.join("scoop/shims"));
        }
    }
    if os == "macos" {
        dirs.push(PathBuf::from("/opt/homebrew/bin"));
        dirs.push(PathBuf::from("/usr/local/bin"));
    }
    dirs
}

/// The first of `dirs` that holds `name`.
pub fn locate(dirs: &[PathBuf], name: &str, exists: &dyn Fn(&Path) -> bool) -> Option<PathBuf> {
    dirs.iter()
        .map(|dir| dir.join(name))
        .find(|candidate| exists(candidate))
}

/// The installed CLI, or `None` when this machine has none the app can find.
pub fn binary() -> Option<PathBuf> {
    let dirs = search_paths(
        &std::env::var("PATH").unwrap_or_default(),
        dirs::home_dir().as_deref(),
        std::env::consts::OS,
    );
    locate(&dirs, binary_name(), &|candidate| candidate.is_file())
}

/// Runs the CLI and collects its output, giving up after `timeout`. The child is
/// killed when it is dropped, so a call that ran long leaves no process behind.
pub async fn run(binary: &Path, args: &[&str], timeout: Duration) -> Result<CliOutput, String> {
    let mut command = tokio::process::Command::new(binary);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    // The app has no console of its own, so a console program run from it would
    // otherwise flash a window over the app on every check.
    #[cfg(windows)]
    command.creation_flags(CREATE_NO_WINDOW);
    let child = command
        .spawn()
        .map_err(|error| format!("could not run {}: {error}", binary.display()))?;
    match tokio::time::timeout(timeout, child.wait_with_output()).await {
        Ok(Ok(output)) => Ok(CliOutput {
            code: output.status.code(),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        }),
        Ok(Err(error)) => Err(format!("could not run {}: {error}", binary.display())),
        Err(_) => Err(format!(
            "{} did not answer within {}s",
            binary.display(),
            timeout.as_secs()
        )),
    }
}

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// What `oxide update --check --json` answers with, mirroring the CLI's own
/// `Check` (`crates/cli/src/update.rs`): the release the check resolved and what
/// it found this installation to be. The front-end decides nothing about
/// releases itself — `updateAvailable` and `installable` are the CLI's answers,
/// so the app offers the same release the terminal would install.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateCheck {
    /// The version the installed binary reports.
    pub current: String,
    /// The newest CLI release, or the one a pin asked for.
    pub latest: String,
    pub tag: String,
    pub pinned: bool,
    /// Whether there is an update to install.
    pub update_available: bool,
    /// `cargo`, `homebrew`, `prebuilt binary` or `unknown`.
    pub installation: String,
    /// Whether `oxide update` can replace this installation in place: Homebrew
    /// belongs to `brew upgrade`, and a binary that is not a release needs
    /// `--force` first.
    pub installable: bool,
    pub path: String,
    /// What the terminal would say about installing this one.
    pub advice: Option<String>,
    pub release_url: String,
}

/// Reads the check out of the CLI's own JSON.
pub fn parse_check(text: &str) -> Result<UpdateCheck, String> {
    serde_json::from_str(text.trim())
        .map_err(|error| format!("could not read the update check: {error}"))
}

/// Whether the CLI refused the check because it does not know `--json`.
///
/// An `oxide` released before this app is the CLI most machines have, and it
/// answers a flag it predates with clap's own `unexpected argument '--json'`
/// plus its usage line. That is not a check that failed: there is no release to
/// report, but the installation is older than the report this app reads — and
/// `oxide update` updates it on every version, which is the one thing worth
/// offering here, since a user looking for an update is exactly the one running
/// the older binary.
pub fn rejects_json(output: &CliOutput) -> bool {
    output.stderr.contains("unexpected argument '--json'")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn exists(path: &Path) -> bool {
        path == Path::new("/home/dev/.local/bin/oxide")
    }

    #[test]
    fn the_install_directories_are_searched_after_the_path() {
        // The list is split the way this host's environment holds it (a colon on
        // Unix, a semicolon on Windows), so the fixture is joined rather than
        // spelled: a POSIX-colon string is one entry on Windows.
        let path_var = std::env::join_paths(["/usr/bin", "/usr/local/bin"]).unwrap();
        let dirs = search_paths(
            path_var.to_str().unwrap(),
            Some(Path::new("/home/dev")),
            "macos",
        );
        assert_eq!(dirs[0], PathBuf::from("/usr/bin"));
        assert_eq!(dirs[1], PathBuf::from("/usr/local/bin"));
        assert!(dirs.contains(&PathBuf::from("/home/dev/.local/bin")));
        assert!(dirs.contains(&PathBuf::from("/home/dev/.cargo/bin")));
        // A Finder launch has none of these on its `PATH`, so a Homebrew install
        // has to be found by name.
        assert!(dirs.contains(&PathBuf::from("/opt/homebrew/bin")));
        assert!(dirs.contains(&PathBuf::from("/usr/local/bin")));
    }

    #[test]
    fn an_empty_path_entry_is_not_a_directory() {
        let path_var = std::env::join_paths(["", "/usr/bin", ""]).unwrap();
        let dirs = search_paths(
            path_var.to_str().unwrap(),
            Some(Path::new("/home/dev")),
            "linux",
        );
        // An empty entry means the current directory and is dropped, while the
        // directory that was named beside it is kept.
        assert!(dirs.contains(&PathBuf::from("/usr/bin")), "{dirs:?}");
        for dir in dirs {
            assert!(!dir.as_os_str().is_empty(), "{dir:?}");
        }
    }

    #[test]
    fn the_first_directory_holding_the_binary_wins() {
        let dirs = vec![
            PathBuf::from("/nowhere"),
            PathBuf::from("/home/dev/.local/bin"),
        ];
        assert_eq!(
            locate(&dirs, "oxide", &exists),
            Some(PathBuf::from("/home/dev/.local/bin/oxide"))
        );
        assert_eq!(locate(&dirs, "oxide", &|_| false), None);
    }

    #[test]
    fn a_check_is_read_as_the_cli_wrote_it() {
        let check = parse_check(
            r#"{
              "current": "0.31.0",
              "latest": "0.33.0",
              "tag": "v0.33.0",
              "pinned": false,
              "updateAvailable": true,
              "installation": "prebuilt binary",
              "installable": true,
              "path": "/home/dev/.local/bin/oxide",
              "advice": "Update available; run `oxide update` to install v0.33.0.",
              "releaseUrl": "https://github.com/jaysonwu991/oxide/releases/tag/v0.33.0"
            }"#,
        )
        .expect("the CLI's own shape parses");
        assert_eq!(check.current, "0.31.0");
        assert_eq!(check.tag, "v0.33.0");
        assert!(check.update_available && check.installable);
        assert_eq!(check.installation, "prebuilt binary");
        assert_eq!(check.path, "/home/dev/.local/bin/oxide");
        assert!(check.advice.is_some());

        // Up to date, with nothing to install and no sentence to repeat.
        let fresh = parse_check(
            r#"{"current":"0.33.0","latest":"0.33.0","tag":"v0.33.0","pinned":false,
                "updateAvailable":false,"installation":"prebuilt binary","installable":true,
                "path":"/home/dev/.local/bin/oxide","advice":null,
                "releaseUrl":"https://github.com/jaysonwu991/oxide/releases/tag/v0.33.0"}"#,
        )
        .expect("a null advice is not a failure");
        assert!(!fresh.update_available);
        assert_eq!(fresh.advice, None);
    }

    #[test]
    fn a_check_that_is_not_the_clis_json_is_reported_rather_than_guessed() {
        let error = parse_check("Error: requesting the manifest").expect_err("not JSON");
        assert!(
            error.starts_with("could not read the update check:"),
            "{error}"
        );
    }

    #[test]
    fn a_cli_too_old_to_know_json_is_not_read_as_a_failed_check() {
        // Measured against the released 0.33.0 binary, which predates the flag.
        let old = CliOutput {
            code: Some(2),
            stdout: String::new(),
            stderr: "error: unexpected argument '--json' found\n\nUsage: oxide update --check\n\n\
                    For more information, try '--help'.\n"
                .into(),
        };
        assert!(!old.succeeded());
        assert!(rejects_json(&old));

        // A check that could not reach GitHub says something else, and so does a
        // message that merely mentions the flag.
        let offline = CliOutput {
            code: Some(1),
            stdout: String::new(),
            stderr:
                "Error: requesting the manifest\n\nCaused by: Operation timed out (os error 60)"
                    .into(),
        };
        assert!(!rejects_json(&offline));
        assert!(!rejects_json(&CliOutput {
            code: Some(2),
            stderr: "error: unexpected argument '--check' found".into(),
            ..old
        }));
    }

    #[test]
    fn a_failure_reads_as_what_the_cli_said() {
        let said = CliOutput {
            code: Some(1),
            stdout: String::new(),
            stderr: "Error: requesting the manifest\n\nCaused by: connection reset\n".into(),
        };
        assert!(!said.succeeded());
        assert_eq!(
            failure(&said, "the update check"),
            "Error: requesting the manifest\n\nCaused by: connection reset"
        );

        // A call that printed nothing still says how it ended.
        let silent = CliOutput {
            code: Some(2),
            stdout: String::new(),
            stderr: String::new(),
        };
        assert_eq!(
            failure(&silent, "the update"),
            "the update exited with code 2"
        );
        assert_eq!(
            failure(
                &CliOutput {
                    code: None,
                    ..silent.clone()
                },
                "the update"
            ),
            "the update was killed"
        );
    }

    #[test]
    fn an_install_reports_both_streams() {
        let output = CliOutput {
            code: Some(0),
            stdout: "Oxide update\nDownloaded oxide-darwin-arm64.tar.gz\n".into(),
            stderr: "warning: no checksum published for this release".into(),
        };
        assert!(output.succeeded());
        assert_eq!(
            output.text(),
            "Oxide update\nDownloaded oxide-darwin-arm64.tar.gz\n\
             warning: no checksum published for this release"
        );
    }
}
