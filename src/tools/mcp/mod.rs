mod config;
mod transport;

pub(crate) use config::ServerConfig;
use rmcp::{
    RoleClient,
    model::{CallToolRequest, CallToolRequestParams, ServerResult},
    service::{Peer, PeerRequestOptions, RunningService, ServiceError},
};
use serde_json::{Value, json};
use std::{collections::BTreeMap, error::Error, sync::Arc, time::Duration};
use tokio::{
    sync::{Mutex, RwLock},
    time::timeout,
};

// ******************************** 已连接的服务与工具 ********************************

/// 一项 MCP 工具，保存模型看到的定义，以及向原服务发起调用所需的连接和原名。
struct McpTool {
    peer: Peer<RoleClient>,
    original_name: String,
    definition: Value,
    supports_parallel: bool,
}

/// 一个已注册的服务及其公布的工具，持有连接直到服务断开。
struct McpServer {
    service: RunningService<RoleClient, ()>,
    transport: &'static str,
    tools: BTreeMap<String, Arc<McpTool>>,
}

impl McpServer {
    /// 同时检查 MCP 会话和底层传输，任一关闭都视为已断开。
    fn connected(&self) -> bool {
        !self.service.is_closed() && !self.service.peer().is_transport_closed()
    }
}

// ******************************** 服务注册与连接管理 ********************************

/// 所有会话共享的 MCP 客户端，负责连接、断开服务和生成可用工具快照。
#[derive(Default)]
pub(crate) struct McpClient {
    servers: RwLock<BTreeMap<String, McpServer>>,
    // 连接与断开按顺序完成，避免重复启动同名子进程。
    connection_changes: Mutex<()>, // mutex 互斥锁，同一时间只允许一个连接或断开操作
}

impl McpClient {
    /// 连接服务并获取完整工具清单。所有工具名校验通过后，才将整批工具发布给模型。
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
            // 对模型使用带服务前缀的名称，同名工具可以来自不同服务。发给 MCP 时还原原名。
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

    /// 列出供网页展示的服务名称、传输方式、实时连接状态和工具名。
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

    /// 先从注册表移除服务，再尝试关闭原连接。此后取得的工具快照会排除该服务。
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

    /// 收集当前仍连接的工具，供下一次请求模型和执行其返回的调用共同使用。
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

    /// 服务端退出时取走注册表里的所有连接，并逐个关闭。
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

// ******************************** 通过快照调用工具 ********************************

/// 一次模型请求的工具快照。同一批工具调用沿用当时公布的连接，无需持有服务表的锁。
pub(crate) struct McpTools {
    by_name: BTreeMap<String, Arc<McpTool>>,
}

impl McpTools {
    /// 返回工具定义的迭代器，供上层组装本次模型请求的工具清单。
    pub(super) fn definitions(&self) -> impl Iterator<Item = &Value> {
        self.by_name.values().map(|tool| &tool.definition)
    }

    /// 查询指定工具的并发标记，来源是服务配置或工具声明的只读提示。
    pub(super) fn supports_parallel(&self, name: &str) -> bool {
        self.by_name
            .get(name)
            .is_some_and(|tool| tool.supports_parallel)
    }

    /// 找到模型指定的工具并通过快照中的连接调用它。成功时优先返回结构化 JSON，其次是文本。
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
        if tool.peer.is_transport_closed() {
            return Err("MCP 服务已断开，工具调用未发出".into());
        }
        let pending = tool
            .peer
            .send_cancellable_request(
                request.into(),
                PeerRequestOptions::with_timeout(Duration::from_secs(30)),
            )
            .await
            .map_err(|error| match error {
                ServiceError::TransportClosed => "MCP 服务已断开，工具调用未发出".into(),
                _ => format!("MCP 工具调用未发出：{error}"),
            })?;
        // 已提交的调用可能在远端执行。此后等待超时或断线，无法据此判断工具是否成功。
        let reply = pending
            .await_response()
            .await
            .map_err(|error| match error {
                ServiceError::McpError(_) => format!("MCP 调用失败：{error}"),
                _ => format!("MCP 工具执行结果未知：{error}。重试前请先核实执行情况"),
            })?;
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
