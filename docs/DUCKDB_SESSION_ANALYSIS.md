# Analyzing Claude Code Session JSONL with DuckDB

## Overview

Claude Code stores session transcripts as JSONL files under `~/.claude/projects/<project>/`. Each line is a JSON object representing a message, tool use, progress event, or system event. DuckDB can natively ingest these files and query the nested JSON structures with SQL.

## Setup

```bash
brew install duckdb

# Copy a session JSONL to the working directory
cp ~/.claude/projects/<project>/<session-id>.jsonl session.jsonl

# Import into DuckDB
duckdb session.duckdb <<'SQL'
CREATE TABLE raw AS
SELECT * FROM read_json_auto(
    'session.jsonl',
    format='newline_delimited',
    maximum_object_size=10485760
);
SQL
```

DuckDB auto-detects the schema from the JSONL, including nested structs like `message.usage.*` and `data.*`.

## Schema

The auto-inferred schema includes 30 columns. Key ones:

| Column | Type | Description |
|---|---|---|
| `type` | VARCHAR | `user`, `assistant`, `progress`, `system`, `file-history-snapshot` |
| `timestamp` | VARCHAR | ISO 8601 timestamp |
| `sessionId` | UUID | Session identifier |
| `message` | STRUCT | Contains `role`, `model`, `stop_reason`, `usage`, `content` |
| `message.usage` | STRUCT | Token counts: `input_tokens`, `cache_read_input_tokens`, `output_tokens` |
| `data` | STRUCT | Hook/progress metadata: `hookEvent`, `command`, `elapsedTimeSeconds` |
| `toolUseID` | VARCHAR | Links tool calls to their results |

## Example Queries

### Message type distribution

```sql
SELECT type, count(*) as cnt
FROM raw
GROUP BY type
ORDER BY cnt DESC;
```

### Token usage per assistant turn

```sql
SELECT
    message.id as msg_id,
    message.model as model,
    message.stop_reason as stop_reason,
    message.usage.input_tokens as input_tokens,
    message.usage.cache_read_input_tokens as cache_read_tokens,
    message.usage.output_tokens as output_tokens,
    timestamp
FROM raw
WHERE type = 'assistant' AND message.usage IS NOT NULL
ORDER BY timestamp;
```

### Total token usage and estimated cost

```sql
SELECT
    count(*) as turns,
    sum(message.usage.input_tokens) as total_input,
    sum(message.usage.cache_read_input_tokens) as total_cache_read,
    sum(message.usage.output_tokens) as total_output,
    -- Opus pricing: $15/M input, $1.50/M cache read, $75/M output
    round(sum(message.usage.input_tokens) * 15.0 / 1000000, 4) as input_cost,
    round(sum(message.usage.cache_read_input_tokens) * 1.50 / 1000000, 4) as cache_read_cost,
    round(sum(message.usage.output_tokens) * 75.0 / 1000000, 4) as output_cost,
    round(
        sum(message.usage.input_tokens) * 15.0 / 1000000 +
        sum(message.usage.cache_read_input_tokens) * 1.50 / 1000000 +
        sum(message.usage.output_tokens) * 75.0 / 1000000
    , 4) as total_cost
FROM raw
WHERE type = 'assistant' AND message.usage IS NOT NULL;
```

## Sample Results

Session `9e8009d5` from the `claude-question` project (94 lines, ~20 min session):

**Message distribution:**

| Type | Count |
|---|---|
| progress | 48 |
| assistant | 20 |
| user | 17 |
| file-history-snapshot | 5 |
| system | 4 |

**Token usage summary:**

| Metric | Value |
|---|---|
| Assistant turns | 20 |
| Uncached input tokens | 109 |
| Cache-read tokens | 766,060 |
| Output tokens | 2,075 |
| Input cost | $0.0016 |
| Cache-read cost | $1.1491 |
| Output cost | $0.1556 |
| **Total estimated cost** | **$1.31** |

Nearly all input came from prompt caching (~99.99%), keeping costs well below what uncached reads would cost ($11.49 vs $1.15 at cache pricing).

## Conversation Flow Visualization

Each unique `message.id` in the assistant rows represents one API round-trip to Claude. By grouping on `message.id` and measuring gaps between consecutive calls, we can classify what happens between turns:

- **User idle** (>60s gap) — the human is reading, thinking, or away
- **Tool execution** (5–60s gap) — a tool ran and returned results
- **Agentic loop** (<5s gap) — Claude immediately made another API call

