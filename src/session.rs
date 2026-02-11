use crate::api::{ContentBlock, Message, Role, Usage};
use serde::Serialize;
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};

#[derive(Serialize)]
struct TranscriptLine<'a> {
    #[serde(rename = "type")]
    turn_type: &'a str,
    #[serde(rename = "sessionId")]
    session_id: &'a str,
    uuid: String,
    #[serde(rename = "parentUuid")]
    parent_uuid: Option<String>,
    timestamp: String,
    cwd: &'a str,
    version: &'a str,
    message: TranscriptMessage<'a>,
}

#[derive(Serialize)]
struct TranscriptMessage<'a> {
    role: &'a str,
    content: &'a [ContentBlock],
    #[serde(skip_serializing_if = "Option::is_none")]
    usage: Option<&'a Usage>,
}

pub struct Session {
    session_id: String,
    cwd: String,
    dir: PathBuf,
    parent_uuid: Option<String>,
    first_prompt: Option<String>,
    model: String,
    start_time: String,
}

impl Session {
    pub fn new(cwd: &str, model: &str) -> Self {
        let now = chrono::Utc::now();
        let date = now.format("%Y-%m-%d").to_string();
        let id = uuid::Uuid::new_v4();
        let session_id = format!("{date}-{id}");
        let dir = Path::new(".entire").join("metadata").join(&session_id);
        Self {
            session_id,
            cwd: cwd.to_string(),
            dir,
            parent_uuid: None,
            first_prompt: None,
            model: model.to_string(),
            start_time: now.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        }
    }

    /// Append a user turn to the JSONL transcript.
    pub fn append_user_turn(&mut self, message: &Message) {
        if self.first_prompt.is_none()
            && let Some(ContentBlock::Text { text }) = message.content.first()
        {
            self.first_prompt = Some(text.clone());
        }
        self.append_line("user", message, None);
    }

    /// Append an assistant turn to the JSONL transcript with token usage.
    pub fn append_assistant_turn(&mut self, message: &Message, usage: &Usage) {
        self.append_line("assistant", message, Some(usage));
    }

    fn append_line(&mut self, turn_type: &str, message: &Message, usage: Option<&Usage>) {
        let uuid = uuid::Uuid::new_v4().to_string();
        let role = match message.role {
            Role::User => "user",
            Role::Assistant => "assistant",
        };
        let line = TranscriptLine {
            turn_type,
            session_id: &self.session_id,
            uuid: uuid.clone(),
            parent_uuid: self.parent_uuid.take(),
            timestamp: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
            cwd: &self.cwd,
            version: env!("CARGO_PKG_VERSION"),
            message: TranscriptMessage {
                role,
                content: &message.content,
                usage,
            },
        };
        self.parent_uuid = Some(uuid);
        if let Err(e) = self.write_jsonl_line(&line) {
            eprintln!("[session] write error: {e}");
        }
    }

