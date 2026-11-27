mod calls;
mod config;
mod transport;

use crate::conversation::ConversationStore;
pub(super) use calls::McpTools;
pub(crate) use config::ServerConfig;
use rmcp::{
    RoleClient,
    service::{Peer, RunningService},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{collections::BTreeMap, error::Error, sync::Arc, time::Duration};
use tokio::{
    sync::{Mutex, RwLock},
    task::block_in_place,
    time::{Instant, timeout_at},
};

pub(crate) type McpError = Box<dyn Error + Send + Sync>;
pub(crate) type Catalog = BTreeMap<String, ToolGrant>;

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ToolGrant {
    original: String,
    enabled: bool,
    discovered: bool,
}

/// 持久化记录只含服务和工具的标识及授权。连接参数随活动连接一起释放。
pub(crate) struct Binding {
    pub(crate) id: i64,
    pub(crate) name: String,
    pub(crate) transport: String,
    pub(crate) revision: i64,
    pub(crate) catalog: Catalog,
}

struct McpTool {
    peer: Peer<RoleClient>,
    original_name: String,
    definition: Value,
    supports_parallel: bool,
}

struct McpServer {
    service: RunningService<RoleClient, ()>,
    config: ServerConfig,
    tools: BTreeMap<String, Arc<McpTool>>,
}

impl McpServer {
    fn connected(&self) -> bool {
        !self.service.is_closed() && !self.service.peer().is_transport_closed()
    }

    async fn close(mut self) {
        let _ = self
            .service
            .close_with_timeout(Duration::from_secs(2))
            .await;
    }

    async fn connect(name: &str, config: ServerConfig) -> Result<Self, McpError> {
        let deadline = Instant::now() + Duration::from_secs(15);
        let timed_out = |_| format!("MCP 服务 {name} 握手或获取工具清单超时");
        let service = timeout_at(deadline, transport::connect(config.transport.clone()))
            .await
            .map_err(timed_out)?
            .map_err(|e| format!("MCP 服务 {name} 连接失败：{e}"))?;
        let mut server = Self {
            service,
            config,
            tools: BTreeMap::new(),
        };
        let discovery = match timeout_at(deadline, server.discover(name)).await {
            Ok(result) => result,
            Err(elapsed) => Err(timed_out(elapsed).into()),
        };
        if let Err(error) = discovery {
            server.close().await;
            return Err(error);
        }
        Ok(server)
    }

    async fn discover(&mut self, name: &str) -> Result<(), McpError> {
        let remote_tools = self
            .service
            .list_all_tools()
            .await
            .map_err(|e| format!("MCP 服务 {name} 获取工具清单失败：{e}"))?;
        for tool in remote_tools {
            let original_name = tool.name.into_owned();
            let suffix: String = original_name
                .chars()
                .map(|ch| {
                    if ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-') {
                        ch
                    } else {
                        '_'
                    }
                })
                .collect();
            let public = format!("mcp__{name}__{suffix}");
            if suffix.is_empty() || public.len() > 64 || self.tools.contains_key(&public) {
                return Err(format!("MCP 工具无法使用名称 {public}").into());
            }
            let definition = super::function_definition(
                &public,
                tool.description.as_deref().unwrap_or_default(),
                tool.input_schema.as_ref(),
            );
            self.tools.insert(
                public,
                Arc::new(McpTool {
                    peer: self.service.peer().clone(),
                    original_name,
                    definition,
                    supports_parallel: self.config.supports_parallel_tool_calls
                        || tool
                            .annotations
                            .is_some_and(|h| h.read_only_hint == Some(true)),
                }),
            );
        }
        Ok(())
    }
}

struct Slot {
    binding: Binding,
    live: Option<McpServer>,
}

pub(crate) struct McpClient {
    agent_id: i64,
    store: ConversationStore,
    slots: RwLock<BTreeMap<i64, Slot>>,
    changes: Mutex<()>,
}

