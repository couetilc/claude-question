use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{BufRead, BufReader};

use crate::models::ToolCall;

/// Parse the JSONL log and print formatted usage statistics.
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

    if !log_path.exists() {
        println!("No tool usage data yet.");
        return Ok(());
    }

    let metadata = fs::metadata(&log_path)?;
    let file_size = metadata.len();

    let file = fs::File::open(&log_path)?;
    let reader = BufReader::new(file);

    let mut total: u64 = 0;
    let mut sessions: HashSet<String> = HashSet::new();
    let mut by_tool: HashMap<String, u64> = HashMap::new();
    let mut by_date: HashMap<String, u64> = HashMap::new();
    let mut files_read: HashMap<String, u64> = HashMap::new();
    let mut bash_cmds: HashMap<String, u64> = HashMap::new();
    let mut by_cwd: HashMap<String, u64> = HashMap::new();

    for line in reader.lines() {
        let line = line?;
        if line.is_empty() {
            continue;
        }
        let record: ToolCall = match serde_json::from_str(&line) {
            Ok(r) => r,
            Err(_) => continue,
        };

        total += 1;
        sessions.insert(record.session.clone());
        *by_tool.entry(record.tool.clone()).or_default() += 1;

        let date = record.ts.get(..10).unwrap_or(&record.ts).to_string();
        *by_date.entry(date).or_default() += 1;

        if !record.cwd.is_empty() {
            *by_cwd.entry(record.cwd.clone()).or_default() += 1;
        }

        if record.tool == "Read" {
            if let Some(path) = record.input.get("file_path").and_then(|v| v.as_str()) {
                *files_read.entry(path.to_string()).or_default() += 1;
            }
        }

        if record.tool == "Bash" {
            if let Some(cmd) = record.input.get("command").and_then(|v| v.as_str()) {
                if let Some(first_word) = cmd.split_whitespace().next() {
                    if first_word.chars().all(|c| c.is_alphanumeric() || "_./-".contains(c)) {
                        *bash_cmds.entry(first_word.to_string()).or_default() += 1;
                    }
                }
            }
        }
    }

    println!("=== Claude Code Tool Usage Stats ===");
    println!("Total tool calls: {total}");
    println!("Across {} session(s)", sessions.len());
    println!("Log size: {}", human_size(file_size));
    println!();

    println!("--- Calls by tool ---");
    print_sorted_map(&by_tool, usize::MAX, true);
    println!();

    println!("--- Calls by date ---");
    print_sorted_map(&by_date, usize::MAX, false);
    println!();

    println!("--- Top 10 files read ---");
    print_sorted_map(&files_read, 10, true);
    println!();

    println!("--- Top 10 Bash commands ---");
    print_sorted_map(&bash_cmds, 10, true);
    println!();

    println!("--- Calls by project directory ---");
    print_sorted_map(&by_cwd, usize::MAX, true);

    Ok(())
}

fn print_sorted_map(map: &HashMap<String, u64>, limit: usize, desc: bool) {
    let mut entries: Vec<_> = map.iter().collect();
    if desc {
        entries.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
    } else {
        entries.sort_by(|a, b| a.0.cmp(b.0));
    }
    for (key, count) in entries.into_iter().take(limit) {
        println!("  {count:<4} {key}");
    }
}

fn human_size(bytes: u64) -> String {
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
