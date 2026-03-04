use std::path::{Path, PathBuf};

use rusqlite::Connection;

use crate::db;

/// A plan discovered from a transcript file.
#[derive(Debug)]
struct DiscoveredPlan {
    session_id: String,
    tool_use_id: String,
    timestamp: String,
    plan_text: String,
}

/// Backfill plans from historical transcript files.
#[cfg(not(tarpaulin_include))]
pub fn run() {
    if let Err(e) = try_run() {
        eprintln!("claude-track backfill: {e}");
        std::process::exit(1);
    }
}

fn try_run() -> Result<(), Box<dyn std::error::Error>> {
    let home = dirs::home_dir().ok_or("could not determine home directory")?;
    let projects_dir = home.join(".claude").join("projects");
    let db_path = home.join(".claude").join("claude-track.db");

    let conn = db::open_db(&db_path)?;
    let output = backfill_from(&projects_dir, &conn)?;
    print!("{output}");
    Ok(())
}

/// Scan transcript files under `projects_dir` and import plans into the database.
/// Returns user-facing summary output.
pub fn backfill_from(
    projects_dir: &Path,
    conn: &Connection,
) -> Result<String, Box<dyn std::error::Error>> {
    if !projects_dir.exists() {
        return Ok(format!(
            "No projects directory found at {}\nNothing to backfill.\n",
            projects_dir.display()
        ));
    }

    let transcripts = find_transcripts(projects_dir);
    let mut existing_ids = db::get_all_plan_tool_use_ids(conn)?;

    let mut total_found = 0u64;
    let mut total_imported = 0u64;
    let mut total_skipped = 0u64;

    for transcript in &transcripts {
        let session_id = transcript
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("unknown");

        let plans = extract_plans_from_transcript(transcript, session_id);

        for plan in plans {
            total_found += 1;
            if existing_ids.contains(&plan.tool_use_id) {
                total_skipped += 1;
                continue;
            }
            db::insert_plan(
                conn,
                &plan.session_id,
                &plan.tool_use_id,
                &plan.timestamp,
                &plan.plan_text,
            )?;
            existing_ids.insert(plan.tool_use_id);
            total_imported += 1;
        }
    }

    let mut output = format!(
        "Scanned {} transcript files.\nFound {} plans: {} imported, {} skipped (already exist).\n",
        transcripts.len(),
        total_found,
        total_imported,
        total_skipped,
    );
    if transcripts.is_empty() {
        output.push_str("No transcript files found.\n");
    }
    Ok(output)
}

/// Find all *.jsonl transcript files under project subdirectories.
/// Returns a sorted list for deterministic processing.
fn find_transcripts(projects_dir: &Path) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    let entries = match std::fs::read_dir(projects_dir) {
        Ok(e) => e,
        Err(_) => return paths,
    };
    for entry in entries.flatten() {
        let subdir = entry.path();
        if !subdir.is_dir() {
            continue;
        }
        let sub_entries = match std::fs::read_dir(&subdir) {
            Ok(e) => e,
            Err(_) => continue,
        };
        for sub_entry in sub_entries.flatten() {
            let path = sub_entry.path();
            if path.extension().and_then(|e| e.to_str()) == Some("jsonl") {
                paths.push(path);
            }
        }
    }
    paths.sort();
    paths
}

