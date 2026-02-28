use std::fs;
use std::io::{self, BufRead, Write};

/// Remove the PostToolUse hook from settings and optionally delete the log.
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

    // Determine the command string to match against
    let binary_path = std::env::current_exe()?
        .to_str()
        .ok_or("binary path is not valid UTF-8")?
        .to_string();
    let command = format!("{binary_path} log");

    // Remove hook from settings.json
    if settings_path.exists() {
        let contents = fs::read_to_string(&settings_path)?;
        let mut settings: serde_json::Value = serde_json::from_str(&contents)?;

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
                        hooks.iter().any(|hook| {
                            hook.get("command").and_then(|c| c.as_str()) == Some(&command)
                        })
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
            // Clean up empty hooks object (check after potential removal)
            if hooks.is_empty() {
                if let Some(obj) = settings.as_object_mut() {
                    obj.remove("hooks");
                }
            }
        }

        if modified {
            let formatted = serde_json::to_string_pretty(&settings)?;
            fs::write(&settings_path, formatted)?;
            println!("Hook removed from {}", settings_path.display());
        } else {
            println!("No matching PostToolUse hook found in settings.");
        }
    } else {
        println!("No settings.json found.");
    }

    // Ask about log data
    if log_path.exists() {
        let line_count = {
            let file = fs::File::open(&log_path)?;
            io::BufReader::new(file).lines().count()
        };

        print!("Delete usage log? ({line_count} entries in {}) [y/N] ", log_path.display());
        io::stdout().flush()?;

        let mut answer = String::new();
        io::stdin().lock().read_line(&mut answer)?;

        if answer.trim().eq_ignore_ascii_case("y") {
            fs::remove_file(&log_path)?;
            println!("Log deleted.");
        } else {
            println!("Log kept at {}", log_path.display());
        }
    }

    println!("Uninstalled successfully.");

    Ok(())
}
