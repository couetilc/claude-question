use std::fs;
use std::io::{self, BufReader, Read, Seek, SeekFrom};
use std::path::Path;

use chrono::Utc;
use rusqlite::Connection;

use crate::db;
use crate::models::{AggregatedTokenUsage, HookInput, TranscriptLine};

/// Hook entrypoint: reads JSON from stdin, dispatches by event, writes to SQLite.
/// Always exits 0 so the hook never blocks Claude Code.
#[cfg(not(tarpaulin_include))]
pub fn run() {
    if let Err(e) = try_run() {
        eprintln!("claude-track hook: {e}");
    }
}

fn try_run() -> Result<(), Box<dyn std::error::Error>> {
    let db_path = db::db_path()?;
    let conn = db::open_db(&db_path)?;
    dispatch(io::stdin().lock(), &conn)
}

/// Parse hook input from `reader` and dispatch to the appropriate handler.
pub fn dispatch(reader: impl Read, conn: &Connection) -> Result<(), Box<dyn std::error::Error>> {
    let input: HookInput = serde_json::from_reader(reader)?;
    let now = Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
    let event = input.hook_event_name.as_deref().unwrap_or("PostToolUse");

    match event {
        "SessionStart" => handle_session_start(&input, &now, conn),
        "SessionEnd" => handle_session_end(&input, &now, conn),
        "UserPromptSubmit" => handle_user_prompt(&input, &now, conn),
        "Stop" => handle_stop(&input, &now, conn),
        "PreToolUse" => handle_pre_tool_use(&input, &now, conn),
        "PostToolUse" => handle_post_tool_use(&input, &now, conn),
        _ => Ok(()), // Unknown event, silently ignore
    }
}

fn handle_session_start(
    input: &HookInput,
    now: &str,
    conn: &Connection,
) -> Result<(), Box<dyn std::error::Error>> {
    db::insert_session_start(
        conn,
        input.session_id.as_deref().unwrap_or_default(),
        now,
        input.reason.as_deref().unwrap_or_default(),
        input.cwd.as_deref().unwrap_or_default(),
        input.transcript_path.as_deref().unwrap_or_default(),
    )
}

fn handle_session_end(
    input: &HookInput,
    now: &str,
    conn: &Connection,
) -> Result<(), Box<dyn std::error::Error>> {
    db::update_session_end(
        conn,
        input.session_id.as_deref().unwrap_or_default(),
        now,
        input.reason.as_deref().unwrap_or_default(),
    )
}

fn handle_user_prompt(
    input: &HookInput,
    now: &str,
    conn: &Connection,
) -> Result<(), Box<dyn std::error::Error>> {
    db::insert_prompt(
        conn,
        input.session_id.as_deref().unwrap_or_default(),
        now,
        input.prompt.as_deref().unwrap_or_default(),
    )
}

fn handle_stop(
    input: &HookInput,
    now: &str,
    conn: &Connection,
) -> Result<(), Box<dyn std::error::Error>> {
    let session_id = input.session_id.as_deref().unwrap_or_default();

    // Try to find the transcript path: first from input, then from DB
    let transcript_path = input
        .transcript_path
        .clone()
        .or_else(|| db::get_transcript_path(conn, session_id).ok().flatten());

    if let Some(path) = transcript_path {
        let path = Path::new(&path);

        // Get current DB state (or defaults)
        let (cur_input, cur_cc, cur_cr, cur_output, cur_calls, cur_offset, cur_model) =
            db::get_session_token_state(conn, session_id)?
                .unwrap_or((0, 0, 0, 0, 0, 0, String::new()));

        // Check for file shrink
        let file_len = fs::metadata(path).map(|m| m.len()).unwrap_or(0);
        let effective_offset = if (cur_offset as u64) > file_len {
            0
        } else {
            cur_offset as u64
        };

        // Parse only new content
        let (delta, new_offset) = parse_transcript_from_offset(path, effective_offset);

        // Determine final values
        let (new_input, new_cc, new_cr, new_output, new_calls) =
            if effective_offset == 0 && cur_offset > 0 {
                // File shrank: delta IS cumulative, don't add to existing
                (
                    delta.input_tokens,
                    delta.cache_creation_tokens,
                    delta.cache_read_tokens,
                    delta.output_tokens,
                    delta.api_call_count,
                )
            } else {
                // Normal: add delta to existing
                (
                    cur_input + delta.input_tokens,
                    cur_cc + delta.cache_creation_tokens,
                    cur_cr + delta.cache_read_tokens,
                    cur_output + delta.output_tokens,
                    cur_calls + delta.api_call_count,
                )
            };

        // Use existing model if delta didn't find one
        let model = if delta.model.is_empty() {
            &cur_model
        } else {
            &delta.model
        };

        db::insert_token_usage(
            conn,
            session_id,
            now,
            model,
            new_input,
            new_cc,
            new_cr,
            new_output,
            new_calls,
            new_offset as i64,
        )?;

        let pending_ids = db::get_pending_plan_tool_use_ids(conn, session_id)?;
        if !pending_ids.is_empty() {
            let acceptances = parse_plan_acceptances(path, &pending_ids);
            for (tool_use_id, accepted) in acceptances {
                db::update_plan_accepted(conn, &tool_use_id, accepted)?;
            }
        }
    }
    Ok(())
}

fn handle_pre_tool_use(
    input: &HookInput,
    now: &str,
    conn: &Connection,
) -> Result<(), Box<dyn std::error::Error>> {
    let input_json = input
        .tool_input
        .as_ref()
        .map(|v| serde_json::to_string(v).unwrap_or_default())
        .unwrap_or_default();

    let session_id = input.session_id.as_deref().unwrap_or_default();
    let tool_use_id = input.tool_use_id.as_deref().unwrap_or_default();

    db::insert_tool_use(
        conn,
        tool_use_id,
        session_id,
        input.tool_name.as_deref().unwrap_or_default(),
        now,
        input.cwd.as_deref().unwrap_or_default(),
        &input_json,
    )?;

    if input.tool_name.as_deref() == Some("ExitPlanMode") {
        let plan_text = input
            .tool_input
            .as_ref()
            .and_then(|v| v.get("plan"))
            .and_then(|v| v.as_str())
            .unwrap_or_default();
        db::insert_plan(conn, session_id, tool_use_id, now, plan_text)?;
    }

    Ok(())
}

