use std::fs;
use std::io::{BufRead, Write};
use std::path::Path;

/// Remove the PostToolUse hook from settings and optionally delete the log.
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
    let log_path = claude_dir.join("tool-usage.jsonl");

    let binary_path = std::env::current_exe()?
        .to_str()
        .ok_or("binary path is not valid UTF-8")?
        .to_string();
    let command = format!("{binary_path} log");

    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let output = uninstall_from(
        &settings_path,
        &log_path,
        &command,
        &mut stdin.lock(),
        &mut stdout.lock(),
    )?;
    print!("{output}");

    Ok(())
}

/// Run the uninstall logic against the given paths.
/// Reads user confirmation from `input` and writes prompts to `prompt_out`.
/// Returns summary output for the caller to print.
pub fn uninstall_from(
    settings_path: &Path,
    log_path: &Path,
    command: &str,
    input: &mut dyn BufRead,
    prompt_out: &mut dyn Write,
) -> Result<String, Box<dyn std::error::Error>> {
    let mut output = String::new();

    // Remove hook from settings.json
    if settings_path.exists() {
        let contents = fs::read_to_string(settings_path)?;
        let mut settings: serde_json::Value = serde_json::from_str(&contents)?;

        if unpatch_settings(&mut settings, command) {
            let formatted = serde_json::to_string_pretty(&settings)?;
            fs::write(settings_path, formatted)?;
            output.push_str(&format!(
                "Hook removed from {}\n",
                settings_path.display()
            ));
        } else {
            output.push_str("No matching PostToolUse hook found in settings.\n");
        }
    } else {
        output.push_str("No settings.json found.\n");
    }

    // Ask about log data
    if log_path.exists() {
        let line_count = {
            let file = fs::File::open(log_path)?;
            std::io::BufReader::new(file).lines().count()
        };

        write!(
            prompt_out,
            "Delete usage log? ({line_count} entries in {}) [y/N] ",
            log_path.display()
        )?;
        prompt_out.flush()?;

        let mut answer = String::new();
        input.read_line(&mut answer)?;

        if answer.trim().eq_ignore_ascii_case("y") {
            fs::remove_file(log_path)?;
            output.push_str("Log deleted.\n");
        } else {
            output.push_str(&format!("Log kept at {}\n", log_path.display()));
        }
    }

    output.push_str("Uninstalled successfully.\n");

    Ok(output)
}

