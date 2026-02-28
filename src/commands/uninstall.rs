use std::fs;
use std::io::{BufRead, Write};
use std::path::Path;

use crate::commands::install::HOOK_EVENTS;

/// Remove all hooks from settings and optionally delete data files.
#[cfg(not(tarpaulin_include))]
pub fn run() {
    if let Err(e) = try_run() {
        eprintln!("claude-track uninstall: {e}");
        std::process::exit(1);
    }
}

fn try_run() -> Result<(), Box<dyn std::error::Error>> {
    let claude_dir = dirs::home_dir()
        .ok_or("could not determine home directory")?
        .join(".claude");

    let settings_path = claude_dir.join("settings.json");
    let db_path = claude_dir.join("claude-track.db");
    let log_path = claude_dir.join("tool-usage.jsonl");

    let binary_path = std::env::current_exe()?
        .to_str()
        .ok_or("binary path is not valid UTF-8")?
        .to_string();
    let command = format!("{binary_path} hook");

    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let output = uninstall_from(
        &settings_path,
        &db_path,
        &log_path,
        &command,
        &mut stdin.lock(),
        &mut stdout.lock(),
    )?;
    print!("{output}");

    Ok(())
}

/// Run the uninstall logic against the given paths.
pub fn uninstall_from(
    settings_path: &Path,
    db_path: &Path,
    log_path: &Path,
    command: &str,
    input: &mut dyn BufRead,
    prompt_out: &mut dyn Write,
) -> Result<String, Box<dyn std::error::Error>> {
    let mut output = String::new();

    // Remove hooks from settings.json
    if settings_path.exists() {
        let contents = fs::read_to_string(settings_path)?;
        let mut settings: serde_json::Value = serde_json::from_str(&contents)?;

        let removed = unpatch_settings(&mut settings, command);
        if removed > 0 {
            let formatted = serde_json::to_string_pretty(&settings)?;
            fs::write(settings_path, formatted)?;
            output.push_str(&format!(
                "Removed {removed} hook(s) from {}\n",
                settings_path.display()
            ));
        } else {
            output.push_str("No matching hooks found in settings.\n");
        }
    } else {
        output.push_str("No settings.json found.\n");
    }

    // Ask about database
    if db_path.exists() {
        let size = fs::metadata(db_path).map(|m| m.len()).unwrap_or(0);
        write!(
            prompt_out,
            "Delete tracking database? ({} at {}) [y/N] ",
            crate::commands::stats::human_size(size),
            db_path.display()
        )?;
        prompt_out.flush()?;

        let mut answer = String::new();
        input.read_line(&mut answer)?;

        if answer.trim().eq_ignore_ascii_case("y") {
            fs::remove_file(db_path)?;
            output.push_str("Database deleted.\n");
        } else {
            output.push_str(&format!("Database kept at {}\n", db_path.display()));
        }
    }

    // Ask about legacy log
    if log_path.exists() {
        write!(
            prompt_out,
            "Delete legacy JSONL log? ({}) [y/N] ",
            log_path.display()
        )?;
        prompt_out.flush()?;

        let mut answer = String::new();
        input.read_line(&mut answer)?;

        if answer.trim().eq_ignore_ascii_case("y") {
            fs::remove_file(log_path)?;
            output.push_str("Legacy log deleted.\n");
        } else {
            output.push_str(&format!("Legacy log kept at {}\n", log_path.display()));
        }
    }

    output.push_str("Uninstalled successfully.\n");

    Ok(output)
}

/// Remove hook entries for all 6 events matching `command`.
/// Cleans up empty arrays and empty hooks objects.
/// Returns the number of events from which hooks were removed.
pub fn unpatch_settings(settings: &mut serde_json::Value, command: &str) -> usize {
    let mut removed = 0;

    for event in HOOK_EVENTS {
        if remove_hook_from_event(settings, event, command) {
            removed += 1;
        }
    }

    // Also check for legacy "log" command hooks
    let legacy_command = command.replace(" hook", " log");
    if legacy_command != command {
        for event in &["PostToolUse"] {
            if remove_hook_from_event(settings, event, &legacy_command) {
                removed += 1;
            }
        }
    }

    // Clean up empty arrays and empty hooks object
    cleanup_empty_hooks(settings);

    removed
}

/// Remove entries matching `command` from a single event. Returns true if any were removed.
fn remove_hook_from_event(
    settings: &mut serde_json::Value,
    event: &str,
    command: &str,
) -> bool {
    if let Some(event_hooks) = settings
        .get_mut("hooks")
        .and_then(|h| h.get_mut(event))
        .and_then(|p| p.as_array_mut())
    {
        let before_len = event_hooks.len();
        event_hooks.retain(|entry| {
            let matches = entry
                .get("hooks")
                .and_then(|h| h.as_array())
                .map(|hooks| {
                    hooks
                        .iter()
                        .any(|hook| hook.get("command").and_then(|c| c.as_str()) == Some(command))
                })
                .unwrap_or(false);
            !matches
        });
        event_hooks.len() != before_len
    } else {
        false
    }
}

