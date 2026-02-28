use serde::{Deserialize, Serialize};

/// Raw input received from the Claude Code hook on stdin.
#[derive(Debug, Deserialize)]
pub struct HookInput {
    pub tool_name: Option<String>,
    pub session_id: Option<String>,
    pub cwd: Option<String>,
    pub tool_input: Option<serde_json::Value>,
}

/// A single tool-call record persisted to the JSONL log.
#[derive(Debug, Serialize, Deserialize)]
pub struct ToolCall {
    pub ts: String,
    pub tool: String,
    pub session: String,
    pub cwd: String,
    pub input: serde_json::Value,
}
