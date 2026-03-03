# UI Design: claude-track Web Dashboard

## Data Hierarchy

```
Session                          (root entity)
│
├── metadata
│   ├── session_id               (PK, universal join key)
│   ├── started_at / ended_at    (session boundary timestamps)
│   ├── start_reason / end_reason
│   ├── cwd                      (project directory)
│   └── transcript_path
│
├── Token Usage                  (1:1 with session, aggregate)
│   ├── model
│   ├── input_tokens / output_tokens
│   ├── cache_creation_tokens / cache_read_tokens
│   └── api_call_count
│
├── Turns                        (derived, not stored — see below)
│   │
│   ├── Turn 1
│   │   ├── Prompt               (user message that starts the turn)
│   │   │   ├── timestamp
│   │   │   └── prompt_text
│   │   │
│   │   └── Tool Uses[]          (Claude's actions in response)
│   │       ├── tool_name
│   │       ├── tool_use_id
│   │       ├── timestamp
│   │       ├── cwd
│   │       ├── input            (JSON)
│   │       └── response_summary (first 500 chars)
│   │
│   ├── Turn 2
│   │   ├── Prompt
│   │   └── Tool Uses[]
│   │
│   └── ...
│
└── Plans[]                      (subset of tool_uses where tool_name = ExitPlanMode)
    ├── tool_use_id              (joins to tool_uses)
    ├── timestamp
    └── plan_text
```

## Shared Data Points Across the Hierarchy

| Data Point        | Tables Present In                          | Role                                      |
|-------------------|--------------------------------------------|--------------------------------------------|
| `session_id`      | all 5 tables                               | Universal join key, groups everything      |
| `timestamp`       | prompts, tool_uses, token_usage, plans     | Temporal ordering, turn derivation         |
| `cwd`             | sessions, tool_uses                        | Tracks directory context (can change mid-session) |
| `tool_use_id`     | tool_uses, plans                           | Links plan records to their tool_use row   |
| `transcript_path` | sessions (stored), token_usage (consumed)  | Source for token aggregation               |

## Deriving Turns: Tying Prompts to Tool Calls

The database has no explicit turn/exchange concept. Prompts and tool calls both have
timestamps and share `session_id`. The reconstruction rule is:

**A turn is a prompt plus all tool calls that occur after it and before the next prompt
(or session end).**

### SQL to derive turns

```sql
-- Assign each tool_use to the most recent preceding prompt in the same session
SELECT
    t.id,
    t.tool_name,
    t.timestamp AS tool_ts,
    p.id AS prompt_id,
    p.prompt_text,
    p.timestamp AS prompt_ts
FROM tool_uses t
JOIN prompts p ON p.session_id = t.session_id
WHERE p.timestamp = (
    SELECT MAX(p2.timestamp)
    FROM prompts p2
    WHERE p2.session_id = t.session_id
      AND p2.timestamp <= t.timestamp
)
ORDER BY t.timestamp;
```

### Edge cases

- **Tool calls before first prompt**: Session hooks (SessionStart) can fire before any
  UserPromptSubmit. These tool calls have no parent prompt — treat as "session setup".
- **Simultaneous timestamps**: Prompts and tool calls recorded in the same second.
  The prompt logically comes first (user submits, then Claude acts). Ties break in
  favor of the prompt owning subsequent tool calls.
- **No prompts in session**: Some sessions may have tool calls but no recorded prompts
  (e.g., if hooks were installed mid-session). These tool calls are unattributed.

## Web UI Layout

### Page Structure

```
┌─────────────────────────────────────────────────────┐
│  claude-track                          [settings]   │
├─────────────────────────────────────────────────────┤
│                                                     │
│  Dashboard (aggregate stats)                        │
│  ┌─────────┐  ┌─────────┐  ┌─────────┐  ┌────────┐│
│  │Sessions │  │ Tokens  │  │ Tools   │  │ Cost   ││
│  │  count  │  │  total  │  │  total  │  │ total  ││
│  └─────────┘  └─────────┘  └─────────┘  └────────┘│
│                                                     │
│  ┌─────────────────────────────────────────────────┐│
│  │ Activity timeline (sessions per day, bar chart) ││
│  └─────────────────────────────────────────────────┘│
│                                                     │
│  Session List                                       │
│  ┌─────────────────────────────────────────────────┐│
│  │ ▶ 2026-03-03 15:24  /repos/claude-question     ││
│  │   model: opus-4  │ 8 prompts │ 142 tool calls  ││
│  │   tokens: 45k in / 12k out │ cost: $2.34       ││
│  ├─────────────────────────────────────────────────┤│
│  │ ▶ 2026-03-02 01:11  /repos/dotfiles            ││
│  │   model: sonnet-4 │ 3 prompts │ 57 tool calls  ││
│  └─────────────────────────────────────────────────┘│
│                                                     │
│  Expanded Session (click ▶ to expand)               │
│  ┌─────────────────────────────────────────────────┐│
│  │ ▼ 2026-03-03 15:24  /repos/claude-question     ││
│  │                                                 ││
│  │  Turn 1                                         ││
│  │  ┌──────────────────────────────────────────┐   ││
│  │  │ 👤 "make sure entire is removed from..." │   ││
│  │  │                                          │   ││
│  │  │  15:00:37  Grep  (pattern: "entire")     │   ││
│  │  │  15:00:38  Grep  (pattern: "entire")     │   ││
│  │  │  15:00:38  Glob  (*.json)                │   ││
│  │  │  15:00:44  Read  settings.json           │   ││
│  │  │  15:00:48  Write settings.json           │   ││
│  │  └──────────────────────────────────────────┘   ││
│  │                                                 ││
│  │  Turn 2                                         ││
│  │  ┌──────────────────────────────────────────┐   ││
│  │  │ 👤 "Can you check other projects too?"   │   ││
│  │  │                                          │   ││
│  │  │  15:05:38  Bash  (find ~/.claude/...)    │   ││
│  │  │  15:05:38  Glob  (**/settings.json)      │   ││
│  │  │  15:05:39  Bash  (cat ...)               │   ││
│  │  └──────────────────────────────────────────┘   ││
│  │                                                 ││
│  └─────────────────────────────────────────────────┘│
│                                                     │
└─────────────────────────────────────────────────────┘
```

### Views

1. **Dashboard** — Aggregate stats across all sessions: total sessions, tokens,
   costs, tool call breakdown. Activity heatmap or timeline.

2. **Session list** — Sortable/filterable table. Each row shows session metadata,
   prompt count, tool count, token summary, estimated cost. Click to expand.

3. **Session detail** — Reconstructed turn-by-turn conversation. Each turn shows
   the user prompt followed by Claude's tool calls with summarized inputs.
   Token usage sidebar for the session.

4. **Analytics** — Tool usage distribution (pie/bar), cost over time (line),
   most-edited files, most-run commands, model usage breakdown.

### Tool Call Display

Tool calls should render contextually based on `tool_name`:

| Tool      | Display input as                          |
|-----------|-------------------------------------------|
| Read      | File path                                 |
| Write     | File path                                 |
| Edit      | File path + old/new snippet               |
| Bash      | Command string                            |
| Grep      | Pattern + path                            |
| Glob      | Pattern                                   |
| Agent     | Description + subagent type               |
| WebFetch  | URL                                       |
| WebSearch | Query                                     |

### Cost Estimation

Derive from token_usage using per-model pricing:

```
cost = (input_tokens * input_price)
     + (cache_creation_tokens * cache_write_price)
     + (cache_read_tokens * cache_read_price)
     + (output_tokens * output_price)
```

Model prices should be configurable (stored in a config, not hardcoded).
