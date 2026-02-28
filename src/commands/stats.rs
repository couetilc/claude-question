use std::fmt;
use std::path::Path;

use rusqlite::Connection;

use crate::db;

/// Print usage statistics from the SQLite database.
#[cfg(not(tarpaulin_include))]
pub fn run() {
    if let Err(e) = try_run() {
        eprintln!("claude-track stats: {e}");
    }
}

fn try_run() -> Result<(), Box<dyn std::error::Error>> {
    let db_path = db::db_path()?;
    print!("{}", run_with_path(&db_path)?);
    Ok(())
}

/// Generate the stats report for the given DB path.
pub fn run_with_path(db_path: &Path) -> Result<String, Box<dyn std::error::Error>> {
    if !db_path.exists() {
        return Ok("No tracking data yet. Run `claude-track install` to start tracking.\n".to_string());
    }

    let file_size = std::fs::metadata(db_path)?.len();
    let conn = db::open_db(db_path)?;

    Ok(format_report(&conn, file_size, db_path))
}

/// Build the full stats report from the database.
pub fn format_report(conn: &Connection, file_size: u64, db_path: &Path) -> String {
    let mut out = String::new();

    fmt::write(&mut out, format_args!("=== Claude Code Usage Stats ===\n")).unwrap();
    fmt::write(
        &mut out,
        format_args!("Database: {} ({})\n", db_path.display(), human_size(file_size)),
    )
    .unwrap();

    if let Ok(Some(since)) = tracking_since(conn) {
        fmt::write(&mut out, format_args!("Tracking since: {since}\n")).unwrap();
    }
    out.push('\n');

    // --- Sessions ---
    out.push_str(&format_sessions_section(conn));

    // --- Token Usage ---
    out.push_str(&format_tokens_section(conn));

    // --- Prompts ---
    out.push_str(&format_prompts_section(conn));

    // --- Tool Usage ---
    out.push_str(&format_tool_usage_section(conn));

    // --- Top 10 Files Read ---
    out.push_str(&format_top_files_section(conn));

    // --- Top 10 Bash Commands ---
    out.push_str(&format_top_bash_section(conn));

    // --- Activity by Date ---
    out.push_str(&format_activity_by_date_section(conn));

    // --- By Project ---
    out.push_str(&format_by_project_section(conn));

    out
}

fn tracking_since(conn: &Connection) -> Result<Option<String>, rusqlite::Error> {
    conn.query_row(
        "SELECT MIN(COALESCE(started_at, ended_at)) FROM sessions",
        [],
        |r| r.get(0),
    )
}

fn format_sessions_section(conn: &Connection) -> String {
    let mut out = String::new();
    out.push_str("--- Sessions ---\n");

    let total: i64 = conn
        .query_row("SELECT COUNT(*) FROM sessions", [], |r| r.get(0))
        .unwrap_or(0);
    fmt::write(&mut out, format_args!("  Total sessions: {total}\n")).unwrap();

    // Total duration: sum of (ended_at - started_at) for completed sessions
    let total_seconds: i64 = conn
        .query_row(
            "SELECT COALESCE(SUM(
                CAST((julianday(ended_at) - julianday(started_at)) * 86400 AS INTEGER)
            ), 0) FROM sessions WHERE ended_at IS NOT NULL AND started_at IS NOT NULL",
            [],
            |r| r.get(0),
        )
        .unwrap_or(0);
    fmt::write(
        &mut out,
        format_args!("  Total duration: {}\n", format_duration(total_seconds)),
    )
    .unwrap();

    let completed: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sessions WHERE ended_at IS NOT NULL AND started_at IS NOT NULL",
            [],
            |r| r.get(0),
        )
        .unwrap_or(0);
    if completed > 0 {
        let avg = total_seconds / completed;
        fmt::write(
            &mut out,
            format_args!("  Avg session: {}\n", format_duration(avg)),
        )
        .unwrap();
    }

    let today: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sessions WHERE started_at LIKE date('now') || '%'",
            [],
            |r| r.get(0),
        )
        .unwrap_or(0);
    fmt::write(&mut out, format_args!("  Sessions today: {today}\n")).unwrap();

    out.push('\n');
    out
}

