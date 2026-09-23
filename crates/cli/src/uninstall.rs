use anyhow::{Context, Result};
use std::fs;
use std::io::{self, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, Clone, Copy)]
pub struct Options {
    pub keep_config: bool,
    pub keep_data: bool,
    pub dry_run: bool,
    pub force: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InstallMethod {
    Cargo,
    Homebrew,
    Prebuilt,
    Unknown,
}

impl InstallMethod {
    fn label(self) -> &'static str {
        match self {
            Self::Cargo => "cargo",
            Self::Homebrew => "homebrew",
            Self::Prebuilt => "prebuilt binary",
            Self::Unknown => "unknown",
        }
    }
}

#[derive(Debug)]
struct RemovalGroup {
    label: &'static str,
    paths: Vec<PathBuf>,
    keep: bool,
}

pub fn run(options: Options) -> Result<()> {
    let config_root =
        oxide_core::config::config_dir().context("resolving platform config directory")?;
    let home_config = dirs::home_dir().map(|home| home.join(".oxide"));
    let executable = std::env::current_exe().context("resolving oxide executable")?;
    run_with_paths(options, config_root, home_config, executable)
}

fn run_with_paths(
    options: Options,
    config_root: PathBuf,
    home_config: Option<PathBuf>,
    executable: PathBuf,
) -> Result<()> {
    let method = detect_install_method(&executable);
    let groups = collect_groups(&config_root, home_config.as_deref(), options)?;

    println!("Uninstall Oxide");
    println!("Installation method: {}", method.label());
    println!();
    println!("The following will be removed:");
    for group in &groups {
        if group.paths.is_empty() {
            continue;
        }
        let size = group.paths.iter().map(|path| path_size(path)).sum::<u64>();
        let marker = if group.keep { '○' } else { '✓' };
        let suffix = if group.keep { " (keeping)" } else { "" };
        println!(
            " {marker} {}: {} ({}){suffix}",
            group.label,
            summarize_paths(&group.paths),
            format_size(size),
        );
    }
    match method {
        InstallMethod::Cargo => println!(" ✓ Package: cargo uninstall oxide"),
        InstallMethod::Homebrew => println!(" ✓ Package: brew uninstall oxide"),
        InstallMethod::Prebuilt => println!(" ✓ Binary: {}", executable.display()),
        InstallMethod::Unknown => {}
    }

    if options.dry_run {
        println!();
        println!("Dry run - no changes made");
        println!("Done");
        return Ok(());
    }

    if !options.force && !confirm()? {
        println!("Cancelled");
        return Ok(());
    }

    let mut errors = Vec::new();
    for group in &groups {
        if group.keep {
            println!("Skipping {}", group.label);
            continue;
        }
        for path in &group.paths {
            if let Err(error) = remove_path(path) {
                errors.push(format!("{}: {error:#}", path.display()));
            }
        }
        if !group.paths.is_empty() {
            println!("Removed {}", group.label);
        }
    }
    let _ = fs::remove_dir(&config_root);

    match method {
        InstallMethod::Cargo => run_package_uninstall("cargo", &["uninstall", "oxide"]),
        InstallMethod::Homebrew => run_package_uninstall("brew", &["uninstall", "oxide"]),
        InstallMethod::Prebuilt => print_binary_removal(&executable),
        InstallMethod::Unknown => {
            println!();
            println!(
                "Could not detect how Oxide was installed; remove {} manually if needed.",
                executable.display()
            );
        }
    }

    if !errors.is_empty() {
        println!();
        eprintln!("Some operations failed:");
        for error in errors {
            eprintln!(" - {error}");
        }
    }

    println!();
    println!("Thank you for using Oxide!");
    println!("Done");
    Ok(())
}

fn collect_groups(
    config_root: &Path,
    home_config: Option<&Path>,
    options: Options,
) -> Result<Vec<RemovalGroup>> {
    let mut config = Vec::new();
    let mut data = Vec::new();
    let mut cache = Vec::new();
    let mut state = Vec::new();

    if config_root.is_dir() {
        for entry in fs::read_dir(config_root)
            .with_context(|| format!("reading {}", config_root.display()))?
        {
            let path = entry
                .with_context(|| format!("reading {}", config_root.display()))?
                .path();
            match path.file_name().and_then(|name| name.to_str()) {
                Some("sessions" | "snapshots" | "memory") => data.push(path),
                Some("model-cache.json" | "truncated") => cache.push(path),
                Some("trust.json") => state.push(path),
                _ => config.push(path),
            }
        }
    }
    if let Some(path) = home_config.filter(|path| path.exists()) {
        config.push(path.to_path_buf());
    }

    Ok(vec![
        RemovalGroup {
            label: "Data",
            paths: data,
            keep: options.keep_data,
        },
        RemovalGroup {
            label: "Cache",
            paths: cache,
            keep: false,
        },
        RemovalGroup {
            label: "Config",
            paths: config,
            keep: options.keep_config,
        },
        RemovalGroup {
            label: "State",
            paths: state,
            keep: false,
        },
    ])
}

fn confirm() -> Result<bool> {
    if !io::stdin().is_terminal() {
        anyhow::bail!("confirmation requires a terminal; rerun with --force")
    }
    print!("Are you sure you want to uninstall? [y/N] ");
    io::stdout()
        .flush()
        .context("flushing confirmation prompt")?;
    let mut answer = String::new();
    io::stdin()
        .read_line(&mut answer)
        .context("reading confirmation")?;
    Ok(matches!(
        answer.trim().to_ascii_lowercase().as_str(),
        "y" | "yes"
    ))
}