    fn write_jsonl_line(&self, line: &TranscriptLine) -> std::io::Result<()> {
        fs::create_dir_all(&self.dir)?;
        let path = self.dir.join("full.jsonl");
        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)?;
        let json = serde_json::to_string(line).map_err(std::io::Error::other)?;
        writeln!(file, "{json}")
    }

    /// Write supporting files (prompt.txt, context.md) at session end.
    pub fn write_supporting_files(&self, conversation: &[Message]) {
        if let Err(e) = self.write_files_inner(conversation) {
            eprintln!("[session] supporting files error: {e}");
        }
    }

    fn write_files_inner(&self, conversation: &[Message]) -> std::io::Result<()> {
        fs::create_dir_all(&self.dir)?;

        // prompt.txt
        if let Some(prompt) = &self.first_prompt {
            fs::write(self.dir.join("prompt.txt"), prompt)?;
        }

        // context.md
        let mut ctx = format!(
            "# Session {}\n\n- Model: {}\n- Started: {}\n- CWD: {}\n\n## Key Actions\n\n",
            self.session_id, self.model, self.start_time, self.cwd
        );
        for msg in conversation {
            for block in &msg.content {
                if let ContentBlock::ToolUse { name, input, .. } = block {
                    let first_arg = input
                        .as_object()
                        .and_then(|m| m.values().next())
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    ctx.push_str(&format!("- **{name}**: {first_arg}\n"));
                }
            }
        }
        fs::write(self.dir.join("context.md"), ctx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::ContentBlock;
    use serde_json::Value;

    fn make_session(dir: &Path) -> Session {
        Session {
            session_id: "2026-02-11-test-uuid".into(),
            cwd: "/test/project".into(),
            dir: dir.to_path_buf(),
            parent_uuid: None,
            first_prompt: None,
            model: "test-model".into(),
            start_time: "2026-02-11T00:00:00Z".into(),
        }
    }

    fn user_msg(text: &str) -> Message {
        Message {
            role: Role::User,
            content: vec![ContentBlock::Text { text: text.into() }],
        }
    }

    fn assistant_msg(text: &str) -> Message {
        Message {
            role: Role::Assistant,
            content: vec![ContentBlock::Text { text: text.into() }],
        }
    }

    fn assistant_tool_msg() -> Message {
        Message {
            role: Role::Assistant,
            content: vec![
                ContentBlock::Text {
                    text: "Let me check.".into(),
                },
                ContentBlock::ToolUse {
                    id: "t1".into(),
                    name: "read_file".into(),
                    input: serde_json::json!({"path": "src/main.rs"}),
                },
            ],
        }
    }

    fn tool_result_msg() -> Message {
        Message {
            role: Role::User,
            content: vec![ContentBlock::ToolResult {
                tool_use_id: "t1".into(),
                content: "file contents".into(),
                is_error: None,
            }],
        }
    }

    #[test]
    fn session_creates_jsonl_on_user_turn() {
        let dir = tempfile::tempdir().unwrap();
        let session_dir = dir.path().join("session");
        let mut session = make_session(&session_dir);
        session.append_user_turn(&user_msg("hello"));

        let jsonl = fs::read_to_string(session_dir.join("full.jsonl")).unwrap();
        let lines: Vec<&str> = jsonl.trim().lines().collect();
        assert_eq!(lines.len(), 1);

        let v: Value = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(v["type"], "user");
        assert_eq!(v["message"]["role"], "user");
        assert_eq!(v["message"]["content"][0]["text"], "hello");
        assert_eq!(v["sessionId"], "2026-02-11-test-uuid");
        assert!(v["parentUuid"].is_null(), "first line has null parentUuid");
    }

    #[test]
    fn session_chains_parent_uuids() {
        let dir = tempfile::tempdir().unwrap();
        let session_dir = dir.path().join("session");
        let mut session = make_session(&session_dir);
        let usage = Usage::default();

        session.append_user_turn(&user_msg("first"));
        session.append_assistant_turn(&assistant_msg("response"), &usage);
        session.append_user_turn(&user_msg("second"));

        let jsonl = fs::read_to_string(session_dir.join("full.jsonl")).unwrap();
        let lines: Vec<Value> = jsonl
            .trim()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert_eq!(lines.len(), 3);

        // First has null parent
        assert!(lines[0]["parentUuid"].is_null());
        // Second's parent is first's uuid
        assert_eq!(lines[1]["parentUuid"], lines[0]["uuid"]);
        // Third's parent is second's uuid
        assert_eq!(lines[2]["parentUuid"], lines[1]["uuid"]);
    }

    #[test]
    fn session_captures_first_prompt() {
        let dir = tempfile::tempdir().unwrap();
        let session_dir = dir.path().join("session");
        let mut session = make_session(&session_dir);

        session.append_user_turn(&user_msg("explain main.rs"));
        session.append_user_turn(&user_msg("also this"));

        assert_eq!(session.first_prompt.as_deref(), Some("explain main.rs"));
    }

    #[test]
    fn session_assistant_turn_includes_usage() {
        let dir = tempfile::tempdir().unwrap();
        let session_dir = dir.path().join("session");
        let mut session = make_session(&session_dir);
        let usage = Usage {
            input_tokens: 1200,
            output_tokens: 350,
            cache_creation_input_tokens: 0,
            cache_read_input_tokens: 800,
        };

        session.append_user_turn(&user_msg("test"));
        session.append_assistant_turn(&assistant_msg("reply"), &usage);

        let jsonl = fs::read_to_string(session_dir.join("full.jsonl")).unwrap();
        let lines: Vec<Value> = jsonl
            .trim()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();

        // User turn has no usage
        assert!(lines[0]["message"]["usage"].is_null());
        // Assistant turn has usage
        assert_eq!(lines[1]["message"]["usage"]["input_tokens"], 1200);
        assert_eq!(lines[1]["message"]["usage"]["output_tokens"], 350);
        assert_eq!(lines[1]["message"]["usage"]["cache_read_input_tokens"], 800);
    }

    #[test]
    fn session_writes_prompt_txt() {
        let dir = tempfile::tempdir().unwrap();
        let session_dir = dir.path().join("session");
        let mut session = make_session(&session_dir);

        session.append_user_turn(&user_msg("explain main.rs"));
        session.write_supporting_files(&[user_msg("explain main.rs")]);

        let prompt = fs::read_to_string(session_dir.join("prompt.txt")).unwrap();
        assert_eq!(prompt, "explain main.rs");
    }

    #[test]
    fn session_writes_context_md_with_tool_actions() {
        let dir = tempfile::tempdir().unwrap();
        let session_dir = dir.path().join("session");
        let mut session = make_session(&session_dir);

        session.append_user_turn(&user_msg("read the file"));
        let conversation = vec![
            user_msg("read the file"),
            assistant_tool_msg(),
            tool_result_msg(),
            assistant_msg("done"),
        ];
        session.write_supporting_files(&conversation);

        let ctx = fs::read_to_string(session_dir.join("context.md")).unwrap();
        assert!(ctx.contains("# Session 2026-02-11-test-uuid"));
        assert!(ctx.contains("Model: test-model"));
        assert!(ctx.contains("**read_file**: src/main.rs"));
    }

    #[test]
    fn session_context_md_without_tools() {
        let dir = tempfile::tempdir().unwrap();
        let session_dir = dir.path().join("session");
        let mut session = make_session(&session_dir);

        session.append_user_turn(&user_msg("hello"));
        session.write_supporting_files(&[user_msg("hello"), assistant_msg("hi")]);

        let ctx = fs::read_to_string(session_dir.join("context.md")).unwrap();
        assert!(ctx.contains("## Key Actions"));
        // No tool actions listed
        assert!(!ctx.contains("**"));
    }

    #[test]
    fn session_incremental_append() {
        let dir = tempfile::tempdir().unwrap();
        let session_dir = dir.path().join("session");
        let mut session = make_session(&session_dir);
        let usage = Usage::default();

        // Append multiple turns, verify file grows incrementally
        session.append_user_turn(&user_msg("q1"));
        let count1 = fs::read_to_string(session_dir.join("full.jsonl"))
            .unwrap()
            .lines()
            .count();
        assert_eq!(count1, 1);

        session.append_assistant_turn(&assistant_msg("a1"), &usage);
        let count2 = fs::read_to_string(session_dir.join("full.jsonl"))
            .unwrap()
            .lines()
            .count();
        assert_eq!(count2, 2);

        session.append_user_turn(&user_msg("q2"));
        let count3 = fs::read_to_string(session_dir.join("full.jsonl"))
            .unwrap()
            .lines()
            .count();
        assert_eq!(count3, 3);
    }

    #[test]
    fn session_jsonl_lines_are_valid_json() {
        let dir = tempfile::tempdir().unwrap();
        let session_dir = dir.path().join("session");
        let mut session = make_session(&session_dir);
        let usage = Usage::default();

        session.append_user_turn(&user_msg("test"));
        session.append_assistant_turn(&assistant_tool_msg(), &usage);
        session.append_user_turn(&tool_result_msg());

        let jsonl = fs::read_to_string(session_dir.join("full.jsonl")).unwrap();
        for (i, line) in jsonl.trim().lines().enumerate() {
            let parsed: Result<Value, _> = serde_json::from_str(line);
            assert!(parsed.is_ok(), "line {i} is invalid JSON: {line}");
        }
    }

    #[test]
    fn session_has_required_fields() {
        let dir = tempfile::tempdir().unwrap();
        let session_dir = dir.path().join("session");
        let mut session = make_session(&session_dir);

        session.append_user_turn(&user_msg("test"));

        let jsonl = fs::read_to_string(session_dir.join("full.jsonl")).unwrap();
        let v: Value = serde_json::from_str(jsonl.trim()).unwrap();

        // All required fields present
        assert!(v["type"].is_string());
        assert!(v["sessionId"].is_string());
        assert!(v["uuid"].is_string());
        assert!(v["timestamp"].is_string());
        assert!(v["cwd"].is_string());
        assert!(v["version"].is_string());
        assert!(v["message"].is_object());
    }

    #[test]
    fn session_no_prompt_txt_without_user_text() {
        let dir = tempfile::tempdir().unwrap();
        let session_dir = dir.path().join("session");
        let session = make_session(&session_dir);

        // Write supporting files without any user turns
        session.write_supporting_files(&[]);

        assert!(!session_dir.join("prompt.txt").exists());
        // context.md should still be written
        assert!(session_dir.join("context.md").exists());
    }

    #[test]
    fn session_tool_result_as_first_user_turn_no_prompt() {
        let dir = tempfile::tempdir().unwrap();
        let session_dir = dir.path().join("session");
        let mut session = make_session(&session_dir);

        // First user turn is a tool_result (no text content)
        session.append_user_turn(&tool_result_msg());
        assert!(session.first_prompt.is_none());
    }
}
