use std::fs;
use std::path::{Path, PathBuf};

/// The 6 hook events we register.
pub const HOOK_EVENTS: &[&str] = &[
    "SessionStart",
    "SessionEnd",
    "UserPromptSubmit",
    "Stop",
    "PreToolUse",
    "PostToolUse",
];

/// The standard install directory for user-local binaries.
pub fn install_dir() -> Result<PathBuf, Box<dyn std::error::Error>> {
    let home = dirs::home_dir().ok_or("could not determine home directory")?;
    Ok(home.join(".local").join("bin"))
}

/// Copy the binary to the install directory. Returns the installed path.
pub fn copy_binary(src: &Path, dest_dir: &Path) -> Result<PathBuf, Box<dyn std::error::Error>> {
    fs::create_dir_all(dest_dir)?;
    let dest = dest_dir.join("claude-track");

    let need_copy = if dest.exists() {
        src.canonicalize()? != dest.canonicalize()?
    } else {
        true
    };

    if need_copy {
        fs::copy(src, &dest)?;
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&dest, fs::Permissions::from_mode(0o755))?;
    }

    Ok(dest)
}

/// Install all hooks into ~/.claude/settings.json.
#[cfg(not(tarpaulin_include))]
pub fn run() {
    if let Err(e) = try_run() {
        eprintln!("claude-track install: {e}");
        std::process::exit(1);
    }
}

fn try_run() -> Result<(), Box<dyn std::error::Error>> {
    let current_exe = std::env::current_exe()?;
    let dest_dir = install_dir()?;
    let installed_path = copy_binary(&current_exe, &dest_dir)?;
    let installed_str = installed_path
        .to_str()
        .ok_or("installed path is not valid UTF-8")?;

    let settings_path = dirs::home_dir()
        .ok_or("could not determine home directory")?
        .join(".claude")
        .join("settings.json");

    let command = format!("{installed_str} hook");
    let output = install_to(&settings_path, &command)?;

    println!("Binary installed to {installed_str}");
    print!("{output}");

    // Hint if install dir is not on PATH
    if let Ok(path_var) = std::env::var("PATH") {
        let dest_dir_str = dest_dir.to_str().unwrap_or("");
        if !path_var.split(':').any(|p| p == dest_dir_str) {
            println!(
                "Tip: Add {} to your PATH to run `claude-track` from anywhere.",
                dest_dir.display()
            );
        }
    }

    Ok(())
}

/// Install all 6 hooks into the given settings file. Returns user-facing output.
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

    let added = patch_settings(&mut settings, command);

    if added > 0 {
        write_settings(&settings, settings_path)?;

        Ok(format!(
            "Registered {added} hook(s) in {}\n\
             Installed successfully.\n\
             \n\
             \x20 Tracking starts on your next Claude Code session.\n\
             \x20 View stats anytime:  claude-track stats\n",
            settings_path.display()
        ))
    } else {
        Ok("All hooks are already installed.\n".to_string())
    }
}

/// Add hook entries for all 6 events. Returns the number of hooks actually added.
pub fn patch_settings(settings: &mut serde_json::Value, command: &str) -> usize {
    let mut added = 0;

    for event in HOOK_EVENTS {
        if !is_hook_installed(settings, event, command) {
            add_hook_entry(settings, event, command);
            added += 1;
        }
    }

    added
}

/// Check if a hook command is already registered for the given event.
fn is_hook_installed(settings: &serde_json::Value, event: &str, command: &str) -> bool {
    settings
        .get("hooks")
        .and_then(|h| h.get(event))
        .and_then(|p| p.as_array())
        .map(|entries| {
            entries.iter().any(|entry| {
                entry
                    .get("hooks")
                    .and_then(|h| h.as_array())
                    .map(|hooks| {
                        hooks
                            .iter()
                            .any(|hook| hook.get("command").and_then(|c| c.as_str()) == Some(command))
                    })
                    .unwrap_or(false)
            })
        })
        .unwrap_or(false)
}

