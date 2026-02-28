use std::collections::{HashMap, HashSet};
use std::fmt;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::Path;

use crate::models::ToolCall;

/// Accumulated statistics from a tool-usage log.
#[derive(Debug, Default)]
pub struct Stats {
    pub total: u64,
    pub sessions: HashSet<String>,
    pub by_tool: HashMap<String, u64>,
    pub by_date: HashMap<String, u64>,
    pub files_read: HashMap<String, u64>,
    pub bash_cmds: HashMap<String, u64>,
    pub by_cwd: HashMap<String, u64>,
}

impl Stats {
    /// Parse stats from a JSONL reader in a single pass.
    pub fn from_reader(reader: impl BufRead) -> Self {
        let mut stats = Stats::default();

        for line in reader.lines() {
            let line = match line {
                Ok(l) => l,
                Err(_) => continue,
            };
            if line.is_empty() {
                continue;
            }
            let record: ToolCall = match serde_json::from_str(&line) {
                Ok(r) => r,
                Err(_) => continue,
            };

            stats.total += 1;
            stats.sessions.insert(record.session.clone());
            *stats.by_tool.entry(record.tool.clone()).or_default() += 1;

            let date = record.ts.get(..10).unwrap_or(&record.ts).to_string();
            *stats.by_date.entry(date).or_default() += 1;

            if !record.cwd.is_empty() {
                *stats.by_cwd.entry(record.cwd.clone()).or_default() += 1;
            }

            if record.tool == "Read" {
                if let Some(path) = record.input.get("file_path").and_then(|v| v.as_str()) {
                    *stats.files_read.entry(path.to_string()).or_default() += 1;
                }
            }

            if record.tool == "Bash" {
                if let Some(cmd) = record.input.get("command").and_then(|v| v.as_str()) {
                    if let Some(first_word) = cmd.split_whitespace().next() {
                        if first_word
                            .chars()
                            .all(|c| c.is_alphanumeric() || "_./-".contains(c))
                        {
                            *stats.bash_cmds.entry(first_word.to_string()).or_default() += 1;
                        }
                    }
                }
            }
        }

        stats
    }
}

/// Parse the JSONL log and print formatted usage statistics.
#[cfg(not(tarpaulin_include))]
pub fn run() {
    if let Err(e) = try_run() {
        eprintln!("claude-track stats: {e}");
    }
}

fn try_run() -> Result<(), Box<dyn std::error::Error>> {
    let log_path = dirs::home_dir()
        .ok_or("could not determine home directory")?
        .join(".claude")
        .join("tool-usage.jsonl");

    print!("{}", run_with_path(&log_path)?);
    Ok(())
}

/// Generate the stats report for the given log path. Returns the formatted string.
pub fn run_with_path(log_path: &Path) -> Result<String, Box<dyn std::error::Error>> {
    if !log_path.exists() {
        return Ok("No tool usage data yet.\n".to_string());
    }

    let file_size = fs::metadata(log_path)?.len();
    let file = fs::File::open(log_path)?;
    let stats = Stats::from_reader(BufReader::new(file));

    Ok(format_report(&stats, file_size))
}

/// Format the stats report as a string.
pub fn format_report(stats: &Stats, file_size: u64) -> String {
    let mut out = String::new();

    fmt::write(&mut out, format_args!("=== Claude Code Tool Usage Stats ===\n")).unwrap();
    fmt::write(&mut out, format_args!("Total tool calls: {}\n", stats.total)).unwrap();
    fmt::write(
        &mut out,
        format_args!("Across {} session(s)\n", stats.sessions.len()),
    )
    .unwrap();
    fmt::write(
        &mut out,
        format_args!("Log size: {}\n\n", human_size(file_size)),
    )
    .unwrap();

    fmt::write(&mut out, format_args!("--- Calls by tool ---\n")).unwrap();
    out.push_str(&format_sorted_map(&stats.by_tool, usize::MAX, true));
    out.push('\n');

    fmt::write(&mut out, format_args!("--- Calls by date ---\n")).unwrap();
    out.push_str(&format_sorted_map(&stats.by_date, usize::MAX, false));
    out.push('\n');

    fmt::write(&mut out, format_args!("--- Top 10 files read ---\n")).unwrap();
    out.push_str(&format_sorted_map(&stats.files_read, 10, true));
    out.push('\n');

    fmt::write(&mut out, format_args!("--- Top 10 Bash commands ---\n")).unwrap();
    out.push_str(&format_sorted_map(&stats.bash_cmds, 10, true));
    out.push('\n');

    fmt::write(
        &mut out,
        format_args!("--- Calls by project directory ---\n"),
    )
    .unwrap();
    out.push_str(&format_sorted_map(&stats.by_cwd, usize::MAX, true));

    out
}

