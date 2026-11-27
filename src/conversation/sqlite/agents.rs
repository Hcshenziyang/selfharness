use super::ConversationStore;
use crate::agent::{Agent, AgentSettings, AgentSummary};
use rusqlite::{OptionalExtension, Result, Row, params};

impl ConversationStore {
    /// 侧栏只读取切换助手需要的信息。
    pub(crate) fn agents(&self) -> Result<Vec<AgentSummary>> {
        self.db()
            .prepare("SELECT id, name, model FROM agents ORDER BY id")?
            .query_map([], |row| {
                Ok(AgentSummary {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    model: row.get(2)?,
                })
            })?
            .collect()
    }

    pub(crate) fn agent(&self, id: i64) -> Result<Agent> {
        self.db().query_row(
            "SELECT id, name, model, instructions, memory, revision FROM agents WHERE id = ?1",
            [id],
            read_agent,
        )
    }

    pub(crate) fn create_agent(&self, settings: &AgentSettings) -> Result<Agent> {
        self.db().query_row(
            "INSERT INTO agents (name, model, instructions, memory) VALUES (?1, ?2, ?3, ?4)
             RETURNING id, name, model, instructions, memory, revision",
            params![
                settings.name,
                settings.model,
                settings.instructions,
                settings.memory
            ],
            read_agent,
        )
    }

    /// 同一份设置在两个标签页打开时，后提交的旧版本不会覆盖已经保存的笔记。
    pub(crate) fn update_agent(
        &self,
        id: i64,
        revision: i64,
        settings: &AgentSettings,
    ) -> Result<Option<Agent>> {
        let updated = self
            .db()
            .query_row(
                "UPDATE agents SET name = ?1, model = ?2, instructions = ?3, memory = ?4,
                revision = revision + 1 WHERE id = ?5 AND revision = ?6
             RETURNING id, name, model, instructions, memory, revision",
                params![
                    settings.name,
                    settings.model,
                    settings.instructions,
                    settings.memory,
                    id,
                    revision
                ],
                read_agent,
            )
            .optional()?;
        if updated.is_none() {
            self.agent(id)?;
        }
        Ok(updated)
    }
}

fn read_agent(row: &Row<'_>) -> Result<Agent> {
    Ok(Agent {
        id: row.get(0)?,
        settings: AgentSettings {
            name: row.get(1)?,
            model: row.get(2)?,
            instructions: row.get(3)?,
            memory: row.get(4)?,
        },
        revision: row.get(5)?,
    })
}
