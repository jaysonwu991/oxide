use std::path::Path;

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

/// Recognizes the install locations the installers use: Cargo's bin directory,
/// a Homebrew Cellar, and the `~/.local/bin` (or `%LOCALAPPDATA%\Programs`) a
/// prebuilt archive is unpacked into. Anything else — a `target/debug` build, a
/// distribution package, a path someone moved the binary to — is unknown.
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
    InstallMethod::Unknown
}

#[cfg(test)]
mod tests {
    use super::*;

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
