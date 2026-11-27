mod agents;
mod tools;

use super::{Conversation, ConversationInfo, Message};
use crate::agent::DEFAULT_AGENT_ID;
use rusqlite::{Connection, Result, Row, params};
use serde_json::json;
use std::{
    path::Path,
    sync::{Arc, Mutex, MutexGuard},
};

// ******************************** 会话库与会话列表 ********************************

/// 持有 SQLite 连接，管理会话元信息和按顺序存储的消息记录。
#[derive(Clone)]
pub(crate) struct ConversationStore {
    connection: Arc<Mutex<Connection>>,
}

impl ConversationStore {
    /// 打开本地数据库并初始化会话表、消息表及其外键约束。
    pub(crate) fn open(path: &Path, default_model: &str) -> Result<Self> {
        let mut db = Connection::open(path)?;
        // 旧表的主键会复用已删除的 ID。迁移时让 messages 的外键继续指向正式表名。
        db.execute_batch("PRAGMA foreign_keys = OFF; PRAGMA legacy_alter_table = ON;")?;
        let tx = db.transaction()?;
        tx.execute_batch(
            "CREATE TABLE IF NOT EXISTS agents (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                name TEXT NOT NULL,
                model TEXT NOT NULL,
                instructions TEXT NOT NULL DEFAULT '',
                memory TEXT NOT NULL DEFAULT '',
                revision INTEGER NOT NULL DEFAULT 1
            );",
        )?;
        tx.execute(
            "INSERT INTO agents (id, name, model) VALUES (?1, '默认助手', ?2)
             ON CONFLICT(id) DO NOTHING",
            params![DEFAULT_AGENT_ID, default_model],
        )?;
        let old_schema: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM pragma_table_info('conversations')
             WHERE name = 'title' AND \"notnull\" = 1)",
            [],
            |row| row.get(0),
        )?;
        if old_schema {
            tx.execute_batch("ALTER TABLE conversations RENAME TO conversations_old;")?;
        }
        tx.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS conversations (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                agent_id INTEGER NOT NULL DEFAULT 1 REFERENCES agents(id),
                title TEXT,
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
        if old_schema {
            tx.execute_batch(
                "INSERT INTO conversations (id, title, created_at, updated_at)
                SELECT id, CASE WHEN title = '新对话' AND NOT EXISTS
                    (SELECT 1 FROM messages WHERE conversation_id = c.id) THEN NULL ELSE title END,
                    created_at, updated_at FROM conversations_old c;
                DROP TABLE conversations_old;",
            )?;
        }
        // 已经使用 AUTOINCREMENT 的旧库直接加列，保留历史 ID 和 sqlite_sequence 高水位。
        let has_agent_id: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM pragma_table_info('conversations') WHERE name = 'agent_id')",
            [],
            |row| row.get(0),
        )?;
        if !has_agent_id {
            tx.execute_batch(
                "ALTER TABLE conversations ADD COLUMN agent_id INTEGER NOT NULL DEFAULT 1 REFERENCES agents(id);",
            )?;
        }
        tx.execute_batch(
            "CREATE INDEX IF NOT EXISTS conversations_agent_updated
             ON conversations(agent_id, updated_at DESC, id DESC);",
        )?;
        tools::init(&tx)?;
        tx.commit()?;
        db.execute_batch("PRAGMA legacy_alter_table = OFF; PRAGMA foreign_keys = ON;")?;
        Ok(Self {
            connection: Arc::new(Mutex::new(db)),
        })
    }

    fn db(&self) -> MutexGuard<'_, Connection> {
        self.connection
            .lock()
            .expect("SQLite connection mutex poisoned")
    }

    /// 创建一个空会话，返回网页后续发送消息时使用的 ID 和初始标题。
    pub(crate) fn create(&self, agent_id: i64) -> Result<ConversationInfo> {
        self.db().query_row(
            "INSERT INTO conversations (agent_id) SELECT id FROM agents WHERE id = ?1
             RETURNING id, coalesce(title, '新对话'), agent_id",
            [agent_id],
            conversation_info,
        )
    }

    /// 按最近更新时间列出会话，时间相同时将 ID 较大的排在前面。
    pub(crate) fn list(&self, agent_id: i64) -> Result<Vec<ConversationInfo>> {
        let list: Vec<_> = self
            .db()
            .prepare(
                "SELECT id, coalesce(title, '新对话'), agent_id FROM conversations
                      WHERE agent_id = ?1 ORDER BY updated_at DESC, id DESC",
            )?
            .query_map([agent_id], conversation_info)?
            .collect::<Result<_>>()?;
        if list.is_empty() {
            self.db().query_row(
                "SELECT id FROM agents WHERE id = ?1",
                [agent_id],
                |_| Ok(()),
            )?;
        }
        Ok(list)
    }

    /// 按 ID 查会话标题，找不到时返回数据库查询错误。
    pub(crate) fn info(&self, id: i64) -> Result<ConversationInfo> {
        self.db().query_row(
            "SELECT id, coalesce(title, '新对话'), agent_id FROM conversations WHERE id = ?1",
            [id],
            conversation_info,
        )
    }

    /// 显式标题优先于首轮自动生成的标题，包括首轮仍在运行时的改名。
    pub(crate) fn rename(&self, id: i64, title: &str) -> Result<ConversationInfo> {
        self.db().query_row(
            "UPDATE conversations SET title = ?2 WHERE id = ?1 RETURNING id, title, agent_id",
            params![id, title],
            conversation_info,
        )
    }

    /// 在同一事务中移除消息和会话。旧页面持有的 ID 不会再分配给新会话。
    pub(crate) fn delete(&mut self, id: i64) -> Result<()> {
        let mut db = self.db();
        let tx = db.transaction()?;
        tx.execute("DELETE FROM messages WHERE conversation_id = ?1", [id])?;
        if tx.execute("DELETE FROM conversations WHERE id = ?1", [id])? == 0 {
            return Err(rusqlite::Error::QueryReturnedNoRows);
        }
        tx.commit()
    }

    // ******************************** 读取历史与保存回合 ********************************

    /// 按数据库中的位置还原完整历史，同时记下下一次增量写入的起点。
    pub(crate) fn load(&self, id: i64) -> Result<Conversation> {
        let info = self.info(id)?;
        let agent = self.agent(info.agent_id)?.settings;
        let db = self.db();
        let mut statement = db.prepare(
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
            agent,
            saved_len: messages.len(),
            messages,
            next_position,
        })
    }

    /// 在一个事务中追加本回合的新记录、更新会话时间，并用首条提问的前 40 个字符命名新会话。
    pub(crate) fn save_turn(&mut self, conversation: &Conversation) -> Result<ConversationInfo> {
        let mut db = self.db();
        let transaction = db.transaction()?;
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
            "UPDATE conversations SET title = coalesce(title, substr(?1, 1, 40), '新对话'),
                updated_at = strftime('%Y-%m-%d %H:%M:%f', 'now')
             WHERE id = ?2 RETURNING id, title, agent_id",
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

/// 读取会话 ID、标题和所属助手。
fn conversation_info(row: &Row<'_>) -> Result<ConversationInfo> {
    Ok(ConversationInfo {
        id: row.get(0)?,
        title: row.get(1)?,
        agent_id: row.get(2)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::AgentToolRegistry;
    use std::{sync::mpsc, time::Duration};

    #[tokio::test(flavor = "multi_thread", worker_threads = 1)]
    async fn waiting_for_the_tool_snapshot_leaves_a_worker_available() {
        let store = ConversationStore::open(Path::new(":memory:"), "test").unwrap();
        let turn = AgentToolRegistry::new(store.clone())
            .turn(DEFAULT_AGENT_ID, "")
            .unwrap();
        let timeout = Duration::from_secs(3);
        let (snapshot, responsive) = {
            let _db = store.db();
            let (started, waiting) = mpsc::channel();
            let snapshot = tokio::spawn(async move {
                started.send(()).unwrap();
                turn.snapshot().await.unwrap();
            });
            waiting.recv_timeout(timeout).unwrap();
            // 快照在等 SQLite 时，无关任务应能在锁释放前得到调度。
            let (pulse, received) = mpsc::channel();
            tokio::spawn(async move {
                let _ = pulse.send(());
            });
            (snapshot, received.recv_timeout(timeout).is_ok())
        };
        snapshot.await.unwrap();
        assert!(
            responsive,
            "工具快照等待 SQLite 时占住了唯一的 Tokio 工作线程"
        );
    }
}