fn missing() -> McpError {
    rusqlite::Error::QueryReturnedNoRows.into()
}

impl McpClient {
    pub(super) fn load(agent_id: i64, store: ConversationStore) -> rusqlite::Result<Self> {
        store.tool_policy(agent_id)?;
        let slots = store
            .bindings(agent_id)?
            .into_iter()
            .map(|binding| {
                (
                    binding.id,
                    Slot {
                        binding,
                        live: None,
                    },
                )
            })
            .collect();
        Ok(Self {
            agent_id,
            store,
            slots: RwLock::new(slots),
            changes: Mutex::new(()),
        })
    }

    pub(crate) async fn add_server(
        &self,
        name: String,
        config: ServerConfig,
    ) -> Result<(), McpError> {
        config::validate_name(&name)?;
        let _change = self.changes.lock().await;
        if self
            .slots
            .read()
            .await
            .values()
            .any(|s| s.binding.name == name)
        {
            return Err(format!("MCP 服务 {name} 已存在").into());
        }
        self.connect(None, name, config).await
    }

    pub(crate) async fn replace_server(
        &self,
        id: i64,
        replacement: Option<ServerConfig>,
    ) -> Result<(), McpError> {
        let _change = self.changes.lock().await;
        let (name, config) = {
            let slots = self.slots.read().await;
            let slot = slots.get(&id).ok_or_else(missing)?;
            let config = replacement
                .or_else(|| slot.live.as_ref().map(|live| live.config.clone()))
                .ok_or("请重新填写连接配置及所需凭据")?;
            (slot.binding.name.clone(), config)
        };
        self.connect(Some(id), name, config).await
    }

    /// 握手时仍可使用旧连接。持久化与发布在同一写锁内完成，再关闭已替换的连接。
    async fn connect(
        &self,
        id: Option<i64>,
        name: String,
        config: ServerConfig,
    ) -> Result<(), McpError> {
        let live = McpServer::connect(&name, config).await?;
        let (retired, result) = {
            let mut slots = self.slots.write().await;
            match block_in_place(|| self.save_discovery(&slots, id, name, &live)) {
                Ok(binding) => {
                    let previous = slots.insert(
                        binding.id,
                        Slot {
                            binding,
                            live: Some(live),
                        },
                    );
                    (previous.and_then(|slot| slot.live), Ok(()))
                }
                Err(error) => (Some(live), Err(error)),
            }
        };
        if let Some(server) = retired {
            server.close().await;
        }
        result
    }

    fn save_discovery(
        &self,
        slots: &BTreeMap<i64, Slot>,
        id: Option<i64>,
        name: String,
        live: &McpServer,
    ) -> Result<Binding, McpError> {
        let old = id.and_then(|id| slots.get(&id));
        let mut catalog = Catalog::new();
        for (public, tool) in &live.tools {
            if slots.values().any(|slot| {
                Some(slot.binding.id) != id && slot.binding.catalog.contains_key(public)
            }) {
                return Err(format!("MCP 工具无法使用名称 {public}").into());
            }
            let enabled = old
                .and_then(|slot| slot.binding.catalog.get(public))
                .is_some_and(|grant| grant.enabled && grant.original == tool.original_name);
            catalog.insert(
                public.clone(),
                ToolGrant {
                    original: tool.original_name.clone(),
                    enabled,
                    discovered: true,
                },
            );
        }
        // 暂时撤下的工具仍可撤销授权，以原名重新出现前无法调用。
        if let Some(old) = old {
            for (public, grant) in &old.binding.catalog {
                if grant.enabled && !catalog.contains_key(public) {
                    catalog.insert(
                        public.clone(),
                        ToolGrant {
                            discovered: false,
                            ..grant.clone()
                        },
                    );
                }
            }
        }
        let transport = match &live.config.transport {
            config::TransportConfig::Stdio { .. } => "stdio",
            config::TransportConfig::Http { .. } => "http",
        };
        let (id, revision) =
            self.store
                .save_binding(self.agent_id, id, &name, transport, &catalog)?;
        Ok(Binding {
            id,
            name,
            transport: transport.into(),
            revision,
            catalog,
        })
    }

