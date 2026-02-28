#!/bin/bash
# Logs every tool call to a JSONL file for usage tracking across sessions.

LOG_FILE="$HOME/.claude/tool-usage.jsonl"
INPUT=$(cat)

TIMESTAMP=$(date -u +"%Y-%m-%dT%H:%M:%SZ")

echo "$INPUT" | jq -c \
  --arg ts "$TIMESTAMP" \
  '{ts: $ts, tool: .tool_name, session: .session_id, cwd: .cwd, input: .tool_input}' \
  >> "$LOG_FILE"

exit 0
