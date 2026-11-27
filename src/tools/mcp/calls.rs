use super::McpTool;
use rmcp::{
    model::{CallToolRequest, CallToolRequestParams, ServerResult},
    service::{PeerRequestOptions, ServiceError},
};
use serde_json::Value;
use std::{collections::BTreeMap, sync::Arc, time::Duration};

// ******************************** 通过快照调用工具 ********************************

/// 一次模型请求的工具快照。同一批工具调用沿用当时公布的连接，无需持有服务表的锁。
pub(crate) struct McpTools {
    pub(super) by_name: BTreeMap<String, Arc<McpTool>>,
}

impl McpTools {
    /// 返回工具定义的迭代器，供上层组装本次模型请求的工具清单。
    pub(crate) fn definitions(&self) -> impl Iterator<Item = &Value> {
        self.by_name.values().map(|tool| &tool.definition)
    }

    /// 查询指定工具的并发标记，来源是服务配置或工具声明的只读提示。
    pub(crate) fn supports_parallel(&self, name: &str) -> bool {
        self.by_name
            .get(name)
            .is_some_and(|tool| tool.supports_parallel)
    }

    /// 找到模型指定的工具并通过快照中的连接调用它。成功时优先返回结构化 JSON，其次是文本。
    pub(crate) async fn call(&self, name: &str, arguments: &Value) -> Result<String, String> {
        let tool = self
            .by_name
            .get(name)
            .ok_or_else(|| format!("本次请求没有可用的已授权工具 {name}"))?;
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