/// Add a single hook entry for the given event.
fn add_hook_entry(settings: &mut serde_json::Value, event: &str, command: &str) {
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
    let event_hooks = hooks
        .as_object_mut()
        .unwrap()
        .entry(event)
        .or_insert_with(|| serde_json::json!([]));
    event_hooks.as_array_mut().unwrap().push(hook_entry);
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
        let added = patch_settings(&mut settings, "claude-track hook");
        assert_eq!(added, 6);

        for event in HOOK_EVENTS {
            let hooks = settings["hooks"][event].as_array().unwrap();
            assert_eq!(hooks.len(), 1);
            assert_eq!(hooks[0]["matcher"], ".*");
            assert_eq!(hooks[0]["hooks"][0]["type"], "command");
            assert_eq!(hooks[0]["hooks"][0]["command"], "claude-track hook");
        }
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
        let added = patch_settings(&mut settings, "claude-track hook");
        assert_eq!(added, 6);

        // PostToolUse should have 2 entries now
        let hooks = settings["hooks"]["PostToolUse"].as_array().unwrap();
        assert_eq!(hooks.len(), 2);

        // Other events should have 1
        let hooks = settings["hooks"]["SessionStart"].as_array().unwrap();
        assert_eq!(hooks.len(), 1);
    }

    #[test]
    fn patch_already_installed() {
        let mut settings = serde_json::json!({});
        patch_settings(&mut settings, "claude-track hook");
        let added = patch_settings(&mut settings, "claude-track hook");
        assert_eq!(added, 0);
    }

    #[test]
    fn patch_partially_installed() {
        let mut settings = serde_json::json!({
            "hooks": {
                "PostToolUse": [{
                    "matcher": ".*",
                    "hooks": [{"type": "command", "command": "claude-track hook"}]
                }],
                "PreToolUse": [{
                    "matcher": ".*",
                    "hooks": [{"type": "command", "command": "claude-track hook"}]
                }]
            }
        });
        let added = patch_settings(&mut settings, "claude-track hook");
        assert_eq!(added, 4); // 6 - 2 already installed
    }

    #[test]
    fn patch_settings_preserves_existing_keys() {
        let mut settings = serde_json::json!({
            "other_key": "value",
            "hooks": {
                "SomeOtherHook": []
            }
        });
        patch_settings(&mut settings, "cmd hook");
        assert_eq!(settings["other_key"], "value");
        assert!(settings["hooks"]["SomeOtherHook"].is_array());
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

        let output = install_to(&settings_path, "claude-track hook").unwrap();
        assert!(output.contains("Registered 6 hook(s)"));
        assert!(output.contains("Installed successfully."));
        assert!(output.contains("claude-track stats"));

        // Verify all hooks written
        let content = fs::read_to_string(&settings_path).unwrap();
        let settings: serde_json::Value = serde_json::from_str(&content).unwrap();
        for event in HOOK_EVENTS {
            assert_eq!(
                settings["hooks"][event][0]["hooks"][0]["command"],
                "claude-track hook"
            );
        }
    }

    #[test]
    fn install_to_already_installed() {
        let dir = TempDir::new().unwrap();
        let settings_path = dir.path().join("settings.json");

        install_to(&settings_path, "claude-track hook").unwrap();
        let output = install_to(&settings_path, "claude-track hook").unwrap();
        assert!(output.contains("already installed"));
    }

    #[test]
    fn install_to_creates_parent_dirs() {
        let dir = TempDir::new().unwrap();
        let settings_path = dir.path().join("deep").join("nested").join("settings.json");

        let output = install_to(&settings_path, "cmd hook").unwrap();
        assert!(output.contains("Installed successfully."));
        assert!(settings_path.exists());
    }

    #[test]
    fn is_hook_installed_true() {
        let settings = serde_json::json!({
            "hooks": {
                "PostToolUse": [{
                    "matcher": ".*",
                    "hooks": [{"type": "command", "command": "claude-track hook"}]
                }]
            }
        });
        assert!(is_hook_installed(&settings, "PostToolUse", "claude-track hook"));
    }

    #[test]
    fn is_hook_installed_false_different_command() {
        let settings = serde_json::json!({
            "hooks": {
                "PostToolUse": [{
                    "matcher": ".*",
                    "hooks": [{"type": "command", "command": "other-tool"}]
                }]
            }
        });
        assert!(!is_hook_installed(&settings, "PostToolUse", "claude-track hook"));
    }

    #[test]
    fn is_hook_installed_false_no_event() {
        let settings = serde_json::json!({
            "hooks": {}
        });
        assert!(!is_hook_installed(&settings, "PostToolUse", "claude-track hook"));
    }

    #[test]
    fn is_hook_installed_false_no_hooks_key() {
        let settings = serde_json::json!({});
        assert!(!is_hook_installed(&settings, "PostToolUse", "claude-track hook"));
    }

    #[test]
    fn install_dir_returns_local_bin() {
        let dir = install_dir().unwrap();
        assert!(dir.ends_with(".local/bin"));
    }

    #[test]
    fn copy_binary_to_new_dir() {
        let dir = TempDir::new().unwrap();
        let src = dir.path().join("source-binary");
        fs::write(&src, b"binary content").unwrap();

        let dest_dir = dir.path().join("install");
        let result = copy_binary(&src, &dest_dir).unwrap();

        assert_eq!(result, dest_dir.join("claude-track"));
        assert_eq!(fs::read_to_string(&result).unwrap(), "binary content");

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let perms = fs::metadata(&result).unwrap().permissions();
            assert_eq!(perms.mode() & 0o777, 0o755);
        }
    }

    #[test]
    fn copy_binary_overwrites_existing() {
        let dir = TempDir::new().unwrap();
        let src = dir.path().join("source-binary");
        fs::write(&src, b"new content").unwrap();

        let dest_dir = dir.path().join("install");
        fs::create_dir_all(&dest_dir).unwrap();
        fs::write(dest_dir.join("claude-track"), b"old content").unwrap();

        let result = copy_binary(&src, &dest_dir).unwrap();
        assert_eq!(fs::read_to_string(&result).unwrap(), "new content");
    }

    #[test]
    fn copy_binary_same_file_is_noop() {
        let dir = TempDir::new().unwrap();
        let dest_dir = dir.path();
        let binary = dest_dir.join("claude-track");
        fs::write(&binary, b"content").unwrap();

        let result = copy_binary(&binary, dest_dir).unwrap();
        assert_eq!(result, binary);
        assert_eq!(fs::read_to_string(&result).unwrap(), "content");
    }
}