### Conversation flow query

```sql
WITH api_calls AS (
    SELECT
        message.id as msg_id,
        min(timestamp::TIMESTAMP) as started,
        max(timestamp::TIMESTAMP) as ended,
        max(message.stop_reason) as stop_reason,
        max(message.usage.output_tokens) as output_tokens,
        max(message.usage.cache_read_input_tokens) as cache_read_tokens
    FROM raw
    WHERE type = 'assistant' AND message.id IS NOT NULL
    GROUP BY message.id
),
with_gaps AS (
    SELECT *,
        row_number() OVER (ORDER BY started) as turn,
        epoch_ms(started - lag(ended) OVER (ORDER BY started)) / 1000.0 as gap_s,
        epoch_ms(ended - started) / 1000.0 as response_s
    FROM api_calls
),
with_convos AS (
    SELECT *,
        sum(CASE WHEN gap_s > 60 OR gap_s IS NULL THEN 1 ELSE 0 END)
            OVER (ORDER BY turn) as convo_num
    FROM with_gaps
)
SELECT
    convo_num as "Prompt#",
    turn as "Turn",
    strftime(started, '%H:%M:%S') as "Time",
    CASE
        WHEN gap_s IS NULL THEN 'SESSION START'
        WHEN gap_s > 60 THEN 'USER (' || round(gap_s/60, 1) || 'm idle)'
        WHEN gap_s > 5 THEN '  tool (' || round(gap_s, 1) || 's)'
        ELSE '  loop (' || round(gap_s, 1) || 's)'
    END as "Event",
    CASE stop_reason
        WHEN 'end_turn' THEN 'REPLY'
        WHEN 'tool_use' THEN 'TOOL'
        ELSE stop_reason
    END as "Action",
    output_tokens as "Tokens",
    repeat('*', least(output_tokens / 5, 40)::INT) as "Token Bar"
FROM with_convos
ORDER BY turn;
```

### Sample flow output

```
 Prompt# | Turn | Time     | Event            | Action | Tokens | Token Bar
---------+------+----------+------------------+--------+--------+------------------------------------------
       1 |    1 | 02:25:02 | SESSION START    | TOOL   |    193 | ***************************************
       1 |    2 | 02:25:11 |   tool (7.5s)    | REPLY  |    172 | **********************************
       2 |    3 | 02:30:06 | USER (4.9m idle) | TOOL   |    189 | **************************************
       3 |    4 | 02:32:21 | USER (2.2m idle) | TOOL   |    208 | ****************************************
       4 |    5 | 02:33:26 | USER (1.1m idle) | REPLY  |    187 | *************************************
       5 |    6 | 02:42:52 | USER (9.4m idle) | TOOL   |    243 | ****************************************
       5 |    7 | 02:42:55 |   loop (3.5s)    | TOOL   |    101 | ********************
       5 |    8 | 02:42:58 |   loop (2.5s)    | TOOL   |    167 | *********************************
       5 |    9 | 02:43:02 |   loop (4.4s)    | REPLY  |     36 | *******
       5 |   10 | 02:43:08 |   tool (6.2s)    | TOOL   |    228 | ****************************************
       5 |   11 | 02:43:12 |   loop (2.8s)    | TOOL   |    167 | *********************************
       6 |   12 | 02:45:55 | USER (2.7m idle) | REPLY  |     44 | *********
```

Reading the flow: Prompt #5 triggered a 6-turn agentic loop — Claude called tools 5 times before delivering a final reply. Prompts #2–4 were simpler one-shot tool→reply patterns.

### Time budget

Where did the 21-minute session go?

| Category | Seconds | Minutes | % |
|---|---|---|---|
| User idle time | 1,220.8 | 20.3 | 97.4% |
| Tool execution | 26.9 | 0.4 | 2.1% |
| Claude response time | 5.3 | 0.1 | 0.4% |
| **Total** | **1,253.0** | **20.9** | **100%** |

The bottleneck is overwhelmingly human think-time. Claude's actual compute was 5.3 seconds across 12 API calls — the rest was waiting for tools (27s) and the user (20+ minutes).

## Prompt Complexity Ranking

Not all prompts are equal. A two-word "do that" can trigger a 6-turn agentic loop while a detailed question gets a single-turn reply. We can rank prompts by the amount of agentic work they trigger.

### Complexity classification

