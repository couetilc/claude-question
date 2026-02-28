#!/bin/bash
# Install Claude Code tool usage tracking hook.
set -e

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
HOOKS_DIR="$HOME/.claude/hooks"
SETTINGS="$HOME/.claude/settings.json"

# Check for jq dependency
if ! command -v jq &> /dev/null; then
  echo "Error: jq is required but not installed."
  echo "  brew install jq  (macOS)"
  echo "  apt install jq   (Linux)"
  exit 1
fi

# Create hooks directory
mkdir -p "$HOOKS_DIR"

# Copy scripts
cp "$SCRIPT_DIR/hooks/track-tool-usage.sh" "$HOOKS_DIR/"
cp "$SCRIPT_DIR/hooks/view-tool-stats.sh" "$HOOKS_DIR/"
chmod +x "$HOOKS_DIR/track-tool-usage.sh" "$HOOKS_DIR/view-tool-stats.sh"

# Add hook to settings.json
HOOK_ENTRY='{
  "matcher": ".*",
  "hooks": [
    {
      "type": "command",
      "command": "$HOME/.claude/hooks/track-tool-usage.sh"
    }
  ]
}'

if [ ! -f "$SETTINGS" ]; then
  echo "{}" > "$SETTINGS"
fi

# Check if hook is already installed
if jq -e '.hooks.PostToolUse[]? | select(.hooks[]?.command == "$HOME/.claude/hooks/track-tool-usage.sh")' "$SETTINGS" &> /dev/null; then
  echo "Hook is already installed."
else
  # Add the PostToolUse hook, creating the array if needed
  UPDATED=$(jq --argjson hook "$HOOK_ENTRY" '.hooks.PostToolUse = ((.hooks.PostToolUse // []) + [$hook])' "$SETTINGS")
  echo "$UPDATED" > "$SETTINGS"
  echo "Hook added to $SETTINGS"
fi

echo "Installed successfully."
echo ""
echo "  Tracking starts on your next Claude Code session."
echo "  View stats anytime:  ~/.claude/hooks/view-tool-stats.sh"
