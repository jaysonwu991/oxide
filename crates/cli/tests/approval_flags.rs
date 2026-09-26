//! The approval flags need a mode that can ask a question and carry the answer
//! back: the interactive TUI, or `--mode rpc`. Print mode has no such channel,
//! so it refuses them rather than running the tool they were meant to gate.

use std::path::PathBuf;
use std::process::{Command, Output, Stdio};

fn oxide(home: &PathBuf, args: &[&str]) -> Output {
    // A private config directory keeps the test away from the developer's own
    // credentials and settings, whatever platform this runs on.
    Command::new(env!("CARGO_BIN_EXE_oxide"))
        .args(args)
        .env("HOME", home)
        .env("XDG_CONFIG_HOME", home)
        .env("APPDATA", home)
        .env("LOCALAPPDATA", home)
        .stdin(Stdio::null())
        .output()
        .expect("running oxide")
}

#[test]
fn a_mode_without_an_answer_channel_refuses_the_approval_flags() {
    let home = std::env::temp_dir().join(format!("oxide_approval_flags_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&home);
    std::fs::create_dir_all(&home).unwrap();

    let cases: [&[&str]; 3] = [
        &["--ask-approvals", "-p", "hi"],
        &["--no-ask-approvals", "-p", "hi"],
        &["--ask-approvals", "--mode", "json", "hi"],
    ];
    for args in cases {
        let out = oxide(&home, args);
        assert!(!out.status.success(), "{args:?} should fail");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            stderr.contains("need the interactive TUI or --mode rpc"),
            "{args:?} reported {stderr}"
        );
    }

    // The TUI can prompt and answer, so it passes the check and only fails
    // later, for want of a terminal to draw in.
    let out = oxide(&home, &["--ask-approvals"]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !stderr.contains("need the interactive TUI"),
        "the TUI accepts --ask-approvals: {stderr}"
    );

    std::fs::remove_dir_all(&home).ok();
}
