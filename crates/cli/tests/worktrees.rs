use serde_json::Value;
use std::path::Path;
use std::process::{Command, Stdio};

fn oxide(home: &Path, project: &Path, args: &[&str]) -> Value {
    let output = Command::new(env!("CARGO_BIN_EXE_oxide"))
        .args(args)
        .arg("--project")
        .arg(project)
        .arg("--json")
        .env("HOME", home)
        .env("XDG_CONFIG_HOME", home)
        .env("APPDATA", home)
        .env("LOCALAPPDATA", home)
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

#[test]
fn a_worktree_reads_and_updates_its_projects_trust() {
    let root = std::env::temp_dir().join(format!("oxide_cli_worktree_{}", std::process::id()));
    let project = root.join("project");
    let home = root.join("home");
    std::fs::create_dir_all(&project).unwrap();
    std::fs::create_dir_all(&home).unwrap();
    for args in [
        vec!["init", "--quiet"],
        vec![
            "-c",
            "user.name=Oxide",
            "-c",
            "user.email=oxide@example.com",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "--allow-empty",
            "--quiet",
            "-m",
            "first",
        ],
    ] {
        let output = Command::new("git")
            .current_dir(&project)
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let entry = oxide(&home, &project, &["worktree", "create", "topic"]);
    let checkout = Path::new(entry["path"].as_str().unwrap());
    oxide(&home, &project, &["trust", "set", "trusted"]);
    let inherited = oxide(&home, checkout, &["trust", "show"]);
    assert_eq!(inherited["trusted"], true);
    assert_eq!(inherited["project"], entry["project"]);
    oxide(&home, checkout, &["trust", "set", "untrusted"]);
    assert_eq!(oxide(&home, &project, &["trust", "show"])["trusted"], false);
    oxide(&home, &project, &["worktree", "remove", "topic"]);
    std::fs::remove_dir_all(root).unwrap();
}
