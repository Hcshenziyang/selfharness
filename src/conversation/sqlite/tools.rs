use super::ConversationStore;
use crate::tools::{Binding, Catalog, ToolPolicy};
use rusqlite::{Connection, Result, params, types::Type};

pub(super) fn init(db: &Connection) -> Result<()> {
    db.execute_batch(
        "CREATE TABLE IF NOT EXISTS agent_tool_settings (
            agent_id INTEGER PRIMARY KEY REFERENCES agents(id) ON DELETE CASCADE,
            revision INTEGER NOT NULL, echo INTEGER NOT NULL, memory_update INTEGER NOT NULL,
            read_file INTEGER NOT NULL, write_file INTEGER NOT NULL, workspace_root TEXT
        );
        CREATE TABLE IF NOT EXISTS mcp_bindings (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            agent_id INTEGER NOT NULL REFERENCES agents(id) ON DELETE CASCADE,
            name TEXT NOT NULL, transport TEXT NOT NULL, catalog TEXT NOT NULL, revision INTEGER NOT NULL DEFAULT 1,
            UNIQUE (agent_id, name)
        );"
    )
}

impl ConversationStore {
    pub(crate) fn tool_policy(&self, agent_id: i64) -> Result<ToolPolicy> {
        self.db().query_row(
            "SELECT coalesce(s.revision, 0), coalesce(s.echo, 1), coalesce(s.memory_update, 0),
                    coalesce(s.read_file, 0), coalesce(s.write_file, 0), s.workspace_root
             FROM agents a LEFT JOIN agent_tool_settings s ON s.agent_id = a.id WHERE a.id = ?1",
            [agent_id],
            |r| {
                Ok(ToolPolicy {
                    revision: r.get(0)?,
                    echo: r.get(1)?,
                    memory_update: r.get(2)?,
                    read_file: r.get(3)?,
                    write_file: r.get(4)?,
                    workspace_root: r.get(5)?,
                })
            },
        )
    }

    pub(crate) fn save_tool_policy(&self, agent_id: i64, policy: &ToolPolicy) -> Result<bool> {
        let updated = self.db().execute(
            "INSERT INTO agent_tool_settings (agent_id, revision, echo, memory_update, read_file, write_file, workspace_root)
             SELECT a.id, ?2 + 1, ?3, ?4, ?5, ?6, ?7 FROM agents a
             LEFT JOIN agent_tool_settings s ON s.agent_id = a.id
             WHERE a.id = ?1 AND coalesce(s.revision, 0) = ?2
             ON CONFLICT(agent_id) DO UPDATE SET revision = excluded.revision, echo = excluded.echo,
                 memory_update = excluded.memory_update, read_file = excluded.read_file,
                 write_file = excluded.write_file, workspace_root = excluded.workspace_root",
            params![agent_id, policy.revision, policy.echo, policy.memory_update,
                policy.read_file, policy.write_file, policy.workspace_root]
        )?;
        Ok(updated == 1)
    }

    /// 另一回合或手动编辑已改过笔记时，保留先保存的内容。
    pub(crate) fn update_memory(
        &self,
        agent_id: i64,
        previous: &str,
        memory: &str,
    ) -> Result<bool> {
        Ok(self.db().execute(
            "UPDATE agents SET memory = ?3, revision = revision + 1
            WHERE id = ?1 AND memory = ?2",
            params![agent_id, previous, memory],
        )? == 1)
    }

    pub(crate) fn bindings(&self, agent_id: i64) -> Result<Vec<Binding>> {
        let db = self.db();
        let mut select = db.prepare(
            "SELECT id, name, transport, catalog, revision
             FROM mcp_bindings WHERE agent_id = ?1 ORDER BY id",
        )?;
        select
            .query_map([agent_id], |row| {
                let catalog = serde_json::from_str(&row.get::<_, String>(3)?).map_err(|e| {
                    rusqlite::Error::FromSqlConversionFailure(3, Type::Text, Box::new(e))
                })?;
                Ok(Binding {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    transport: row.get(2)?,
                    revision: row.get(4)?,
                    catalog,
                })
            })?
            .collect()
    }

    pub(crate) fn save_binding(
        &self,
        agent_id: i64,
        id: Option<i64>,
        name: &str,
        transport: &str,
        catalog: &Catalog,
    ) -> Result<(i64, i64)> {
        let catalog = serde_json::to_value(catalog)
            .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
        let db = self.db();
        if let Some(id) = id {
            db.query_row(
                "UPDATE mcp_bindings SET transport = ?3, catalog = ?4, revision = revision + 1
                 WHERE agent_id = ?1 AND id = ?2 RETURNING id, revision",
                params![agent_id, id, transport, catalog],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
        } else {
            db.query_row(
                "INSERT INTO mcp_bindings (agent_id, name, transport, catalog)
                 VALUES (?1, ?2, ?3, ?4) RETURNING id, revision",
                params![agent_id, name, transport, catalog],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
        }
    }

    pub(crate) fn remove_binding(&self, agent_id: i64, id: i64) -> Result<bool> {
        Ok(self.db().execute(
            "DELETE FROM mcp_bindings WHERE agent_id = ?1 AND id = ?2",
            params![agent_id, id],
        )? == 1)
    }
}
