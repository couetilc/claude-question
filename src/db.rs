use std::path::{Path, PathBuf};

use rusqlite::{params, Connection};

/// Return the default database path: ~/.claude/claude-track.db
pub fn db_path() -> Result<PathBuf, Box<dyn std::error::Error>> {
    let home = dirs::home_dir().ok_or("could not determine home directory")?;
    Ok(home.join(".claude").join("claude-track.db"))
}

/// Open (or create) the SQLite database at the given path and initialize the schema.
pub fn open_db(path: &Path) -> Result<Connection, Box<dyn std::error::Error>> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let conn = Connection::open(path)?;
    conn.execute_batch("PRAGMA journal_mode=WAL;")?;
    init_db(&conn)?;
    Ok(conn)
}

/// Create all tables if they don't exist.
pub fn init_db(conn: &Connection) -> Result<(), Box<dyn std::error::Error>> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS sessions (
            session_id      TEXT PRIMARY KEY,
            started_at      TEXT,
            ended_at        TEXT,
            start_reason    TEXT,
            end_reason      TEXT,
            cwd             TEXT,
            transcript_path TEXT
        );

        CREATE TABLE IF NOT EXISTS tool_uses (
            id               INTEGER PRIMARY KEY AUTOINCREMENT,
            tool_use_id      TEXT,
            session_id       TEXT,
            tool_name        TEXT,
            timestamp        TEXT,
            cwd              TEXT,
            input            TEXT,
            response_summary TEXT
        );

        CREATE TABLE IF NOT EXISTS prompts (
            id          INTEGER PRIMARY KEY AUTOINCREMENT,
            session_id  TEXT,
            timestamp   TEXT,
            prompt_text TEXT
        );

        CREATE TABLE IF NOT EXISTS token_usage (
            id                      INTEGER PRIMARY KEY AUTOINCREMENT,
            session_id              TEXT,
            timestamp               TEXT,
            model                   TEXT,
            input_tokens            INTEGER DEFAULT 0,
            cache_creation_tokens   INTEGER DEFAULT 0,
            cache_read_tokens       INTEGER DEFAULT 0,
            output_tokens           INTEGER DEFAULT 0,
            api_call_count          INTEGER DEFAULT 0
        );",
    )?;
    Ok(())
}

/// Insert or update a session start record. Uses INSERT OR IGNORE so repeated starts
/// for the same session_id don't fail.
pub fn insert_session_start(
    conn: &Connection,
    session_id: &str,
    started_at: &str,
    start_reason: &str,
    cwd: &str,
    transcript_path: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    conn.execute(
        "INSERT OR IGNORE INTO sessions (session_id, started_at, start_reason, cwd, transcript_path)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![session_id, started_at, start_reason, cwd, transcript_path],
    )?;
    Ok(())
}

/// Update the session row with end data.
pub fn update_session_end(
    conn: &Connection,
    session_id: &str,
    ended_at: &str,
    end_reason: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let rows = conn.execute(
        "UPDATE sessions SET ended_at = ?1, end_reason = ?2 WHERE session_id = ?3",
        params![ended_at, end_reason, session_id],
    )?;
    // If no session row exists yet (e.g. SessionStart wasn't captured), create one
    if rows == 0 {
        conn.execute(
            "INSERT INTO sessions (session_id, ended_at, end_reason) VALUES (?1, ?2, ?3)",
            params![session_id, ended_at, end_reason],
        )?;
    }
    Ok(())
}

/// Insert a tool use record (from PreToolUse).
pub fn insert_tool_use(
    conn: &Connection,
    tool_use_id: &str,
    session_id: &str,
    tool_name: &str,
    timestamp: &str,
    cwd: &str,
    input: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    conn.execute(
        "INSERT INTO tool_uses (tool_use_id, session_id, tool_name, timestamp, cwd, input)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![tool_use_id, session_id, tool_name, timestamp, cwd, input],
    )?;
    Ok(())
}

/// Update an existing tool use with response_summary (from PostToolUse).
/// If no matching row exists, inserts a new one.
pub fn update_tool_use_response(
    conn: &Connection,
    tool_use_id: &str,
    session_id: &str,
    tool_name: &str,
    timestamp: &str,
    cwd: &str,
    input: &str,
    response_summary: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let rows = conn.execute(
        "UPDATE tool_uses SET response_summary = ?1 WHERE tool_use_id = ?2",
        params![response_summary, tool_use_id],
    )?;
    if rows == 0 {
        conn.execute(
            "INSERT INTO tool_uses (tool_use_id, session_id, tool_name, timestamp, cwd, input, response_summary)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![tool_use_id, session_id, tool_name, timestamp, cwd, input, response_summary],
        )?;
    }
    Ok(())
}

/// Insert a prompt record.
pub fn insert_prompt(
    conn: &Connection,
    session_id: &str,
    timestamp: &str,
    prompt_text: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    conn.execute(
        "INSERT INTO prompts (session_id, timestamp, prompt_text) VALUES (?1, ?2, ?3)",
        params![session_id, timestamp, prompt_text],
    )?;
    Ok(())
}

