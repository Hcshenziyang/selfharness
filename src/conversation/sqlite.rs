use super::{Conversation, ConversationInfo, Message};
use rusqlite::{Connection, Result, Row, params};
use serde_json::json;
use std::path::Path;

// ******************************** 会话库与会话列表 ********************************

/// 持有 SQLite 连接，管理会话元信息和按顺序存储的消息记录。
pub(crate) struct ConversationStore {
    db: Connection,
}

impl ConversationStore {
    /// 打开本地数据库并初始化会话表、消息表及其外键约束。
    pub(crate) fn open(path: &Path) -> Result<Self> {
        let db = Connection::open(path)?;
        db.execute_batch(
            "
            PRAGMA foreign_keys = ON;
            CREATE TABLE IF NOT EXISTS conversations (
                id INTEGER PRIMARY KEY,
                title TEXT NOT NULL DEFAULT '新对话',
                created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
                updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%d %H:%M:%f', 'now'))
            );
            CREATE TABLE IF NOT EXISTS messages (
                id INTEGER PRIMARY KEY,
                conversation_id INTEGER NOT NULL REFERENCES conversations(id),
                position INTEGER NOT NULL,
                payload TEXT NOT NULL,
                created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
                UNIQUE (conversation_id, position)
            );
        ",
        )?;
        Ok(Self { db })
    }

    /// 创建一个空会话，返回网页后续发送消息时使用的 ID 和初始标题。
    pub(crate) fn create(&self) -> Result<ConversationInfo> {
        self.db.query_row(
            "INSERT INTO conversations DEFAULT VALUES RETURNING id, title",
            [],
            conversation_info,
        )
    }

    /// 按最近更新时间列出会话，时间相同时将 ID 较大的排在前面。
    pub(crate) fn list(&self) -> Result<Vec<ConversationInfo>> {
        self.db
            .prepare("SELECT id, title FROM conversations ORDER BY updated_at DESC, id DESC")?
            .query_map([], conversation_info)?
            .collect()
    }

    /// 按 ID 查会话标题，找不到时返回数据库查询错误。
    pub(crate) fn info(&self, id: i64) -> Result<ConversationInfo> {
        self.db.query_row(
            "SELECT id, title FROM conversations WHERE id = ?1",
            [id],
            conversation_info,
        )
    }

    // ******************************** 读取历史与保存回合 ********************************

    /// 按数据库中的位置还原完整历史，同时记下下一次增量写入的起点。
    pub(crate) fn load(&self, id: i64) -> Result<Conversation> {
        let info = self.info(id)?;
        let mut statement = self.db.prepare(
            "SELECT position, payload FROM messages WHERE conversation_id = ?1 ORDER BY position",
        )?;
        let rows = statement.query_map([id], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, Message>(1)?))
        })?;
        let mut messages = Vec::new();
        let mut next_position = 0;
        for row in rows {
            let (position, item) = row?;
            restore_item(item, &mut messages);
            next_position = position + 1;
        }
        Ok(Conversation {
            info,
            saved_len: messages.len(),
            messages,
            next_position,
        })
    }

    /// 在一个事务中追加本回合的新记录、更新会话时间，并用首条提问的前 40 个字符命名新会话。
    pub(crate) fn save_turn(&mut self, conversation: &Conversation) -> Result<ConversationInfo> {
        let transaction = self.db.transaction()?;
        {
            let mut insert = transaction.prepare(
                "INSERT INTO messages (conversation_id, position, payload) VALUES (?1, ?2, ?3)",
            )?;
            for (offset, message) in conversation.messages[conversation.saved_len..]
                .iter()
                .enumerate()
            {
                insert.execute(params![
                    conversation.info.id,
                    conversation.next_position + offset as i64,
                    message
                ])?;
            }
        }
        let first_question = if conversation.saved_len == 0 {
            conversation
                .messages
                .first()
                .and_then(|message| message["content"].as_str())
        } else {
            None
        };
        let info = transaction.query_row(
            "UPDATE conversations SET title = coalesce(substr(?1, 1, 40), title),
                updated_at = strftime('%Y-%m-%d %H:%M:%f', 'now')
             WHERE id = ?2 RETURNING id, title",
            params![first_question, conversation.info.id],
            conversation_info,
        )?;
        transaction.commit()?;
        Ok(info)
    }
}

// ******************************** 数据库记录转换 ********************************

/// 把已有会话中的 Chat Completions 记录读成 Responses 输入项。原始数据库行保留。
fn restore_item(item: Message, messages: &mut Vec<Message>) {
    if item.get("type").is_some() {
        messages.push(item);
        return;
    }
    match item["role"].as_str() {
        Some("assistant") => {
            if let Some(text) = item["content"].as_str() {
                messages.push(json!({ "type": "message", "role": "assistant", "content": text }));
            }
            if let Some(calls) = item["tool_calls"].as_array() {
                for call in calls {
                    messages.push(json!({
                        "type": "function_call",
                        "call_id": call["id"],
                        "name": call["function"]["name"],
                        "arguments": call["function"]["arguments"]
                    }));
                }
            }
        }
        Some("tool") => messages.push(json!({
            "type": "function_call_output", "call_id": item["tool_call_id"], "output": item["content"]
        })),
        _ => messages.push(item),
    }
}

/// 将查询结果中的 id、title 两列读为会话摘要。
fn conversation_info(row: &Row<'_>) -> Result<ConversationInfo> {
    Ok(ConversationInfo {
        id: row.get(0)?,
        title: row.get(1)?,
    })
}