/// Extract ExitPlanMode plans from a transcript file.
/// Scans assistant lines for ExitPlanMode tool_use blocks.
fn extract_plans_from_transcript(path: &Path, session_id: &str) -> Vec<DiscoveredPlan> {
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };

    let mut plans = Vec::new();

    for line in content.lines() {
        if line.is_empty() {
            continue;
        }
        let val: serde_json::Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => continue,
        };

        if val.get("type").and_then(|v| v.as_str()) != Some("assistant") {
            continue;
        }

        let timestamp = val
            .get("timestamp")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        let content_arr = match val
            .get("message")
            .and_then(|m| m.get("content"))
            .and_then(|c| c.as_array())
        {
            Some(arr) => arr,
            None => continue,
        };

        for block in content_arr {
            if block.get("type").and_then(|v| v.as_str()) != Some("tool_use") {
                continue;
            }
            if block.get("name").and_then(|v| v.as_str()) != Some("ExitPlanMode") {
                continue;
            }
            let id = match block.get("id").and_then(|v| v.as_str()) {
                Some(id) => id.to_string(),
                None => continue,
            };
            let plan_text = block
                .get("input")
                .and_then(|i| i.get("plan"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            plans.push(DiscoveredPlan {
                session_id: session_id.to_string(),
                tool_use_id: id,
                timestamp: timestamp.clone(),
                plan_text,
            });
        }
    }

    plans
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn test_conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        db::init_db(&conn).unwrap();
        conn
    }

    fn make_assistant_line(tool_use_id: &str, plan_text: &str, timestamp: &str) -> String {
        serde_json::json!({
            "type": "assistant",
            "timestamp": timestamp,
            "message": {
                "content": [{
                    "type": "tool_use",
                    "id": tool_use_id,
                    "name": "ExitPlanMode",
                    "input": { "plan": plan_text }
                }]
            }
        })
        .to_string()
    }

    // --- extract_plans_from_transcript tests ---

    #[test]
    fn extract_single_plan() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("session1.jsonl");
        let content = format!(
            "{}\n",
            make_assistant_line("toolu_1", "my plan", "2026-01-01T00:00:00Z"),
        );
        fs::write(&path, content).unwrap();

        let plans = extract_plans_from_transcript(&path, "session1");
        assert_eq!(plans.len(), 1);
        assert_eq!(plans[0].session_id, "session1");
        assert_eq!(plans[0].tool_use_id, "toolu_1");
        assert_eq!(plans[0].timestamp, "2026-01-01T00:00:00Z");
        assert_eq!(plans[0].plan_text, "my plan");
    }

    #[test]
    fn extract_multiple_plans() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("s.jsonl");
        let content = format!(
            "{}\n{}\n",
            make_assistant_line("toolu_1", "plan 1", "2026-01-01T00:00:00Z"),
            make_assistant_line("toolu_2", "plan 2", "2026-01-01T01:00:00Z"),
        );
        fs::write(&path, content).unwrap();

        let plans = extract_plans_from_transcript(&path, "s1");
        assert_eq!(plans.len(), 2);
        assert_eq!(plans[0].plan_text, "plan 1");
        assert_eq!(plans[1].plan_text, "plan 2");
    }

    #[test]
    fn extract_no_plans_in_transcript() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("s.jsonl");
        // A transcript with no ExitPlanMode
        let content = serde_json::json!({
            "type": "assistant",
            "message": {
                "content": [{
                    "type": "tool_use",
                    "id": "toolu_1",
                    "name": "Read",
                    "input": { "file_path": "/foo" }
                }]
            }
        })
        .to_string();
        fs::write(&path, format!("{content}\n")).unwrap();

        let plans = extract_plans_from_transcript(&path, "s1");
        assert!(plans.is_empty());
    }

    #[test]
    fn extract_empty_file() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("s.jsonl");
        fs::write(&path, "").unwrap();

        let plans = extract_plans_from_transcript(&path, "s1");
        assert!(plans.is_empty());
    }

    #[test]
    fn extract_missing_file() {
        let plans = extract_plans_from_transcript(Path::new("/nonexistent/file.jsonl"), "s1");
        assert!(plans.is_empty());
    }

    #[test]
    fn extract_skips_empty_lines() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("s.jsonl");
        let content = format!(
            "\n{}\n\n",
            make_assistant_line("toolu_1", "plan", "2026-01-01T00:00:00Z"),
        );
        fs::write(&path, content).unwrap();

        let plans = extract_plans_from_transcript(&path, "s1");
        assert_eq!(plans.len(), 1);
    }

    #[test]
    fn extract_invalid_json_lines() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("s.jsonl");
        let content = format!(
            "not json\n{}\n",
            make_assistant_line("toolu_1", "plan", "2026-01-01T00:00:00Z"),
        );
        fs::write(&path, content).unwrap();

        let plans = extract_plans_from_transcript(&path, "s1");
        assert_eq!(plans.len(), 1);
    }

    #[test]
    fn extract_missing_plan_field() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("s.jsonl");
        // ExitPlanMode with no plan field in input
        let content = serde_json::json!({
            "type": "assistant",
            "timestamp": "2026-01-01T00:00:00Z",
            "message": {
                "content": [{
                    "type": "tool_use",
                    "id": "toolu_1",
                    "name": "ExitPlanMode",
                    "input": {}
                }]
            }
        })
        .to_string();
        fs::write(&path, format!("{content}\n")).unwrap();

        let plans = extract_plans_from_transcript(&path, "s1");
        assert_eq!(plans.len(), 1);
        assert_eq!(plans[0].plan_text, "");
    }

    #[test]
    fn extract_missing_timestamp() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("s.jsonl");
        // No timestamp field
        let content = serde_json::json!({
            "type": "assistant",
            "message": {
                "content": [{
                    "type": "tool_use",
                    "id": "toolu_1",
                    "name": "ExitPlanMode",
                    "input": { "plan": "plan text" }
                }]
            }
        })
        .to_string();
        fs::write(&path, format!("{content}\n")).unwrap();

        let plans = extract_plans_from_transcript(&path, "s1");
        assert_eq!(plans.len(), 1);
        assert_eq!(plans[0].timestamp, "");
    }

    #[test]
    fn extract_non_array_content() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("s.jsonl");
        // content is a string, not an array
        let content = serde_json::json!({
            "type": "assistant",
            "message": {
                "content": "just a string"
            }
        })
        .to_string();
        fs::write(&path, format!("{content}\n")).unwrap();

        let plans = extract_plans_from_transcript(&path, "s1");
        assert!(plans.is_empty());
    }

    #[test]
    fn extract_tool_use_no_id() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("s.jsonl");
        // ExitPlanMode tool_use with no id field
        let content = serde_json::json!({
            "type": "assistant",
            "timestamp": "2026-01-01T00:00:00Z",
            "message": {
                "content": [{
                    "type": "tool_use",
                    "name": "ExitPlanMode",
                    "input": { "plan": "plan text" }
                }]
            }
        })
        .to_string();
        fs::write(&path, format!("{content}\n")).unwrap();

        let plans = extract_plans_from_transcript(&path, "s1");
        assert!(plans.is_empty());
    }

    #[test]
    fn extract_non_exit_plan_mode_tool() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("s.jsonl");
        let content = serde_json::json!({
            "type": "assistant",
            "timestamp": "2026-01-01T00:00:00Z",
            "message": {
                "content": [{
                    "type": "tool_use",
                    "id": "toolu_1",
                    "name": "EnterPlanMode",
                    "input": {}
                }]
            }
        })
        .to_string();
        fs::write(&path, format!("{content}\n")).unwrap();

        let plans = extract_plans_from_transcript(&path, "s1");
        assert!(plans.is_empty());
    }

    // --- find_transcripts tests ---

    #[test]
    fn find_transcripts_empty_dir() {
        let dir = TempDir::new().unwrap();
        let paths = find_transcripts(dir.path());
        assert!(paths.is_empty());
    }

    #[test]
    fn find_transcripts_with_files() {
        let dir = TempDir::new().unwrap();
        let sub = dir.path().join("project1");
        fs::create_dir_all(&sub).unwrap();
        fs::write(sub.join("a.jsonl"), "").unwrap();
        fs::write(sub.join("b.jsonl"), "").unwrap();
        fs::write(sub.join("c.txt"), "").unwrap(); // not jsonl

        let paths = find_transcripts(dir.path());
        assert_eq!(paths.len(), 2);
        assert!(paths.iter().all(|p| p.extension().unwrap() == "jsonl"));
    }

    #[test]
    fn find_transcripts_skips_non_dir_entries() {
        let dir = TempDir::new().unwrap();
        // Create a file (not a directory) at the top level
        fs::write(dir.path().join("not-a-dir.txt"), "").unwrap();
        let sub = dir.path().join("project1");
        fs::create_dir_all(&sub).unwrap();
        fs::write(sub.join("a.jsonl"), "").unwrap();

        let paths = find_transcripts(dir.path());
        assert_eq!(paths.len(), 1);
    }

    #[test]
    fn find_transcripts_unreadable_subdir() {
        use std::os::unix::fs::PermissionsExt;
        let dir = TempDir::new().unwrap();
        let sub = dir.path().join("unreadable");
        fs::create_dir_all(&sub).unwrap();
        fs::write(sub.join("a.jsonl"), "").unwrap();
        // Remove read permissions
        fs::set_permissions(&sub, fs::Permissions::from_mode(0o000)).unwrap();

        let paths = find_transcripts(dir.path());
        assert!(paths.is_empty());

        // Restore permissions for cleanup
        fs::set_permissions(&sub, fs::Permissions::from_mode(0o755)).unwrap();
    }

    #[test]
    fn find_transcripts_nonexistent_dir() {
        let paths = find_transcripts(Path::new("/nonexistent/dir"));
        assert!(paths.is_empty());
    }

    // --- backfill_from tests ---

    #[test]
    fn backfill_no_projects_dir() {
        let conn = test_conn();
        let output = backfill_from(Path::new("/nonexistent/projects"), &conn).unwrap();
        assert!(output.contains("No projects directory found"));
        assert!(output.contains("Nothing to backfill"));
    }

    #[test]
    fn backfill_empty_projects_dir() {
        let dir = TempDir::new().unwrap();
        let conn = test_conn();
        let output = backfill_from(dir.path(), &conn).unwrap();
        assert!(output.contains("Scanned 0 transcript files"));
        assert!(output.contains("No transcript files found"));
    }

    #[test]
    fn backfill_with_plans() {
        let dir = TempDir::new().unwrap();
        let sub = dir.path().join("project1");
        fs::create_dir_all(&sub).unwrap();

        let content = format!(
            "{}\n",
            make_assistant_line("toolu_1", "my plan", "2026-01-01T00:00:00Z"),
        );
        fs::write(sub.join("sess123.jsonl"), content).unwrap();

        let conn = test_conn();
        let output = backfill_from(dir.path(), &conn).unwrap();
        assert!(output.contains("1 imported"));

        // Verify in DB
        let plan_text: String = conn
            .query_row(
                "SELECT plan_text FROM plans WHERE tool_use_id='toolu_1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(plan_text, "my plan");
    }

    #[test]
    fn backfill_deduplication() {
        let dir = TempDir::new().unwrap();
        let sub = dir.path().join("project1");
        fs::create_dir_all(&sub).unwrap();

        let content = format!(
            "{}\n",
            make_assistant_line("toolu_1", "my plan", "2026-01-01T00:00:00Z"),
        );
        fs::write(sub.join("sess123.jsonl"), content).unwrap();

        let conn = test_conn();
        // Pre-insert the plan
        db::insert_plan(&conn, "sess123", "toolu_1", "2026-01-01T00:00:00Z", "my plan").unwrap();

        let output = backfill_from(dir.path(), &conn).unwrap();
        assert!(output.contains("1 skipped"));
        assert!(output.contains("0 imported"));

        // Should still be exactly 1 row
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM plans", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn backfill_session_id_from_filename() {
        let dir = TempDir::new().unwrap();
        let sub = dir.path().join("project1");
        fs::create_dir_all(&sub).unwrap();

        let content = format!(
            "{}\n",
            make_assistant_line("toolu_1", "plan", "2026-01-01T00:00:00Z"),
        );
        fs::write(sub.join("my-session-uuid.jsonl"), content).unwrap();

        let conn = test_conn();
        backfill_from(dir.path(), &conn).unwrap();

        let session_id: String = conn
            .query_row(
                "SELECT session_id FROM plans WHERE tool_use_id='toolu_1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(session_id, "my-session-uuid");
    }
}
