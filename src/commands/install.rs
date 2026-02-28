use std::fs;

/// Install the PostToolUse hook into ~/.claude/settings.json.
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

    let claude_dir = dirs::home_dir()
        .ok_or("could not determine home directory")?
        .join(".claude");

    let settings_path = claude_dir.join("settings.json");

    // Read or create settings
    let mut settings: serde_json::Value = if settings_path.exists() {
        let contents = fs::read_to_string(&settings_path)?;
        serde_json::from_str(&contents)?
    } else {
        fs::create_dir_all(&claude_dir)?;
        serde_json::json!({})
    };

    let command = format!("{binary_path} log");

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
                    hooks.iter().any(|hook| {
                        hook.get("command").and_then(|c| c.as_str()) == Some(&command)
                    })
                })
                .unwrap_or(false)
        });
        if already_installed {
            println!("Hook is already installed.");
            return Ok(());
        }
    }

    // Build the hook entry
    let hook_entry = serde_json::json!({
        "matcher": ".*",
        "hooks": [
            {
                "type": "command",
                "command": command,
            }
        ]
    });

    // Ensure hooks.PostToolUse array exists and append
    let hooks = settings
        .as_object_mut()
        .ok_or("settings is not an object")?
        .entry("hooks")
        .or_insert_with(|| serde_json::json!({}));
    let post_tool_use = hooks
        .as_object_mut()
        .ok_or("hooks is not an object")?
        .entry("PostToolUse")
        .or_insert_with(|| serde_json::json!([]));
    post_tool_use
        .as_array_mut()
        .ok_or("PostToolUse is not an array")?
        .push(hook_entry);

    // Write settings back
    let formatted = serde_json::to_string_pretty(&settings)?;
    fs::write(&settings_path, formatted)?;

    println!("Hook added to {}", settings_path.display());
    println!("Installed successfully.");
    println!();
    println!("  Tracking starts on your next Claude Code session.");
    println!("  View stats anytime:  claude-track stats");

    Ok(())
}
