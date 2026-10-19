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

#[test]
fn handoff_preserves_a_chat_in_the_shared_remote_session_store() {
    let root = std::env::temp_dir().join(format!("oxide_cli_handoff_{}", std::process::id()));
    let project = root.join("project");
    let home = root.join("home");
    std::fs::create_dir_all(&project).unwrap();
    std::fs::create_dir_all(&home).unwrap();
    let git = |args: &[&str]| {
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
    };
    git(&["init", "--quiet"]);
    git(&["config", "user.name", "Oxide"]);
    git(&["config", "user.email", "oxide@example.com"]);
    git(&["config", "commit.gpgsign", "false"]);
    git(&["commit", "--allow-empty", "-qm", "first"]);
    let remote = "https://example.com/oxide-handoff.git";
    git(&["remote", "add", "origin", remote]);
    let hash = remote.bytes().fold(0xcbf2_9ce4_8422_2325u64, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01b3)
    });
    let config = if cfg!(target_os = "macos") {
        home.join("Library/Application Support")
    } else {
        home.clone()
    };
    let sessions = config.join("Oxide/sessions").join(format!("{hash:016x}"));
    std::fs::create_dir_all(&sessions).unwrap();
    let log = sessions.join("handoff-chat.jsonl");
    let header = serde_json::json!({"type":"session","version":3,"id":"handoff-chat","timestamp":"2026-10-19T00:00:00.000Z","cwd":project.canonicalize().unwrap()});
    let message = serde_json::json!({"type":"message","id":"msg1","parentId":null,"timestamp":"2026-10-19T00:00:00.000Z","message":{"role":"user","content":"keep my history"}});
    std::fs::write(&log, format!("{header}\n{message}\n")).unwrap();
    std::fs::write(project.join("new.txt"), "my changes\n").unwrap();
    let project_text = project.to_str().unwrap();
    let moved = oxide(
        &home,
        &project,
        &[
            "worktree",
            "handoff",
            "handoff-chat",
            "--source",
            project_text,
            "--to-worktree",
        ],
    );
    let checkout = moved["cwd"].as_str().unwrap();
    assert!(Path::new(checkout).join("new.txt").exists());
    assert!(!project.join("new.txt").exists());
    let stored = std::fs::read_to_string(&log).unwrap();
    assert!(stored.contains(checkout));
    assert!(stored.contains("keep my history"));
    oxide(
        &home,
        &project,
        &["worktree", "handoff", "handoff-chat", "--source", checkout],
    );
    assert!(project.join("new.txt").exists());
    let again = oxide(
        &home,
        &project,
        &[
            "worktree",
            "handoff",
            "handoff-chat",
            "--source",
            project_text,
            "--to-worktree",
        ],
    );
    assert_eq!(again["cwd"], moved["cwd"]);
    assert_eq!(
        std::fs::read_dir(sessions)
            .unwrap()
            .filter(|entry| entry
                .as_ref()
                .unwrap()
                .path()
                .extension()
                .is_some_and(|ext| ext == "jsonl"))
            .count(),
        1
    );
    std::fs::remove_dir_all(root).unwrap();
}
