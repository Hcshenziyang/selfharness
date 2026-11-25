mod mcp;

use futures_util::future::join_all;
pub(crate) use mcp::{McpClient, McpTools, ServerConfig};
use serde_json::{Value, json};

// ******************************** 提供给模型的工具清单 ********************************

/// 合并内置 echo 和当前 MCP 快照中的工具定义，供一次模型请求使用。
pub(crate) fn definitions(mcp: &McpTools) -> Value {
    let mut definitions = vec![function_definition(
        "echo",
        "原样返回给定的文字",
        json!({
            "type": "object",
            "properties": {
                "text": { "type": "string" }
            },
            "required": ["text"]
        }),
    )];
    definitions.extend(mcp.definitions().cloned());
    Value::Array(definitions)
}

/// 把工具名称、描述和参数 schema 组装为 Responses 接口接受的函数定义。
fn function_definition(name: &str, description: &str, parameters: impl serde::Serialize) -> Value {
    json!({
        "type": "function",
        "name": name,
        "description": description,
        "parameters": parameters,
        // 按工具原有的 schema 调用，保留 MCP 工具的可选参数。
        "strict": false
    })
}

// ******************************** 工具调度与执行 ********************************

/// 执行一次工具调用所需的名称和 JSON 参数字符串，借自模型本次回复。
pub(crate) struct ToolRequest<'a> {
    pub(crate) name: &'a str,
    pub(crate) arguments: &'a str,
}

/// 使用向模型公布工具时的快照执行整批调用。每个请求按原顺序返回一条结果，包括错误。
pub(crate) async fn call_batch(requests: &[ToolRequest<'_>], snapshot: &McpTools) -> Vec<String> {
    let mut results = Vec::with_capacity(requests.len());
    // 相邻且都允许并发的调用放在同一组，独占调用单独成组。
    for group in requests.chunk_by(|left, right| {
        supports_parallel(left.name, snapshot) && supports_parallel(right.name, snapshot)
    }) {
        let pending = group.iter().map(|request| call(request, snapshot));
        // join_all 按传入顺序返回，整组结束后才开始下一组。
        for result in join_all(pending).await {
            results.push(result.unwrap_or_else(|error| json!({ "error": error }).to_string()));
        }
    }
    results
}

/// echo 可以并发，MCP 工具沿用快照中记录的并发标记。
fn supports_parallel(name: &str, snapshot: &McpTools) -> bool {
    name == "echo" || snapshot.supports_parallel(name)
}

/// 解析这条调用的参数，再按名称分派给内置工具或快照中的 MCP 服务。
async fn call(request: &ToolRequest<'_>, snapshot: &McpTools) -> Result<String, String> {
    let arguments: Value = serde_json::from_str(request.arguments)
        .map_err(|error| format!("工具参数不是有效 JSON：{error}"))?;
    match request.name {
        "echo" => {
            let text = arguments["text"]
                .as_str()
                .ok_or("echo 需要一个字符串类型的 text 参数")?;

            Ok(text.to_string())
        }
        name => snapshot.call(name, &arguments).await,
    }
}
