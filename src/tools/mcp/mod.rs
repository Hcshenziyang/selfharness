mod config;
mod transport;

pub(crate) use config::ServerConfig;
use rmcp::{
    RoleClient,
    model::{CallToolRequest, CallToolRequestParams, ServerResult},
    service::{Peer, PeerRequestOptions, RunningService},
};
use serde_json::{Value, json};
use std::{collections::BTreeMap, error::Error, sync::Arc, time::Duration};
use tokio::{
    sync::{Mutex, RwLock},
    time::timeout,
};

struct McpTool {
    peer: Peer<RoleClient>,
    original_name: String,
    definition: Value,
    supports_parallel: bool,
}

struct McpServer {
    service: RunningService<RoleClient, ()>,
    transport: &'static str,
    tools: BTreeMap<String, Arc<McpTool>>,
}

impl McpServer {
    fn connected(&self) -> bool {
        !self.service.is_closed() && !self.service.peer().is_transport_closed()
    }
}

// 每轮保留当时公布给模型的工具和连接，执行时不占服务表的锁。
pub(crate) struct McpTools {
    by_name: BTreeMap<String, Arc<McpTool>>,
}

#[derive(Default)]
pub(crate) struct McpClient {
    servers: RwLock<BTreeMap<String, McpServer>>,
    // 连接与断开按顺序完成，避免重复启动同名子进程。正在执行的回合持有自己的工具清单。
    connection_changes: Mutex<()>,
}

impl McpClient {
    /// Add a new server to the McpClient, given a name and a ServerConfig.
    pub(crate) async fn add_server(
        &self,
        name: String,
        config: ServerConfig,
    ) -> Result<(), Box<dyn Error + Send + Sync>> {
        config::validate_name(&name)?;
        let _change = self.connection_changes.lock().await;
        if self.servers.read().await.contains_key(&name) {
            return Err(format!("MCP 服务 {name} 已存在").into());
        }
        let transport_name = match &config.transport {
            config::TransportConfig::Stdio { .. } => "stdio",
            config::TransportConfig::Http { .. } => "http",
        };
        let (service, remote_tools) = timeout(Duration::from_secs(15), async {
            let service = transport::connect(config.transport).await?;
            let tools = service.list_all_tools().await?;
            Ok::<_, Box<dyn Error + Send + Sync>>((service, tools))
        })
        .await
        .map_err(|_| format!("MCP 服务 {name} 握手或获取工具清单超时"))?
        .map_err(|error| format!("MCP 服务 {name} 连接失败：{error}"))?;
        let mut servers = self.servers.write().await;
        let mut new_tools = BTreeMap::new();
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
            let tool_name = format!("mcp__{name}__{suffix}");
            if suffix.is_empty()
                || tool_name.len() > 64
                || servers
                    .values()
                    .any(|server| server.tools.contains_key(&tool_name))
                || new_tools.contains_key(&tool_name)
            {
                return Err(format!("MCP 工具无法使用名称 {tool_name}").into());
            }
            let definition = super::function_definition(
                &tool_name,
                tool.description.as_deref().unwrap_or_default(),
                tool.input_schema.as_ref(),
            );
            new_tools.insert(
                tool_name,
                Arc::new(McpTool {
                    peer: service.peer().clone(),
                    original_name,
                    definition,
                    supports_parallel: config.supports_parallel_tool_calls
                        || tool
                            .annotations
                            .is_some_and(|hints| hints.read_only_hint == Some(true)),
                }),
            );
        }
        servers.insert(
            name,
            McpServer {
                service,
                transport: transport_name,
                tools: new_tools,
            },
        );
        Ok(())
    }

    /// Return a list of all servers, with their name, type, connection status, and tools
    pub(crate) async fn servers(&self) -> Vec<Value> {
        self.servers
            .read()
            .await
            .iter()
            .map(|(name, server)| {
                json!({ "name": name, "type": server.transport, "connected": server.connected(),
                    "tools": server.tools.keys().collect::<Vec<_>>() })
            })
            .collect()
    }

    /// Remove a server
    pub(crate) async fn remove_server(&self, name: &str) -> Result<(), &'static str> {
        let _change = self.connection_changes.lock().await;
        let mut server = self
            .servers
            .write()
            .await
            .remove(name)
            .ok_or("MCP 服务不存在")?;
        let _ = server
            .service
            .close_with_timeout(Duration::from_secs(2))
            .await;
        Ok(())
    }

    pub(crate) async fn tools(&self) -> McpTools {
        let servers = self.servers.read().await;
        let mut by_name = BTreeMap::new();
        for server in servers.values().filter(|server| server.connected()) {
            for (name, tool) in &server.tools {
                by_name.insert(name.clone(), Arc::clone(tool));
            }
        }
        McpTools { by_name }
    }

    pub(crate) async fn shutdown(&self) {
        let servers = std::mem::take(&mut *self.servers.write().await);
        for mut server in servers.into_values() {
            let _ = server
                .service
                .close_with_timeout(Duration::from_secs(2))
                .await;
        }
    }
}

impl McpTools {
    /// Return an iterator(迭代器) over the tool definitions advertised for this turn.
    pub(super) fn definitions(&self) -> impl Iterator<Item = &Value> {
        self.by_name.values().map(|tool| &tool.definition)
    }

    pub(super) fn supports_parallel(&self, name: &str) -> bool {
        self.by_name
            .get(name)
            .is_some_and(|tool| tool.supports_parallel)
    }

    /// Call a tool by name with the given arguments
    pub(super) async fn call(&self, name: &str, arguments: &Value) -> Result<String, String> {
        let tool = self
            .by_name
            .get(name)
            .ok_or_else(|| format!("未知工具：{name}"))?;
        let arguments = arguments
            .as_object()
            .ok_or("MCP 工具参数必须为 JSON 对象")?;
        let request = CallToolRequest::new(
            CallToolRequestParams::new(tool.original_name.clone())
                .with_arguments(arguments.clone()),
        );
        let call_error = |error| format!("MCP 调用失败：{error}");
        let pending = tool
            .peer
            .send_cancellable_request(
                request.into(),
                PeerRequestOptions::with_timeout(Duration::from_secs(30)),
            )
            .await
            .map_err(call_error)?;
        let reply = pending.await_response().await.map_err(call_error)?;
        let ServerResult::CallToolResult(result) = reply else {
            return Err("MCP 服务未返回工具结果".into());
        };
        let text_blocks = result
            .content
            .iter()
            .filter_map(|item| item.as_text().map(|value| value.text.as_str()))
            .collect::<Vec<_>>();
        if result.is_error == Some(true) {
            let text = text_blocks.join("\n");
            return Err(if text.is_empty() {
                "MCP 工具执行失败".into()
            } else {
                text
            });
        }
        if let Some(value) = result.structured_content {
            return Ok(value.to_string());
        }
        if text_blocks.is_empty() && !result.content.is_empty() {
            return Err("暂只处理 MCP 的文本和结构化 JSON 结果".into());
        }
        Ok(text_blocks.join("\n"))
    }
}