/// Parse transcript JSONL for plan acceptance/rejection results.
/// For each matching tool_use_id, returns (tool_use_id, accepted).
/// `is_error` absent → accepted, `is_error: true` → rejected.
pub fn parse_plan_acceptances(path: &Path, tool_use_ids: &[String]) -> Vec<(String, bool)> {
    if tool_use_ids.is_empty() {
        return Vec::new();
    }
    let content = match fs::read_to_string(path) {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };
    let mut results = Vec::new();
    for line in content.lines() {
        if line.is_empty() {
            continue;
        }
        let val: serde_json::Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => continue,
        };
        if val.get("type").and_then(|v| v.as_str()) != Some("user") {
            continue;
        }
        let content_arr = match val
            .get("message")
            .and_then(|m| m.get("content"))
            .and_then(|c| c.as_array())
        {
            Some(arr) => arr,
            None => continue,
        };
        for block in content_arr {
            if block.get("type").and_then(|v| v.as_str()) != Some("tool_result") {
                continue;
            }
            let tuid = match block.get("tool_use_id").and_then(|v| v.as_str()) {
                Some(id) => id,
                None => continue,
            };
            if tool_use_ids.iter().any(|id| id == tuid) {
                let is_error = block.get("is_error").and_then(|v| v.as_bool()).unwrap_or(false);
                results.push((tuid.to_string(), !is_error));
            }
        }
    }
    results
}

fn handle_post_tool_use(
    input: &HookInput,
    now: &str,
    conn: &Connection,
) -> Result<(), Box<dyn std::error::Error>> {
    let input_json = input
        .tool_input
        .as_ref()
        .map(|v| serde_json::to_string(v).unwrap_or_default())
        .unwrap_or_default();

    let response_summary = input
        .tool_response
        .as_ref()
        .map(|v| truncate_response(v))
        .unwrap_or_default();

    db::update_tool_use_response(
        conn,
        input.tool_use_id.as_deref().unwrap_or_default(),
        input.session_id.as_deref().unwrap_or_default(),
        input.tool_name.as_deref().unwrap_or_default(),
        now,
        input.cwd.as_deref().unwrap_or_default(),
        &input_json,
        &response_summary,
    )
}

/// Truncate a tool response to a short summary (max 500 chars).
fn truncate_response(value: &serde_json::Value) -> String {
    let s = match value {
        serde_json::Value::String(s) => s.clone(),
        other => serde_json::to_string(other).unwrap_or_default(),
    };
    if s.len() > 500 {
        format!("{}...", &s[..497])
    } else {
        s
    }
}

/// Read file contents from a byte offset. Returns None on any I/O error
/// (metadata, seek, or read failure after a successful open).
#[cfg(not(tarpaulin_include))]
fn read_file_from_offset(file: &mut fs::File, start_offset: u64) -> Option<String> {
    let file_len = file.metadata().ok()?.len();
    if start_offset >= file_len {
        return None;
    }
    if start_offset > 0 {
        file.seek(SeekFrom::Start(start_offset)).ok()?;
    }
    let mut buf = String::new();
    BufReader::new(file).read_to_string(&mut buf).ok()?;
    Some(buf)
}

/// Parse a transcript JSONL file and aggregate token usage (full parse from start).
#[cfg(test)]
pub fn parse_transcript(path: &Path) -> AggregatedTokenUsage {
    parse_transcript_from_offset(path, 0).0
}

/// Parse a transcript JSONL file starting from `start_offset` bytes.
/// Returns `(delta_usage, new_offset)` where `new_offset` is the byte position
/// after the last successfully parsed line.
pub fn parse_transcript_from_offset(path: &Path, start_offset: u64) -> (AggregatedTokenUsage, u64) {
    let mut file = match fs::File::open(path) {
        Ok(f) => f,
        Err(_) => return (AggregatedTokenUsage::default(), start_offset),
    };

    let remaining = match read_file_from_offset(&mut file, start_offset) {
        Some(s) => s,
        None => return (AggregatedTokenUsage::default(), start_offset),
    };

    let mut agg = AggregatedTokenUsage::default();
    let mut offset = start_offset;
    let remaining_bytes = remaining.as_bytes();
    let mut pos = 0;

    while pos < remaining_bytes.len() {
        // Find next newline
        let line_end = remaining_bytes[pos..].iter().position(|&b| b == b'\n');

        let (line_str, next_pos, has_newline) = match line_end {
            Some(end) => (&remaining[pos..pos + end], pos + end + 1, true),
            None => (&remaining[pos..], remaining_bytes.len(), false),
        };

        if line_str.is_empty() {
            pos = next_pos;
            offset = start_offset + pos as u64;
            continue;
        }

        match serde_json::from_str::<TranscriptLine>(line_str) {
            Ok(tl) => {
                pos = next_pos;
                offset = start_offset + pos as u64;

                if tl.line_type.as_deref() != Some("assistant") {
                    continue;
                }

                if let Some(msg) = tl.message {
                    if let Some(model) = &msg.model {
                        if agg.model.is_empty() {
                            agg.model = model.clone();
                        }
                    }
                    if let Some(usage) = msg.usage {
                        agg.input_tokens += usage.input_tokens.unwrap_or(0);
                        agg.output_tokens += usage.output_tokens.unwrap_or(0);
                        agg.cache_creation_tokens +=
                            usage.cache_creation_input_tokens.unwrap_or(0);
                        agg.cache_read_tokens += usage.cache_read_input_tokens.unwrap_or(0);
                        agg.api_call_count += 1;
                    }
                }
            }
            Err(_) => {
                if !has_newline {
                    // Partial line at EOF — don't advance offset
                    break;
                }
                // Complete line but invalid JSON — skip it
                pos = next_pos;
                offset = start_offset + pos as u64;
            }
        }
    }

    (agg, offset)
}