/// Insert a token usage record.
pub fn insert_token_usage(
    conn: &Connection,
    session_id: &str,
    timestamp: &str,
    model: &str,
    input_tokens: i64,
    cache_creation_tokens: i64,
    cache_read_tokens: i64,
    output_tokens: i64,
    api_call_count: i64,
) -> Result<(), Box<dyn std::error::Error>> {
    conn.execute(
        "INSERT INTO token_usage (session_id, timestamp, model, input_tokens, cache_creation_tokens, cache_read_tokens, output_tokens, api_call_count)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            session_id,
            timestamp,
            model,
            input_tokens,
            cache_creation_tokens,
            cache_read_tokens,
            output_tokens,
            api_call_count,
        ],
    )?;
    Ok(())
}

/// Insert a migrated tool use (from legacy JSONL, no tool_use_id).
pub fn insert_migrated_tool_use(
    conn: &Connection,
    session_id: &str,
    tool_name: &str,
    timestamp: &str,
    cwd: &str,
    input: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    conn.execute(
        "INSERT INTO tool_uses (session_id, tool_name, timestamp, cwd, input)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![session_id, tool_name, timestamp, cwd, input],
    )?;
    Ok(())
}

/// Delete duplicate token_usage rows, keeping only the one with the lowest id
/// per duplicate group. Returns the number of rows deleted.
pub fn dedup_token_usage(conn: &Connection) -> Result<usize, Box<dyn std::error::Error>> {
    let deleted = conn.execute(
        "DELETE FROM token_usage WHERE id NOT IN (
            SELECT MIN(id) FROM token_usage
            GROUP BY session_id, timestamp, model, input_tokens,
                     cache_creation_tokens, cache_read_tokens,
                     output_tokens, api_call_count
        )",
        [],
    )?;
    Ok(deleted)
}

