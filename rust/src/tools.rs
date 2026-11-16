use serde_json::{Value, json};

// 把工具说明和真正执行工具的代码放在一起，方便对照查看。
pub(crate) fn definitions() -> Value {
    // 工具说明随请求发给模型，让模型知道能调用什么、参数该怎么写。
    json!([{
        "type": "function",
        "function": {
            "name": "add_numbers",
            "description": "计算两个数字的和",
            "parameters": {
                "type": "object",
                "properties": {
                    "a": { "type": "number", "description": "第一个数字" },
                    "b": { "type": "number", "description": "第二个数字" }
                },
                "required": ["a", "b"],
                "additionalProperties": false
            }
        }
    }])
}

pub(crate) fn execute_tool(name: &str, arguments: &str) -> String {
    // 模型给出的是 JSON 字符串，不是可信的函数参数；格式不对也要告诉模型。
    if name != "add_numbers" {
        return json!({ "error": format!("未知工具：{name}") }).to_string();
    }
    let Ok(arguments) = serde_json::from_str::<Value>(arguments) else {
        return json!({ "error": "工具参数不是有效的 JSON" }).to_string();
    };
    let Some(object) = arguments.as_object() else {
        return json!({ "error": "工具参数必须是对象" }).to_string();
    };
    if object.keys().any(|key| key != "a" && key != "b") {
        return json!({ "error": "工具参数只允许包含 a 和 b" }).to_string();
    }
    let (Some(a), Some(b)) = (
        object.get("a").and_then(Value::as_f64),
        object.get("b").and_then(Value::as_f64),
    ) else {
        return json!({ "error": "工具参数 a 和 b 都必须是数字" }).to_string();
    };
    match serde_json::Number::from_f64(a + b) {
        Some(sum) => json!({ "result": sum }).to_string(),
        None => json!({ "error": "计算结果超出可表示范围" }).to_string(),
    }
}