    pub(crate) async fn servers(&self) -> Vec<Value> {
        self.slots.read().await.values().map(|slot| {
            let binding = &slot.binding;
            let mut tools = Vec::new();
            let mut enabled = Vec::new();
            let mut unavailable = Vec::new();
            for (name, grant) in &binding.catalog {
                if grant.discovered {
                    tools.push(name);
                    if grant.enabled { enabled.push(name); }
                } else {
                    unavailable.push(name);
                }
            }
            json!({ "id": binding.id, "name": binding.name, "type": binding.transport, "revision": binding.revision,
                "connected": slot.live.as_ref().is_some_and(McpServer::connected),
                "can_reconnect": slot.live.is_some(), "tools": tools, "enabled_tools": enabled, "unavailable_tools": unavailable })
        }).collect()
    }

    pub(crate) async fn set_grants(
        &self,
        id: i64,
        revision: i64,
        changes: BTreeMap<String, bool>,
    ) -> Result<bool, McpError> {
        let mut slots = self.slots.write().await;
        let binding = &mut slots.get_mut(&id).ok_or_else(missing)?.binding;
        if binding.revision != revision {
            return Ok(false);
        }
        if changes.is_empty() {
            return Ok(true);
        }
        let mut catalog = binding.catalog.clone();
        for (name, enabled) in changes {
            catalog
                .get_mut(&name)
                .ok_or_else(|| format!("服务没有工具 {name}"))?
                .enabled = enabled;
        }
        catalog.retain(|_, grant| grant.discovered || grant.enabled);
        if catalog == binding.catalog {
            return Ok(true);
        }
        let (_, revision) = block_in_place(|| {
            self.store.save_binding(
                self.agent_id,
                Some(id),
                &binding.name,
                &binding.transport,
                &catalog,
            )
        })?;
        binding.catalog = catalog;
        binding.revision = revision;
        Ok(true)
    }

    pub(crate) async fn disconnect(&self, id: i64) -> Result<(), McpError> {
        let _change = self.changes.lock().await;
        let previous = self
            .slots
            .write()
            .await
            .get_mut(&id)
            .ok_or_else(missing)?
            .live
            .take();
        if let Some(server) = previous {
            server.close().await;
        }
        Ok(())
    }

    pub(crate) async fn remove_server(&self, id: i64) -> Result<(), McpError> {
        let _change = self.changes.lock().await;
        let slot = {
            let mut slots = self.slots.write().await;
            if !block_in_place(|| self.store.remove_binding(self.agent_id, id))? {
                return Err(missing());
            }
            slots.remove(&id).ok_or_else(missing)?
        };
        if let Some(server) = slot.live {
            server.close().await;
        }
        Ok(())
    }

    /// 在同一时点读取内置权限和已授权的活动连接。快照只持有当时的 peer。
    pub(super) async fn snapshot(&self) -> rusqlite::Result<super::ToolSnapshot> {
        let slots = self.slots.read().await;
        let policy = block_in_place(|| self.store.tool_policy(self.agent_id))?;
        let mut by_name = BTreeMap::new();
        for slot in slots.values() {
            if let Some(live) = slot.live.as_ref().filter(|s| s.connected()) {
                for (public, tool) in &live.tools {
                    if slot
                        .binding
                        .catalog
                        .get(public)
                        .is_some_and(|grant| grant.enabled)
                    {
                        by_name.insert(public.clone(), Arc::clone(tool));
                    }
                }
            }
        }
        Ok(super::ToolSnapshot {
            policy,
            mcp: McpTools { by_name },
        })
    }

    pub(super) async fn shutdown(&self) {
        let slots = std::mem::take(&mut *self.slots.write().await);
        for slot in slots.into_values() {
            if let Some(server) = slot.live {
                server.close().await;
            }
        }
    }
}
