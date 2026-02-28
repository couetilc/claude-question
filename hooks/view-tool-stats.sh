#!/bin/bash
# View tool usage statistics from the JSONL log.

LOG_FILE="$HOME/.claude/tool-usage.jsonl"

if [ ! -f "$LOG_FILE" ]; then
  echo "No tool usage data yet."
  exit 0
fi

TOTAL=$(wc -l < "$LOG_FILE" | tr -d ' ')
SESSIONS=$(jq -r '.session' "$LOG_FILE" | sort -u | wc -l | tr -d ' ')

echo "=== Claude Code Tool Usage Stats ==="
echo "Total tool calls: $TOTAL"
echo "Across $SESSIONS session(s)"
echo ""
echo "--- Calls by tool ---"
jq -r '.tool' "$LOG_FILE" | sort | uniq -c | sort -rn | awk '{printf "  %-25s %s\n", $2, $1}'
echo ""
echo "--- Calls by date ---"
jq -r '.ts[:10]' "$LOG_FILE" | sort | uniq -c | sort | awk '{printf "  %s  %s\n", $2, $1}'