fn format_tokens_section(conn: &Connection) -> String {
    let mut out = String::new();
    out.push_str("--- Token Usage ---\n");

    let (input_tokens, cache_creation, cache_read, output_tokens, api_calls): (
        i64,
        i64,
        i64,
        i64,
        i64,
    ) = conn
        .query_row(
            "SELECT
                COALESCE(SUM(input_tokens), 0),
                COALESCE(SUM(cache_creation_tokens), 0),
                COALESCE(SUM(cache_read_tokens), 0),
                COALESCE(SUM(output_tokens), 0),
                COALESCE(SUM(api_call_count), 0)
            FROM token_usage",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )
        .unwrap_or((0, 0, 0, 0, 0));

    fmt::write(
        &mut out,
        format_args!("  Input tokens:        {:>12}\n", format_number(input_tokens)),
    )
    .unwrap();
    fmt::write(
        &mut out,
        format_args!("  Cache creation:      {:>12}\n", format_number(cache_creation)),
    )
    .unwrap();
    fmt::write(
        &mut out,
        format_args!("  Cache reads:         {:>12}\n", format_number(cache_read)),
    )
    .unwrap();
    fmt::write(
        &mut out,
        format_args!("  Output tokens:       {:>12}\n", format_number(output_tokens)),
    )
    .unwrap();
    fmt::write(
        &mut out,
        format_args!("  API calls:           {:>12}\n", format_number(api_calls)),
    )
    .unwrap();

    let total_cache_eligible = cache_creation + cache_read;
    if total_cache_eligible > 0 {
        let hit_rate = (cache_read as f64 / total_cache_eligible as f64) * 100.0;
        fmt::write(
            &mut out,
            format_args!("  Cache hit rate:      {:>11.1}%\n", hit_rate),
        )
        .unwrap();
    }

    // Approximate cost using Claude Sonnet 4 pricing
    let cost = estimate_cost(input_tokens, cache_creation, cache_read, output_tokens);
    fmt::write(
        &mut out,
        format_args!("  Est. cost (approx):  {:>11}\n", format_cost(cost)),
    )
    .unwrap();

    out.push('\n');
    out
}

fn format_prompts_section(conn: &Connection) -> String {
    let mut out = String::new();
    out.push_str("--- Prompts ---\n");

    let total: i64 = conn
        .query_row("SELECT COUNT(*) FROM prompts", [], |r| r.get(0))
        .unwrap_or(0);
    fmt::write(&mut out, format_args!("  Total prompts: {total}\n")).unwrap();

    let session_count: i64 = conn
        .query_row(
            "SELECT COUNT(DISTINCT session_id) FROM prompts",
            [],
            |r| r.get(0),
        )
        .unwrap_or(0);
    if session_count > 0 {
        let avg_per_session = total as f64 / session_count as f64;
        fmt::write(
            &mut out,
            format_args!("  Avg per session: {avg_per_session:.1}\n"),
        )
        .unwrap();
    }

    let avg_length: f64 = conn
        .query_row(
            "SELECT COALESCE(AVG(LENGTH(prompt_text)), 0) FROM prompts",
            [],
            |r| r.get(0),
        )
        .unwrap_or(0.0);
    fmt::write(
        &mut out,
        format_args!("  Avg length: {} chars\n", avg_length as i64),
    )
    .unwrap();

    out.push('\n');
    out
}

fn format_tool_usage_section(conn: &Connection) -> String {
    let mut out = String::new();
    out.push_str("--- Tool Usage ---\n");

    let total: i64 = conn
        .query_row("SELECT COUNT(*) FROM tool_uses", [], |r| r.get(0))
        .unwrap_or(0);
    fmt::write(&mut out, format_args!("  Total tool calls: {total}\n")).unwrap();

    let mut stmt = conn
        .prepare(
            "SELECT tool_name, COUNT(*) as cnt FROM tool_uses
             GROUP BY tool_name ORDER BY cnt DESC",
        )
        .unwrap();
    let rows: Vec<(String, i64)> = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .filter_map(|r| r.ok())
        .collect();
    for (tool, count) in &rows {
        fmt::write(&mut out, format_args!("  {count:<6} {tool}\n")).unwrap();
    }

    out.push('\n');
    out
}

fn format_top_files_section(conn: &Connection) -> String {
    let mut out = String::new();
    out.push_str("--- Top 10 Files Read ---\n");

    let mut stmt = conn
        .prepare(
            "SELECT json_extract(input, '$.file_path') as fp, COUNT(*) as cnt
             FROM tool_uses WHERE tool_name = 'Read' AND fp IS NOT NULL
             GROUP BY fp ORDER BY cnt DESC LIMIT 10",
        )
        .unwrap();
    let rows: Vec<(String, i64)> = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .filter_map(|r| r.ok())
        .collect();
    for (path, count) in &rows {
        fmt::write(&mut out, format_args!("  {count:<4} {path}\n")).unwrap();
    }

    out.push('\n');
    out
}

fn format_top_bash_section(conn: &Connection) -> String {
    let mut out = String::new();
    out.push_str("--- Top 10 Bash Commands ---\n");

    // Extract first word of bash commands from JSON input
    let mut stmt = conn
        .prepare(
            "SELECT json_extract(input, '$.command') as cmd FROM tool_uses
             WHERE tool_name = 'Bash' AND cmd IS NOT NULL",
        )
        .unwrap();
    let commands: Vec<String> = stmt
        .query_map([], |r| r.get::<_, String>(0))
        .unwrap()
        .filter_map(|r| r.ok())
        .collect();

    let mut cmd_counts = std::collections::HashMap::new();
    for cmd in &commands {
        if let Some(first_word) = cmd.split_whitespace().next() {
            if first_word
                .chars()
                .all(|c| c.is_alphanumeric() || "_./-".contains(c))
            {
                *cmd_counts.entry(first_word.to_string()).or_insert(0i64) += 1;
            }
        }
    }

    let mut sorted: Vec<_> = cmd_counts.into_iter().collect();
    sorted.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    for (cmd, count) in sorted.iter().take(10) {
        fmt::write(&mut out, format_args!("  {count:<4} {cmd}\n")).unwrap();
    }

    out.push('\n');
    out
}

fn format_activity_by_date_section(conn: &Connection) -> String {
    let mut out = String::new();
    out.push_str("--- Activity by Date ---\n");

    let mut stmt = conn
        .prepare(
            "SELECT SUBSTR(timestamp, 1, 10) as dt, COUNT(*) as cnt
             FROM tool_uses WHERE dt IS NOT NULL
             GROUP BY dt ORDER BY dt",
        )
        .unwrap();
    let rows: Vec<(String, i64)> = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .filter_map(|r| r.ok())
        .collect();
    for (date, count) in &rows {
        fmt::write(&mut out, format_args!("  {date}  {count}\n")).unwrap();
    }

    out.push('\n');
    out
}

fn format_by_project_section(conn: &Connection) -> String {
    let mut out = String::new();
    out.push_str("--- By Project ---\n");

    let mut stmt = conn
        .prepare(
            "SELECT cwd, COUNT(*) as cnt FROM tool_uses
             WHERE cwd IS NOT NULL AND cwd != ''
             GROUP BY cwd ORDER BY cnt DESC",
        )
        .unwrap();
    let rows: Vec<(String, i64)> = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .filter_map(|r| r.ok())
        .collect();
    for (project, count) in &rows {
        fmt::write(&mut out, format_args!("  {count:<6} {project}\n")).unwrap();
    }

    out
}

/// Format seconds into a human-readable duration string.
pub fn format_duration(seconds: i64) -> String {
    if seconds < 60 {
        return format!("{seconds}s");
    }
    let hours = seconds / 3600;
    let minutes = (seconds % 3600) / 60;
    if hours > 0 {
        format!("{hours}h {minutes}m")
    } else {
        format!("{minutes}m")
    }
}

/// Format a byte count as a human-readable string.
pub fn human_size(bytes: u64) -> String {
    const GB: u64 = 1_073_741_824;
    const MB: u64 = 1_048_576;
    const KB: u64 = 1_024;
    if bytes >= GB {
        format!("{:.1} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.1} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.1} KB", bytes as f64 / KB as f64)
    } else {
        format!("{bytes} B")
    }
}

/// Format an integer with comma separators.
pub fn format_number(n: i64) -> String {
    let s = n.to_string();
    let bytes = s.as_bytes();
    let mut result = String::new();
    for (i, &b) in bytes.iter().enumerate() {
        if i > 0 && (bytes.len() - i) % 3 == 0 {
            result.push(',');
        }
        result.push(b as char);
    }
    result
}

/// Format a cost in dollars.
pub fn format_cost(cost: f64) -> String {
    format!("${cost:.2}")
}

/// Estimate cost using approximate Claude Sonnet 4 pricing.
/// Input: $3/MTok, Cache creation: $3.75/MTok, Cache read: $0.30/MTok, Output: $15/MTok
pub fn estimate_cost(input: i64, cache_creation: i64, cache_read: i64, output: i64) -> f64 {
    (input as f64 * 3.0 / 1_000_000.0)
        + (cache_creation as f64 * 3.75 / 1_000_000.0)
        + (cache_read as f64 * 0.30 / 1_000_000.0)
        + (output as f64 * 15.0 / 1_000_000.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn test_conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        db::init_db(&conn).unwrap();
        conn
    }

    #[test]
    fn human_size_bytes() {
        assert_eq!(human_size(0), "0 B");
        assert_eq!(human_size(512), "512 B");
        assert_eq!(human_size(1023), "1023 B");
    }

    #[test]
    fn human_size_kb() {
        assert_eq!(human_size(1024), "1.0 KB");
        assert_eq!(human_size(1536), "1.5 KB");
    }

    #[test]
    fn human_size_mb() {
        assert_eq!(human_size(1_048_576), "1.0 MB");
        assert_eq!(human_size(2_621_440), "2.5 MB");
    }

    #[test]
    fn human_size_gb() {
        assert_eq!(human_size(1_073_741_824), "1.0 GB");
        assert_eq!(human_size(3_221_225_472), "3.0 GB");
    }

    #[test]
    fn format_duration_seconds() {
        assert_eq!(format_duration(0), "0s");
        assert_eq!(format_duration(30), "30s");
        assert_eq!(format_duration(59), "59s");
    }

    #[test]
    fn format_duration_minutes() {
        assert_eq!(format_duration(60), "1m");
        assert_eq!(format_duration(90), "1m");
        assert_eq!(format_duration(3599), "59m");
    }

    #[test]
    fn format_duration_hours() {
        assert_eq!(format_duration(3600), "1h 0m");
        assert_eq!(format_duration(5400), "1h 30m");
        assert_eq!(format_duration(7200), "2h 0m");
    }

    #[test]
    fn format_number_small() {
        assert_eq!(format_number(0), "0");
        assert_eq!(format_number(42), "42");
        assert_eq!(format_number(999), "999");
    }

    #[test]
    fn format_number_thousands() {
        assert_eq!(format_number(1000), "1,000");
        assert_eq!(format_number(1234567), "1,234,567");
    }

    #[test]
    fn format_cost_values() {
        assert_eq!(format_cost(0.0), "$0.00");
        assert_eq!(format_cost(12.345), "$12.35");
    }

    #[test]
    fn estimate_cost_basic() {
        let cost = estimate_cost(1_000_000, 0, 0, 0);
        assert!((cost - 3.0).abs() < 0.01);
    }

    #[test]
    fn estimate_cost_all_components() {
        let cost = estimate_cost(1_000_000, 1_000_000, 1_000_000, 1_000_000);
        let expected = 3.0 + 3.75 + 0.30 + 15.0;
        assert!((cost - expected).abs() < 0.01);
    }

    #[test]
    fn run_with_path_missing_db() {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("nonexistent.db");
        let output = run_with_path(&db_path).unwrap();
        assert!(output.contains("No tracking data yet"));
    }

    #[test]
    fn format_report_empty_db() {
        let conn = test_conn();
        let report = format_report(&conn, 1024, Path::new("/test.db"));

        assert!(report.contains("=== Claude Code Usage Stats ==="));
        assert!(report.contains("/test.db"));
        assert!(report.contains("1.0 KB"));
        assert!(report.contains("--- Sessions ---"));
        assert!(report.contains("Total sessions: 0"));
        assert!(report.contains("--- Token Usage ---"));
        assert!(report.contains("--- Prompts ---"));
        assert!(report.contains("Total prompts: 0"));
        assert!(report.contains("--- Tool Usage ---"));
        assert!(report.contains("Total tool calls: 0"));
        assert!(report.contains("--- Top 10 Files Read ---"));
        assert!(report.contains("--- Top 10 Bash Commands ---"));
        assert!(report.contains("--- Activity by Date ---"));
        assert!(report.contains("--- By Project ---"));
    }

    #[test]
    fn format_report_with_data() {
        let conn = test_conn();

        // Add session
        db::insert_session_start(&conn, "s1", "2026-02-27T00:00:00Z", "startup", "/proj", "/t").unwrap();
        db::update_session_end(&conn, "s1", "2026-02-27T01:00:00Z", "logout").unwrap();

        // Add tool uses
        db::insert_tool_use(
            &conn,
            "tu1",
            "s1",
            "Read",
            "2026-02-27T00:05:00Z",
            "/proj",
            r#"{"file_path":"/src/main.rs"}"#,
        )
        .unwrap();
        db::insert_tool_use(
            &conn,
            "tu2",
            "s1",
            "Bash",
            "2026-02-27T00:10:00Z",
            "/proj",
            r#"{"command":"cargo build"}"#,
        )
        .unwrap();

        // Add prompt
        db::insert_prompt(&conn, "s1", "2026-02-27T00:00:00Z", "fix the bug please").unwrap();

        // Add token usage
        db::insert_token_usage(
            &conn,
            "s1",
            "2026-02-27T00:30:00Z",
            "claude-sonnet-4-20250514",
            1000,
            2000,
            3000,
            500,
            5,
        )
        .unwrap();

        let report = format_report(&conn, 2048, Path::new("/test.db"));

        assert!(report.contains("Total sessions: 1"));
        assert!(report.contains("Tracking since:"));
        assert!(report.contains("Total tool calls: 2"));
        assert!(report.contains("Read"));
        assert!(report.contains("Bash"));
        assert!(report.contains("/src/main.rs"));
        assert!(report.contains("cargo"));
        assert!(report.contains("Total prompts: 1"));
        assert!(report.contains("Avg per session:"));
        assert!(report.contains("Avg length:"));
        assert!(report.contains("Input tokens:"));
        assert!(report.contains("Cache hit rate:"));
        assert!(report.contains("Est. cost"));
        assert!(report.contains("2026-02-27"));
        assert!(report.contains("/proj"));
    }

    #[test]
    fn format_report_sessions_without_end() {
        let conn = test_conn();
        db::insert_session_start(&conn, "s1", "2026-02-27T00:00:00Z", "startup", "/proj", "/t").unwrap();

        let report = format_report(&conn, 0, Path::new("/test.db"));
        assert!(report.contains("Total sessions: 1"));
        // No avg session since no completed sessions
        assert!(!report.contains("Avg session:"));
    }

    #[test]
    fn format_report_sessions_with_avg() {
        let conn = test_conn();
        db::insert_session_start(&conn, "s1", "2026-02-27T00:00:00Z", "startup", "/proj", "/t").unwrap();
        db::update_session_end(&conn, "s1", "2026-02-27T01:00:00Z", "logout").unwrap();

        let report = format_report(&conn, 0, Path::new("/test.db"));
        assert!(report.contains("Avg session:"));
    }

    #[test]
    fn format_report_no_cache_hit_rate_when_zero() {
        let conn = test_conn();
        let report = format_report(&conn, 0, Path::new("/test.db"));
        assert!(!report.contains("Cache hit rate:"));
    }

    #[test]
    fn format_report_prompts_no_avg_when_empty() {
        let conn = test_conn();
        let report = format_report(&conn, 0, Path::new("/test.db"));
        // Should show total 0 but not avg per session
        assert!(report.contains("Total prompts: 0"));
        assert!(!report.contains("Avg per session:"));
    }

    #[test]
    fn run_with_path_existing_db() {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("claude-track.db");
        let conn = db::open_db(&db_path).unwrap();
        db::insert_session_start(&conn, "s1", "ts", "startup", "/p", "/t").unwrap();
        drop(conn);

        let output = run_with_path(&db_path).unwrap();
        assert!(output.contains("Total sessions: 1"));
    }

    #[test]
    fn tracking_since_returns_earliest() {
        let conn = test_conn();
        db::insert_session_start(&conn, "s2", "2026-02-28T00:00:00Z", "startup", "/p", "/t").unwrap();
        db::insert_session_start(&conn, "s1", "2026-02-27T00:00:00Z", "startup", "/p", "/t").unwrap();
        let since = tracking_since(&conn).unwrap();
        assert_eq!(since.unwrap(), "2026-02-27T00:00:00Z");
    }

    #[test]
    fn tracking_since_empty() {
        let conn = test_conn();
        let since = tracking_since(&conn).unwrap();
        assert!(since.is_none());
    }

    #[test]
    fn format_top_bash_filters_special() {
        let conn = test_conn();
        db::insert_tool_use(
            &conn,
            "tu1",
            "s1",
            "Bash",
            "ts",
            "/p",
            r#"{"command":"echo hello && rm -rf /"}"#,
        )
        .unwrap();
        let section = format_top_bash_section(&conn);
        assert!(section.contains("echo"));
    }

    #[test]
    fn format_tool_usage_empty() {
        let conn = test_conn();
        let section = format_tool_usage_section(&conn);
        assert!(section.contains("Total tool calls: 0"));
    }

    #[test]
    fn format_by_project_skips_empty_cwd() {
        let conn = test_conn();
        db::insert_tool_use(&conn, "tu1", "s1", "Read", "ts", "", "{}").unwrap();
        let section = format_by_project_section(&conn);
        // Should not show empty cwd row
        let lines: Vec<&str> = section.lines().collect();
        assert_eq!(lines.len(), 1); // Just the header
    }
}
