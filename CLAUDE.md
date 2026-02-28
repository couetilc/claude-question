# claude-track

A single Rust binary that tracks Claude Code usage analytics via 6 hooks, storing data in a local SQLite database. Zero runtime dependencies (SQLite is bundled).

## Build & run

```
cargo build --release
./target/release/claude-track <subcommand>
```

## Subcommands

- **`hook`** — Hook entrypoint. Reads JSON from stdin, dispatches by `hook_event_name`, writes to SQLite (`~/.claude/claude-track.db`). Always exits 0.
- **`stats`** — Queries SQLite and prints usage statistics (sessions, token usage with cost estimates, prompts, tool calls, top files, top bash commands, activity by date, by project).
- **`install`** — Registers all 6 hooks in `~/.claude/settings.json`.
- **`uninstall`** — Removes all hooks from settings and optionally deletes the database and legacy log.
- **`migrate`** — Imports legacy `~/.claude/tool-usage.jsonl` records into the `tool_uses` table.
- **`query`** — Runs an ad-hoc SQL query against the tracking database. Usage: `claude-track query "SELECT ..."`.

## Hook coverage

| Hook Event | What we capture |
|---|---|
| SessionStart | Insert into `sessions` (session_id, started_at, start_reason, cwd, transcript_path) |
| SessionEnd | Update `sessions` row (ended_at, end_reason) |
| UserPromptSubmit | Insert into `prompts` (session_id, timestamp, prompt_text) |
| Stop | Parse transcript file, aggregate token usage, insert into `token_usage` |
| PreToolUse | Insert into `tool_uses` (tool_name, tool_use_id, input) |
| PostToolUse | Update matching `tool_uses` row with response_summary, or insert if no PreToolUse |

## Project layout

```
Cargo.toml
src/
  main.rs              # CLI definition (clap derive) + subcommand dispatch
  models.rs            # HookInput, ToolCall, transcript parsing structs (serde)
  db.rs                # SQLite schema, init, insert/update/query helpers
  commands/
    mod.rs
    hook.rs            # hook subcommand (dispatches all 6 events)
    stats.rs           # stats subcommand (queries SQLite)
    install.rs         # install subcommand (registers 6 hooks)
    uninstall.rs       # uninstall subcommand (removes hooks + data)
    migrate.rs         # migrate subcommand (JSONL → SQLite)
    query.rs           # query subcommand (ad-hoc SQL)
tests/
  integration.rs       # CLI integration tests
```

## Database schema

Location: `~/.claude/claude-track.db`

4 normalized tables: `sessions`, `tool_uses`, `prompts`, `token_usage`. See `src/db.rs` for full schema.

## Key dependencies

- `clap` (derive) — CLI parsing
- `serde` + `serde_json` — JSON serialization
- `chrono` — UTC timestamps
- `dirs` — cross-platform home directory resolution
- `rusqlite` (bundled) — SQLite database

## Testing

All tests must pass with 100% code coverage.

```
cargo test
cargo tarpaulin --skip-clean
```