#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    use tempfile::TempDir;

    fn test_conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        db::init_db(&conn).unwrap();
        conn
    }

    #[test]
    fn dispatch_session_start() {
        let conn = test_conn();
        let json = r#"{"hook_event_name":"SessionStart","session_id":"s1","cwd":"/proj","transcript_path":"/tmp/t.jsonl","reason":"startup"}"#;
        dispatch(Cursor::new(json), &conn).unwrap();

        let reason: String = conn
            .query_row("SELECT start_reason FROM sessions WHERE session_id='s1'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(reason, "startup");
    }

    #[test]
    fn dispatch_session_end() {
        let conn = test_conn();
        // First start the session
        let start = r#"{"hook_event_name":"SessionStart","session_id":"s1","cwd":"/proj","transcript_path":"/t","reason":"startup"}"#;
        dispatch(Cursor::new(start), &conn).unwrap();

        let end = r#"{"hook_event_name":"SessionEnd","session_id":"s1","reason":"logout"}"#;
        dispatch(Cursor::new(end), &conn).unwrap();

        let reason: String = conn
            .query_row("SELECT end_reason FROM sessions WHERE session_id='s1'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(reason, "logout");
    }

    #[test]
    fn dispatch_session_end_without_start() {
        let conn = test_conn();
        let end = r#"{"hook_event_name":"SessionEnd","session_id":"s_new","reason":"clear"}"#;
        dispatch(Cursor::new(end), &conn).unwrap();

        let reason: String = conn
            .query_row("SELECT end_reason FROM sessions WHERE session_id='s_new'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(reason, "clear");
    }

    #[test]
    fn dispatch_user_prompt() {
        let conn = test_conn();
        let json = r#"{"hook_event_name":"UserPromptSubmit","session_id":"s1","prompt":"fix the bug"}"#;
        dispatch(Cursor::new(json), &conn).unwrap();

        let text: String = conn
            .query_row("SELECT prompt_text FROM prompts", [], |r| r.get(0))
            .unwrap();
        assert_eq!(text, "fix the bug");
    }

    #[test]
    fn dispatch_pre_tool_use() {
        let conn = test_conn();
        let json = r#"{"hook_event_name":"PreToolUse","session_id":"s1","tool_name":"Read","tool_use_id":"tu1","tool_input":{"file_path":"/foo"},"cwd":"/proj"}"#;
        dispatch(Cursor::new(json), &conn).unwrap();

        let (tool, input): (String, String) = conn
            .query_row("SELECT tool_name, input FROM tool_uses WHERE tool_use_id='tu1'", [], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .unwrap();
        assert_eq!(tool, "Read");
        assert!(input.contains("file_path"));
    }

    #[test]
    fn dispatch_post_tool_use_updates_existing() {
        let conn = test_conn();
        // PreToolUse first
        let pre = r#"{"hook_event_name":"PreToolUse","session_id":"s1","tool_name":"Read","tool_use_id":"tu1","tool_input":{"file_path":"/foo"},"cwd":"/proj"}"#;
        dispatch(Cursor::new(pre), &conn).unwrap();

        // PostToolUse updates
        let post = r#"{"hook_event_name":"PostToolUse","session_id":"s1","tool_name":"Read","tool_use_id":"tu1","tool_input":{"file_path":"/foo"},"tool_response":"file contents","cwd":"/proj"}"#;
        dispatch(Cursor::new(post), &conn).unwrap();

        let resp: String = conn
            .query_row("SELECT response_summary FROM tool_uses WHERE tool_use_id='tu1'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(resp, "file contents");
    }

    #[test]
    fn dispatch_post_tool_use_inserts_if_no_pre() {
        let conn = test_conn();
        let post = r#"{"hook_event_name":"PostToolUse","session_id":"s1","tool_name":"Bash","tool_use_id":"tu2","tool_input":{"command":"ls"},"tool_response":"output","cwd":"/proj"}"#;
        dispatch(Cursor::new(post), &conn).unwrap();

        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM tool_uses WHERE tool_use_id='tu2'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn dispatch_default_event_is_post_tool_use() {
        let conn = test_conn();
        // No hook_event_name — defaults to PostToolUse
        let json = r#"{"session_id":"s1","tool_name":"Read","tool_use_id":"tu3","tool_input":{},"cwd":"/proj"}"#;
        dispatch(Cursor::new(json), &conn).unwrap();

        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM tool_uses WHERE tool_use_id='tu3'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn dispatch_unknown_event() {
        let conn = test_conn();
        let json = r#"{"hook_event_name":"FutureEvent","session_id":"s1"}"#;
        dispatch(Cursor::new(json), &conn).unwrap();
        // Should succeed silently
    }

    #[test]
    fn dispatch_invalid_json_errors() {
        let conn = test_conn();
        let result = dispatch(Cursor::new("not json"), &conn);
        assert!(result.is_err());
    }

    #[test]
    fn dispatch_empty_fields() {
        let conn = test_conn();
        let json = r#"{"hook_event_name":"PreToolUse"}"#;
        dispatch(Cursor::new(json), &conn).unwrap();

        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM tool_uses", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn dispatch_stop_with_transcript() {
        let dir = TempDir::new().unwrap();
        let transcript_path = dir.path().join("transcript.jsonl");
        let transcript_content = format!(
            "{}\n{}\n",
            r#"{"type":"assistant","message":{"model":"claude-sonnet-4-20250514","usage":{"input_tokens":100,"output_tokens":50,"cache_creation_input_tokens":200,"cache_read_input_tokens":300}}}"#,
            r#"{"type":"assistant","message":{"model":"claude-sonnet-4-20250514","usage":{"input_tokens":150,"output_tokens":75}}}"#,
        );
        fs::write(&transcript_path, &transcript_content).unwrap();

        let conn = test_conn();
        let json = format!(
            r#"{{"hook_event_name":"Stop","session_id":"s1","transcript_path":"{}"}}"#,
            transcript_path.display()
        );
        dispatch(Cursor::new(json.as_bytes()), &conn).unwrap();

        let (model, inp, out, cc, cr, calls): (String, i64, i64, i64, i64, i64) = conn
            .query_row(
                "SELECT model, input_tokens, output_tokens, cache_creation_tokens, cache_read_tokens, api_call_count FROM token_usage WHERE session_id='s1'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)),
            )
            .unwrap();
        assert_eq!(model, "claude-sonnet-4-20250514");
        assert_eq!(inp, 250);
        assert_eq!(out, 125);
        assert_eq!(cc, 200);
        assert_eq!(cr, 300);
        assert_eq!(calls, 2);
    }

    #[test]
    fn dispatch_stop_with_transcript_from_db() {
        let dir = TempDir::new().unwrap();
        let transcript_path = dir.path().join("transcript.jsonl");
        let transcript_content =
            r#"{"type":"assistant","message":{"model":"m","usage":{"input_tokens":10,"output_tokens":5}}}"#;
        fs::write(&transcript_path, format!("{transcript_content}\n")).unwrap();

        let conn = test_conn();
        // Store transcript_path via SessionStart
        db::insert_session_start(
            &conn,
            "s1",
            "ts",
            "startup",
            "/proj",
            &transcript_path.display().to_string(),
        )
        .unwrap();

        // Stop without transcript_path in input — should look up from DB
        let json = r#"{"hook_event_name":"Stop","session_id":"s1"}"#;
        dispatch(Cursor::new(json), &conn).unwrap();

        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM token_usage WHERE session_id='s1'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn dispatch_stop_no_transcript() {
        let conn = test_conn();
        let json = r#"{"hook_event_name":"Stop","session_id":"s1"}"#;
        dispatch(Cursor::new(json), &conn).unwrap();

        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM token_usage", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn parse_transcript_missing_file() {
        let agg = parse_transcript(Path::new("/nonexistent/path.jsonl"));
        assert_eq!(agg.api_call_count, 0);
        assert_eq!(agg.model, "");
    }

    #[test]
    fn parse_transcript_empty_file() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("empty.jsonl");
        fs::write(&path, "").unwrap();

        let agg = parse_transcript(&path);
        assert_eq!(agg.api_call_count, 0);
    }

    #[test]
    fn parse_transcript_skips_non_assistant() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("transcript.jsonl");
        let content = format!(
            "{}\n{}\n",
            r#"{"type":"user","message":{}}"#,
            r#"{"type":"assistant","message":{"model":"m","usage":{"input_tokens":10,"output_tokens":5}}}"#,
        );
        fs::write(&path, content).unwrap();

        let agg = parse_transcript(&path);
        assert_eq!(agg.api_call_count, 1);
        assert_eq!(agg.input_tokens, 10);
    }

    #[test]
    fn parse_transcript_skips_invalid_lines() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("transcript.jsonl");
        let content = format!(
            "{}\n{}\n{}\n",
            "not json",
            "",
            r#"{"type":"assistant","message":{"model":"m","usage":{"input_tokens":10,"output_tokens":5}}}"#,
        );
        fs::write(&path, content).unwrap();

        let agg = parse_transcript(&path);
        assert_eq!(agg.api_call_count, 1);
    }

    #[test]
    fn parse_transcript_no_usage_field() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("transcript.jsonl");
        let content = r#"{"type":"assistant","message":{"model":"m"}}"#;
        fs::write(&path, format!("{content}\n")).unwrap();

        let agg = parse_transcript(&path);
        assert_eq!(agg.api_call_count, 0);
        assert_eq!(agg.model, "m");
    }

    #[test]
    fn parse_transcript_no_model() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("transcript.jsonl");
        let content = r#"{"type":"assistant","message":{"usage":{"input_tokens":10,"output_tokens":5}}}"#;
        fs::write(&path, format!("{content}\n")).unwrap();

        let agg = parse_transcript(&path);
        assert_eq!(agg.api_call_count, 1);
        assert_eq!(agg.model, "");
    }

    #[test]
    fn parse_transcript_no_message() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("transcript.jsonl");
        let content = r#"{"type":"assistant"}"#;
        fs::write(&path, format!("{content}\n")).unwrap();

        let agg = parse_transcript(&path);
        assert_eq!(agg.api_call_count, 0);
    }

    #[test]
    fn truncate_response_short() {
        let val = serde_json::json!("short text");
        assert_eq!(truncate_response(&val), "short text");
    }

    #[test]
    fn truncate_response_long() {
        let long = "x".repeat(600);
        let val = serde_json::json!(long);
        let result = truncate_response(&val);
        assert_eq!(result.len(), 500);
        assert!(result.ends_with("..."));
    }

    #[test]
    fn truncate_response_non_string() {
        let val = serde_json::json!({"key": "value"});
        let result = truncate_response(&val);
        assert!(result.contains("key"));
    }

    #[test]
    fn post_tool_use_long_response_truncated() {
        let conn = test_conn();
        let long_response = "x".repeat(600);
        let json = format!(
            r#"{{"hook_event_name":"PostToolUse","session_id":"s1","tool_name":"Read","tool_use_id":"tu_long","tool_input":{{}},"tool_response":"{}","cwd":"/proj"}}"#,
            long_response
        );
        dispatch(Cursor::new(json.as_bytes()), &conn).unwrap();

        let resp: String = conn
            .query_row("SELECT response_summary FROM tool_uses WHERE tool_use_id='tu_long'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(resp.len(), 500);
        assert!(resp.ends_with("..."));
    }

    // --- parse_transcript_from_offset tests ---

    fn assistant_line(input_tokens: i64, output_tokens: i64) -> String {
        format!(
            r#"{{"type":"assistant","message":{{"model":"claude-sonnet-4-20250514","usage":{{"input_tokens":{},"output_tokens":{}}}}}}}"#,
            input_tokens, output_tokens
        )
    }

    fn assistant_line_with_cache(input: i64, output: i64, cc: i64, cr: i64) -> String {
        format!(
            r#"{{"type":"assistant","message":{{"model":"claude-sonnet-4-20250514","usage":{{"input_tokens":{},"output_tokens":{},"cache_creation_input_tokens":{},"cache_read_input_tokens":{}}}}}}}"#,
            input, output, cc, cr
        )
    }

    #[test]
    fn incremental_parse_two_stages() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("transcript.jsonl");

        // Write 2 lines
        let line1 = assistant_line(100, 50);
        let line2 = assistant_line(150, 75);
        let content = format!("{line1}\n{line2}\n");
        fs::write(&path, &content).unwrap();

        // Parse from 0
        let (agg1, offset1) = parse_transcript_from_offset(&path, 0);
        assert_eq!(agg1.input_tokens, 250);
        assert_eq!(agg1.output_tokens, 125);
        assert_eq!(agg1.api_call_count, 2);
        assert_eq!(offset1 as usize, content.len());

        // Append 2 more lines
        let line3 = assistant_line(200, 100);
        let line4 = assistant_line(50, 25);
        let extra = format!("{line3}\n{line4}\n");
        let mut file = fs::OpenOptions::new().append(true).open(&path).unwrap();
        std::io::Write::write_all(&mut file, extra.as_bytes()).unwrap();

        // Parse from previous offset — should only get delta
        let (agg2, offset2) = parse_transcript_from_offset(&path, offset1);
        assert_eq!(agg2.input_tokens, 250); // 200 + 50
        assert_eq!(agg2.output_tokens, 125); // 100 + 25
        assert_eq!(agg2.api_call_count, 2);
        assert_eq!(offset2 as usize, content.len() + extra.len());
    }

    #[test]
    fn offset_file_shrink_resets_to_zero() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("transcript.jsonl");

        let line = assistant_line(10, 5);
        fs::write(&path, format!("{line}\n")).unwrap();

        // Parse from offset larger than file size
        let (agg, new_offset) = parse_transcript_from_offset(&path, 99999);
        assert_eq!(agg.api_call_count, 0);
        assert_eq!(new_offset, 99999); // returns start_offset unchanged
    }

    #[test]
    fn offset_no_new_data() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("transcript.jsonl");

        let line = assistant_line(10, 5);
        let content = format!("{line}\n");
        fs::write(&path, &content).unwrap();

        // Parse from offset == file length
        let (agg, new_offset) = parse_transcript_from_offset(&path, content.len() as u64);
        assert_eq!(agg.api_call_count, 0);
        assert_eq!(agg.input_tokens, 0);
        assert_eq!(new_offset, content.len() as u64);
    }

    #[test]
    fn partial_line_at_eof_not_advanced() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("transcript.jsonl");

        let line1 = assistant_line(100, 50);
        let partial = r#"{"type":"assistant","message":{"model":"m","usage":{"#;
        // First line has trailing newline, partial line does NOT
        let content = format!("{line1}\n{partial}");
        fs::write(&path, &content).unwrap();

        let (agg, new_offset) = parse_transcript_from_offset(&path, 0);
        assert_eq!(agg.api_call_count, 1);
        assert_eq!(agg.input_tokens, 100);
        // Offset should be just past line1's newline, not past the partial
        assert_eq!(new_offset as usize, line1.len() + 1);
    }

    #[test]
    fn missing_file_returns_default() {
        let (agg, offset) = parse_transcript_from_offset(Path::new("/nonexistent/path.jsonl"), 42);
        assert_eq!(agg.api_call_count, 0);
        assert_eq!(agg.model, "");
        assert_eq!(offset, 42); // start_offset unchanged
    }

    #[test]
    fn parse_transcript_wrapper_full_cumulative() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("transcript.jsonl");

        let line1 = assistant_line(100, 50);
        let line2 = assistant_line(200, 100);
        fs::write(&path, format!("{line1}\n{line2}\n")).unwrap();

        let agg = parse_transcript(&path);
        assert_eq!(agg.input_tokens, 300);
        assert_eq!(agg.output_tokens, 150);
        assert_eq!(agg.api_call_count, 2);
    }

    // --- Risk condition tests ---

    #[test]
    fn risk1_file_truncation_rewrite() {
        // DB has offset=500 and accumulated tokens, but file is now 100 bytes.
        // handle_stop should detect shrink, re-parse from 0, and replace (not add to) existing values.
        let dir = TempDir::new().unwrap();
        let transcript_path = dir.path().join("transcript.jsonl");
        let conn = test_conn();

        // Initial transcript and stop
        let line1 = assistant_line(100, 50);
        let line2 = assistant_line(200, 100);
        let content = format!("{line1}\n{line2}\n");
        fs::write(&transcript_path, &content).unwrap();

        let json = format!(
            r#"{{"hook_event_name":"Stop","session_id":"s1","transcript_path":"{}"}}"#,
            transcript_path.display()
        );
        dispatch(Cursor::new(json.as_bytes()), &conn).unwrap();

        // Verify first stop
        let (inp, out, calls): (i64, i64, i64) = conn
            .query_row(
                "SELECT input_tokens, output_tokens, api_call_count FROM token_usage WHERE session_id='s1'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(inp, 300);
        assert_eq!(out, 150);
        assert_eq!(calls, 2);

        // Simulate file truncation/rewrite with smaller content
        let new_line = assistant_line(10, 5);
        fs::write(&transcript_path, format!("{new_line}\n")).unwrap();

        // Second stop — file has shrunk
        dispatch(Cursor::new(json.as_bytes()), &conn).unwrap();

        let (inp2, out2, calls2): (i64, i64, i64) = conn
            .query_row(
                "SELECT input_tokens, output_tokens, api_call_count FROM token_usage WHERE session_id='s1'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        // Should be the new file's values, NOT 300+10
        assert_eq!(inp2, 10);
        assert_eq!(out2, 5);
        assert_eq!(calls2, 1);
    }

    #[test]
    fn risk2_partial_last_line_at_eof() {
        // Transcript with incomplete JSON line at end. Parser should not advance
        // offset past it, so the next Stop re-reads the now-complete line.
        let dir = TempDir::new().unwrap();
        let transcript_path = dir.path().join("transcript.jsonl");
        let conn = test_conn();

        let line1 = assistant_line(100, 50);
        let partial = r#"{"type":"assistant","message":{"model":"m","usage":{"input_tokens":200,"output_tokens":100"#;
        fs::write(&transcript_path, format!("{line1}\n{partial}")).unwrap();

        let json = format!(
            r#"{{"hook_event_name":"Stop","session_id":"s1","transcript_path":"{}"}}"#,
            transcript_path.display()
        );
        dispatch(Cursor::new(json.as_bytes()), &conn).unwrap();

        // Should only see line1
        let (inp, calls): (i64, i64) = conn
            .query_row(
                "SELECT input_tokens, api_call_count FROM token_usage WHERE session_id='s1'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(inp, 100);
        assert_eq!(calls, 1);

        // Now complete the partial line
        let completed_line = r#"{"type":"assistant","message":{"model":"m","usage":{"input_tokens":200,"output_tokens":100}}}"#;
        fs::write(&transcript_path, format!("{line1}\n{completed_line}\n")).unwrap();

        dispatch(Cursor::new(json.as_bytes()), &conn).unwrap();

        // Should now include both lines
        let (inp2, calls2): (i64, i64) = conn
            .query_row(
                "SELECT input_tokens, api_call_count FROM token_usage WHERE session_id='s1'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(inp2, 300);
        assert_eq!(calls2, 2);
    }

    #[test]
    fn risk4_accumulation_correctness_three_stages() {
        // Write a known transcript, parse in 3 stages (simulating 3 stops),
        // verify final DB totals exactly match a single full parse.
        let dir = TempDir::new().unwrap();
        let transcript_path = dir.path().join("transcript.jsonl");
        let conn = test_conn();

        let lines = vec![
            assistant_line_with_cache(100, 50, 10, 20),
            assistant_line_with_cache(200, 100, 30, 40),
            assistant_line_with_cache(300, 150, 50, 60),
            assistant_line_with_cache(400, 200, 70, 80),
            assistant_line_with_cache(500, 250, 90, 100),
            assistant_line_with_cache(600, 300, 110, 120),
        ];

        let json = format!(
            r#"{{"hook_event_name":"Stop","session_id":"s1","transcript_path":"{}"}}"#,
            transcript_path.display()
        );

        // Stage 1: first 2 lines
        fs::write(&transcript_path, format!("{}\n{}\n", lines[0], lines[1])).unwrap();
        dispatch(Cursor::new(json.as_bytes()), &conn).unwrap();

        // Stage 2: append 2 more lines
        let mut file = fs::OpenOptions::new().append(true).open(&transcript_path).unwrap();
        std::io::Write::write_all(&mut file, format!("{}\n{}\n", lines[2], lines[3]).as_bytes()).unwrap();
        drop(file);
        dispatch(Cursor::new(json.as_bytes()), &conn).unwrap();

        // Stage 3: append final 2 lines
        let mut file = fs::OpenOptions::new().append(true).open(&transcript_path).unwrap();
        std::io::Write::write_all(&mut file, format!("{}\n{}\n", lines[4], lines[5]).as_bytes()).unwrap();
        drop(file);
        dispatch(Cursor::new(json.as_bytes()), &conn).unwrap();

        // Verify final DB values
        let (inp, out, cc, cr, calls): (i64, i64, i64, i64, i64) = conn
            .query_row(
                "SELECT input_tokens, output_tokens, cache_creation_tokens, cache_read_tokens, api_call_count FROM token_usage WHERE session_id='s1'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
            )
            .unwrap();

        // Compare against a single full parse
        let full_agg = parse_transcript(&transcript_path);
        assert_eq!(inp, full_agg.input_tokens);
        assert_eq!(out, full_agg.output_tokens);
        assert_eq!(cc, full_agg.cache_creation_tokens);
        assert_eq!(cr, full_agg.cache_read_tokens);
        assert_eq!(calls, full_agg.api_call_count);

        // Also verify exact expected values
        assert_eq!(inp, 2100); // 100+200+300+400+500+600
        assert_eq!(out, 1050); // 50+100+150+200+250+300
        assert_eq!(cc, 360);   // 10+30+50+70+90+110
        assert_eq!(cr, 420);   // 20+40+60+80+100+120
        assert_eq!(calls, 6);
    }

    #[test]
    fn model_preserved_across_incremental_parses() {
        // First stop finds model in assistant messages.
        // Subsequent stops parse only new non-assistant lines (no model).
        // Existing model should be preserved.
        let dir = TempDir::new().unwrap();
        let transcript_path = dir.path().join("transcript.jsonl");
        let conn = test_conn();

        let line1 = assistant_line(100, 50);
        fs::write(&transcript_path, format!("{line1}\n")).unwrap();

        let json = format!(
            r#"{{"hook_event_name":"Stop","session_id":"s1","transcript_path":"{}"}}"#,
            transcript_path.display()
        );
        dispatch(Cursor::new(json.as_bytes()), &conn).unwrap();

        let model1: String = conn
            .query_row("SELECT model FROM token_usage WHERE session_id='s1'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(model1, "claude-sonnet-4-20250514");

        // Append a user line (no model info)
        let mut file = fs::OpenOptions::new().append(true).open(&transcript_path).unwrap();
        std::io::Write::write_all(&mut file, b"{\"type\":\"user\",\"message\":{}}\n").unwrap();
        drop(file);

        dispatch(Cursor::new(json.as_bytes()), &conn).unwrap();

        // Model should be preserved
        let model2: String = conn
            .query_row("SELECT model FROM token_usage WHERE session_id='s1'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(model2, "claude-sonnet-4-20250514");
    }

    #[test]
    fn three_consecutive_stops_offset_advances() {
        // Full cycle: 3 consecutive Stop events with growing transcript.
        // After each stop, verify DB has correct cumulative totals and offset advances.
        let dir = TempDir::new().unwrap();
        let transcript_path = dir.path().join("transcript.jsonl");
        let conn = test_conn();

        let json = format!(
            r#"{{"hook_event_name":"Stop","session_id":"s1","transcript_path":"{}"}}"#,
            transcript_path.display()
        );

        // Stop 1
        let line1 = assistant_line(100, 50);
        fs::write(&transcript_path, format!("{line1}\n")).unwrap();
        dispatch(Cursor::new(json.as_bytes()), &conn).unwrap();

        let (inp, out, calls, offset): (i64, i64, i64, i64) = conn
            .query_row(
                "SELECT input_tokens, output_tokens, api_call_count, last_transcript_offset FROM token_usage WHERE session_id='s1'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .unwrap();
        assert_eq!(inp, 100);
        assert_eq!(out, 50);
        assert_eq!(calls, 1);
        let offset1 = offset;
        assert!(offset1 > 0);

        // Stop 2
        let line2 = assistant_line(200, 100);
        let mut file = fs::OpenOptions::new().append(true).open(&transcript_path).unwrap();
        std::io::Write::write_all(&mut file, format!("{line2}\n").as_bytes()).unwrap();
        drop(file);
        dispatch(Cursor::new(json.as_bytes()), &conn).unwrap();

        let (inp, out, calls, offset): (i64, i64, i64, i64) = conn
            .query_row(
                "SELECT input_tokens, output_tokens, api_call_count, last_transcript_offset FROM token_usage WHERE session_id='s1'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .unwrap();
        assert_eq!(inp, 300);
        assert_eq!(out, 150);
        assert_eq!(calls, 2);
        let offset2 = offset;
        assert!(offset2 > offset1);

        // Stop 3
        let line3 = assistant_line(300, 150);
        let mut file = fs::OpenOptions::new().append(true).open(&transcript_path).unwrap();
        std::io::Write::write_all(&mut file, format!("{line3}\n").as_bytes()).unwrap();
        drop(file);
        dispatch(Cursor::new(json.as_bytes()), &conn).unwrap();

        let (inp, out, calls, offset): (i64, i64, i64, i64) = conn
            .query_row(
                "SELECT input_tokens, output_tokens, api_call_count, last_transcript_offset FROM token_usage WHERE session_id='s1'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .unwrap();
        assert_eq!(inp, 600);
        assert_eq!(out, 300);
        assert_eq!(calls, 3);
        assert!(offset > offset2);

        // Verify exactly one token_usage row
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM token_usage WHERE session_id='s1'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 1);
    }

    // --- Plan tracking tests ---

    #[test]
    fn dispatch_pre_tool_use_exit_plan_mode() {
        let conn = test_conn();
        let json = r#"{"hook_event_name":"PreToolUse","session_id":"s1","tool_name":"ExitPlanMode","tool_use_id":"toolu_plan1","tool_input":{"plan":"Build a REST API"},"cwd":"/proj"}"#;
        dispatch(Cursor::new(json), &conn).unwrap();

        // Should be in tool_uses
        let tool: String = conn
            .query_row("SELECT tool_name FROM tool_uses WHERE tool_use_id='toolu_plan1'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(tool, "ExitPlanMode");

        // Should also be in plans
        let (plan_text, accepted): (String, Option<i32>) = conn
            .query_row(
                "SELECT plan_text, accepted FROM plans WHERE tool_use_id='toolu_plan1'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(plan_text, "Build a REST API");
        assert!(accepted.is_none());
    }

    #[test]
    fn dispatch_pre_tool_use_exit_plan_mode_no_plan_field() {
        let conn = test_conn();
        let json = r#"{"hook_event_name":"PreToolUse","session_id":"s1","tool_name":"ExitPlanMode","tool_use_id":"toolu_plan1","tool_input":{},"cwd":"/proj"}"#;
        dispatch(Cursor::new(json), &conn).unwrap();

        let plan_text: String = conn
            .query_row("SELECT plan_text FROM plans WHERE tool_use_id='toolu_plan1'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(plan_text, "");
    }

    #[test]
    fn dispatch_pre_tool_use_non_plan_tool() {
        let conn = test_conn();
        let json = r#"{"hook_event_name":"PreToolUse","session_id":"s1","tool_name":"Read","tool_use_id":"tu1","tool_input":{"file_path":"/foo"},"cwd":"/proj"}"#;
        dispatch(Cursor::new(json), &conn).unwrap();

        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM plans", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn dispatch_stop_resolves_accepted_plan() {
        let dir = TempDir::new().unwrap();
        let transcript_path = dir.path().join("transcript.jsonl");
        let conn = test_conn();

        // Insert a pending plan
        db::insert_plan(&conn, "s1", "toolu_plan1", "ts1", "my plan").unwrap();

        // Write transcript with accepted tool_result and an assistant line for token parsing
        let transcript = format!(
            "{}\n{}\n",
            r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu_plan1","content":"User has approved your plan."}]}}"#,
            r#"{"type":"assistant","message":{"model":"m","usage":{"input_tokens":10,"output_tokens":5}}}"#,
        );
        fs::write(&transcript_path, &transcript).unwrap();

        let json = format!(
            r#"{{"hook_event_name":"Stop","session_id":"s1","transcript_path":"{}"}}"#,
            transcript_path.display()
        );
        dispatch(Cursor::new(json.as_bytes()), &conn).unwrap();

        let accepted: i32 = conn
            .query_row("SELECT accepted FROM plans WHERE tool_use_id='toolu_plan1'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(accepted, 1);
    }

    #[test]
    fn dispatch_stop_resolves_rejected_plan() {
        let dir = TempDir::new().unwrap();
        let transcript_path = dir.path().join("transcript.jsonl");
        let conn = test_conn();

        db::insert_plan(&conn, "s1", "toolu_plan1", "ts1", "my plan").unwrap();

        let transcript = format!(
            "{}\n{}\n",
            r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu_plan1","content":"The user doesn't want to proceed.","is_error":true}]}}"#,
            r#"{"type":"assistant","message":{"model":"m","usage":{"input_tokens":10,"output_tokens":5}}}"#,
        );
        fs::write(&transcript_path, &transcript).unwrap();

        let json = format!(
            r#"{{"hook_event_name":"Stop","session_id":"s1","transcript_path":"{}"}}"#,
            transcript_path.display()
        );
        dispatch(Cursor::new(json.as_bytes()), &conn).unwrap();

        let accepted: i32 = conn
            .query_row("SELECT accepted FROM plans WHERE tool_use_id='toolu_plan1'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(accepted, 0);
    }

    #[test]
    fn dispatch_stop_no_pending_plans() {
        let dir = TempDir::new().unwrap();
        let transcript_path = dir.path().join("transcript.jsonl");
        let transcript_content = r#"{"type":"assistant","message":{"model":"m","usage":{"input_tokens":10,"output_tokens":5}}}"#;
        fs::write(&transcript_path, format!("{transcript_content}\n")).unwrap();

        let conn = test_conn();
        let json = format!(
            r#"{{"hook_event_name":"Stop","session_id":"s1","transcript_path":"{}"}}"#,
            transcript_path.display()
        );
        dispatch(Cursor::new(json.as_bytes()), &conn).unwrap();

        // Token usage should still work
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM token_usage WHERE session_id='s1'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn dispatch_stop_multiple_plans_mixed() {
        let dir = TempDir::new().unwrap();
        let transcript_path = dir.path().join("transcript.jsonl");
        let conn = test_conn();

        db::insert_plan(&conn, "s1", "toolu_a", "ts1", "plan a").unwrap();
        db::insert_plan(&conn, "s1", "toolu_b", "ts2", "plan b").unwrap();

        let transcript = format!(
            "{}\n{}\n{}\n",
            r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu_a","content":"User has approved your plan."}]}}"#,
            r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu_b","content":"The user doesn't want to proceed.","is_error":true}]}}"#,
            r#"{"type":"assistant","message":{"model":"m","usage":{"input_tokens":10,"output_tokens":5}}}"#,
        );
        fs::write(&transcript_path, &transcript).unwrap();

        let json = format!(
            r#"{{"hook_event_name":"Stop","session_id":"s1","transcript_path":"{}"}}"#,
            transcript_path.display()
        );
        dispatch(Cursor::new(json.as_bytes()), &conn).unwrap();

        let accepted_a: i32 = conn
            .query_row("SELECT accepted FROM plans WHERE tool_use_id='toolu_a'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(accepted_a, 1);

        let accepted_b: i32 = conn
            .query_row("SELECT accepted FROM plans WHERE tool_use_id='toolu_b'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(accepted_b, 0);
    }

    // --- parse_plan_acceptances tests ---

    #[test]
    fn parse_plan_acceptances_accepted() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("transcript.jsonl");
        let content = r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu_plan1","content":"User has approved your plan."}]}}"#;
        fs::write(&path, format!("{content}\n")).unwrap();

        let results = parse_plan_acceptances(&path, &["toolu_plan1".to_string()]);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], ("toolu_plan1".to_string(), true));
    }

    #[test]
    fn parse_plan_acceptances_rejected() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("transcript.jsonl");
        let content = r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu_plan1","content":"The user doesn't want to proceed.","is_error":true}]}}"#;
        fs::write(&path, format!("{content}\n")).unwrap();

        let results = parse_plan_acceptances(&path, &["toolu_plan1".to_string()]);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], ("toolu_plan1".to_string(), false));
    }

    #[test]
    fn parse_plan_acceptances_mixed() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("transcript.jsonl");
        let content = format!(
            "{}\n{}\n",
            r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu_a","content":"User has approved your plan."}]}}"#,
            r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu_b","content":"The user doesn't want to proceed.","is_error":true}]}}"#,
        );
        fs::write(&path, content).unwrap();

        let results = parse_plan_acceptances(
            &path,
            &["toolu_a".to_string(), "toolu_b".to_string()],
        );
        assert_eq!(results.len(), 2);
        assert!(results.contains(&("toolu_a".to_string(), true)));
        assert!(results.contains(&("toolu_b".to_string(), false)));
    }

    #[test]
    fn parse_plan_acceptances_no_matching_ids() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("transcript.jsonl");
        let content = r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu_other","content":"User has approved your plan."}]}}"#;
        fs::write(&path, format!("{content}\n")).unwrap();

        let results = parse_plan_acceptances(&path, &["toolu_plan1".to_string()]);
        assert!(results.is_empty());
    }

    #[test]
    fn parse_plan_acceptances_missing_file() {
        let results = parse_plan_acceptances(
            Path::new("/nonexistent/path.jsonl"),
            &["toolu_plan1".to_string()],
        );
        assert!(results.is_empty());
    }

    #[test]
    fn parse_plan_acceptances_empty_file() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("transcript.jsonl");
        fs::write(&path, "").unwrap();

        let results = parse_plan_acceptances(&path, &["toolu_plan1".to_string()]);
        assert!(results.is_empty());
    }

    #[test]
    fn parse_plan_acceptances_string_content() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("transcript.jsonl");
        // User message with string content (not array) — should be skipped
        let content = r#"{"type":"user","message":{"role":"user","content":"hello world"}}"#;
        fs::write(&path, format!("{content}\n")).unwrap();

        let results = parse_plan_acceptances(&path, &["toolu_plan1".to_string()]);
        assert!(results.is_empty());
    }

    #[test]
    fn parse_plan_acceptances_empty_ids() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("transcript.jsonl");
        fs::write(&path, "anything\n").unwrap();

        let results = parse_plan_acceptances(&path, &[]);
        assert!(results.is_empty());
    }

    #[test]
    fn parse_plan_acceptances_skips_empty_and_invalid_lines() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("transcript.jsonl");
        let content = format!(
            "\n{}\n{}\n{}\n",
            "not json at all",
            r#"{"type":"user","message":{"role":"user","content":[{"type":"text","text":"hello"},{"type":"tool_result","tool_use_id":"toolu_plan1","content":"User has approved your plan."}]}}"#,
            "",
        );
        fs::write(&path, content).unwrap();

        let results = parse_plan_acceptances(&path, &["toolu_plan1".to_string()]);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], ("toolu_plan1".to_string(), true));
    }

    #[test]
    fn parse_plan_acceptances_skips_block_without_tool_use_id() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("transcript.jsonl");
        // tool_result block missing tool_use_id field
        let content = r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","content":"some content"}]}}"#;
        fs::write(&path, format!("{content}\n")).unwrap();

        let results = parse_plan_acceptances(&path, &["toolu_plan1".to_string()]);
        assert!(results.is_empty());
    }
}
