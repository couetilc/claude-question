use std::fs::{self, OpenOptions};
use std::io::{self, Write};

use chrono::Utc;

use crate::models::{HookInput, ToolCall};

/// Read JSON from stdin, append a JSONL record to ~/.claude/tool-usage.jsonl.
/// Always exits 0 so the hook never blocks Claude Code.
pub fn run() {
    if let Err(e) = try_run() {
        eprintln!("claude-track log: {e}");
    }
}

fn try_run() -> Result<(), Box<dyn std::error::Error>> {
    let input: HookInput = serde_json::from_reader(io::stdin().lock())?;

    let record = ToolCall {
        ts: Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string(),
        tool: input.tool_name.unwrap_or_default(),
        session: input.session_id.unwrap_or_default(),
        cwd: input.cwd.unwrap_or_default(),
        input: input.tool_input.unwrap_or(serde_json::Value::Null),
    };

    let log_path = dirs::home_dir()
        .ok_or("could not determine home directory")?
        .join(".claude")
        .join("tool-usage.jsonl");

    // Ensure parent directory exists
    if let Some(parent) = log_path.parent() {
        fs::create_dir_all(parent)?;
    }

    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)?;

    let line = serde_json::to_string(&record)?;
    writeln!(file, "{line}")?;

    Ok(())
}
