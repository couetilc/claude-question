# Entire.io Data Collection Analysis

Reverse-engineered from an entire.io v0.4.2 installation in a Claude Code
project. This document describes everything entire collects, where it gets the
data, and how to replicate the same collection with a small custom tool.

## Table of Contents

- [Integration Points](#integration-points)
- [Data Schema: Claude Code Hooks](#data-schema-claude-code-hooks)
- [Data Schema: Transcript File](#data-schema-transcript-file)
- [Data Schema: Git Hooks](#data-schema-git-hooks)
- [Data Schema: Checkpoint Branch](#data-schema-checkpoint-branch)
- [Data Schema: Local State](#data-schema-local-state)
- [Derived Metrics](#derived-metrics)
- [Implementation Approach](#implementation-approach)

---

## Integration Points

Entire installs itself in five places:

| Location | Purpose |
|---|---|
| `.claude/settings.json` hooks | 7 Claude Code hook subscriptions (session lifecycle + tool use) |
| `.git/hooks/` | 4 git hooks (commit-msg, post-commit, pre-push, prepare-commit-msg) |
| `.entire/` directory | Local state, logs, tmp files, metadata |
| `entire/checkpoints/v1` git branch | Orphan branch storing checkpoints with full transcripts |
| `~/.local/bin/entire` | The CLI binary itself |

---

## Data Schema: Claude Code Hooks

Claude Code hooks receive JSON on **stdin**. Every hook gets these common fields:

```json
{
  "session_id": "uuid",
  "transcript_path": "~/.claude/projects/<encoded-path>/<session-uuid>.jsonl",
  "cwd": "/absolute/path/to/project",
  "permission_mode": "default",
  "hook_event_name": "EventName"
}
```

Entire subscribes to these events:

### SessionStart

```json
{
  "reason": "startup" | "resume" | "clear" | "compact"
}
```

Entire logs `session-start` with the `model_session_id` and `transcript_path`.
It also receives `$CLAUDE_ENV_FILE` as an environment variable — a path to a
file where you can write `KEY=VALUE` lines to inject environment variables into
the session.

### SessionEnd

```json
{
  "reason": "clear" | "logout" | "prompt_input_exit" | "bypass_permissions_disabled" | "other"
}
```

Entire logs `session-end` and transitions the session state machine to `ended`.

### UserPromptSubmit

```json
{
  "prompt": "the user's text"
}
```

Entire logs the event and records a `pre-prompt` snapshot to `.entire/tmp/`:

```json
{
  "session_id": "uuid",
  "timestamp": "2026-02-16T21:17:29Z",
  "untracked_files": null | ["file1.txt", ...],
  "last_transcript_identifier": "uuid",
  "step_transcript_start": 150
}
```

The `step_transcript_start` is the line number in the transcript `.jsonl` file
at the time the prompt was submitted — this lets entire later extract just the
new transcript lines for that turn.

### Stop

```json
{
  "stop_hook_active": false,
  "last_assistant_message": "Claude's final response text"
}
```

Entire logs the event and may trigger a checkpoint save (committing transcript
data to the shadow branch). On `Stop`, entire transitions the session from
`active` to `idle` and computes incremental checkpoints.

### PreToolUse (matcher: `Task`)

```json
{
  "tool_name": "Task",
  "tool_use_id": "toolu_01abc...",
  "tool_input": {
    "prompt": "the task prompt",
    "description": "short description",
    "subagent_type": "Explore" | "general-purpose" | "Plan" | ...,
    "model": "sonnet" | "opus" | "haiku" | null
  }
}
```

Entire logs `pre-task` and saves a snapshot to `.entire/tmp/pre-task-<tool_use_id>.json`:

```json
{
  "tool_use_id": "toolu_01abc...",
  "timestamp": "2026-02-24T01:49:32Z",
  "untracked_files": ["file1.txt", ...]
}
```

### PostToolUse (matcher: `Task`)

```json
{
  "tool_name": "Task",
  "tool_use_id": "toolu_01abc...",
  "tool_input": { ... },
  "tool_response": "the agent's final output"
}
```

Entire logs `post-task` with the `agent_id` and `subagent_type`, then may save
a task-level checkpoint to the shadow branch.

### PostToolUse (matcher: `TodoWrite`)

Same shape as Task but for todo list changes. Entire logs `post-todo`.

---

## Data Schema: Transcript File

The transcript is a JSONL file written by Claude Code itself at the path
provided in `transcript_path`. Entire reads this file to build checkpoints.

Location: `~/.claude/projects/<url-encoded-project-path>/<session-uuid>.jsonl`

Each line is a JSON object. The observed types and their schemas:

### `user` — User messages and tool results

```json
{
  "type": "user",
  "message": {
    "role": "user",
    "content": "the user's prompt text"
  },
  "uuid": "uuid",
  "parentUuid": "uuid",
  "timestamp": "ISO-8601",
  "cwd": "/path/to/project",
  "sessionId": "uuid",
  "version": "2.1.62",
  "gitBranch": "main",
  "isSidechain": false,
  "userType": "external",
  "planContent": "..." // present when exiting plan mode
}
```

When a user line carries a tool result (from Claude calling a tool):

```json
{
  "type": "user",
  "message": { "role": "user", "content": "..." },
  "toolUseResult": {
    "type": "text",
    "file": "..." // optional, for file-reading tools
  },
  "sourceToolAssistantUUID": "uuid"
}
```

### `assistant` — Claude's responses

```json
{
  "type": "assistant",
  "message": {
    "role": "assistant",
    "model": "claude-opus-4-6",
    "id": "msg_01abc...",
    "content": [
      { "type": "thinking", "thinking": "...", "signature": "..." },
      { "type": "text", "text": "visible response" },
      { "type": "tool_use", "id": "toolu_01abc", "name": "Read", "input": { "file_path": "..." } }
    ],
    "usage": {
      "input_tokens": 13,
      "cache_creation_input_tokens": 2526,
      "cache_read_input_tokens": 828411,
      "output_tokens": 1219
    },
    "stop_reason": "end_turn" | "tool_use" | null
  },
  "requestId": "req_01abc...",
  "uuid": "uuid",
  "parentUuid": "uuid",
  "timestamp": "ISO-8601"
}
```

Content array element types:
- `thinking` — extended thinking blocks (with cryptographic `signature`)
- `text` — visible text output
- `tool_use` — tool invocation with `name` and `input`

### `progress` — Hook and tool execution progress

```json
{
  "type": "progress",
  "data": {
    "type": "hook_progress" | "bash_progress",
    "hookEvent": "SessionStart",
    "hookName": "SessionStart:clear",
    "command": "entire hooks claude-code session-start"
  },
  "toolUseID": "uuid",
  "parentToolUseID": "uuid",
  "timestamp": "ISO-8601"
}
```

### `file-history-snapshot` — File backup state

```json
{
  "type": "file-history-snapshot",
  "messageId": "uuid",
  "snapshot": {
    "messageId": "uuid",
    "trackedFileBackups": {},
    "timestamp": "ISO-8601"
  },
  "isSnapshotUpdate": false
}
```

### `system` — System messages

Injected by Claude Code (CLAUDE.md content, system reminders, etc.).

### `queue-operation` — Internal queue state

Rare; internal Claude Code bookkeeping.

### Observed type frequencies (from a real session with 572 lines)

| Type | Count |
|---|---|
| `progress:hook_progress` | 152 |
| `user` | 112 |
| `progress:bash_progress` | 107 |
| `assistant` (tool_use) | 98 |
| `assistant` (text) | 43 |
| `file-history-snapshot` | 36 |
| `system` | 16 |
| `assistant` (thinking) | 6 |
| `queue-operation` | 2 |

---

## Data Schema: Git Hooks

Four git hooks call `entire hooks git <hook-name>`:

### `prepare-commit-msg`

Fires before the commit message editor. Entire injects git trailers:

```
Entire-Session: <session-uuid>
Entire-Strategy: manual-commit
Entire-Agent: Claude Code
Ephemeral-branch: entire/<short-hash>-<hash>
```

These trailers link each git commit to the Claude Code session that produced it.

### `commit-msg`

Fires after the message is written. Entire strips the trailer if there's no
actual user content in the commit message (allows aborting empty commits). Exits
non-zero to abort the commit if needed.

### `post-commit`

Fires after a successful commit. Entire:

1. Detects the `Entire-Checkpoint` trailer on the new commit
2. Reads the transcript file from `transcript_path`
3. Computes **attribution** — diffs the committed files to determine agent vs.
   human lines
4. Condenses the session into a checkpoint on the shadow branch
5. Transitions the session state from `active` to `active_committed`

### `pre-push`

Fires before `git push`. Entire piggybacks on the user's push to also push the
`entire/checkpoints/v1` branch to the same remote. This is how checkpoint data
ends up on GitHub.

---

## Data Schema: Checkpoint Branch

Entire maintains an **orphan branch** `entire/checkpoints/v1` with no shared
history with `main`. Each commit on this branch has the message format
`Checkpoint: <checkpoint-id>` and contains a tree of files organized by
checkpoint ID.

### Tree structure

```
<id-prefix>/<checkpoint-id>/
  metadata.json              # top-level checkpoint metadata
  0/                         # session segment 0
    metadata.json            # per-session metadata
    full.jsonl               # complete Claude Code transcript (copy)
    prompt.txt               # initial user prompt for the session
    context.md               # human-readable summary of all prompts
    content_hash.txt         # sha256 hash of transcript content
  1/                         # session segment 1 (if multiple)
    ...
```

The `<id-prefix>` is the first two characters of the checkpoint ID (git-style
fanout for filesystem performance).

### Top-level `metadata.json`

```json
{
  "cli_version": "0.4.2",
  "checkpoint_id": "7f481f80dd30",
  "strategy": "manual-commit",
  "branch": "main",
  "checkpoints_count": 3,
  "files_touched": [
    "path/to/file1.py",
    "path/to/file2.py"
  ],
  "sessions": [
    {
      "metadata": "/<prefix>/<id>/0/metadata.json",
      "transcript": "/<prefix>/<id>/0/full.jsonl",
      "context": "/<prefix>/<id>/0/context.md",
      "content_hash": "/<prefix>/<id>/0/content_hash.txt",
      "prompt": "/<prefix>/<id>/0/prompt.txt"
    }
  ],
  "token_usage": {
    "input_tokens": 85,
    "cache_creation_tokens": 96583,
    "cache_read_tokens": 5638800,
    "output_tokens": 32028,
    "api_call_count": 65
  }
}
```

### Per-session `metadata.json`

```json
{
  "cli_version": "0.4.2",
  "checkpoint_id": "03f272c1f400",
  "session_id": "uuid",
  "strategy": "manual-commit",
  "created_at": "2026-02-22T22:18:56.061134Z",
  "branch": "main",
  "checkpoints_count": 1,
  "files_touched": ["path/to/file.py", ...],
  "agent": "Claude Code",
  "transcript_identifier_at_start": "uuid",
  "checkpoint_transcript_start": 529,
  "transcript_lines_at_start": 529,
  "token_usage": {
    "input_tokens": 20,
    "cache_creation_tokens": 62909,
    "cache_read_tokens": 1182992,
    "output_tokens": 3502,
    "api_call_count": 14
  },
  "initial_attribution": {
    "calculated_at": "2026-02-22T22:18:55.680356Z",
    "agent_lines": 36,
    "human_added": 0,
    "human_modified": 11,
    "human_removed": 2,
    "total_committed": 47,
    "agent_percentage": 76.6
  }
}
```

### `context.md`

A human-readable document generated from the transcript:

```markdown
# Session Context

## User Prompts

### Prompt 1
<full text of user's first prompt>

### Prompt 2
<full text of user's second prompt>

### Prompt 3
[Request interrupted by user for tool use]
...
```

### `content_hash.txt`

A single line: `sha256:<hex-digest>` — hash of the `full.jsonl` content for
deduplication.

### `prompt.txt`

The full text of the first user prompt that started the session.

### Commit metadata

Each checkpoint commit also has trailers in its commit message:

```
Entire-Session: <session-uuid>
Entire-Strategy: manual-commit
Entire-Agent: Claude Code
Ephemeral-branch: entire/<short-hash>-<hash>
```

### Shadow branches

During active sessions, entire maintains ephemeral branches named
`entire/<commit-hash>-<hash>` for in-progress work. These are deleted when the
session transitions to idle after a commit.

---

## Data Schema: Local State

### `.entire/settings.json`

```json
{
  "strategy": "manual-commit",
  "enabled": true,
  "telemetry": true
}
```

### `.entire/logs/entire.log`

Structured JSONL log. Each line:

```json
{
  "time": "2026-02-16T15:55:53.313809-05:00",
  "level": "INFO",
  "msg": "<event-name>",
  "component": "<component>",
  "session_id": "uuid",
  ...event-specific fields
}
```

Observed event types and their fields:

| msg | component | extra fields |
|---|---|---|
| `session-start` | `hooks` | `agent`, `hook`, `hook_type`, `model_session_id`, `transcript_path` |
| `session-end` | `hooks` | `agent`, `hook`, `model_session_id` |
| `user-prompt-submit` | `hooks` | `agent`, `hook`, `model_session_id`, `transcript_path` |
| `stop` | `hooks` | `agent`, `hook`, `model_session_id`, `transcript_path` |
| `pre-task` | `hooks` | `agent`, `hook`, `hook_type: "subagent"`, `tool_use_id` |
| `post-task` | `hooks` | `agent`, `hook`, `hook_type: "subagent"`, `tool_use_id`, `agent_id`, `subagent_type` |
| `phase transition` | `session` | `event` (TurnStart/TurnEnd/GitCommit/SessionStop), `from`, `to` |
| `checkpoint saved` | `checkpoint` | `strategy`, `checkpoint_type`, `checkpoint_count`, `modified_files`, `new_files`, `deleted_files`, `shadow_branch`, `branch_created` |
| `task checkpoint saved` | `checkpoint` | `strategy`, `checkpoint_type: "task"`, `tool_use_id`, `subagent_type`, `modified_files`, `shadow_branch` |
| `attribution calculated` | `attribution` | `agent_lines`, `human_added`, `human_modified`, `human_removed`, `total_committed`, `agent_percentage`, `accumulated_user_added`, `accumulated_user_removed`, `files_touched` |
| `session condensed` | `checkpoint` | `checkpoint_id`, `checkpoints_condensed`, `transcript_lines` |
| `shadow branch deleted` | `checkpoint` | `shadow_branch` |
| `prepare-commit-msg: agent commit trailer added` | `checkpoint` | `source`, `checkpoint_id`, `session_id` |

### Session state machine

Entire tracks a per-session state with these transitions:

```
(none) --SessionStart--> (none)
(none) --TurnStart-----> active
active --TurnEnd-------> idle
active --GitCommit-----> active_committed
active_committed --TurnEnd--> idle
idle   --TurnStart-----> active
idle   --SessionStop---> ended
active --SessionStop---> ended
```

### `.entire/tmp/`

Ephemeral snapshot files for in-progress work:

- `pre-prompt-<session-id>.json` — saved on each UserPromptSubmit
- `pre-task-<tool-use-id>.json` — saved on each PreToolUse(Task)

### `.entire/metadata/`

Access is denied to Claude via the permissions config. Likely stores condensed
session summaries and accumulated attribution data.

---

## Derived Metrics

Entire computes these metrics from the raw data:

### Token usage (aggregated from transcript)

- `input_tokens` — prompt tokens sent to Claude
- `cache_creation_tokens` — tokens written to prompt cache
- `cache_read_tokens` — tokens served from cache
- `output_tokens` — tokens Claude generated
- `api_call_count` — number of API round-trips

### Attribution (computed from git diffs)

- `agent_lines` — lines of code written by Claude
- `human_added` — lines added by the human after Claude's output
- `human_modified` — lines modified by the human
- `human_removed` — lines removed by the human
- `total_committed` — total lines in the commit
- `agent_percentage` — `agent_lines / total_committed * 100`

Attribution is computed by diffing the working tree at checkpoint time against
the state before the session started, then classifying each changed line as
agent-authored or human-edited.

---

## Implementation Approach

The current `claude-track` binary handles PostToolUse logging. To replicate
entire's full data collection, there are three tiers of increasing complexity.

### Tier 1: Comprehensive Event Logging (extend current tool)

Expand `claude-track` to handle all hook events, not just PostToolUse. This
gives you a complete event log equivalent to entire's `entire.log`.

**New subcommand**: `claude-track hook` (replaces `log`, handles all events).

**New hooks to register** (in `install` subcommand):

```json
{
  "hooks": {
    "SessionStart": [{
      "matcher": "",
      "hooks": [{ "type": "command", "command": "claude-track hook" }]
    }],
    "SessionEnd": [{
      "matcher": "",
      "hooks": [{ "type": "command", "command": "claude-track hook" }]
    }],
    "UserPromptSubmit": [{
      "matcher": "",
      "hooks": [{ "type": "command", "command": "claude-track hook" }]
    }],
    "Stop": [{
      "matcher": "",
      "hooks": [{ "type": "command", "command": "claude-track hook" }]
    }],
    "PreToolUse": [{
      "matcher": ".*",
      "hooks": [{ "type": "command", "command": "claude-track hook" }]
    }],
    "PostToolUse": [{
      "matcher": ".*",
      "hooks": [{ "type": "command", "command": "claude-track hook" }]
    }]
  }
}
```

**Event-specific stdin fields to capture**:

```rust
struct HookInput {
    // Common fields (all events)
    session_id: Option<String>,
    transcript_path: Option<String>,
    cwd: Option<String>,
    hook_event_name: Option<String>,

    // UserPromptSubmit
    prompt: Option<String>,

    // Stop
    last_assistant_message: Option<String>,
    stop_hook_active: Option<bool>,

    // SessionStart / SessionEnd
    reason: Option<String>,

    // PreToolUse / PostToolUse
    tool_name: Option<String>,
    tool_use_id: Option<String>,
    tool_input: Option<serde_json::Value>,
    tool_response: Option<serde_json::Value>,
}
```

**Output log format** (one JSONL line per event):

```json
{
  "ts": "2026-02-27T12:00:00Z",
  "event": "PostToolUse",
  "session": "uuid",
  "cwd": "/path",
  "tool": "Read",
  "tool_use_id": "toolu_01abc",
  "input": {"file_path": "/foo"},
  "response_summary": "200 lines"
}
```

For `UserPromptSubmit`, log `prompt`. For `Stop`, log `last_assistant_message`.
For `SessionStart`/`SessionEnd`, log `reason`. This single JSONL file replaces
both entire's `entire.log` and the `pre-prompt`/`pre-task` tmp files.

### Tier 2: Transcript Archival (checkpoint system)

Archive complete session transcripts tied to git commits, equivalent to entire's
checkpoint branch.

**Approach A: Copy transcripts into a data directory (simpler)**

On `Stop` events, copy the transcript `.jsonl` file (at `transcript_path`) into
a local data directory like `.claude-track/transcripts/<session-id>.jsonl`. On
git commit (via a `post-commit` hook), snapshot the transcript and record which
files were committed.

Pros: No orphan branch complexity. Data stays local by default.
Cons: Not versioned with git. Large transcripts bloat the directory.

**Approach B: Orphan branch (entire's approach)**

Maintain an orphan branch `claude-track/sessions` and commit transcript data
there on each git commit. This keeps session history in git without polluting
the main branch.

Implementation:
1. `post-commit` hook: read the transcript, compute a checkpoint ID, write
   files to the orphan branch using `git hash-object` + `git mktree` +
   `git commit-tree` (no checkout needed)
2. `pre-push` hook: push the orphan branch alongside the user's push
3. Generate `context.md` by extracting user messages from the transcript

The orphan-branch approach requires careful git plumbing but has the advantage
that `git clone` of the repo automatically includes the session history.

**Suggested checkpoint schema** (simplified from entire):

```
<session-id>/
  metadata.json    # session metadata + token usage
  transcript.jsonl # full transcript copy
  prompts.md       # human-readable prompt summary
```

### Tier 3: Attribution Tracking

Compute how much code Claude wrote vs. the human, per commit.

**How entire does it**: On `post-commit`, diff the committed changes against the
pre-session working tree state. Lines that match Claude's tool_use outputs
(Write/Edit tool calls in the transcript) are attributed to the agent; all other
changes are attributed to the human.

**Simpler approach**: Track file state at session start (hash of each file).
On commit, diff each committed file against its session-start state. Cross-
reference with Write/Edit tool calls from the event log to classify lines.

```rust
struct Attribution {
    agent_lines: u32,
    human_added: u32,
    human_modified: u32,
    human_removed: u32,
    total_committed: u32,
    agent_percentage: f64,
}
```

**Token usage** can be extracted from the transcript by summing `usage` fields
on `assistant` type lines:

```rust
struct TokenUsage {
    input_tokens: u64,
    cache_creation_tokens: u64,
    cache_read_tokens: u64,
    output_tokens: u64,
    api_call_count: u64,
}
```

### Recommended Implementation Order

1. **Extend HookInput model** to capture all event types (Tier 1)
2. **Expand `install`** to register all hook events
3. **Add `hook` subcommand** that dispatches based on `hook_event_name`
4. **Update `stats`** to report on session lifecycle, prompts, token usage
5. **Add `post-commit` git hook** for transcript archival (Tier 2)
6. **Add attribution** as a stretch goal (Tier 3)

### Architecture Notes

- The binary must be fast — hooks block Claude's execution. Entire solves this
  by doing minimal work in the hook (append to log, save tmp file) and deferring
  heavy work (transcript parsing, attribution) to `post-commit` time.
- Use `serde_json::Value` for `tool_input` and `tool_response` to avoid
  needing schemas for every tool type.
- The transcript file is append-only during a session, so you can efficiently
  read only new lines by tracking the last-read line offset (like entire's
  `step_transcript_start`).
- For the orphan branch approach, use git plumbing commands (`hash-object`,
  `mktree`, `commit-tree`, `update-ref`) to avoid needing a checkout. This is
  safe to run concurrently with the user's working tree.
