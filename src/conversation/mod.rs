mod sqlite;

use serde::Serialize;
use serde_json::json;
pub(crate) use sqlite::ConversationStore;

// 按顺序保留 Responses 的输入和输出项，工具请求与结果通过 call_id 配对。
pub(crate) type Message = serde_json::Value;

#[derive(Serialize)]
pub(crate) struct ConversationInfo {
    id: i64,
    title: String,
}

pub(crate) struct Conversation {
    id: i64,
    messages: Vec<Message>,
    // 已载入的输入项数；旧库里一行助手消息可能包含多次工具调用。
    saved_len: usize,
    next_position: i64,
}

impl Conversation {
    pub(crate) fn messages(&self) -> &[Message] {
        &self.messages
    }

    pub(crate) fn add_user(&mut self, text: &str) {
        self.messages
            .push(json!({ "type": "message", "role": "user", "content": text }));
    }

    pub(crate) fn add_output(&mut self, output: Vec<Message>) {
        self.messages.extend(output);
    }

    pub(crate) fn add_tool_result(&mut self, call_id: &str, content: String) {
        self.messages
            .push(json!({ "type": "function_call_output", "call_id": call_id, "output": content }));
    }

    pub(crate) fn visible_messages(&self) -> Vec<Message> {
        self.messages
            .iter()
            .filter_map(|message| {
                let role = message["role"].as_str()?;
                let content = match role {
                    "user" => message["content"].as_str()?.to_owned(),
                    "assistant" => assistant_text(message)?,
                    _ => return None,
                };
                Some(json!({ "role": role, "content": content }))
            })
            .collect()
    }
}

pub(crate) fn assistant_text(item: &Message) -> Option<String> {
    if item["type"] != "message" || item["role"] != "assistant" {
        return None;
    }
    if let Some(text) = item["content"].as_str() {
        return (!text.is_empty()).then(|| text.to_owned());
    }
    let text: String = item["content"]
        .as_array()?
        .iter()
        .filter_map(|part| match part["type"].as_str()? {
            "output_text" => part["text"].as_str(),
            "refusal" => part["refusal"].as_str(),
            _ => None,
        })
        .collect();
    (!text.is_empty()).then_some(text)
}
