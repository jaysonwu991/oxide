use std::path::Path;
use std::process::Command;

fn help(launch: &Path, home: &Path, args: &[&str], language: Option<&str>) -> String {
    let mut command = Command::new(env!("CARGO_BIN_EXE_oxide"));
    command
        .current_dir(launch)
        .args(args)
        .env("HOME", home)
        .env("XDG_CONFIG_HOME", home)
        .env("APPDATA", home)
        .env("LOCALAPPDATA", home)
        .env_remove("OXIDE_LANG");
    if let Some(language) = language {
        command.env("OXIDE_LANG", language);
    }
    let output = command.output().expect("running help");
    assert!(
        output.status.success(),
        "{:?}: {}",
        args,
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

#[test]
fn help_uses_the_target_project_and_preserves_override_precedence() {
    let root = std::env::temp_dir().join(format!("oxide_help_language_{}", std::process::id()));
    let launch = root.join("launch");
    let target = root.join("chinese-project");
    let home = root.join("home");
    for dir in [&launch, &target, &home] {
        std::fs::create_dir_all(dir).unwrap();
    }
    for (dir, language) in [(&launch, "en-GB"), (&target, "zh-CN")] {
        std::fs::create_dir_all(dir.join(".oxide")).unwrap();
        std::fs::write(
            dir.join(".oxide/settings.json"),
            format!(r#"{{"language":"{language}"}}"#),
        )
        .unwrap();
    }
    let path = target.to_str().unwrap();
    for args in [
        vec!["-C".into(), path.into(), "--help".into()],
        vec!["--cwd".into(), path.into(), "--help".into()],
        vec![format!("--cwd={path}"), "--help".into()],
        vec!["--cwd=../chinese-project".into(), "--help".into()],
    ] {
        let args: Vec<_> = args.iter().map(String::as_str).collect();
        assert!(
            help(&launch, &home, &args, None).contains("用法："),
            "{args:?}"
        );
        assert!(help(&launch, &home, &args, Some("en-GB")).contains("Usage:"));
    }
    assert!(help(
        &launch,
        &home,
        &["-C", path, "--language", "en-GB", "--help"],
        None
    )
    .contains("Usage:"));
    assert!(help(
        &launch,
        &home,
        &["--language=zh-CN", "--help"],
        Some("en-GB")
    )
    .contains("用法："));
    for value in [
        format!("--system-prompt=--cwd={path}"),
        format!("--system-prompt=-C{path}"),
        "--system-prompt=--language=zh-CN".into(),
    ] {
        assert!(
            help(&launch, &home, &[&value, "--help"], None).contains("Usage:"),
            "{value}"
        );
    }
    std::fs::remove_dir_all(root).unwrap();
}
