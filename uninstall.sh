#!/bin/bash
# Uninstall Claude Code tool usage tracking hook.
set -e

HOOKS_DIR="$HOME/.claude/hooks"
SETTINGS="$HOME/.claude/settings.json"
LOG_FILE="$HOME/.claude/tool-usage.jsonl"

# Remove hook from settings.json
if [ -f "$SETTINGS" ]; then
  if jq -e '.hooks.PostToolUse' "$SETTINGS" &> /dev/null; then
    UPDATED=$(jq 'if .hooks.PostToolUse then
      .hooks.PostToolUse |= map(select(.hooks[]?.command != "$HOME/.claude/hooks/track-tool-usage.sh"))
    else . end
    | if .hooks.PostToolUse == [] then del(.hooks.PostToolUse) else . end
    | if .hooks == {} then del(.hooks) else . end' "$SETTINGS")
    echo "$UPDATED" > "$SETTINGS"
    echo "Hook removed from $SETTINGS"
  else
    echo "No PostToolUse hooks found in settings."
  fi
else
  echo "No settings.json found."
fi

# Remove scripts
rm -f "$HOOKS_DIR/track-tool-usage.sh"
rm -f "$HOOKS_DIR/view-tool-stats.sh"
echo "Hook scripts removed."

# Ask about log data
if [ -f "$LOG_FILE" ]; then
  LINES=$(wc -l < "$LOG_FILE" | tr -d ' ')
  read -p "Delete usage log? ($LINES entries in $LOG_FILE) [y/N] " -n 1 -r
  echo
  if [[ $REPLY =~ ^[Yy]$ ]]; then
    rm -f "$LOG_FILE"
    echo "Log deleted."
  else
    echo "Log kept at $LOG_FILE"
  fi
fi

echo "Uninstalled successfully."
