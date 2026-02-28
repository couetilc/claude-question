use std::fs;
use std::io::{self, BufRead, BufReader, Read};
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
        let agg = parse_transcript(Path::new(&path));
        db::insert_token_usage(
            conn,
            session_id,
            now,
            &agg.model,
            agg.input_tokens,
            agg.cache_creation_tokens,
            agg.cache_read_tokens,
            agg.output_tokens,
            agg.api_call_count,
        )?;
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

    db::insert_tool_use(
        conn,
        input.tool_use_id.as_deref().unwrap_or_default(),
        input.session_id.as_deref().unwrap_or_default(),
        input.tool_name.as_deref().unwrap_or_default(),
        now,
        input.cwd.as_deref().unwrap_or_default(),
        &input_json,
    )
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

/// Parse a transcript JSONL file and aggregate token usage.
pub fn parse_transcript(path: &Path) -> AggregatedTokenUsage {
    let file = match fs::File::open(path) {
        Ok(f) => f,
        Err(_) => return AggregatedTokenUsage::default(),
    };
    parse_transcript_reader(BufReader::new(file))
}

/// Parse transcript from any BufRead source.
pub fn parse_transcript_reader(reader: impl BufRead) -> AggregatedTokenUsage {
    let mut agg = AggregatedTokenUsage::default();

    for line in reader.lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => continue,
        };
        if line.is_empty() {
            continue;
        }
        let tl: TranscriptLine = match serde_json::from_str(&line) {
            Ok(t) => t,
            Err(_) => continue,
        };

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
                agg.cache_creation_tokens += usage.cache_creation_input_tokens.unwrap_or(0);
                agg.cache_read_tokens += usage.cache_read_input_tokens.unwrap_or(0);
                agg.api_call_count += 1;
            }
        }
    }

    agg
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

    #[test]
    fn parse_transcript_reader_skips_io_errors() {
        /// A reader that yields one valid line, then an IO error, then another valid line.
        struct FlakyReader {
            calls: u8,
        }

        impl std::io::Read for FlakyReader {
            fn read(&mut self, _buf: &mut [u8]) -> std::io::Result<usize> {
                unreachable!()
            }
        }

        impl std::io::BufRead for FlakyReader {
            fn fill_buf(&mut self) -> std::io::Result<&[u8]> {
                unreachable!()
            }
            fn consume(&mut self, _amt: usize) {}

            fn read_line(&mut self, buf: &mut String) -> std::io::Result<usize> {
                self.calls += 1;
                match self.calls {
                    1 => {
                        let line = r#"{"type":"assistant","message":{"model":"m","usage":{"input_tokens":10,"output_tokens":5}}}"#;
                        let l = format!("{line}\n");
                        buf.push_str(&l);
                        Ok(l.len())
                    }
                    2 => Err(std::io::Error::new(std::io::ErrorKind::Other, "disk error")),
                    3 => {
                        let line = r#"{"type":"assistant","message":{"model":"m","usage":{"input_tokens":20,"output_tokens":10}}}"#;
                        let l = format!("{line}\n");
                        buf.push_str(&l);
                        Ok(l.len())
                    }
                    _ => Ok(0),
                }
            }
        }

        let reader = FlakyReader { calls: 0 };
        let agg = parse_transcript_reader(reader);
        assert_eq!(agg.api_call_count, 2);
        assert_eq!(agg.input_tokens, 30);
        assert_eq!(agg.output_tokens, 15);
    }
}
