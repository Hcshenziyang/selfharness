mod config;
mod transport;

pub(crate) use config::ServerConfig;
use rmcp::{
    RoleClient,
    model::{CallToolRequest, CallToolRequestParams, ServerResult},
    service::{PeerRequestOptions, RunningService},
};
use serde_json::{Value, json};
use std::{collections::BTreeMap, error::Error, time::Duration};
use tokio::{runtime::Runtime, time::timeout};

struct McpTool {
    server_index: usize,
    original_name: String,
    definition: Value,
}

struct ServerInfo {
    index: usize,
    transport: &'static str,
}

pub(crate) struct McpClient {
    services: Vec<Option<RunningService<RoleClient, ()>>>,
    runtime: Runtime,
    tools: BTreeMap<String, McpTool>,
    servers: BTreeMap<String, ServerInfo>,
}

impl McpClient {
    /// Create a new McpClient with an empty list of services and tools, and a new Tokio runtime
    pub(crate) fn empty() -> Result<Self, Box<dyn Error>> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()?;
        Ok(Self {
            services: Vec::new(),
            runtime,
            tools: BTreeMap::new(),
            servers: BTreeMap::new(),
        })
    }

    /// Add a new server to the McpClient, given a name and a ServerConfig.
    pub(crate) fn add_server(
        &mut self,
        name: String,
        config: ServerConfig,
    ) -> Result<(), Box<dyn Error>> {
        config::validate_name(&name)?;
        if self.servers.contains_key(&name) {
            return Err(format!("MCP 服务 {name} 已存在").into());
        }
        let transport_name = match &config {
            ServerConfig::Stdio { .. } => "stdio",
            ServerConfig::Http { .. } => "http",
        };
        let (service, remote_tools) = self
            .runtime
            .block_on(async {
                timeout(Duration::from_secs(15), async {
                    let service = transport::connect(config).await?;
                    let tools = service.list_all_tools().await?;
                    Ok::<_, Box<dyn Error>>((service, tools))
                })
                .await
            })
            .map_err(|_| format!("MCP 服务 {name} 握手或获取工具清单超时"))?
            .map_err(|error| format!("MCP 服务 {name} 连接失败：{error}"))?;
        let server_index = self.services.len();
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
                || self.tools.contains_key(&tool_name)
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
                McpTool {
                    server_index,
                    original_name,
                    definition,
                },
            );
        }
        self.services.push(Some(service));
        self.tools.extend(new_tools);
        self.servers.insert(
            name,
            ServerInfo {
                index: server_index,
                transport: transport_name,
            },
        );
        Ok(())
    }

    /// Return a list of all servers, with their name, type, connection status, and tools
    pub(crate) fn servers(&self) -> Vec<Value> {
        self.servers.iter().map(|(name, info)| {
            let connected = self.services[info.index]
                .as_ref().is_some_and(|service| !service.is_closed() && !service.peer().is_transport_closed());
            let tools: Vec<_> = self.tools.iter()
                .filter(|(_, tool)| tool.server_index == info.index)
                .map(|(tool_name, _)| tool_name.as_str())
                .collect();
            json!({ "name": name, "type": info.transport, "connected": connected, "tools": tools })
        }).collect()
    }

    /// Remove a server
    pub(crate) fn remove_server(&mut self, name: &str) -> Result<(), Box<dyn Error>> {
        let info = self.servers.remove(name).ok_or("MCP 服务不存在")?;
        self.tools.retain(|_, tool| tool.server_index != info.index);
        if let Some(mut service) = self.services[info.index].take() {
            self.runtime.block_on(async {
                let _ = service.close_with_timeout(Duration::from_secs(2)).await;
            });
        }
        Ok(())
    }

    /// Return an iterator(迭代器) over all tool definitions that are currently connected to a server
    pub(super) fn definitions(&self) -> impl Iterator<Item = &Value> {
        self.tools
            .values()
            .filter(|tool| {
                self.services[tool.server_index]
                    .as_ref()
                    .is_some_and(|service| {
                        !service.is_closed() && !service.peer().is_transport_closed()
                    })
            })
            .map(|tool| &tool.definition)
    }

    /// Call a tool by name with the given arguments
    pub(super) fn call(&self, name: &str, arguments: &Value) -> Result<String, String> {
        let tool = self
            .tools
            .get(name)
            .ok_or_else(|| format!("未知工具：{name}"))?;
        let arguments = arguments
            .as_object()
            .ok_or("MCP 工具参数必须为 JSON 对象")?;
        let request = CallToolRequest::new(
            CallToolRequestParams::new(tool.original_name.clone())
                .with_arguments(arguments.clone()),
        );
        let service = self.services[tool.server_index]
            .as_ref()
            .ok_or("MCP 服务已断开")?;
        let reply = self
            .runtime
            .block_on(async {
                service
                    .send_cancellable_request(
                        request.into(),
                        PeerRequestOptions::with_timeout(Duration::from_secs(30)),
                    )
                    .await?
                    .await_response()
                    .await
            })
            .map_err(|error| format!("MCP 调用失败：{error}"))?;
        let ServerResult::CallToolResult(result) = reply else {
            return Err("MCP 服务未返回工具结果".into());
        };
        let text_blocks = result
            .content
            .iter()
            .filter_map(|item| item.raw.as_text().map(|value| value.text.as_str()))
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
