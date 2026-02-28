use std::collections::BTreeMap;
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

    // --- Models ---
    out.push_str(&format_models_section(conn));

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
    fmt::write(&mut out, format_args!("  Total sessions:  {:>10}\n", format_number(total))).unwrap();

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
        format_args!("  Total duration:  {:>10}\n", format_duration(total_seconds)),
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
            format_args!("  Avg session:     {:>10}\n", format_duration(avg)),
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
    fmt::write(&mut out, format_args!("  Sessions today:  {:>10}\n", format_number(today))).unwrap();

    out.push('\n');
    out
}

fn format_models_section(conn: &Connection) -> String {
    let mut out = String::new();
    out.push_str("--- Models ---\n");

    let mut stmt = conn
        .prepare(
            "SELECT model, COUNT(DISTINCT session_id) as sessions,
                    SUM(input_tokens + output_tokens) as io_tokens
             FROM token_usage WHERE model IS NOT NULL AND model != ''
             GROUP BY model ORDER BY io_tokens DESC",
        )
        .unwrap();
    let rows: Vec<(String, i64, i64)> = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .unwrap()
        .filter_map(|r| r.ok())
        .collect();

    if rows.is_empty() {
        out.push_str("  No model data recorded yet.\n");
    } else {
        let max_tokens = rows.first().map(|(_, _, t)| *t).unwrap_or(0);
        let max_name_len = rows.iter().map(|(m, _, _)| m.len()).max().unwrap_or(10);
        fmt::write(
            &mut out,
            format_args!(
                "  {:<width$}  {:>8}  {:>8}\n",
                "Model", "I/O Toks", "Sessions",
                width = max_name_len,
            ),
        )
        .unwrap();
        fmt::write(
            &mut out,
            format_args!(
                "  {:<width$}  {:>8}  {:>8}\n",
                "─".repeat(max_name_len), "────────", "────────",
                width = max_name_len,
            ),
        )
        .unwrap();
        for (model, sessions, tokens) in &rows {
            let bar = make_bar(*tokens, max_tokens, 20);
            fmt::write(
                &mut out,
                format_args!(
                    "  {:<width$}  {:>8}  {:>8}  {}\n",
                    model,
                    format_number(*tokens),
                    format_number(*sessions),
                    bar,
                    width = max_name_len,
                ),
            )
            .unwrap();
        }
    }

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

    // Per-model cost breakdown
    let mut stmt = conn
        .prepare(
            "SELECT COALESCE(model, ''), SUM(input_tokens), SUM(cache_creation_tokens), SUM(cache_read_tokens), SUM(output_tokens)
             FROM token_usage GROUP BY model",
        )
        .unwrap();
    let model_rows: Vec<(String, i64, i64, i64, i64)> = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)))
        .unwrap()
        .filter_map(|r| r.ok())
        .collect();

    let mut total_cost = 0.0;
    let mut model_costs: Vec<(String, f64)> = Vec::new();
    for (model, inp, cc, cr, out_tok) in &model_rows {
        let cost = estimate_cost_for_model(model, *inp, *cc, *cr, *out_tok);
        total_cost += cost;
        if !model.is_empty() {
            model_costs.push((model.clone(), cost));
        }
    }

    // Show per-model costs when there are multiple models
    if model_costs.len() > 1 {
        for (model, cost) in &model_costs {
            fmt::write(
                &mut out,
                format_args!("  Est. cost ({}): {:>width$}\n", model, format_cost(*cost), width = 30 - model.len()),
            )
            .unwrap();
        }
    }

    fmt::write(
        &mut out,
        format_args!("  Est. cost (total):   {:>11}\n", format_cost(total_cost)),
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
    fmt::write(&mut out, format_args!("  Total prompts:   {:>10}\n", format_number(total))).unwrap();

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
            format_args!("  Avg per session: {:>10.1}\n", avg_per_session),
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
        format_args!("  Avg length:      {:>6} chars\n", avg_length as i64),
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
    fmt::write(&mut out, format_args!("  Total tool calls: {}\n", format_number(total))).unwrap();

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
    let max_count = rows.first().map(|(_, c)| *c).unwrap_or(0);
    // Find the longest tool name for padding
    let max_name_len = rows.iter().map(|(t, _)| t.len()).max().unwrap_or(4);
    if !rows.is_empty() {
        fmt::write(
            &mut out,
            format_args!("  {:>6}  {:<width$}\n", "Calls", "Tool", width = max_name_len),
        )
        .unwrap();
        fmt::write(
            &mut out,
            format_args!("  {:>6}  {:<width$}\n", "──────", "─".repeat(max_name_len), width = max_name_len),
        )
        .unwrap();
    }
    for (tool, count) in &rows {
        let bar = make_bar(*count, max_count, 20);
        fmt::write(
            &mut out,
            format_args!("  {:>6}  {:<width$}  {}\n", format_number(*count), tool, bar, width = max_name_len),
        )
        .unwrap();
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
    if !rows.is_empty() {
        fmt::write(&mut out, format_args!("  {:>6}  {}\n", "Reads", "File")).unwrap();
        fmt::write(&mut out, format_args!("  {:>6}  {}\n", "──────", "────")).unwrap();
    }
    for (path, count) in &rows {
        fmt::write(&mut out, format_args!("  {:>6}  {}\n", format_number(*count), shorten_path(path, 60))).unwrap();
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
    if !sorted.is_empty() {
        fmt::write(&mut out, format_args!("  {:>6}  {}\n", "Runs", "Command")).unwrap();
        fmt::write(&mut out, format_args!("  {:>6}  {}\n", "──────", "───────")).unwrap();
    }
    for (cmd, count) in sorted.iter().take(10) {
        fmt::write(&mut out, format_args!("  {:>6}  {}\n", format_number(*count), cmd)).unwrap();
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
    if !rows.is_empty() {
        fmt::write(&mut out, format_args!("  {}  {:>6}\n", "Date      ", "Calls")).unwrap();
        fmt::write(&mut out, format_args!("  {}  {:>6}\n", "──────────", "──────")).unwrap();
    }
    for (date, count) in &rows {
        fmt::write(&mut out, format_args!("  {}  {:>6}\n", date, format_number(*count))).unwrap();
    }

    out.push('\n');
    out
}

/// Extract project info from a path, identifying worktree subdirectories.
/// Returns `(repo_root, Option<worktree_name>)`.
///
/// If path contains `/.claude/worktrees/<name>`, extracts the repo root
/// (everything before `/.claude/`) and the worktree name. Any trailing
/// subdirectory after the worktree name is discarded.
///
/// Otherwise returns the path as-is with no worktree name.
pub fn extract_project_info(path: &str) -> (String, Option<String>) {
    if let Some(idx) = path.find("/.claude/worktrees/") {
        let repo_root = path[..idx].to_string();
        let after = &path[idx + "/.claude/worktrees/".len()..];
        // Worktree name is the next path component (before any '/')
        let wt_name = after.split('/').next().unwrap_or(after).to_string();
        if wt_name.is_empty() {
            (repo_root, None)
        } else {
            (repo_root, Some(wt_name))
        }
    } else {
        (path.to_string(), None)
    }
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

    // Pass 1: extract project info for each row
    let parsed: Vec<(String, Option<String>, i64)> = rows
        .iter()
        .map(|(path, count)| {
            let (root, wt) = extract_project_info(path);
            (root, wt, *count)
        })
        .collect();

    // Pass 2: aggregate into projects map
    // repo_root -> (own_count, BTreeMap<worktree_name, count>)
    let mut projects: BTreeMap<String, (i64, BTreeMap<String, i64>)> = BTreeMap::new();

    // First, insert all worktree entries and direct (non-subdir) entries
    for (repo_root, wt_name, count) in &parsed {
        let entry = projects.entry(repo_root.clone()).or_insert((0, BTreeMap::new()));
        if let Some(name) = wt_name {
            *entry.1.entry(name.clone()).or_insert(0) += count;
        }
    }

    // Now handle non-worktree entries, merging subdirs into parent roots
    for (repo_root, wt_name, count) in &parsed {
        if wt_name.is_some() {
            continue;
        }
        // Check if this path is a subdirectory of an existing repo root
        let parent = {
            let mut found = None;
            for root in projects.keys() {
                if root != repo_root && repo_root.starts_with(&format!("{}/", root)) {
                    found = Some(root.clone());
                    break;
                }
            }
            found
        };
        if let Some(parent_root) = parent {
            projects.entry(parent_root).or_insert((0, BTreeMap::new())).0 += count;
            // Mark this entry for removal if it was created as empty
        } else {
            projects.entry(repo_root.clone()).or_insert((0, BTreeMap::new())).0 += count;
        }
    }

    // Remove entries that have been fully merged (0 own count, no worktrees)
    projects.retain(|_, (own, wts)| *own > 0 || !wts.is_empty());

    // Sort by total (own + worktrees) descending
    let mut sorted: Vec<(String, i64, Vec<(String, i64)>)> = projects
        .into_iter()
        .map(|(root, (own, wts))| {
            let wt_total: i64 = wts.values().sum();
            let total = own + wt_total;
            let mut wt_sorted: Vec<(String, i64)> = wts.into_iter().collect();
            wt_sorted.sort_by(|a, b| b.1.cmp(&a.1));
            (root, total, wt_sorted)
        })
        .collect();
    sorted.sort_by(|a, b| b.1.cmp(&a.1));

    if !sorted.is_empty() {
        fmt::write(&mut out, format_args!("  {:>6}  {}\n", "Calls", "Project")).unwrap();
        fmt::write(&mut out, format_args!("  {:>6}  {}\n", "──────", "───────")).unwrap();
    }
    for (root, total, worktrees) in &sorted {
        fmt::write(
            &mut out,
            format_args!("  {:>6}  {}\n", format_number(*total), shorten_path(root, 60)),
        )
        .unwrap();
        for (wt_name, count) in worktrees {
            fmt::write(
                &mut out,
                format_args!("  {:>6}    \u{21b3} {}\n", format_number(*count), wt_name),
            )
            .unwrap();
        }
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
#[allow(dead_code)]
pub fn estimate_cost(input: i64, cache_creation: i64, cache_read: i64, output: i64) -> f64 {
    (input as f64 * 3.0 / 1_000_000.0)
        + (cache_creation as f64 * 3.75 / 1_000_000.0)
        + (cache_read as f64 * 0.30 / 1_000_000.0)
        + (output as f64 * 15.0 / 1_000_000.0)
}

/// Estimate cost using model-specific pricing.
/// Opus 4.5+: $5 / $6.25 / $0.50 / $25 per MTok
/// Opus 4.0/4.1: $15 / $18.75 / $1.50 / $75 per MTok
/// Haiku 4.5: $1 / $1.25 / $0.10 / $5 per MTok
/// Haiku 3.5: $0.80 / $1.00 / $0.08 / $4 per MTok
/// Sonnet/default: $3 / $3.75 / $0.30 / $15 per MTok
pub fn estimate_cost_for_model(
    model: &str,
    input: i64,
    cache_creation: i64,
    cache_read: i64,
    output: i64,
) -> f64 {
    let (input_rate, cache_create_rate, cache_read_rate, output_rate) =
        if model.contains("opus") {
            if model.contains("opus-4-5") || model.contains("opus-4-6") {
                // Opus 4.5/4.6
                (5.0, 6.25, 0.50, 25.0)
            } else {
                // Opus 4.0/4.1/3
                (15.0, 18.75, 1.50, 75.0)
            }
        } else if model.contains("haiku") {
            if model.contains("haiku-4-5") {
                // Haiku 4.5
                (1.0, 1.25, 0.10, 5.0)
            } else {
                // Haiku 3.5/3
                (0.80, 1.00, 0.08, 4.0)
            }
        } else {
            // Sonnet (all versions same price)
            (3.0, 3.75, 0.30, 15.0)
        };
    (input as f64 * input_rate / 1_000_000.0)
        + (cache_creation as f64 * cache_create_rate / 1_000_000.0)
        + (cache_read as f64 * cache_read_rate / 1_000_000.0)
        + (output as f64 * output_rate / 1_000_000.0)
}

/// Shorten a path for display: replace home dir with ~, truncate to max_len.
/// For paths still too long, keep first component and last 2 components with `...`.
pub fn shorten_path(path: &str, max_len: usize) -> String {
    let mut p = path.to_string();
    if let Some(home) = dirs::home_dir() {
        let home_str = home.to_string_lossy().to_string();
        if p.starts_with(&home_str) {
            p = format!("~{}", &p[home_str.len()..]);
        }
    }
    if p.len() <= max_len {
        return p;
    }
    // Split into components and keep first + last 2 with ... in between
    let parts: Vec<&str> = p.split('/').filter(|s| !s.is_empty()).collect();
    if parts.len() <= 3 {
        return p;
    }
    let first = parts[0];
    let last_two = &parts[parts.len() - 2..];
    let prefix = if p.starts_with('/') { "/" } else { "" };
    let shortened = format!("{}{}/.../{}/{}", prefix, first, last_two[0], last_two[1]);
    if shortened.len() < p.len() {
        shortened
    } else {
        p
    }
}

/// Build a proportional bar string of the given length using block chars.
pub fn make_bar(count: i64, max_count: i64, max_width: usize) -> String {
    if max_count == 0 {
        return String::new();
    }
    let width = ((count as f64 / max_count as f64) * max_width as f64).round() as usize;
    let width = width.max(if count > 0 { 1 } else { 0 });
    "\u{2588}".repeat(width)
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
        assert!(report.contains("Total sessions:"));
        assert!(report.contains("--- Token Usage ---"));
        assert!(report.contains("--- Prompts ---"));
        assert!(report.contains("Total prompts:"));
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
            0,
        )
        .unwrap();

        let report = format_report(&conn, 2048, Path::new("/test.db"));

        assert!(report.contains("Total sessions:"));
        assert!(report.contains("1"));
        assert!(report.contains("Tracking since:"));
        assert!(report.contains("Total tool calls: 2"));
        assert!(report.contains("Read"));
        assert!(report.contains("Bash"));
        assert!(report.contains("/src/main.rs"));
        assert!(report.contains("cargo"));
        assert!(report.contains("Total prompts:"));
        assert!(report.contains("Avg per session:"));
        assert!(report.contains("Avg length:"));
        assert!(report.contains("--- Models ---"));
        assert!(report.contains("claude-sonnet-4-20250514"));
        assert!(report.contains("Input tokens:"));
        assert!(report.contains("Cache hit rate:"));
        assert!(report.contains("Est. cost"));
        assert!(report.contains("2026-02-27"));
        assert!(report.contains("/proj"));
        // Verify bar chart characters appear for tool usage
        assert!(report.contains("\u{2588}"));
    }

    #[test]
    fn format_report_sessions_without_end() {
        let conn = test_conn();
        db::insert_session_start(&conn, "s1", "2026-02-27T00:00:00Z", "startup", "/proj", "/t").unwrap();

        let report = format_report(&conn, 0, Path::new("/test.db"));
        assert!(report.contains("Total sessions:"));
        assert!(report.contains("1"));
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
        assert!(report.contains("Total prompts:"));
        assert!(report.contains("0"));
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
        assert!(output.contains("Total sessions:"));
        assert!(output.contains("1"));
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

    #[test]
    fn shorten_path_short_unchanged() {
        assert_eq!(shorten_path("/short/path", 60), "/short/path");
    }

    #[test]
    fn shorten_path_replaces_home_dir() {
        let home = dirs::home_dir().unwrap();
        let long = format!("{}/repos/project/src/file.rs", home.display());
        let shortened = shorten_path(&long, 60);
        assert!(shortened.starts_with("~/"));
        assert!(shortened.contains("repos/project/src/file.rs"));
    }

    #[test]
    fn shorten_path_truncates_long() {
        // Build a path that exceeds 60 chars even after ~ substitution
        let home = dirs::home_dir().unwrap();
        let long = format!(
            "{}/very/deeply/nested/directory/structure/with/many/levels/src/file.rs",
            home.display()
        );
        let shortened = shorten_path(&long, 60);
        assert!(shortened.len() <= 60 || shortened.contains("..."));
    }

    #[test]
    fn shorten_path_few_components_unchanged() {
        // Paths with 3 or fewer components don't get truncated with ...
        assert_eq!(shorten_path("/a/b/c", 3), "/a/b/c");
    }

    #[test]
    fn shorten_path_shortened_not_shorter() {
        // When the ... version isn't actually shorter, return original
        // /a/b/c/d = 8 chars, shortened = /a/.../c/d = 10 chars, so original returned
        assert_eq!(shorten_path("/a/b/c/d", 5), "/a/b/c/d");
        // /x/y/z/w = 8 chars, shortened = /x/.../z/w = 10 chars, so original returned
        assert_eq!(shorten_path("/x/y/z/w", 4), "/x/y/z/w");
    }

    #[test]
    fn make_bar_zero_max() {
        assert_eq!(make_bar(5, 0, 20), "");
    }

    #[test]
    fn make_bar_full() {
        let bar = make_bar(100, 100, 20);
        assert_eq!(bar.chars().count(), 20);
        assert!(bar.contains('\u{2588}'));
    }

    #[test]
    fn make_bar_half() {
        let bar = make_bar(50, 100, 20);
        assert_eq!(bar.chars().count(), 10);
    }

    #[test]
    fn make_bar_minimum_one() {
        // Even a small count should show at least 1 block
        let bar = make_bar(1, 10000, 20);
        assert_eq!(bar.chars().count(), 1);
    }

    #[test]
    fn make_bar_zero_count() {
        let bar = make_bar(0, 100, 20);
        assert_eq!(bar, "");
    }

    #[test]
    fn format_tool_usage_with_bar() {
        let conn = test_conn();
        db::insert_tool_use(&conn, "tu1", "s1", "Read", "ts", "/p", "{}").unwrap();
        db::insert_tool_use(&conn, "tu2", "s1", "Read", "ts", "/p", "{}").unwrap();
        db::insert_tool_use(&conn, "tu3", "s1", "Edit", "ts", "/p", "{}").unwrap();
        let section = format_tool_usage_section(&conn);
        // Should contain bar chars and right-aligned counts
        assert!(section.contains("\u{2588}"));
        assert!(section.contains("Read"));
        assert!(section.contains("Edit"));
        assert!(section.contains("2"));
        assert!(section.contains("1"));
    }

    #[test]
    fn format_activity_by_date_right_aligned() {
        let conn = test_conn();
        db::insert_tool_use(&conn, "tu1", "s1", "Read", "2026-02-27T00:00:00Z", "/p", "{}").unwrap();
        let section = format_activity_by_date_section(&conn);
        assert!(section.contains("2026-02-27"));
        assert!(section.contains("1"));
    }

    #[test]
    fn format_sessions_aligned_values() {
        let conn = test_conn();
        let section = format_sessions_section(&conn);
        // All labels should have consistent padding
        assert!(section.contains("Total sessions:"));
        assert!(section.contains("Total duration:"));
        assert!(section.contains("Sessions today:"));
    }

    #[test]
    fn format_prompts_aligned_values() {
        let conn = test_conn();
        let section = format_prompts_section(&conn);
        assert!(section.contains("Total prompts:"));
        assert!(section.contains("Avg length:"));
    }

    #[test]
    fn estimate_cost_for_model_opus_legacy() {
        // Opus 4.0/4.1 use legacy pricing
        let cost = estimate_cost_for_model("claude-opus-4-20250514", 1_000_000, 0, 0, 0);
        assert!((cost - 15.0).abs() < 0.01);
        let cost = estimate_cost_for_model("claude-opus-4-20250514", 0, 0, 0, 1_000_000);
        assert!((cost - 75.0).abs() < 0.01);
    }

    #[test]
    fn estimate_cost_for_model_opus_4_5() {
        let cost = estimate_cost_for_model("claude-opus-4-5-20250514", 1_000_000, 0, 0, 0);
        assert!((cost - 5.0).abs() < 0.01);
        let cost = estimate_cost_for_model("claude-opus-4-5-20250514", 0, 0, 0, 1_000_000);
        assert!((cost - 25.0).abs() < 0.01);
    }

    #[test]
    fn estimate_cost_for_model_opus_4_6() {
        let cost = estimate_cost_for_model("claude-opus-4-6", 1_000_000, 0, 0, 0);
        assert!((cost - 5.0).abs() < 0.01);
        let cost = estimate_cost_for_model("claude-opus-4-6", 0, 0, 0, 1_000_000);
        assert!((cost - 25.0).abs() < 0.01);
        let cost = estimate_cost_for_model("claude-opus-4-6", 0, 0, 1_000_000, 0);
        assert!((cost - 0.50).abs() < 0.01);
    }

    #[test]
    fn estimate_cost_for_model_haiku_legacy() {
        let cost = estimate_cost_for_model("claude-haiku-3-5-20250514", 1_000_000, 0, 0, 0);
        assert!((cost - 0.80).abs() < 0.01);
        let cost = estimate_cost_for_model("claude-haiku-3-5-20250514", 0, 0, 0, 1_000_000);
        assert!((cost - 4.0).abs() < 0.01);
    }

    #[test]
    fn estimate_cost_for_model_haiku_4_5() {
        let cost = estimate_cost_for_model("claude-haiku-4-5-20251001", 1_000_000, 0, 0, 0);
        assert!((cost - 1.0).abs() < 0.01);
        let cost = estimate_cost_for_model("claude-haiku-4-5-20251001", 0, 0, 0, 1_000_000);
        assert!((cost - 5.0).abs() < 0.01);
    }

    #[test]
    fn estimate_cost_for_model_sonnet() {
        let cost = estimate_cost_for_model("claude-sonnet-4-20250514", 1_000_000, 0, 0, 0);
        assert!((cost - 3.0).abs() < 0.01);
        let cost = estimate_cost_for_model("claude-sonnet-4-20250514", 0, 0, 0, 1_000_000);
        assert!((cost - 15.0).abs() < 0.01);
    }

    #[test]
    fn estimate_cost_for_model_unknown() {
        // Unknown models fall back to sonnet pricing
        let cost = estimate_cost_for_model("some-unknown-model", 1_000_000, 0, 0, 0);
        assert!((cost - 3.0).abs() < 0.01);
    }

    #[test]
    fn estimate_cost_for_model_all_components() {
        // Opus 4.6 all-components test
        let cost = estimate_cost_for_model(
            "claude-opus-4-6",
            1_000_000,
            1_000_000,
            1_000_000,
            1_000_000,
        );
        let expected = 5.0 + 6.25 + 0.50 + 25.0;
        assert!((cost - expected).abs() < 0.01);

        // Opus 4.0 legacy all-components test
        let cost = estimate_cost_for_model(
            "claude-opus-4-20250514",
            1_000_000,
            1_000_000,
            1_000_000,
            1_000_000,
        );
        let expected = 15.0 + 18.75 + 1.50 + 75.0;
        assert!((cost - expected).abs() < 0.01);
    }

    #[test]
    fn format_models_section_empty() {
        let conn = test_conn();
        let section = format_models_section(&conn);
        assert!(section.contains("--- Models ---"));
        assert!(section.contains("No model data recorded yet."));
    }

    #[test]
    fn format_models_section_with_data() {
        let conn = test_conn();
        db::insert_token_usage(&conn, "s1", "ts", "claude-sonnet-4-20250514", 1000, 0, 0, 500, 1, 0).unwrap();
        let section = format_models_section(&conn);
        assert!(section.contains("--- Models ---"));
        assert!(section.contains("claude-sonnet-4-20250514"));
        assert!(section.contains("I/O Toks"));
        assert!(section.contains("Sessions"));
        assert!(section.contains("─"));
        assert!(!section.contains("API calls"));
        assert!(section.contains("\u{2588}"));
        // Should show 1,500 (1000 input + 500 output, not including cache tokens)
        assert!(section.contains("1,500"));
    }

    #[test]
    fn format_tokens_section_multi_model_cost() {
        let conn = test_conn();
        db::insert_token_usage(&conn, "s1", "ts", "claude-sonnet-4-20250514", 1_000_000, 0, 0, 0, 1, 0).unwrap();
        db::insert_token_usage(&conn, "s2", "ts", "claude-opus-4-20250514", 1_000_000, 0, 0, 0, 1, 0).unwrap();

        let section = format_tokens_section(&conn);
        // Should show per-model costs when multiple models exist
        assert!(section.contains("Est. cost (claude-sonnet-4-20250514)"));
        assert!(section.contains("Est. cost (claude-opus-4-20250514)"));
        assert!(section.contains("Est. cost (total)"));
    }

    #[test]
    fn format_tokens_section_single_model_no_breakdown() {
        let conn = test_conn();
        db::insert_token_usage(&conn, "s1", "ts", "claude-sonnet-4-20250514", 1000, 0, 0, 500, 1, 0).unwrap();

        let section = format_tokens_section(&conn);
        // Single model should not show per-model breakdown, just total
        assert!(!section.contains("Est. cost (claude-sonnet"));
        assert!(section.contains("Est. cost (total)"));
    }

    #[test]
    fn extract_project_info_worktree_path() {
        let (root, wt) = extract_project_info(
            "/home/user/repos/myproject/.claude/worktrees/cool-feature/src/lib.rs",
        );
        assert_eq!(root, "/home/user/repos/myproject");
        assert_eq!(wt, Some("cool-feature".to_string()));
    }

    #[test]
    fn extract_project_info_worktree_root_no_subdir() {
        let (root, wt) = extract_project_info(
            "/home/user/repos/myproject/.claude/worktrees/cool-feature",
        );
        assert_eq!(root, "/home/user/repos/myproject");
        assert_eq!(wt, Some("cool-feature".to_string()));
    }

    #[test]
    fn extract_project_info_plain_path() {
        let (root, wt) = extract_project_info("/home/user/repos/myproject");
        assert_eq!(root, "/home/user/repos/myproject");
        assert_eq!(wt, None);
    }

    #[test]
    fn extract_project_info_worktrees_trailing_slash() {
        let (root, wt) = extract_project_info(
            "/home/user/repos/myproject/.claude/worktrees/",
        );
        assert_eq!(root, "/home/user/repos/myproject");
        assert_eq!(wt, None);
    }

    #[test]
    fn format_by_project_worktree_nesting() {
        let conn = test_conn();
        let base = "/home/user/repos/myproject";
        let wt1 = format!("{}/.claude/worktrees/feature-a/src", base);
        let wt2 = format!("{}/.claude/worktrees/feature-b", base);

        // 3 tool uses in feature-a worktree, 2 in feature-b, 1 in repo root
        for i in 0..3 {
            db::insert_tool_use(&conn, &format!("a{i}"), "s1", "Read", "ts", &wt1, "{}").unwrap();
        }
        for i in 0..2 {
            db::insert_tool_use(&conn, &format!("b{i}"), "s1", "Read", "ts", &wt2, "{}").unwrap();
        }
        db::insert_tool_use(&conn, "r1", "s1", "Read", "ts", base, "{}").unwrap();

        let section = format_by_project_section(&conn);

        // Total should be 6 (3 + 2 + 1)
        assert!(section.contains("6"));
        // Worktrees should appear with arrow prefix
        assert!(section.contains("\u{21b3} feature-a"));
        assert!(section.contains("\u{21b3} feature-b"));
        // feature-a (3) should come before feature-b (2)
        let a_pos = section.find("feature-a").unwrap();
        let b_pos = section.find("feature-b").unwrap();
        assert!(a_pos < b_pos);
    }

    #[test]
    fn format_by_project_subdir_merging() {
        let conn = test_conn();
        // Tool uses in a subdirectory of a project
        db::insert_tool_use(&conn, "t1", "s1", "Read", "ts", "/home/user/repos/proj", "{}").unwrap();
        db::insert_tool_use(&conn, "t2", "s1", "Read", "ts", "/home/user/repos/proj/src", "{}").unwrap();

        let section = format_by_project_section(&conn);

        // Should show total of 2 for the project root, not separate entries
        assert!(section.contains("2"));
        // Should NOT show /src as a separate line
        let lines: Vec<&str> = section.lines().filter(|l| l.contains("proj")).collect();
        assert_eq!(lines.len(), 1);
    }


}
