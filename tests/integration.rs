use std::process::Command;

fn binary_path() -> std::path::PathBuf {
    // cargo test builds to target/debug
    let mut path = std::env::current_exe().unwrap();
    // Go up from deps/integration-<hash> to target/debug
    path.pop();
    path.pop();
    path.join("claude-track")
}

#[test]
fn cli_no_args_shows_help() {
    let output = Command::new(binary_path())
        .output()
        .expect("failed to run binary");

    // clap exits with code 2 when no subcommand is given
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("Usage"));
}

#[test]
fn cli_help_flag() {
    let output = Command::new(binary_path())
        .arg("--help")
        .output()
        .expect("failed to run binary");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Claude Code usage analytics tracker"));
    assert!(stdout.contains("hook"));
    assert!(stdout.contains("stats"));
    assert!(stdout.contains("install"));
    assert!(stdout.contains("uninstall"));
    assert!(stdout.contains("migrate"));
    assert!(stdout.contains("query"));
}

#[test]
fn cli_hook_subcommand_with_valid_json() {
    let input = r#"{"hook_event_name":"PostToolUse","tool_name":"Read","session_id":"s1","cwd":"/tmp","tool_input":{"file_path":"/foo"}}"#;

    let output = Command::new(binary_path())
        .arg("hook")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            use std::io::Write;
            if let Some(ref mut stdin) = child.stdin {
                stdin.write_all(input.as_bytes()).ok();
            }
            child.wait_with_output()
        })
        .expect("failed to run binary");

    // Hook always exits 0
    assert!(output.status.success());
}

#[test]
fn cli_hook_subcommand_with_invalid_json() {
    let output = Command::new(binary_path())
        .arg("hook")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            use std::io::Write;
            if let Some(ref mut stdin) = child.stdin {
                stdin.write_all(b"not json").ok();
            }
            child.wait_with_output()
        })
        .expect("failed to run binary");

    // Still exits 0 — hook must not block
    assert!(output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("claude-track hook:"));
}

#[test]
fn cli_stats_subcommand_runs() {
    // Stats reads from ~/.claude/claude-track.db — it may or may not exist.
    // Either way it should exit 0.
    let output = Command::new(binary_path())
        .arg("stats")
        .output()
        .expect("failed to run binary");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    // Should show either "No tracking data yet" or the stats header
    assert!(
        stdout.contains("No tracking data yet") || stdout.contains("Claude Code Usage Stats")
    );
}

#[test]
fn cli_invalid_subcommand() {
    let output = Command::new(binary_path())
        .arg("nonexistent")
        .output()
        .expect("failed to run binary");

    assert!(!output.status.success());
}

#[test]
fn cli_install_subcommand_runs() {
    // Install will either add the hooks or say "already installed".
    // Either way it should exit 0.
    let output = Command::new(binary_path())
        .arg("install")
        .output()
        .expect("failed to run binary");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Installed successfully.") || stdout.contains("already installed")
    );
}

#[test]
fn cli_uninstall_subcommand_runs() {
    // Uninstall with empty stdin — will remove hooks (or say not found)
    // and skip data deletion prompts (EOF = no).
    let output = Command::new(binary_path())
        .arg("uninstall")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            // Close stdin immediately (EOF = answer "no" to delete prompts)
            drop(child.stdin.take());
            child.wait_with_output()
        })
        .expect("failed to run binary");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Uninstalled successfully."));
}

#[test]
fn cli_migrate_subcommand_runs() {
    let output = Command::new(binary_path())
        .arg("migrate")
        .output()
        .expect("failed to run binary");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    // Either migrates data or says no file found
    assert!(
        stdout.contains("Migrated") || stdout.contains("No JSONL file found") || stdout.contains("Nothing to migrate")
    );
}

#[test]
fn cli_query_subcommand_runs() {
    let output = Command::new(binary_path())
        .arg("query")
        .arg("SELECT 1 as test")
        .output()
        .expect("failed to run binary");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("test"));
    assert!(stdout.contains("1"));
}
