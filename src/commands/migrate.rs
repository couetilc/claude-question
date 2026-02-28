use std::fs;
use std::io::{BufRead, BufReader};
use std::path::Path;

use rusqlite::Connection;

use crate::db;
use crate::models::ToolCall;

/// Migrate legacy JSONL data into SQLite.
#[cfg(not(tarpaulin_include))]
pub fn run() {
    if let Err(e) = try_run() {
        eprintln!("claude-track migrate: {e}");
        std::process::exit(1);
    }
}

fn try_run() -> Result<(), Box<dyn std::error::Error>> {
    let claude_dir = dirs::home_dir()
        .ok_or("could not determine home directory")?
        .join(".claude");

    let jsonl_path = claude_dir.join("tool-usage.jsonl");
    let db_path = claude_dir.join("claude-track.db");

    let conn = db::open_db(&db_path)?;
    let output = migrate_from(&jsonl_path, &conn)?;
    print!("{output}");
    Ok(())
}

/// Import records from a JSONL file into the tool_uses table.
/// Returns user-facing output.
pub fn migrate_from(
    jsonl_path: &Path,
    conn: &Connection,
) -> Result<String, Box<dyn std::error::Error>> {
    if !jsonl_path.exists() {
        return Ok(format!(
            "No JSONL file found at {}\nNothing to migrate.\n",
            jsonl_path.display()
        ));
    }

    let file = fs::File::open(jsonl_path)?;
    let (imported, skipped) = migrate_reader(BufReader::new(file), conn)?;

    let mut output = format!("Migrated {imported} tool-use records into SQLite.\n");
    if skipped > 0 {
        output.push_str(&format!("Skipped {skipped} invalid lines.\n"));
    }
    output.push_str(&format!("Source: {}\n", jsonl_path.display()));
    Ok(output)
}

/// Import records from any BufRead source. Returns (imported, skipped) counts.
pub fn migrate_reader(
    reader: impl BufRead,
    conn: &Connection,
) -> Result<(u64, u64), Box<dyn std::error::Error>> {
    let mut imported = 0u64;
    let mut skipped = 0u64;

    for line_result in reader.lines() {
        let line = if let Ok(l) = line_result {
            l
        } else {
            skipped += 1;
            continue;
        };
        if line.is_empty() {
            continue;
        }
        let record: ToolCall = if let Ok(r) = serde_json::from_str(&line) {
            r
        } else {
            skipped += 1;
            continue;
        };

        let input_json = serde_json::to_string(&record.input).unwrap_or_default();
        db::insert_migrated_tool_use(
            conn,
            &record.session,
            &record.tool,
            &record.ts,
            &record.cwd,
            &input_json,
        )?;
        imported += 1;
    }

    Ok((imported, skipped))
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
    fn migrate_no_file() {
        let conn = test_conn();
        let output = migrate_from(Path::new("/nonexistent.jsonl"), &conn).unwrap();
        assert!(output.contains("No JSONL file found"));
        assert!(output.contains("Nothing to migrate"));
    }

    #[test]
    fn migrate_valid_records() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("tool-usage.jsonl");
        let content = format!(
            "{}\n{}\n",
            r#"{"ts":"2026-02-27T00:00:00Z","tool":"Read","session":"s1","cwd":"/proj","input":{"file_path":"/foo"}}"#,
            r#"{"ts":"2026-02-27T01:00:00Z","tool":"Bash","session":"s1","cwd":"/proj","input":{"command":"ls"}}"#,
        );
        fs::write(&path, content).unwrap();

        let conn = test_conn();
        let output = migrate_from(&path, &conn).unwrap();
        assert!(output.contains("Migrated 2 tool-use records"));

        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM tool_uses", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 2);
    }

    #[test]
    fn migrate_skips_invalid_lines() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("tool-usage.jsonl");
        let content = format!(
            "{}\n{}\n{}\n",
            r#"{"ts":"2026-02-27T00:00:00Z","tool":"Read","session":"s1","cwd":"/proj","input":{}}"#,
            "not valid json",
            "",
        );
        fs::write(&path, content).unwrap();

        let conn = test_conn();
        let output = migrate_from(&path, &conn).unwrap();
        assert!(output.contains("Migrated 1 tool-use records"));
        assert!(output.contains("Skipped 1 invalid"));
    }

    #[test]
    fn migrate_empty_file() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("tool-usage.jsonl");
        fs::write(&path, "").unwrap();

        let conn = test_conn();
        let output = migrate_from(&path, &conn).unwrap();
        assert!(output.contains("Migrated 0 tool-use records"));
    }

    #[test]
    fn migrate_shows_source_path() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("tool-usage.jsonl");
        fs::write(&path, "").unwrap();

        let conn = test_conn();
        let output = migrate_from(&path, &conn).unwrap();
        assert!(output.contains(&path.display().to_string()));
    }

    #[test]
    fn migrate_reader_skips_io_errors() {
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
                        let line = r#"{"ts":"2026-02-27T00:00:00Z","tool":"Read","session":"s1","cwd":"/proj","input":{}}"#;
                        let l = format!("{line}\n");
                        buf.push_str(&l);
                        Ok(l.len())
                    }
                    2 => Err(std::io::Error::new(std::io::ErrorKind::Other, "disk error")),
                    3 => {
                        let line = r#"{"ts":"2026-02-27T01:00:00Z","tool":"Bash","session":"s1","cwd":"/proj","input":{}}"#;
                        let l = format!("{line}\n");
                        buf.push_str(&l);
                        Ok(l.len())
                    }
                    _ => Ok(0),
                }
            }
        }

        let conn = test_conn();
        let (imported, skipped) = migrate_reader(FlakyReader { calls: 0 }, &conn).unwrap();
        assert_eq!(imported, 2);
        assert_eq!(skipped, 1);
    }

    #[test]
    fn migrate_reader_skips_invalid_json() {
        let data = "not valid json\n\
                    {\"ts\":\"2026-02-27T00:00:00Z\",\"tool\":\"Read\",\"session\":\"s1\",\"cwd\":\"/proj\",\"input\":{}}\n";
        let conn = test_conn();
        let (imported, skipped) = migrate_reader(std::io::Cursor::new(data), &conn).unwrap();
        assert_eq!(imported, 1);
        assert_eq!(skipped, 1);
    }
}
