mod files;
mod mcp;

use crate::{agent::MEMORY_LIMIT, conversation::ConversationStore};
use futures_util::future::join_all;
pub(crate) use mcp::{Binding, Catalog, McpClient, McpError, ServerConfig};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};
use tokio::task::block_in_place;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ToolPolicy {
    pub revision: i64,
    pub echo: bool,
    pub memory_update: bool,
    pub read_file: bool,
    pub write_file: bool,
    pub workspace_root: Option<String>,
}

impl ToolPolicy {
    pub(crate) fn validate(&mut self, previous_root: Option<&str>) -> Result<(), String> {
        let file_access = self.read_file || self.write_file;
        match self.workspace_root.as_deref().filter(|s| !s.is_empty()) {
            None if file_access => return Err("启用文件读写前请先设置工作目录".into()),
            None => self.workspace_root = None,
            Some(root) if Some(root) == previous_root => {
                // 重新保存其他开关时，不能把已替换成链接的旧路径解析为新的授权目录。
                if file_access {
                    files::check_root(root.as_ref())?;
                }
            }
            Some(root) => self.workspace_root = Some(files::normalize_root(root)?),
        }
        Ok(())
    }
}

pub(crate) struct AgentToolRegistry {
    store: ConversationStore,
    agents: Mutex<BTreeMap<i64, Arc<McpClient>>>,
}

impl AgentToolRegistry {
    pub(crate) fn new(store: ConversationStore) -> Self {
        Self {
            store,
            agents: Mutex::default(),
        }
    }

    pub(crate) fn agent(&self, id: i64) -> rusqlite::Result<Arc<McpClient>> {
        use std::collections::btree_map::Entry;
        block_in_place(|| {
            let mut agents = self.agents.lock().unwrap();
            Ok(Arc::clone(match agents.entry(id) {
                Entry::Occupied(e) => e.into_mut(),
                Entry::Vacant(e) => e.insert(Arc::new(McpClient::load(id, self.store.clone())?)),
            }))
        })
    }

    pub(crate) fn turn(&self, agent_id: i64, memory: &str) -> rusqlite::Result<TurnTools> {
        Ok(TurnTools {
            mcp: self.agent(agent_id)?,
            agent_id,
            store: self.store.clone(),
            memory: Mutex::new(memory.to_owned()),
        })
    }

    pub(crate) async fn shutdown(&self) {
        let agents = std::mem::take(&mut *self.agents.lock().unwrap());
        for agent in agents.into_values() {
            agent.shutdown().await;
        }
    }
}

pub(crate) struct TurnTools {
    agent_id: i64,
    store: ConversationStore,
    mcp: Arc<McpClient>,
    memory: Mutex<String>,
}

pub(crate) struct ToolSnapshot {
    policy: ToolPolicy,
    mcp: mcp::McpTools,
}

impl ToolSnapshot {
    pub(crate) fn definitions(&self) -> Value {
        let mut definitions = Vec::new();
        for (enabled, name, description, fields) in [
            (self.policy.echo, "echo", "原样返回文字", &["text"][..]),
            (
                self.policy.memory_update,
                "memory_update",
                "用完整笔记更新本助手的私有记忆，最多 4000 字符，后续对话回合可见",
                &["memory"][..],
            ),
            (
                self.policy.read_file,
                "read_file",
                "读取工作目录内的 UTF-8 文本，path 为相对路径，最多 64 KiB",
                &["path"][..],
            ),
            (
                self.policy.write_file,
                "write_file",
                "在工作目录内创建或替换 UTF-8 文本，path 为相对路径，父目录须已存在，最多 64 KiB",
                &["path", "content"][..],
            ),
        ] {
            if enabled {
                let properties = fields
                    .iter()
                    .map(|field| (field.to_string(), json!({"type":"string"})))
                    .collect::<serde_json::Map<_, _>>();
                definitions.push(function_definition(name, description, json!({
                    "type":"object", "properties":properties, "required": fields, "additionalProperties": false
                })));
            }
        }
        definitions.extend(self.mcp.definitions().cloned());
        Value::Array(definitions)
    }

    fn supports_parallel(&self, name: &str) -> bool {
        match name {
            "echo" => self.policy.echo,
            "read_file" => self.policy.read_file,
            _ => self.mcp.supports_parallel(name),
        }
    }
}

fn function_definition(name: &str, description: &str, parameters: impl Serialize) -> Value {
    json!({ "type": "function", "name": name, "description": description, "parameters": parameters, "strict": false })
}

pub(crate) struct ToolRequest<'a> {
    pub(crate) name: &'a str,
    pub(crate) arguments: &'a str,
}

#[derive(Deserialize)]
struct Echo {
    text: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Memory {
    memory: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReadFile {
    path: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WriteFile {
    path: String,
    content: String,
}

fn parse<T: DeserializeOwned>(input: &str) -> Result<T, String> {
    serde_json::from_str(input).map_err(|e| format!("工具参数不是有效 JSON 或字段不符：{e}"))
}

impl TurnTools {
    fn update_memory(&self, memory: String) -> Result<String, String> {
        if memory.chars().count() > MEMORY_LIMIT {
            return Err("私有记忆最多可保存 4000 个字符".into());
        }
        block_in_place(|| {
            let mut previous = self.memory.lock().unwrap();
            if !self
                .store
                .update_memory(self.agent_id, &previous, &memory)
                .map_err(|e| e.to_string())?
            {
                return Err(
                    "私有记忆已被其他回合或用户修改，请在下一回合读取最新笔记后再更新".into(),
                );
            }
            *previous = memory;
            Ok("已保存私有记忆".into())
        })
    }

    pub(crate) async fn snapshot(&self) -> rusqlite::Result<ToolSnapshot> {
        self.mcp.snapshot().await
    }

    pub(crate) async fn call_batch(
        &self,
        requests: &[ToolRequest<'_>],
        snapshot: &ToolSnapshot,
    ) -> Vec<String> {
        let mut results = Vec::with_capacity(requests.len());
        for group in requests.chunk_by(|a, b| {
            snapshot.supports_parallel(a.name) && snapshot.supports_parallel(b.name)
        }) {
            let pending = group.iter().map(|request| self.call(request, snapshot));
            for result in join_all(pending).await {
                results.push(result.unwrap_or_else(|error| json!({ "error": error }).to_string()));
            }
        }
        results
    }

    async fn call(
        &self,
        request: &ToolRequest<'_>,
        snapshot: &ToolSnapshot,
    ) -> Result<String, String> {
        let policy = &snapshot.policy;
        let args = request.arguments;
        match request.name {
            "echo" if policy.echo => Ok(parse::<Echo>(args)?.text),
            "memory_update" if policy.memory_update => {
                let Memory { memory } = parse(args)?;
                self.update_memory(memory)
            }
            "read_file" if policy.read_file => {
                let ReadFile { path } = parse(args)?;
                let root = policy.workspace_root.clone().ok_or("尚未配置工作目录")?;
                tokio::task::spawn_blocking(move || files::read(&root, &path))
                    .await
                    .map_err(|e| e.to_string())?
                    .map_err(|e| format!("读取文件失败：{e}"))
            }
            "write_file" if policy.write_file => {
                let WriteFile { path, content } = parse(args)?;
                let root = policy.workspace_root.clone().ok_or("尚未配置工作目录")?;
                tokio::task::spawn_blocking(move || files::write(&root, &path, &content))
                    .await
                    .map_err(|e| e.to_string())?
                    .map_err(|e| format!("写入文件失败：{e}"))
            }
            name => snapshot.mcp.call(name, &parse(args)?).await,
        }
    }
}
