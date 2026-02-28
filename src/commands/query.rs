use rusqlite::Connection;

use crate::db;

/// Run an ad-hoc SQL query against the database.
#[cfg(not(tarpaulin_include))]
pub fn run(sql: &str) {
    if let Err(e) = try_run(sql) {
        eprintln!("claude-track query: {e}");
        std::process::exit(1);
    }
}

fn try_run(sql: &str) -> Result<(), Box<dyn std::error::Error>> {
    let db_path = db::db_path()?;
    let conn = db::open_db(&db_path)?;
    let output = execute_query(&conn, sql)?;
    print!("{output}");
    Ok(())
}

/// Execute a SQL query and return tab-separated results as a string.
pub fn execute_query(
    conn: &Connection,
    sql: &str,
) -> Result<String, Box<dyn std::error::Error>> {
    let trimmed = sql.trim();
    if trimmed.is_empty() {
        return Ok("No query provided.\n".to_string());
    }

    execute_query_on(conn, trimmed)
}

/// Run the query on an open connection, return formatted output.
pub fn execute_query_on(
    conn: &Connection,
    sql: &str,
) -> Result<String, Box<dyn std::error::Error>> {
    let mut stmt = conn.prepare(sql)?;
    let col_count = stmt.column_count();
    let col_names: Vec<String> = (0..col_count)
        .map(|i| stmt.column_name(i).unwrap_or("?").to_string())
        .collect();

    let mut out = String::new();
    out.push_str(&col_names.join("\t"));
    out.push('\n');

    let rows = stmt.query_map([], |row| {
        let mut vals = Vec::new();
        for i in 0..col_count {
            let val: String = row
                .get::<_, rusqlite::types::Value>(i)
                .map(|v| format_value(&v))
                .unwrap_or_else(|_| "NULL".to_string());
            vals.push(val);
        }
        Ok(vals)
    })?;

    for row in rows {
        let vals = row?;
        out.push_str(&vals.join("\t"));
        out.push('\n');
    }

    Ok(out)
}

fn format_value(v: &rusqlite::types::Value) -> String {
    match v {
        rusqlite::types::Value::Null => "NULL".to_string(),
        rusqlite::types::Value::Integer(i) => i.to_string(),
        rusqlite::types::Value::Real(f) => f.to_string(),
        rusqlite::types::Value::Text(s) => s.clone(),
        rusqlite::types::Value::Blob(b) => format!("<blob {} bytes>", b.len()),
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    fn test_conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        db::init_db(&conn).unwrap();
        conn
    }

    #[test]
    fn query_empty_sql() {
        let conn = test_conn();
        let output = execute_query(&conn, "").unwrap();
        assert!(output.contains("No query provided"));
    }

    #[test]
    fn query_whitespace_only() {
        let conn = test_conn();
        let output = execute_query(&conn, "   ").unwrap();
        assert!(output.contains("No query provided"));
    }

    #[test]
    fn query_select_from_empty_table() {
        let conn = test_conn();
        let output = execute_query(&conn, "SELECT * FROM sessions").unwrap();
        // Should have header row
        assert!(output.contains("session_id"));
        // Only the header line
        let lines: Vec<&str> = output.lines().collect();
        assert_eq!(lines.len(), 1);
    }

    #[test]
    fn query_select_with_data() {
        let conn = test_conn();
        db::insert_session_start(&conn, "s1", "ts1", "startup", "/proj", "/t").unwrap();
        db::insert_session_start(&conn, "s2", "ts2", "resume", "/proj2", "/t2").unwrap();

        let output = execute_query(&conn, "SELECT session_id, start_reason FROM sessions ORDER BY session_id").unwrap();
        let lines: Vec<&str> = output.lines().collect();
        assert_eq!(lines.len(), 3); // header + 2 rows
        assert!(lines[0].contains("session_id"));
        assert!(lines[1].contains("s1"));
        assert!(lines[1].contains("startup"));
        assert!(lines[2].contains("s2"));
    }

    #[test]
    fn query_count() {
        let conn = test_conn();
        db::insert_prompt(&conn, "s1", "ts", "hello").unwrap();
        db::insert_prompt(&conn, "s1", "ts2", "world").unwrap();

        let output = execute_query(&conn, "SELECT COUNT(*) as cnt FROM prompts").unwrap();
        assert!(output.contains("cnt"));
        assert!(output.contains("2"));
    }

    #[test]
    fn query_invalid_sql() {
        let conn = test_conn();
        let result = execute_query(&conn, "NOT VALID SQL");
        assert!(result.is_err());
    }

    #[test]
    fn query_null_values() {
        let conn = test_conn();
        db::insert_session_start(&conn, "s1", "ts", "startup", "/p", "/t").unwrap();
        let output = execute_query(&conn, "SELECT ended_at FROM sessions WHERE session_id='s1'").unwrap();
        assert!(output.contains("NULL"));
    }

    #[test]
    fn query_integer_values() {
        let conn = test_conn();
        db::insert_token_usage(&conn, "s1", "ts", "model", 100, 200, 300, 50, 3, 0).unwrap();
        let output = execute_query(&conn, "SELECT input_tokens FROM token_usage").unwrap();
        assert!(output.contains("100"));
    }

    #[test]
    fn format_value_types() {
        assert_eq!(format_value(&rusqlite::types::Value::Null), "NULL");
        assert_eq!(format_value(&rusqlite::types::Value::Integer(42)), "42");
        assert_eq!(format_value(&rusqlite::types::Value::Real(3.14)), "3.14");
        assert_eq!(
            format_value(&rusqlite::types::Value::Text("hello".to_string())),
            "hello"
        );
        assert_eq!(
            format_value(&rusqlite::types::Value::Blob(vec![1, 2, 3])),
            "<blob 3 bytes>"
        );
    }
}
