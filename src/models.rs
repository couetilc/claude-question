use serde::{Deserialize, Serialize};

/// Raw input received from any Claude Code hook on stdin.
#[derive(Debug, Deserialize)]
pub struct HookInput {
    /// Which hook event triggered this (e.g. "PreToolUse", "PostToolUse", "Stop", etc.)
    pub hook_event_name: Option<String>,
    pub session_id: Option<String>,
    pub cwd: Option<String>,
    pub transcript_path: Option<String>,

    // Tool-related fields (PreToolUse / PostToolUse)
    pub tool_name: Option<String>,
    pub tool_use_id: Option<String>,
    pub tool_input: Option<serde_json::Value>,
    pub tool_response: Option<serde_json::Value>,

    // Session lifecycle
    pub reason: Option<String>,

    // UserPromptSubmit
    pub prompt: Option<String>,

    // Stop event
    #[allow(dead_code)]
    pub last_assistant_message: Option<String>,

    // Stop event — may contain a stop_hook_active flag
    #[allow(dead_code)]
    pub stop_hook_active: Option<bool>,
}

/// A single tool-call record persisted to the JSONL log (legacy format).
#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub struct ToolCall {
    pub ts: String,
    pub tool: String,
    pub session: String,
    pub cwd: String,
    pub input: serde_json::Value,
}

/// A single line from a Claude Code transcript JSONL file.
#[derive(Debug, Deserialize)]
pub struct TranscriptLine {
    #[serde(rename = "type")]
    pub line_type: Option<String>,
    pub message: Option<TranscriptMessage>,
}

/// The message field inside a transcript line.
#[derive(Debug, Deserialize)]
pub struct TranscriptMessage {
    pub model: Option<String>,
    pub usage: Option<TranscriptUsage>,
}

/// Token usage from a transcript message.
#[derive(Debug, Deserialize)]
pub struct TranscriptUsage {
    pub input_tokens: Option<i64>,
    pub output_tokens: Option<i64>,
    pub cache_creation_input_tokens: Option<i64>,
    pub cache_read_input_tokens: Option<i64>,
}

/// Aggregated token usage from a transcript.
#[derive(Debug, Default)]
pub struct AggregatedTokenUsage {
    pub model: String,
    pub input_tokens: i64,
    pub cache_creation_tokens: i64,
    pub cache_read_tokens: i64,
    pub output_tokens: i64,
    pub api_call_count: i64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hook_input_deserializes_full() {
        let json = r#"{"hook_event_name":"PostToolUse","tool_name":"Read","session_id":"s1","cwd":"/tmp","tool_input":{"file_path":"/foo"}}"#;
        let input: HookInput = serde_json::from_str(json).unwrap();
        assert_eq!(input.hook_event_name.unwrap(), "PostToolUse");
        assert_eq!(input.tool_name.unwrap(), "Read");
        assert_eq!(input.session_id.unwrap(), "s1");
        assert_eq!(input.cwd.unwrap(), "/tmp");
        assert_eq!(input.tool_input.unwrap()["file_path"], "/foo");
    }

    #[test]
    fn hook_input_deserializes_empty() {
        let json = "{}";
        let input: HookInput = serde_json::from_str(json).unwrap();
        assert!(input.hook_event_name.is_none());
        assert!(input.tool_name.is_none());
        assert!(input.session_id.is_none());
        assert!(input.cwd.is_none());
        assert!(input.tool_input.is_none());
    }

    #[test]
    fn hook_input_deserializes_partial() {
        let json = r#"{"tool_name":"Bash"}"#;
        let input: HookInput = serde_json::from_str(json).unwrap();
        assert_eq!(input.tool_name.unwrap(), "Bash");
        assert!(input.session_id.is_none());
    }

    #[test]
    fn hook_input_session_start() {
        let json = r#"{"hook_event_name":"SessionStart","session_id":"s1","cwd":"/proj","transcript_path":"/tmp/t.jsonl","reason":"startup"}"#;
        let input: HookInput = serde_json::from_str(json).unwrap();
        assert_eq!(input.hook_event_name.unwrap(), "SessionStart");
        assert_eq!(input.reason.unwrap(), "startup");
        assert_eq!(input.transcript_path.unwrap(), "/tmp/t.jsonl");
    }

    #[test]
    fn hook_input_session_end() {
        let json = r#"{"hook_event_name":"SessionEnd","session_id":"s1","reason":"logout"}"#;
        let input: HookInput = serde_json::from_str(json).unwrap();
        assert_eq!(input.hook_event_name.unwrap(), "SessionEnd");
        assert_eq!(input.reason.unwrap(), "logout");
    }

