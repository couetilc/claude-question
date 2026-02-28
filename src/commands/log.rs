use std::fs::{self, OpenOptions};
use std::io::{self, Read, Write};
use std::path::Path;

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
    let log_path = dirs::home_dir()
        .ok_or("could not determine home directory")?
        .join(".claude")
        .join("tool-usage.jsonl");

    append_record(io::stdin().lock(), &log_path)
}

/// Parse a HookInput from `reader` and append a JSONL record to `log_path`.
pub fn append_record(reader: impl Read, log_path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let input: HookInput = serde_json::from_reader(reader)?;

    let record = ToolCall {
        ts: Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string(),
        tool: input.tool_name.unwrap_or_default(),
        session: input.session_id.unwrap_or_default(),
        cwd: input.cwd.unwrap_or_default(),
        input: input.tool_input.unwrap_or(serde_json::Value::Null),
    };

    if let Some(parent) = log_path.parent() {
        fs::create_dir_all(parent)?;
    }

    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_path)?;

    let line = serde_json::to_string(&record)?;
    writeln!(file, "{line}")?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    use tempfile::TempDir;

    #[test]
    fn append_record_creates_file_and_writes_jsonl() {
        let dir = TempDir::new().unwrap();
        let log_path = dir.path().join("tool-usage.jsonl");

        let input = r#"{"tool_name":"Read","session_id":"s1","cwd":"/tmp","tool_input":{"file_path":"/foo"}}"#;
        append_record(Cursor::new(input), &log_path).unwrap();

        let content = fs::read_to_string(&log_path).unwrap();
        let lines: Vec<&str> = content.lines().collect();
        assert_eq!(lines.len(), 1);

        let record: ToolCall = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(record.tool, "Read");
        assert_eq!(record.session, "s1");
        assert_eq!(record.cwd, "/tmp");
        assert_eq!(record.input["file_path"], "/foo");
        assert!(!record.ts.is_empty());
    }

    #[test]
    fn append_record_appends_to_existing_file() {
        let dir = TempDir::new().unwrap();
        let log_path = dir.path().join("tool-usage.jsonl");

        let input1 = r#"{"tool_name":"Read","session_id":"s1","cwd":"/tmp","tool_input":{}}"#;
        let input2 = r#"{"tool_name":"Bash","session_id":"s2","cwd":"/home","tool_input":{}}"#;
        append_record(Cursor::new(input1), &log_path).unwrap();
        append_record(Cursor::new(input2), &log_path).unwrap();

        let content = fs::read_to_string(&log_path).unwrap();
        let lines: Vec<&str> = content.lines().collect();
        assert_eq!(lines.len(), 2);

        let r1: ToolCall = serde_json::from_str(lines[0]).unwrap();
        let r2: ToolCall = serde_json::from_str(lines[1]).unwrap();
        assert_eq!(r1.tool, "Read");
        assert_eq!(r2.tool, "Bash");
    }

    #[test]
    fn append_record_handles_missing_fields() {
        let dir = TempDir::new().unwrap();
        let log_path = dir.path().join("tool-usage.jsonl");

        let input = r#"{}"#;
        append_record(Cursor::new(input), &log_path).unwrap();

        let content = fs::read_to_string(&log_path).unwrap();
        let record: ToolCall = serde_json::from_str(content.trim()).unwrap();
        assert_eq!(record.tool, "");
        assert_eq!(record.session, "");
        assert_eq!(record.cwd, "");
        assert_eq!(record.input, serde_json::Value::Null);
    }

    #[test]
    fn append_record_creates_parent_dirs() {
        let dir = TempDir::new().unwrap();
        let log_path = dir.path().join("nested").join("deep").join("tool-usage.jsonl");

        let input = r#"{"tool_name":"Edit"}"#;
        append_record(Cursor::new(input), &log_path).unwrap();

        assert!(log_path.exists());
    }

    #[test]
    fn append_record_rejects_invalid_json() {
        let dir = TempDir::new().unwrap();
        let log_path = dir.path().join("tool-usage.jsonl");

        let result = append_record(Cursor::new("not json"), &log_path);
        assert!(result.is_err());
    }
}
