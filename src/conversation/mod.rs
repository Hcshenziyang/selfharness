mod sqlite;

use serde::Serialize;
use serde_json::json;
pub(crate) use sqlite::ConversationStore;

// ******************************** 会话与消息记录 ********************************

/// Responses 协议中的一项记录，例如用户消息、模型发起的工具调用或工具结果。
pub(crate) type Message = serde_json::Value;

/// 会话列表需要的标识和标题。
#[derive(Serialize)]
pub(crate) struct ConversationInfo {
    id: i64,
    pub(crate) agent_id: i64,
    title: String,
}

/// 一次载入到内存的会话。[`Self::messages`] 供模型续聊，[`Self::visible_messages`] 供网页展示。
pub(crate) struct Conversation {
    pub(crate) info: ConversationInfo,
    pub(crate) agent: crate::agent::AgentSettings,
    messages: Vec<Message>,
    /// 载入时的内存记录数，保存时从这里开始取新增记录。
    saved_len: usize,
    /// 下一条记录在 SQLite 中的位置。旧库的一行可以展开为多条内存记录。
    next_position: i64,
}

impl Conversation {
    /// 按对话顺序返回提交给模型的完整输入，包括工具调用和结果。
    pub(crate) fn messages(&self) -> &[Message] {
        &self.messages
    }

    /// 追加开启本回合的用户消息。
    pub(crate) fn add_user(&mut self, text: &str) {
        self.messages
            .push(json!({ "type": "message", "role": "user", "content": text }));
    }

    /// 按模型返回的顺序追加整批输出，保留其中的工具调用请求。
    pub(crate) fn add_output(&mut self, output: Vec<Message>) {
        self.messages.extend(output);
    }

    /// 用模型给出的调用 ID 将工具结果配回原请求。
    pub(crate) fn add_tool_result(&mut self, call_id: &str, content: String) {
        self.messages
            .push(json!({ "type": "function_call_output", "call_id": call_id, "output": content }));
    }

    // ******************************** 网页可见消息 ********************************

    /// 提取聊天页面展示的用户和助手文本，跳过工具调用等协议记录。
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

/// 提取助手回复。支持纯字符串以及由多段正文或拒答文本组成的内容。
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
