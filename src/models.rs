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
#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub struct ToolCall {
    pub ts: String,
    pub tool: String,
    pub session: String,
    pub cwd: String,
    pub input: serde_json::Value,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hook_input_deserializes_full() {
        let json = r#"{"tool_name":"Read","session_id":"s1","cwd":"/tmp","tool_input":{"file_path":"/foo"}}"#;
        let input: HookInput = serde_json::from_str(json).unwrap();
        assert_eq!(input.tool_name.unwrap(), "Read");
        assert_eq!(input.session_id.unwrap(), "s1");
        assert_eq!(input.cwd.unwrap(), "/tmp");
        assert_eq!(input.tool_input.unwrap()["file_path"], "/foo");
    }

    #[test]
    fn hook_input_deserializes_empty() {
        let json = "{}";
        let input: HookInput = serde_json::from_str(json).unwrap();
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
}