/// Clean up empty event arrays and the hooks object itself.
fn cleanup_empty_hooks(settings: &mut serde_json::Value) {
    if let Some(hooks) = settings.get_mut("hooks").and_then(|h| h.as_object_mut()) {
        let empty_events: Vec<String> = hooks
            .iter()
            .filter(|(_, v)| v.as_array().map(|a| a.is_empty()).unwrap_or(false))
            .map(|(k, _)| k.clone())
            .collect();
        for key in empty_events {
            hooks.remove(&key);
        }
        if hooks.is_empty() {
            if let Some(obj) = settings.as_object_mut() {
                obj.remove("hooks");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    use tempfile::TempDir;

    #[test]
    fn unpatch_removes_all_hooks() {
        let mut settings = serde_json::json!({});
        crate::commands::install::patch_settings(&mut settings, "claude-track hook");
        assert_eq!(settings["hooks"].as_object().unwrap().len(), 6);

        let removed = unpatch_settings(&mut settings, "claude-track hook");
        assert_eq!(removed, 6);
        assert!(settings.get("hooks").is_none());
    }

    #[test]
    fn unpatch_leaves_other_hooks() {
        let mut settings = serde_json::json!({
            "hooks": {
                "PostToolUse": [
                    {
                        "matcher": ".*",
                        "hooks": [{"type": "command", "command": "claude-track hook"}]
                    },
                    {
                        "matcher": ".*",
                        "hooks": [{"type": "command", "command": "other-tool"}]
                    }
                ]
            }
        });

        let removed = unpatch_settings(&mut settings, "claude-track hook");
        assert_eq!(removed, 1);

        let hooks = settings["hooks"]["PostToolUse"].as_array().unwrap();
        assert_eq!(hooks.len(), 1);
        assert_eq!(hooks[0]["hooks"][0]["command"], "other-tool");
    }

    #[test]
    fn unpatch_no_match_returns_zero() {
        let mut settings = serde_json::json!({
            "hooks": {
                "PostToolUse": [
                    {
                        "matcher": ".*",
                        "hooks": [{"type": "command", "command": "other-tool"}]
                    }
                ]
            }
        });

        let removed = unpatch_settings(&mut settings, "claude-track hook");
        assert_eq!(removed, 0);
    }

    #[test]
    fn unpatch_no_hooks_key() {
        let mut settings = serde_json::json!({"other": "value"});
        let removed = unpatch_settings(&mut settings, "cmd hook");
        assert_eq!(removed, 0);
        assert_eq!(settings["other"], "value");
    }

    #[test]
    fn unpatch_removes_legacy_log_command() {
        let mut settings = serde_json::json!({
            "hooks": {
                "PostToolUse": [{
                    "matcher": ".*",
                    "hooks": [{"type": "command", "command": "claude-track log"}]
                }]
            }
        });

        let removed = unpatch_settings(&mut settings, "claude-track hook");
        assert_eq!(removed, 1);
        assert!(settings.get("hooks").is_none());
    }

    #[test]
    fn unpatch_cleans_empty_arrays_keeps_siblings() {
        let mut settings = serde_json::json!({
            "hooks": {
                "PostToolUse": [{
                    "matcher": ".*",
                    "hooks": [{"type": "command", "command": "claude-track hook"}]
                }],
                "SomeOtherHook": [{"matcher": ".*", "hooks": []}]
            }
        });

        unpatch_settings(&mut settings, "claude-track hook");
        assert!(settings["hooks"]["PostToolUse"].is_null());
        assert!(settings["hooks"]["SomeOtherHook"].is_array());
    }

    #[test]
    fn unpatch_preserves_top_level_keys() {
        let mut settings = serde_json::json!({
            "other_key": 42,
            "hooks": {
                "PostToolUse": [{
                    "matcher": ".*",
                    "hooks": [{"type": "command", "command": "claude-track hook"}]
                }]
            }
        });

        unpatch_settings(&mut settings, "claude-track hook");
        assert_eq!(settings["other_key"], 42);
    }

    #[test]
    fn cleanup_empty_hooks_removes_empty_object() {
        let mut settings = serde_json::json!({
            "hooks": {}
        });
        cleanup_empty_hooks(&mut settings);
        assert!(settings.get("hooks").is_none());
    }

    #[test]
    fn cleanup_empty_hooks_removes_empty_arrays() {
        let mut settings = serde_json::json!({
            "hooks": {
                "PostToolUse": [],
                "PreToolUse": [{"matcher": ".*"}]
            }
        });
        cleanup_empty_hooks(&mut settings);
        assert!(settings["hooks"]["PostToolUse"].is_null());
        assert!(settings["hooks"]["PreToolUse"].is_array());
    }

    #[test]
    fn uninstall_from_removes_hooks_keeps_data() {
        let dir = TempDir::new().unwrap();
        let settings_path = dir.path().join("settings.json");
        let db_path = dir.path().join("claude-track.db");
        let log_path = dir.path().join("tool-usage.jsonl");

        let mut settings = serde_json::json!({});
        crate::commands::install::patch_settings(&mut settings, "cmd hook");
        fs::write(&settings_path, serde_json::to_string(&settings).unwrap()).unwrap();
        fs::write(&db_path, "test db").unwrap();
        fs::write(&log_path, "{}\n").unwrap();

        let mut input = Cursor::new(b"n\nn\n");
        let mut prompt = Vec::new();
        let output =
            uninstall_from(&settings_path, &db_path, &log_path, "cmd hook", &mut input, &mut prompt)
                .unwrap();

        assert!(output.contains("Removed 6 hook(s)"));
        assert!(output.contains("Database kept at"));
        assert!(output.contains("Legacy log kept at"));
        assert!(output.contains("Uninstalled successfully."));
        assert!(db_path.exists());
        assert!(log_path.exists());

        let prompt_str = String::from_utf8(prompt).unwrap();
        assert!(prompt_str.contains("Delete tracking database?"));
        assert!(prompt_str.contains("Delete legacy JSONL log?"));
    }

    #[test]
    fn uninstall_from_deletes_data() {
        let dir = TempDir::new().unwrap();
        let settings_path = dir.path().join("settings.json");
        let db_path = dir.path().join("claude-track.db");
        let log_path = dir.path().join("tool-usage.jsonl");

        fs::write(&settings_path, "{}").unwrap();
        fs::write(&db_path, "test db").unwrap();
        fs::write(&log_path, "{}\n").unwrap();

        let mut input = Cursor::new(b"y\ny\n");
        let mut prompt = Vec::new();
        let output =
            uninstall_from(&settings_path, &db_path, &log_path, "cmd hook", &mut input, &mut prompt)
                .unwrap();

        assert!(output.contains("Database deleted."));
        assert!(output.contains("Legacy log deleted."));
        assert!(!db_path.exists());
        assert!(!log_path.exists());
    }

    #[test]
    fn uninstall_from_no_settings_file() {
        let dir = TempDir::new().unwrap();
        let settings_path = dir.path().join("settings.json");
        let db_path = dir.path().join("claude-track.db");
        let log_path = dir.path().join("tool-usage.jsonl");

        let mut input = Cursor::new(b"");
        let mut prompt = Vec::new();
        let output =
            uninstall_from(&settings_path, &db_path, &log_path, "cmd hook", &mut input, &mut prompt)
                .unwrap();

        assert!(output.contains("No settings.json found."));
        assert!(output.contains("Uninstalled successfully."));
    }

    #[test]
    fn uninstall_from_no_matching_hook() {
        let dir = TempDir::new().unwrap();
        let settings_path = dir.path().join("settings.json");
        let db_path = dir.path().join("claude-track.db");
        let log_path = dir.path().join("tool-usage.jsonl");

        let settings = serde_json::json!({
            "hooks": {
                "PostToolUse": [{
                    "matcher": ".*",
                    "hooks": [{"type": "command", "command": "other-tool"}]
                }]
            }
        });
        fs::write(&settings_path, serde_json::to_string(&settings).unwrap()).unwrap();

        let mut input = Cursor::new(b"");
        let mut prompt = Vec::new();
        let output =
            uninstall_from(&settings_path, &db_path, &log_path, "cmd hook", &mut input, &mut prompt)
                .unwrap();

        assert!(output.contains("No matching hooks found"));
    }

    #[test]
    fn uninstall_from_no_data_files() {
        let dir = TempDir::new().unwrap();
        let settings_path = dir.path().join("settings.json");
        let db_path = dir.path().join("claude-track.db");
        let log_path = dir.path().join("tool-usage.jsonl");

        fs::write(&settings_path, "{}").unwrap();

        let mut input = Cursor::new(b"");
        let mut prompt = Vec::new();
        let output =
            uninstall_from(&settings_path, &db_path, &log_path, "cmd hook", &mut input, &mut prompt)
                .unwrap();

        // No prompts about data files
        let prompt_str = String::from_utf8(prompt).unwrap();
        assert!(prompt_str.is_empty());
        assert!(output.contains("Uninstalled successfully."));
    }

    #[test]
    fn uninstall_from_only_db_no_log() {
        let dir = TempDir::new().unwrap();
        let settings_path = dir.path().join("settings.json");
        let db_path = dir.path().join("claude-track.db");
        let log_path = dir.path().join("tool-usage.jsonl");

        fs::write(&settings_path, "{}").unwrap();
        fs::write(&db_path, "test db").unwrap();

        let mut input = Cursor::new(b"n\n");
        let mut prompt = Vec::new();
        let output =
            uninstall_from(&settings_path, &db_path, &log_path, "cmd hook", &mut input, &mut prompt)
                .unwrap();

        assert!(output.contains("Database kept at"));
        assert!(!output.contains("Legacy log"));
    }
}
