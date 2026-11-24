mod mcp;

pub(crate) use mcp::McpClient;
pub(crate) use mcp::ServerConfig;
use serde_json::{Value, json};

pub(crate) fn definitions(mcp: &McpClient) -> Value {
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

pub(crate) fn call(name: &str, arguments: &Value, mcp: &McpClient) -> Result<String, String> {
    match name {
        "echo" => {
            let text = arguments["text"]
                .as_str()
                .ok_or("echo 需要一个字符串类型的 text 参数")?;

            Ok(text.to_string())
        }
        _ => mcp.call(name, arguments),
    }
}
