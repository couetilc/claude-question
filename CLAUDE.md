# claude-track

A single Rust binary that tracks Claude Code tool usage via a PostToolUse hook. Replaces the previous bash scripts + jq dependency with zero runtime dependencies.

## Build & run

```
cargo build --release
./target/release/claude-track <subcommand>
```

## Subcommands

- **`log`** — Hook entrypoint. Reads JSON from stdin, appends a timestamped JSONL record to `~/.claude/tool-usage.jsonl`. Always exits 0.
- **`stats`** — Parses the JSONL log and prints usage statistics (calls by tool, date, top files read, top bash commands, calls by project directory).
- **`install`** — Registers the `claude-track log` command as a PostToolUse hook in `~/.claude/settings.json`.
- **`uninstall`** — Removes the hook from settings and optionally deletes the log file.

## Project layout

```
Cargo.toml
src/
  main.rs              # CLI definition (clap derive) + subcommand dispatch
  models.rs            # HookInput, ToolCall structs (serde)
  commands/
    mod.rs
    log.rs             # log subcommand
    stats.rs           # stats subcommand
    install.rs         # install subcommand
    uninstall.rs       # uninstall subcommand
```

## Key dependencies

- `clap` (derive) — CLI parsing
- `serde` + `serde_json` — JSON serialization
- `chrono` — UTC timestamps
- `dirs` — cross-platform home directory resolution

## Testing

All tests must pass with 100% code coverage.

```
cargo test
cargo tarpaulin --skip-clean
```

## Data format

Each line in `~/.claude/tool-usage.jsonl` is a JSON object:

```json
{"ts":"2026-02-27T12:00:00Z","tool":"Read","session":"abc123","cwd":"/path/to/project","input":{"file_path":"/foo/bar.rs"}}
```