fn detect_install_method(executable: &Path) -> InstallMethod {
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

fn run_package_uninstall(program: &str, args: &[&str]) {
    println!();
    println!("Running {program} {}...", args.join(" "));
    match Command::new(program).args(args).status() {
        Ok(status) if status.success() => println!("Package removed"),
        Ok(status) => eprintln!(
            "Package manager uninstall failed with {status}; run `{program} {}` manually.",
            args.join(" ")
        ),
        Err(error) => eprintln!(
            "Could not run package manager ({error}); run `{program} {}` manually.",
            args.join(" ")
        ),
    }
}

fn print_binary_removal(executable: &Path) {
    println!();
    println!("To finish removing the binary after this command exits, run:");
    #[cfg(windows)]
    println!(" Remove-Item \"{}\"", executable.display());
    #[cfg(not(windows))]
    println!(" rm \"{}\"", executable.display());
}

fn remove_path(path: &Path) -> Result<()> {
    if path.is_dir() {
        fs::remove_dir_all(path).with_context(|| format!("removing directory {}", path.display()))
    } else {
        fs::remove_file(path).with_context(|| format!("removing file {}", path.display()))
    }
}

fn path_size(path: &Path) -> u64 {
    let Ok(metadata) = fs::symlink_metadata(path) else {
        return 0;
    };
    if metadata.is_file() || metadata.file_type().is_symlink() {
        return metadata.len();
    }
    let Ok(entries) = fs::read_dir(path) else {
        return 0;
    };
    entries
        .filter_map(std::result::Result::ok)
        .map(|entry| path_size(&entry.path()))
        .sum()
}

fn summarize_paths(paths: &[PathBuf]) -> String {
    match paths {
        [] => String::new(),
        [path] => path.display().to_string(),
        [first, rest @ ..] => format!("{} (+{} more)", first.display(), rest.len()),
    }
}

fn format_size(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = KB * 1024.0;
    const GB: f64 = MB * 1024.0;
    match bytes {
        0..=1023 => format!("{bytes} B"),
        1024..=1_048_575 => format!("{:.1} KB", bytes as f64 / KB),
        1_048_576..=1_073_741_823 => format!("{:.1} MB", bytes as f64 / MB),
        _ => format!("{:.1} GB", bytes as f64 / GB),
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
        let path = std::env::temp_dir().join(format!(
            "oxide-uninstall-{tag}-{}-{nonce}",
            std::process::id()
        ));
        fs::create_dir_all(&path).unwrap();
        path
    }

    fn options() -> Options {
        Options {
            keep_config: false,
            keep_data: false,
            dry_run: false,
            force: true,
        }
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
            detect_install_method(Path::new("/workspace/target/debug/oxide")),
            InstallMethod::Unknown
        );
    }

    #[test]
    fn classifies_and_preserves_requested_groups() {
        let root = temp_dir("groups");
        let platform = root.join("platform/oxide");
        let home = root.join("home/.oxide");
        fs::create_dir_all(platform.join("sessions")).unwrap();
        fs::create_dir_all(platform.join("truncated")).unwrap();
        fs::create_dir_all(&home).unwrap();
        fs::write(platform.join("config.json"), "{}").unwrap();
        fs::write(platform.join("trust.json"), "{}").unwrap();

        let mut keep = options();
        keep.keep_config = true;
        keep.keep_data = true;
        let groups = collect_groups(&platform, Some(&home), keep).unwrap();

        assert!(
            groups
                .iter()
                .find(|group| group.label == "Data")
                .unwrap()
                .keep
        );
        assert!(
            groups
                .iter()
                .find(|group| group.label == "Config")
                .unwrap()
                .keep
        );
        assert!(
            !groups
                .iter()
                .find(|group| group.label == "Cache")
                .unwrap()
                .keep
        );
        assert!(
            !groups
                .iter()
                .find(|group| group.label == "State")
                .unwrap()
                .keep
        );

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn size_format_matches_cli_units() {
        assert_eq!(format_size(12), "12 B");
        assert_eq!(format_size(1536), "1.5 KB");
        assert_eq!(format_size(2 * 1024 * 1024), "2.0 MB");
    }

    #[test]
    fn dry_run_does_not_remove_anything() {
        let root = temp_dir("dry-run");
        let platform = root.join("platform/oxide");
        fs::create_dir_all(platform.join("sessions")).unwrap();
        fs::write(platform.join("config.json"), "{}").unwrap();

        let mut dry_run = options();
        dry_run.dry_run = true;
        run_with_paths(
            dry_run,
            platform.clone(),
            None,
            root.join("target/debug/oxide"),
        )
        .unwrap();

        assert!(platform.join("sessions").exists());
        assert!(platform.join("config.json").exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn forced_uninstall_honors_keep_flags() {
        let root = temp_dir("force");
        let platform = root.join("platform/oxide");
        fs::create_dir_all(platform.join("sessions")).unwrap();
        fs::create_dir_all(platform.join("truncated")).unwrap();
        fs::write(platform.join("config.json"), "{}").unwrap();
        fs::write(platform.join("trust.json"), "{}").unwrap();

        let mut keep = options();
        keep.keep_config = true;
        keep.keep_data = true;
        run_with_paths(
            keep,
            platform.clone(),
            None,
            root.join("target/debug/oxide"),
        )
        .unwrap();

        assert!(platform.join("sessions").exists());
        assert!(platform.join("config.json").exists());
        assert!(!platform.join("truncated").exists());
        assert!(!platform.join("trust.json").exists());
        fs::remove_dir_all(root).unwrap();
    }
}
