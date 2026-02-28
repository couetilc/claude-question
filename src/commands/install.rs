use std::fs;
use std::path::Path;

/// Install the PostToolUse hook into ~/.claude/settings.json.
#[cfg(not(tarpaulin_include))]
pub fn run() {
    if let Err(e) = try_run() {
        eprintln!("claude-track install: {e}");
        std::process::exit(1);
    }
}

fn try_run() -> Result<(), Box<dyn std::error::Error>> {
    let binary_path = std::env::current_exe()?
        .to_str()
        .ok_or("binary path is not valid UTF-8")?
        .to_string();

    let settings_path = dirs::home_dir()
        .ok_or("could not determine home directory")?
        .join(".claude")
        .join("settings.json");

    let command = format!("{binary_path} log");
    let output = install_to(&settings_path, &command)?;
    print!("{output}");
    Ok(())
}

/// Install the hook into the given settings file. Returns user-facing output.
pub fn install_to(
    settings_path: &Path,
    command: &str,
) -> Result<String, Box<dyn std::error::Error>> {
    let mut settings: serde_json::Value = if settings_path.exists() {
        let contents = fs::read_to_string(settings_path)?;
        serde_json::from_str(&contents)?
    } else {
        if let Some(parent) = settings_path.parent() {
            fs::create_dir_all(parent)?;
        }
        serde_json::json!({})
    };

    if patch_settings(&mut settings, command) {
        write_settings(&settings, settings_path)?;

        Ok(format!(
            "Hook added to {}\n\
             Installed successfully.\n\
             \n\
             \x20 Tracking starts on your next Claude Code session.\n\
             \x20 View stats anytime:  claude-track stats\n",
            settings_path.display()
        ))
    } else {
        Ok("Hook is already installed.\n".to_string())
    }
}

/// Add the PostToolUse hook entry to settings JSON.
/// Returns `true` if the hook was added, `false` if already present.
pub fn patch_settings(settings: &mut serde_json::Value, command: &str) -> bool {
    // Check if hook is already registered
    if let Some(post_tool_use) = settings
        .get("hooks")
        .and_then(|h| h.get("PostToolUse"))
        .and_then(|p| p.as_array())
    {
        let already_installed = post_tool_use.iter().any(|entry| {
            entry
                .get("hooks")
                .and_then(|h| h.as_array())
                .map(|hooks| {
                    hooks
                        .iter()
                        .any(|hook| hook.get("command").and_then(|c| c.as_str()) == Some(command))
                })
                .unwrap_or(false)
        });
        if already_installed {
            return false;
        }
    }

    let hook_entry = serde_json::json!({
        "matcher": ".*",
        "hooks": [
            {
                "type": "command",
                "command": command,
            }
        ]
    });

    let hooks = settings
        .as_object_mut()
        .unwrap()
        .entry("hooks")
        .or_insert_with(|| serde_json::json!({}));
    let post_tool_use = hooks
        .as_object_mut()
        .unwrap()
        .entry("PostToolUse")
        .or_insert_with(|| serde_json::json!([]));
    post_tool_use.as_array_mut().unwrap().push(hook_entry);

    true
}

/// Write settings to the given path, creating parent directories if needed.
pub fn write_settings(
    settings: &serde_json::Value,
    settings_path: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    if let Some(parent) = settings_path.parent() {
        fs::create_dir_all(parent)?;
    }
    let formatted = serde_json::to_string_pretty(settings)?;
    fs::write(settings_path, formatted)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn patch_empty_settings() {
        let mut settings = serde_json::json!({});
        let added = patch_settings(&mut settings, "/usr/bin/claude-track log");
        assert!(added);

        let hooks = settings["hooks"]["PostToolUse"].as_array().unwrap();
        assert_eq!(hooks.len(), 1);
        assert_eq!(hooks[0]["matcher"], ".*");
        assert_eq!(hooks[0]["hooks"][0]["type"], "command");
        assert_eq!(hooks[0]["hooks"][0]["command"], "/usr/bin/claude-track log");
    }

    #[test]
    fn patch_existing_settings_with_other_hooks() {
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
        let added = patch_settings(&mut settings, "/usr/bin/claude-track log");
        assert!(added);

        let hooks = settings["hooks"]["PostToolUse"].as_array().unwrap();
        assert_eq!(hooks.len(), 2);
    }

    #[test]
    fn patch_already_installed() {
        let mut settings = serde_json::json!({
            "hooks": {
                "PostToolUse": [
                    {
                        "matcher": ".*",
                        "hooks": [{"type": "command", "command": "/usr/bin/claude-track log"}]
                    }
                ]
            }
        });
        let added = patch_settings(&mut settings, "/usr/bin/claude-track log");
        assert!(!added);

        let hooks = settings["hooks"]["PostToolUse"].as_array().unwrap();
        assert_eq!(hooks.len(), 1);
    }

    #[test]
    fn patch_settings_preserves_existing_keys() {
        let mut settings = serde_json::json!({
            "other_key": "value",
            "hooks": {
                "PreToolUse": []
            }
        });
        patch_settings(&mut settings, "cmd log");
        assert_eq!(settings["other_key"], "value");
        assert!(settings["hooks"]["PreToolUse"].is_array());
        assert!(settings["hooks"]["PostToolUse"].is_array());
    }

    #[test]
    fn write_settings_creates_file() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("nested").join("settings.json");

        let settings = serde_json::json!({"key": "value"});
        write_settings(&settings, &path).unwrap();

        let content = fs::read_to_string(&path).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();
        assert_eq!(parsed["key"], "value");
    }

    #[test]
    fn install_to_fresh_settings() {
        let dir = TempDir::new().unwrap();
        let settings_path = dir.path().join("settings.json");

        let output = install_to(&settings_path, "claude-track log").unwrap();
        assert!(output.contains("Installed successfully."));
        assert!(output.contains("claude-track stats"));

        // Verify file was written
        let content = fs::read_to_string(&settings_path).unwrap();
        let settings: serde_json::Value = serde_json::from_str(&content).unwrap();
        assert_eq!(
            settings["hooks"]["PostToolUse"][0]["hooks"][0]["command"],
            "claude-track log"
        );
    }

    #[test]
    fn install_to_already_installed() {
        let dir = TempDir::new().unwrap();
        let settings_path = dir.path().join("settings.json");

        let existing = serde_json::json!({
            "hooks": {
                "PostToolUse": [{
                    "matcher": ".*",
                    "hooks": [{"type": "command", "command": "claude-track log"}]
                }]
            }
        });
        fs::write(&settings_path, serde_json::to_string_pretty(&existing).unwrap()).unwrap();

        let output = install_to(&settings_path, "claude-track log").unwrap();
        assert!(output.contains("already installed"));
    }

    #[test]
    fn install_to_creates_parent_dirs() {
        let dir = TempDir::new().unwrap();
        let settings_path = dir.path().join("deep").join("nested").join("settings.json");

        let output = install_to(&settings_path, "cmd log").unwrap();
        assert!(output.contains("Installed successfully."));
        assert!(settings_path.exists());
    }
}