| API Calls | Label | Meaning |
|---|---|---|
| 10+ | HEAVY | Deep multi-step agentic work |
| 5–9 | COMPLEX | Significant tool chaining |
| 3–4 | MODERATE | A few tool calls before replying |
| 2 | SIMPLE | One tool call then reply |
| 1 | TRIVIAL | Direct answer, no tools |

### Complexity ranking query

```sql
WITH api_calls AS (
    SELECT
        message.id as msg_id,
        min(timestamp::TIMESTAMP) as started,
        max(timestamp::TIMESTAMP) as ended,
        max(message.stop_reason) as stop_reason,
        max(message.usage.output_tokens) as output_tokens
    FROM raw
    WHERE type = 'assistant' AND message.id IS NOT NULL
    GROUP BY message.id
),
with_gaps AS (
    SELECT *,
        row_number() OVER (ORDER BY started) as turn,
        epoch_ms(started - lag(ended) OVER (ORDER BY started)) / 1000.0 as gap_s
    FROM api_calls
),
with_prompts AS (
    SELECT *,
        sum(CASE WHEN gap_s > 60 OR gap_s IS NULL THEN 1 ELSE 0 END)
            OVER (ORDER BY turn) as prompt_num
    FROM with_gaps
),
prompt_stats AS (
    SELECT
        prompt_num,
        count(*) as api_calls,
        count(*) FILTER (WHERE stop_reason = 'tool_use') as tool_calls,
        sum(output_tokens) as total_tokens,
        round(epoch_ms(max(ended) - min(started)) / 1000.0, 1) as duration_s,
        min(started) as first_ts
    FROM with_prompts
    GROUP BY prompt_num
)
SELECT
    prompt_num as "#",
    api_calls,
    tool_calls as tools,
    total_tokens as tokens,
    duration_s || 's' as duration,
    CASE
        WHEN api_calls >= 10 THEN '**** HEAVY'
        WHEN api_calls >= 5 THEN '*** COMPLEX'
        WHEN api_calls >= 3 THEN '** MODERATE'
        WHEN api_calls >= 2 THEN '* SIMPLE'
        ELSE '  TRIVIAL'
    END as complexity,
    strftime(first_ts, '%H:%M:%S') as start_time
FROM prompt_stats
ORDER BY api_calls DESC;
```

### Sample results (large session, 1.1MB)

```
 # | api_calls | tools | tokens | duration | complexity  | start_time
---+-----------+-------+--------+----------+-------------+-----------
 3 |        12 |     9 |   1724 | 111.8s   | **** HEAVY  | 02:05:39
 7 |         9 |     7 |   2126 | 143.5s   | *** COMPLEX | 02:47:11
 1 |         7 |     6 |   4596 | 160.0s   | *** COMPLEX | 02:00:01
13 |         3 |     3 |    256 | 7.9s     | ** MODERATE | 03:27:37
 5 |         3 |     1 |    508 | 68.1s    | ** MODERATE | 02:18:54
 2 |         2 |     1 |    390 | 8.9s     | * SIMPLE    | 02:03:47
 8 |         2 |     1 |    898 | 3.6s     | * SIMPLE    | 02:50:56
12 |         2 |     2 |   1266 | 3.7s     | * SIMPLE    | 03:25:46
```

The prompts behind those top entries:

| # | Complexity | Prompt |
|---|---|---|
| 3 | HEAVY (12 calls) | "use uv" — set up a Django project with uv package manager |
| 7 | COMPLEX (9 calls) | "what's the file and directory structure look like in those folders" |
| 1 | COMPLEX (7 calls) | "Does Django have model definitions that are read-only..." |

Short, ambiguous prompts ("use uv", "do that") often trigger the deepest agentic loops because Claude has to explore the codebase to figure out what to do. Specific questions tend to resolve in fewer turns.

## Future Directions

1. **Bulk import** — Load all session JSONL files across every project into one DuckDB table, then rank sessions/projects by cost, token usage, or turn count.
2. **Tool use analysis** — Extract tool call blocks from assistant message `content` to find which tools get used most, average tokens per tool type, etc.
3. **Conversation flow visualization** — Map the user→assistant→tool→assistant chain with timestamps to see where time is spent (thinking vs. tool execution vs. user idle).
4. **Compare with claude-track** — Cross-reference what DuckDB pulls from raw JSONL against what `claude-track` stores in SQLite to find data gaps or new capture opportunities.