/// Remove the PostToolUse hook entry matching `command` from settings JSON.
/// Cleans up empty PostToolUse array and empty hooks object.
/// Returns `true` if a hook was removed, `false` otherwise.
pub fn unpatch_settings(settings: &mut serde_json::Value, command: &str) -> bool {
    let mut modified = false;

    if let Some(post_tool_use) = settings
        .get_mut("hooks")
        .and_then(|h| h.get_mut("PostToolUse"))
        .and_then(|p| p.as_array_mut())
    {
        let before_len = post_tool_use.len();
        post_tool_use.retain(|entry| {
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
        if post_tool_use.len() != before_len {
            modified = true;
        }
    }

    // Clean up empty PostToolUse array
    if let Some(hooks) = settings.get_mut("hooks").and_then(|h| h.as_object_mut()) {
        if hooks
            .get("PostToolUse")
            .and_then(|p| p.as_array())
            .map(|a| a.is_empty())
            .unwrap_or(false)
        {
            hooks.remove("PostToolUse");
        }
        if hooks.is_empty() {
            if let Some(obj) = settings.as_object_mut() {
                obj.remove("hooks");
            }
        }
    }

    modified
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    use tempfile::TempDir;

    #[test]
    fn unpatch_removes_matching_hook() {
        let mut settings = serde_json::json!({
            "hooks": {
                "PostToolUse": [
                    {
                        "matcher": ".*",
                        "hooks": [{"type": "command", "command": "/bin/claude-track log"}]
                    }
                ]
            }
        });

        let modified = unpatch_settings(&mut settings, "/bin/claude-track log");
        assert!(modified);
        assert!(settings.get("hooks").is_none());
    }

    #[test]
    fn unpatch_leaves_other_hooks() {
        let mut settings = serde_json::json!({
            "hooks": {
                "PostToolUse": [
                    {
                        "matcher": ".*",
                        "hooks": [{"type": "command", "command": "/bin/claude-track log"}]
                    },
                    {
                        "matcher": ".*",
                        "hooks": [{"type": "command", "command": "other-tool"}]
                    }
                ]
            }
        });

        let modified = unpatch_settings(&mut settings, "/bin/claude-track log");
        assert!(modified);

        let hooks = settings["hooks"]["PostToolUse"].as_array().unwrap();
        assert_eq!(hooks.len(), 1);
        assert_eq!(hooks[0]["hooks"][0]["command"], "other-tool");
    }

    #[test]
    fn unpatch_no_match_returns_false() {
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

        let modified = unpatch_settings(&mut settings, "/bin/claude-track log");
        assert!(!modified);

        let hooks = settings["hooks"]["PostToolUse"].as_array().unwrap();
        assert_eq!(hooks.len(), 1);
    }

    #[test]
    fn unpatch_no_hooks_key() {
        let mut settings = serde_json::json!({"other": "value"});
        let modified = unpatch_settings(&mut settings, "cmd log");
        assert!(!modified);
        assert_eq!(settings["other"], "value");
    }

    #[test]
    fn unpatch_no_post_tool_use_key() {
        let mut settings = serde_json::json!({
            "hooks": {
                "PreToolUse": []
            }
        });
        let modified = unpatch_settings(&mut settings, "cmd log");
        assert!(!modified);
        assert!(settings["hooks"]["PreToolUse"].is_array());
    }

    #[test]
    fn unpatch_cleans_empty_hooks_but_keeps_siblings() {
        let mut settings = serde_json::json!({
            "hooks": {
                "PostToolUse": [
                    {
                        "matcher": ".*",
                        "hooks": [{"type": "command", "command": "cmd log"}]
                    }
                ],
                "PreToolUse": [{"matcher": ".*", "hooks": []}]
            }
        });

        unpatch_settings(&mut settings, "cmd log");
        assert!(settings["hooks"]["PostToolUse"].is_null());
        assert!(settings["hooks"]["PreToolUse"].is_array());
    }

    #[test]
    fn unpatch_preserves_top_level_keys() {
        let mut settings = serde_json::json!({
            "other_key": 42,
            "hooks": {
                "PostToolUse": [
                    {
                        "matcher": ".*",
                        "hooks": [{"type": "command", "command": "cmd log"}]
                    }
                ]
            }
        });

        unpatch_settings(&mut settings, "cmd log");
        assert_eq!(settings["other_key"], 42);
        assert!(settings.get("hooks").is_none());
    }

    #[test]
    fn uninstall_from_removes_hook_and_keeps_log() {
        let dir = TempDir::new().unwrap();
        let settings_path = dir.path().join("settings.json");
        let log_path = dir.path().join("tool-usage.jsonl");

        let settings = serde_json::json!({
            "hooks": {
                "PostToolUse": [{
                    "matcher": ".*",
                    "hooks": [{"type": "command", "command": "cmd log"}]
                }]
            }
        });
        fs::write(&settings_path, serde_json::to_string(&settings).unwrap()).unwrap();
        fs::write(&log_path, "{}\n{}\n").unwrap();

        let mut input = Cursor::new(b"n\n");
        let mut prompt = Vec::new();
        let output =
            uninstall_from(&settings_path, &log_path, "cmd log", &mut input, &mut prompt).unwrap();

        assert!(output.contains("Hook removed from"));
        assert!(output.contains("Log kept at"));
        assert!(output.contains("Uninstalled successfully."));
        assert!(log_path.exists());

        // Verify prompt was written
        let prompt_str = String::from_utf8(prompt).unwrap();
        assert!(prompt_str.contains("Delete usage log?"));
        assert!(prompt_str.contains("2 entries"));
    }

    #[test]
    fn uninstall_from_removes_hook_and_deletes_log() {
        let dir = TempDir::new().unwrap();
        let settings_path = dir.path().join("settings.json");
        let log_path = dir.path().join("tool-usage.jsonl");

        let settings = serde_json::json!({
            "hooks": {
                "PostToolUse": [{
                    "matcher": ".*",
                    "hooks": [{"type": "command", "command": "cmd log"}]
                }]
            }
        });
        fs::write(&settings_path, serde_json::to_string(&settings).unwrap()).unwrap();
        fs::write(&log_path, "{}\n").unwrap();

        let mut input = Cursor::new(b"y\n");
        let mut prompt = Vec::new();
        let output =
            uninstall_from(&settings_path, &log_path, "cmd log", &mut input, &mut prompt).unwrap();

        assert!(output.contains("Log deleted."));
        assert!(!log_path.exists());
    }

    #[test]
    fn uninstall_from_no_settings_file() {
        let dir = TempDir::new().unwrap();
        let settings_path = dir.path().join("settings.json");
        let log_path = dir.path().join("tool-usage.jsonl");

        let mut input = Cursor::new(b"");
        let mut prompt = Vec::new();
        let output =
            uninstall_from(&settings_path, &log_path, "cmd log", &mut input, &mut prompt).unwrap();

        assert!(output.contains("No settings.json found."));
        assert!(output.contains("Uninstalled successfully."));
    }

    #[test]
    fn uninstall_from_no_matching_hook() {
        let dir = TempDir::new().unwrap();
        let settings_path = dir.path().join("settings.json");
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
            uninstall_from(&settings_path, &log_path, "cmd log", &mut input, &mut prompt).unwrap();

        assert!(output.contains("No matching PostToolUse hook found"));
    }

    #[test]
    fn uninstall_from_no_log_file() {
        let dir = TempDir::new().unwrap();
        let settings_path = dir.path().join("settings.json");
        let log_path = dir.path().join("tool-usage.jsonl");

        fs::write(&settings_path, "{}").unwrap();

        let mut input = Cursor::new(b"");
        let mut prompt = Vec::new();
        let output =
            uninstall_from(&settings_path, &log_path, "cmd log", &mut input, &mut prompt).unwrap();

        // No prompt about log deletion
        let prompt_str = String::from_utf8(prompt).unwrap();
        assert!(prompt_str.is_empty());
        assert!(output.contains("Uninstalled successfully."));
    }
}
