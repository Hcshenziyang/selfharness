use serde_json::Value;

// 提供给模型的工具清单，与 call 中的名称和参数保持一致。
pub fn definitions() -> Value {
    serde_json::json!([{
        "type": "function",
        "function": {
            "name": "echo",
            "description": "原样返回传入的文字",
            "parameters": {
                "type": "object",
                "properties": {
                    "text": {"type": "string", "description": "要返回的文字"}
                },
                "required": ["text"],
                "additionalProperties": false
            }
        }
    }])
}

pub fn call(name: &str, arguments: &Value) -> Result<String, String> {
    match name {
        "echo" => {
            let text = arguments["text"]
                .as_str()
                .ok_or("echo 的 text 参数必须是字符串")?;

            Ok(echo(text))
        }
        _ => Err(format!("未知工具函数: {name}")),
    }
}

// 示例工具：原样返回传入的文字。
fn echo(text: &str) -> String {
    text.to_string()
}
