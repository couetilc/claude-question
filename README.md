# claude-track

A local-only analytics tracker for [Claude Code](https://docs.anthropic.com/en/docs/claude-code). It hooks into Claude Code's event system to record your sessions, prompts, tool usage, and token consumption in a SQLite database on your machine. Nothing leaves your computer.

## Quick start

```sh
cargo build --release
./target/release/claude-track install
```

That's it. The installer copies the binary to `~/.local/bin/` and registers hooks in `~/.claude/settings.json`. Tracking begins on your next Claude Code session. View your stats anytime:

```sh
claude-track stats
```

## What it tracks

claude-track captures six events during a Claude Code session:

| Event | What's recorded |
|---|---|
| **Session start/end** | When you opened and closed Claude Code, from which directory, and why (new session vs. resume) |
| **Prompts** | The text of each prompt you submit |
| **Tool use** | Which tools Claude called (Read, Bash, Write, etc.), what input they received, and a summary of the response |
| **Token usage** | Input/output tokens, cache hits, API call counts, and which model was used |

All data lives in `~/.claude/claude-track.db` — a single SQLite file you can query directly:

```sh
claude-track query "SELECT tool_name, COUNT(*) as n FROM tool_uses GROUP BY tool_name ORDER BY n DESC LIMIT 10"
```

## Subcommands

| Command | Purpose |
|---|---|
| `install` | Copy the binary to `~/.local/bin/` and register hooks (idempotent) |
| `uninstall` | Remove hooks and optionally delete the database |
| `stats` | Print a summary of sessions, token costs, top tools, activity over time, and per-project breakdowns |
| `query` | Run arbitrary SQL against the tracking database |
| `migrate` | Import records from the legacy `~/.claude/tool-usage.jsonl` format |
| `hook` | Internal entrypoint called by Claude Code (you won't run this directly) |

## How it works

Claude Code supports [hooks](https://docs.anthropic.com/en/docs/claude-code/hooks) — shell commands that run in response to lifecycle events. claude-track registers a single binary as the handler for all six hook events. When Claude Code fires an event, it pipes JSON to stdin, and claude-track parses it and writes to SQLite.

A few design choices worth noting:

- **Single binary, single command.** All six hooks call `claude-track hook`. The binary reads `hook_event_name` from the JSON payload and dispatches internally, keeping installation trivial.

- **Incremental transcript parsing.** Token usage is extracted from Claude Code's transcript files. Rather than re-parsing the entire file on every Stop event, claude-track tracks a byte offset and only reads new lines. If the file shrinks (e.g. a new session reuses the path), it resets and parses from the beginning.

- **Upsert-based token aggregation.** Each session gets one token usage row, updated cumulatively. This avoids duplicate counting when multiple Stop events fire for the same session.

- **Idempotent installation.** Running `install` multiple times is safe. It detects existing hooks, cleans up stale entries from previous binary paths, and deduplicates any token records.

## Stats output

`claude-track stats` produces a report covering:

- Total sessions and cumulative duration
- Token usage with estimated API costs, broken down by model
- Most-used tools and most-run bash commands
- Activity by date
- Per-project breakdowns (with worktree nesting)

## Uninstalling

```sh
claude-track uninstall
```

This removes the hooks from `~/.claude/settings.json` and prompts you about whether to delete the binary and database.
