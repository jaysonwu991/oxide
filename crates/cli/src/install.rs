use std::path::{Path, PathBuf};

/// How the running binary was installed, which decides whether a command may
/// replace it (`oxide update`) or must hand the job to a package manager.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallMethod {
    Cargo,
    Homebrew,
    Prebuilt,
    Unknown,
}

impl InstallMethod {
    pub fn label(self) -> &'static str {
        match self {
            Self::Cargo => "cargo",
            Self::Homebrew => "homebrew",
            Self::Prebuilt => "prebuilt binary",
            Self::Unknown => "unknown",
        }
    }
}

/// The file an installer leaves beside the binary it unpacked. It names that
/// directory as a released install, which is how `oxide update` recognizes one
/// that is not in a location it knows by name: a custom `OXIDE_INSTALL_DIR`, or
/// a binary `--force` replaced somewhere else. `oxide update` writes it after
/// it replaces a binary, so the install is recognized from then on.
pub const MARKER_NAME: &str = ".oxide-install";

/// The marker beside `executable`, whether or not it exists.
pub fn marker(executable: &Path) -> PathBuf {
    executable.with_file_name(MARKER_NAME)
}

/// Whether the installer marked this directory as a released install.
pub fn is_released(executable: &Path) -> bool {
    marker(executable).is_file()
}

/// Records a released install beside `executable`, naming what put it there.
pub fn record(executable: &Path, version: &str, repo: &str) -> std::io::Result<()> {
    std::fs::write(
        marker(executable),
        format!("source {repo}\nversion {version}\n"),
    )
}

/// Recognizes the install locations the installers use: Cargo's bin directory,
/// a Homebrew Cellar, and the `~/.local/bin` (or `%LOCALAPPDATA%\Programs`) a
/// prebuilt archive is unpacked into — or any other directory an installer left
/// its marker in, so a custom `OXIDE_INSTALL_DIR` is still a released install.
/// Anything else — a `target/debug` build, a distribution package, a path
/// someone moved the binary to — is unknown.
pub fn detect_install_method(executable: &Path) -> InstallMethod {
    let text = executable.to_string_lossy().replace('\\', "/");
    if text.contains("/Cellar/oxide/") {
        return InstallMethod::Homebrew;
    }
    if text.ends_with("/.cargo/bin/oxide") || text.ends_with("/.cargo/bin/oxide.exe") {
        return InstallMethod::Cargo;
    }
    if text.ends_with("/.local/bin/oxide")
        || text
            .to_ascii_lowercase()
            .ends_with("/programs/oxide/oxide.exe")
    {
        return InstallMethod::Prebuilt;
    }
    if is_released(executable) {
        return InstallMethod::Prebuilt;
    }
    InstallMethod::Unknown
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_custom_install_directory_is_a_prebuilt_install() {
        let dir = std::env::temp_dir().join(format!(
            "oxide-install-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let executable = dir.join("oxide");
        assert_eq!(detect_install_method(&executable), InstallMethod::Unknown);

        record(&executable, "0.26.0", "jaysonwu991/oxide").unwrap();

        assert_eq!(detect_install_method(&executable), InstallMethod::Prebuilt);
        assert_eq!(
            std::fs::read_to_string(dir.join(MARKER_NAME)).unwrap(),
            "source jaysonwu991/oxide\nversion 0.26.0\n"
        );
        assert!(is_released(&executable));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn detects_supported_install_methods() {
        assert_eq!(
            detect_install_method(Path::new("/Users/me/.cargo/bin/oxide")),
            InstallMethod::Cargo
        );
        assert_eq!(
            detect_install_method(Path::new("/opt/homebrew/Cellar/oxide/1.0/bin/oxide")),
            InstallMethod::Homebrew
        );
        assert_eq!(
            detect_install_method(Path::new("/Users/me/.local/bin/oxide")),
            InstallMethod::Prebuilt
        );
        assert_eq!(
            detect_install_method(Path::new(
                "/Users/me/AppData/Local/Programs/Oxide/oxide.exe"
            )),
            InstallMethod::Prebuilt
        );
        assert_eq!(
            detect_install_method(Path::new("/workspace/target/debug/oxide")),
            InstallMethod::Unknown
        );
    }
}