    #[test]
    fn hook_input_user_prompt() {
        let json = r#"{"hook_event_name":"UserPromptSubmit","session_id":"s1","prompt":"hello world"}"#;
        let input: HookInput = serde_json::from_str(json).unwrap();
        assert_eq!(input.prompt.unwrap(), "hello world");
    }

    #[test]
    fn hook_input_stop_event() {
        let json = r#"{"hook_event_name":"Stop","session_id":"s1","transcript_path":"/tmp/t.jsonl","stop_hook_active":true}"#;
        let input: HookInput = serde_json::from_str(json).unwrap();
        assert_eq!(input.hook_event_name.unwrap(), "Stop");
        assert!(input.stop_hook_active.unwrap());
    }

    #[test]
    fn hook_input_pre_tool_use() {
        let json = r#"{"hook_event_name":"PreToolUse","session_id":"s1","tool_name":"Read","tool_use_id":"tu1","tool_input":{"file_path":"/foo"},"cwd":"/proj"}"#;
        let input: HookInput = serde_json::from_str(json).unwrap();
        assert_eq!(input.tool_use_id.unwrap(), "tu1");
    }

    #[test]
    fn hook_input_post_tool_use() {
        let json = r#"{"hook_event_name":"PostToolUse","session_id":"s1","tool_name":"Read","tool_use_id":"tu1","tool_input":{},"tool_response":"file contents","cwd":"/proj"}"#;
        let input: HookInput = serde_json::from_str(json).unwrap();
        assert_eq!(input.tool_response.unwrap(), "file contents");
    }

    #[test]
    fn hook_input_ignores_unknown_fields() {
        let json = r#"{"hook_event_name":"PostToolUse","unknown_field":"value","tool_name":"Read"}"#;
        let input: HookInput = serde_json::from_str(json).unwrap();
        assert_eq!(input.tool_name.unwrap(), "Read");
    }

    #[test]
    fn tool_call_round_trip() {
        let record = ToolCall {
            ts: "2026-02-27T12:00:00Z".to_string(),
            tool: "Read".to_string(),
            session: "s1".to_string(),
            cwd: "/tmp".to_string(),
            input: serde_json::json!({"file_path": "/foo"}),
        };

        let json = serde_json::to_string(&record).unwrap();
        let parsed: ToolCall = serde_json::from_str(&json).unwrap();
        assert_eq!(record, parsed);
    }

    #[test]
    fn tool_call_deserializes_from_jsonl_format() {
        let line = r#"{"ts":"2026-02-27T12:00:00Z","tool":"Bash","session":"abc","cwd":"/home","input":{"command":"ls"}}"#;
        let record: ToolCall = serde_json::from_str(line).unwrap();
        assert_eq!(record.tool, "Bash");
        assert_eq!(record.input["command"], "ls");
    }

    #[test]
    fn transcript_line_deserializes() {
        let json = r#"{"type":"assistant","message":{"model":"claude-sonnet-4-20250514","usage":{"input_tokens":100,"output_tokens":50,"cache_creation_input_tokens":200,"cache_read_input_tokens":300}}}"#;
        let line: TranscriptLine = serde_json::from_str(json).unwrap();
        assert_eq!(line.line_type.unwrap(), "assistant");
        let msg = line.message.unwrap();
        assert_eq!(msg.model.unwrap(), "claude-sonnet-4-20250514");
        let usage = msg.usage.unwrap();
        assert_eq!(usage.input_tokens.unwrap(), 100);
        assert_eq!(usage.output_tokens.unwrap(), 50);
        assert_eq!(usage.cache_creation_input_tokens.unwrap(), 200);
        assert_eq!(usage.cache_read_input_tokens.unwrap(), 300);
    }

    #[test]
    fn transcript_line_partial() {
        let json = r#"{"type":"user"}"#;
        let line: TranscriptLine = serde_json::from_str(json).unwrap();
        assert_eq!(line.line_type.unwrap(), "user");
        assert!(line.message.is_none());
    }

    #[test]
    fn transcript_line_ignores_unknown_fields() {
        let json = r#"{"type":"assistant","unknown":"x","message":{"model":"m"}}"#;
        let line: TranscriptLine = serde_json::from_str(json).unwrap();
        assert_eq!(line.message.unwrap().model.unwrap(), "m");
    }

    #[test]
    fn aggregated_token_usage_default() {
        let agg = AggregatedTokenUsage::default();
        assert_eq!(agg.model, "");
        assert_eq!(agg.input_tokens, 0);
        assert_eq!(agg.cache_creation_tokens, 0);
        assert_eq!(agg.cache_read_tokens, 0);
        assert_eq!(agg.output_tokens, 0);
        assert_eq!(agg.api_call_count, 0);
    }
}