/// Get the transcript_path for a given session.
pub fn get_transcript_path(
    conn: &Connection,
    session_id: &str,
) -> Result<Option<String>, Box<dyn std::error::Error>> {
    let mut stmt =
        conn.prepare("SELECT transcript_path FROM sessions WHERE session_id = ?1")?;
    let result = stmt
        .query_row(params![session_id], |row| row.get::<_, Option<String>>(0))
        .ok()
        .flatten();
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mem_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        init_db(&conn).unwrap();
        conn
    }

    #[test]
    fn init_db_creates_tables() {
        let conn = mem_db();
        // Verify all four tables exist
        let tables: Vec<String> = conn
            .prepare("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();
        assert!(tables.contains(&"sessions".to_string()));
        assert!(tables.contains(&"tool_uses".to_string()));
        assert!(tables.contains(&"prompts".to_string()));
        assert!(tables.contains(&"token_usage".to_string()));
    }

    #[test]
    fn init_db_idempotent() {
        let conn = mem_db();
        // Calling init again should not error
        init_db(&conn).unwrap();
    }

    #[test]
    fn session_start_and_end() {
        let conn = mem_db();
        insert_session_start(&conn, "s1", "2026-02-27T00:00:00Z", "startup", "/proj", "/tmp/t.jsonl").unwrap();

        let (started, reason, cwd, tp): (String, String, String, String) = conn
            .query_row(
                "SELECT started_at, start_reason, cwd, transcript_path FROM sessions WHERE session_id='s1'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .unwrap();
        assert_eq!(started, "2026-02-27T00:00:00Z");
        assert_eq!(reason, "startup");
        assert_eq!(cwd, "/proj");
        assert_eq!(tp, "/tmp/t.jsonl");

        update_session_end(&conn, "s1", "2026-02-27T01:00:00Z", "logout").unwrap();
        let (ended, end_reason): (String, String) = conn
            .query_row(
                "SELECT ended_at, end_reason FROM sessions WHERE session_id='s1'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(ended, "2026-02-27T01:00:00Z");
        assert_eq!(end_reason, "logout");
    }

    #[test]
    fn session_start_ignore_duplicate() {
        let conn = mem_db();
        insert_session_start(&conn, "s1", "t1", "startup", "/a", "/t1").unwrap();
        insert_session_start(&conn, "s1", "t2", "resume", "/b", "/t2").unwrap();
        // Should keep first insert
        let started: String = conn
            .query_row("SELECT started_at FROM sessions WHERE session_id='s1'", [], |row| row.get(0))
            .unwrap();
        assert_eq!(started, "t1");
    }

    #[test]
    fn session_end_without_start() {
        let conn = mem_db();
        update_session_end(&conn, "s_new", "2026-02-27T01:00:00Z", "logout").unwrap();
        let end_reason: String = conn
            .query_row("SELECT end_reason FROM sessions WHERE session_id='s_new'", [], |row| row.get(0))
            .unwrap();
        assert_eq!(end_reason, "logout");
    }

    #[test]
    fn tool_use_insert_and_update() {
        let conn = mem_db();
        insert_tool_use(&conn, "tu1", "s1", "Read", "ts1", "/proj", r#"{"file_path":"/foo"}"#).unwrap();

        let (tool, input): (String, String) = conn
            .query_row("SELECT tool_name, input FROM tool_uses WHERE tool_use_id='tu1'", [], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })
            .unwrap();
        assert_eq!(tool, "Read");
        assert!(input.contains("file_path"));

        update_tool_use_response(&conn, "tu1", "s1", "Read", "ts1", "/proj", "{}", "ok").unwrap();
        let resp: String = conn
            .query_row("SELECT response_summary FROM tool_uses WHERE tool_use_id='tu1'", [], |row| row.get(0))
            .unwrap();
        assert_eq!(resp, "ok");
    }

    #[test]
    fn tool_use_update_without_pre() {
        let conn = mem_db();
        // PostToolUse without matching PreToolUse — should insert new row
        update_tool_use_response(&conn, "tu2", "s1", "Bash", "ts2", "/proj", r#"{"cmd":"ls"}"#, "output").unwrap();
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM tool_uses WHERE tool_use_id='tu2'", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn prompt_insert() {
        let conn = mem_db();
        insert_prompt(&conn, "s1", "ts1", "hello world").unwrap();
        let text: String = conn
            .query_row("SELECT prompt_text FROM prompts WHERE session_id='s1'", [], |row| row.get(0))
            .unwrap();
        assert_eq!(text, "hello world");
    }

    #[test]
    fn token_usage_insert() {
        let conn = mem_db();
        insert_token_usage(&conn, "s1", "ts1", "claude-sonnet-4-20250514", 100, 200, 300, 50, 1).unwrap();
        let (model, inp, cc, cr, out, calls): (String, i64, i64, i64, i64, i64) = conn
            .query_row(
                "SELECT model, input_tokens, cache_creation_tokens, cache_read_tokens, output_tokens, api_call_count FROM token_usage WHERE session_id='s1'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?, row.get(5)?)),
            )
            .unwrap();
        assert_eq!(model, "claude-sonnet-4-20250514");
        assert_eq!(inp, 100);
        assert_eq!(cc, 200);
        assert_eq!(cr, 300);
        assert_eq!(out, 50);
        assert_eq!(calls, 1);
    }

    #[test]
    fn migrated_tool_use_insert() {
        let conn = mem_db();
        insert_migrated_tool_use(&conn, "s1", "Read", "ts1", "/proj", "{}").unwrap();
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM tool_uses WHERE session_id='s1'", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 1);
        // tool_use_id should be null
        let tuid: Option<String> = conn
            .query_row("SELECT tool_use_id FROM tool_uses WHERE session_id='s1'", [], |row| row.get(0))
            .unwrap();
        assert!(tuid.is_none());
    }

    #[test]
    fn get_transcript_path_found() {
        let conn = mem_db();
        insert_session_start(&conn, "s1", "ts", "startup", "/proj", "/tmp/t.jsonl").unwrap();
        let path = get_transcript_path(&conn, "s1").unwrap();
        assert_eq!(path.unwrap(), "/tmp/t.jsonl");
    }

    #[test]
    fn get_transcript_path_not_found() {
        let conn = mem_db();
        let path = get_transcript_path(&conn, "no_such_session").unwrap();
        assert!(path.is_none());
    }

    #[test]
    fn dedup_token_usage_removes_duplicates() {
        let conn = mem_db();
        // Insert 3 identical rows
        for _ in 0..3 {
            insert_token_usage(&conn, "s1", "ts1", "claude-sonnet-4-20250514", 100, 200, 300, 50, 1).unwrap();
        }
        let count_before: i64 = conn
            .query_row("SELECT COUNT(*) FROM token_usage", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count_before, 3);

        let removed = dedup_token_usage(&conn).unwrap();
        assert_eq!(removed, 2);

        let count_after: i64 = conn
            .query_row("SELECT COUNT(*) FROM token_usage", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count_after, 1);
    }

    #[test]
    fn dedup_token_usage_keeps_distinct_rows() {
        let conn = mem_db();
        insert_token_usage(&conn, "s1", "ts1", "claude-sonnet-4-20250514", 100, 200, 300, 50, 1).unwrap();
        insert_token_usage(&conn, "s2", "ts2", "claude-opus-4-20250514", 500, 0, 0, 200, 3).unwrap();

        let removed = dedup_token_usage(&conn).unwrap();
        assert_eq!(removed, 0);

        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM token_usage", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 2);
    }

    #[test]
    fn open_db_creates_file() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("nested").join("test.db");
        let conn = open_db(&path).unwrap();
        // Verify tables exist
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name IN ('sessions','tool_uses','prompts','token_usage')",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 4);
        assert!(path.exists());
    }

    #[test]
    fn db_path_returns_expected() {
        let path = db_path().unwrap();
        assert!(path.ends_with(".claude/claude-track.db"));
    }
}