/// Format a sorted map section into a string.
pub fn format_sorted_map(map: &HashMap<String, u64>, limit: usize, desc: bool) -> String {
    let mut entries: Vec<_> = map.iter().collect();
    if desc {
        entries.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
    } else {
        entries.sort_by(|a, b| a.0.cmp(b.0));
    }
    let mut out = String::new();
    for (key, count) in entries.into_iter().take(limit) {
        fmt::write(&mut out, format_args!("  {count:<4} {key}\n")).unwrap();
    }
    out
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    use tempfile::TempDir;

    fn make_jsonl(records: &[&str]) -> String {
        records.join("\n")
    }

    fn tool_call_json(tool: &str, session: &str, cwd: &str, ts: &str, input: &str) -> String {
        format!(
            r#"{{"ts":"{ts}","tool":"{tool}","session":"{session}","cwd":"{cwd}","input":{input}}}"#
        )
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
    fn format_sorted_map_descending() {
        let mut map = HashMap::new();
        map.insert("Read".to_string(), 10);
        map.insert("Bash".to_string(), 20);
        map.insert("Edit".to_string(), 5);

        let output = format_sorted_map(&map, usize::MAX, true);
        let lines: Vec<&str> = output.lines().collect();
        assert_eq!(lines.len(), 3);
        assert!(lines[0].contains("Bash"));
        assert!(lines[0].contains("20"));
        assert!(lines[1].contains("Read"));
        assert!(lines[2].contains("Edit"));
    }

    #[test]
    fn format_sorted_map_ascending() {
        let mut map = HashMap::new();
        map.insert("2026-02-28".to_string(), 5);
        map.insert("2026-02-27".to_string(), 10);

        let output = format_sorted_map(&map, usize::MAX, false);
        let lines: Vec<&str> = output.lines().collect();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].contains("2026-02-27"));
        assert!(lines[1].contains("2026-02-28"));
    }

    #[test]
    fn format_sorted_map_with_limit() {
        let mut map = HashMap::new();
        map.insert("a".to_string(), 3);
        map.insert("b".to_string(), 2);
        map.insert("c".to_string(), 1);

        let output = format_sorted_map(&map, 2, true);
        let lines: Vec<&str> = output.lines().collect();
        assert_eq!(lines.len(), 2);
    }

    #[test]
    fn format_sorted_map_empty() {
        let map = HashMap::new();
        let output = format_sorted_map(&map, 10, true);
        assert_eq!(output, "");
    }

    #[test]
    fn stats_empty_input() {
        let stats = Stats::from_reader(Cursor::new(""));
        assert_eq!(stats.total, 0);
        assert!(stats.sessions.is_empty());
    }

    #[test]
    fn stats_counts_totals_and_sessions() {
        let data = make_jsonl(&[
            &tool_call_json("Read", "s1", "/a", "2026-02-27T00:00:00Z", "{}"),
            &tool_call_json("Bash", "s1", "/a", "2026-02-27T01:00:00Z", "{}"),
            &tool_call_json("Read", "s2", "/b", "2026-02-28T00:00:00Z", "{}"),
        ]);

        let stats = Stats::from_reader(Cursor::new(data));
        assert_eq!(stats.total, 3);
        assert_eq!(stats.sessions.len(), 2);
    }

    #[test]
    fn stats_by_tool() {
        let data = make_jsonl(&[
            &tool_call_json("Read", "s1", "/a", "2026-02-27T00:00:00Z", "{}"),
            &tool_call_json("Read", "s1", "/a", "2026-02-27T01:00:00Z", "{}"),
            &tool_call_json("Bash", "s1", "/a", "2026-02-27T02:00:00Z", "{}"),
        ]);

        let stats = Stats::from_reader(Cursor::new(data));
        assert_eq!(stats.by_tool["Read"], 2);
        assert_eq!(stats.by_tool["Bash"], 1);
    }

    #[test]
    fn stats_by_date() {
        let data = make_jsonl(&[
            &tool_call_json("Read", "s1", "/a", "2026-02-27T00:00:00Z", "{}"),
            &tool_call_json("Read", "s1", "/a", "2026-02-27T23:59:59Z", "{}"),
            &tool_call_json("Read", "s1", "/a", "2026-02-28T00:00:00Z", "{}"),
        ]);

        let stats = Stats::from_reader(Cursor::new(data));
        assert_eq!(stats.by_date["2026-02-27"], 2);
        assert_eq!(stats.by_date["2026-02-28"], 1);
    }

    #[test]
    fn stats_files_read() {
        let data = make_jsonl(&[
            &tool_call_json(
                "Read",
                "s1",
                "/a",
                "2026-02-27T00:00:00Z",
                r#"{"file_path":"/foo/bar.rs"}"#,
            ),
            &tool_call_json(
                "Read",
                "s1",
                "/a",
                "2026-02-27T00:00:00Z",
                r#"{"file_path":"/foo/bar.rs"}"#,
            ),
            &tool_call_json(
                "Read",
                "s1",
                "/a",
                "2026-02-27T00:00:00Z",
                r#"{"file_path":"/baz.rs"}"#,
            ),
            // Non-Read tool should not count
            &tool_call_json(
                "Edit",
                "s1",
                "/a",
                "2026-02-27T00:00:00Z",
                r#"{"file_path":"/baz.rs"}"#,
            ),
        ]);

        let stats = Stats::from_reader(Cursor::new(data));
        assert_eq!(stats.files_read["/foo/bar.rs"], 2);
        assert_eq!(stats.files_read["/baz.rs"], 1);
        assert_eq!(stats.files_read.len(), 2);
    }

    #[test]
    fn stats_bash_commands() {
        let data = make_jsonl(&[
            &tool_call_json(
                "Bash",
                "s1",
                "/a",
                "2026-02-27T00:00:00Z",
                r#"{"command":"git status"}"#,
            ),
            &tool_call_json(
                "Bash",
                "s1",
                "/a",
                "2026-02-27T00:00:00Z",
                r#"{"command":"git diff"}"#,
            ),
            &tool_call_json(
                "Bash",
                "s1",
                "/a",
                "2026-02-27T00:00:00Z",
                r#"{"command":"ls -la"}"#,
            ),
        ]);

        let stats = Stats::from_reader(Cursor::new(data));
        assert_eq!(stats.bash_cmds["git"], 2);
        assert_eq!(stats.bash_cmds["ls"], 1);
    }

    #[test]
    fn stats_bash_commands_filters_special_chars() {
        let data = make_jsonl(&[&tool_call_json(
            "Bash",
            "s1",
            "/a",
            "2026-02-27T00:00:00Z",
            r#"{"command":"echo hello && rm -rf /"}"#,
        )]);

        let stats = Stats::from_reader(Cursor::new(data));
        assert_eq!(stats.bash_cmds.get("echo"), Some(&1));
    }

    #[test]
    fn stats_by_cwd() {
        let data = make_jsonl(&[
            &tool_call_json("Read", "s1", "/project-a", "2026-02-27T00:00:00Z", "{}"),
            &tool_call_json("Read", "s1", "/project-a", "2026-02-27T00:00:00Z", "{}"),
            &tool_call_json("Read", "s1", "/project-b", "2026-02-27T00:00:00Z", "{}"),
        ]);

        let stats = Stats::from_reader(Cursor::new(data));
        assert_eq!(stats.by_cwd["/project-a"], 2);
        assert_eq!(stats.by_cwd["/project-b"], 1);
    }

    #[test]
    fn stats_skips_empty_cwd() {
        let data =
            make_jsonl(&[&tool_call_json("Read", "s1", "", "2026-02-27T00:00:00Z", "{}")]);

        let stats = Stats::from_reader(Cursor::new(data));
        assert!(stats.by_cwd.is_empty());
    }

    #[test]
    fn stats_skips_invalid_json_lines() {
        let data = make_jsonl(&[
            &tool_call_json("Read", "s1", "/a", "2026-02-27T00:00:00Z", "{}"),
            "not valid json",
            "",
            &tool_call_json("Bash", "s1", "/a", "2026-02-27T00:00:00Z", "{}"),
        ]);

        let stats = Stats::from_reader(Cursor::new(data));
        assert_eq!(stats.total, 2);
    }

    #[test]
    fn stats_short_timestamp() {
        let data = make_jsonl(&[&tool_call_json("Read", "s1", "/a", "short", "{}")]);

        let stats = Stats::from_reader(Cursor::new(data));
        assert_eq!(stats.by_date["short"], 1);
    }

    #[test]
    fn format_report_includes_all_sections() {
        let data = make_jsonl(&[
            &tool_call_json(
                "Read",
                "s1",
                "/proj",
                "2026-02-27T00:00:00Z",
                r#"{"file_path":"/foo.rs"}"#,
            ),
            &tool_call_json(
                "Bash",
                "s1",
                "/proj",
                "2026-02-27T01:00:00Z",
                r#"{"command":"git status"}"#,
            ),
        ]);
        let stats = Stats::from_reader(Cursor::new(data));
        let report = format_report(&stats, 1024);

        assert!(report.contains("=== Claude Code Tool Usage Stats ==="));
        assert!(report.contains("Total tool calls: 2"));
        assert!(report.contains("Across 1 session(s)"));
        assert!(report.contains("Log size: 1.0 KB"));
        assert!(report.contains("--- Calls by tool ---"));
        assert!(report.contains("Read"));
        assert!(report.contains("Bash"));
        assert!(report.contains("--- Calls by date ---"));
        assert!(report.contains("2026-02-27"));
        assert!(report.contains("--- Top 10 files read ---"));
        assert!(report.contains("/foo.rs"));
        assert!(report.contains("--- Top 10 Bash commands ---"));
        assert!(report.contains("git"));
        assert!(report.contains("--- Calls by project directory ---"));
        assert!(report.contains("/proj"));
    }

    #[test]
    fn format_report_empty_stats() {
        let stats = Stats::default();
        let report = format_report(&stats, 0);

        assert!(report.contains("Total tool calls: 0"));
        assert!(report.contains("Across 0 session(s)"));
        assert!(report.contains("Log size: 0 B"));
    }

    #[test]
    fn run_with_path_missing_file() {
        let dir = TempDir::new().unwrap();
        let log_path = dir.path().join("nonexistent.jsonl");

        let output = run_with_path(&log_path).unwrap();
        assert_eq!(output, "No tool usage data yet.\n");
    }

    #[test]
    fn run_with_path_existing_file() {
        let dir = TempDir::new().unwrap();
        let log_path = dir.path().join("tool-usage.jsonl");

        let line = tool_call_json("Read", "s1", "/a", "2026-02-27T00:00:00Z", "{}");
        fs::write(&log_path, format!("{line}\n")).unwrap();

        let output = run_with_path(&log_path).unwrap();
        assert!(output.contains("Total tool calls: 1"));
        assert!(output.contains("Across 1 session(s)"));
    }

    #[test]
    fn stats_skips_io_errors() {
        /// A reader that yields one valid line then an I/O error then another valid line.
        struct FlakyReader {
            calls: u8,
        }

        impl std::io::Read for FlakyReader {
            fn read(&mut self, _buf: &mut [u8]) -> std::io::Result<usize> {
                // BufRead::lines() drives the iteration; Read::read is not called directly.
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
                        let line = format!(
                            "{}\n",
                            tool_call_json("Read", "s1", "/a", "2026-02-27T00:00:00Z", "{}")
                        );
                        buf.push_str(&line);
                        Ok(line.len())
                    }
                    2 => Err(std::io::Error::new(std::io::ErrorKind::Other, "disk error")),
                    3 => {
                        let line = format!(
                            "{}\n",
                            tool_call_json("Bash", "s1", "/a", "2026-02-27T00:00:00Z", "{}")
                        );
                        buf.push_str(&line);
                        Ok(line.len())
                    }
                    _ => Ok(0), // EOF
                }
            }
        }

        let reader = FlakyReader { calls: 0 };
        let stats = Stats::from_reader(reader);
        // The I/O error on call 2 is skipped; calls 1 and 3 are counted
        assert_eq!(stats.total, 2);
        assert_eq!(stats.by_tool["Read"], 1);
        assert_eq!(stats.by_tool["Bash"], 1);
    }
}
