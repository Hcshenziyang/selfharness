use std::{env, error::Error, io, io::Write};

use serde_json::{Value, json};

// 每个用户问题都有独立的上限，避免模型反复调用工具时一直运行。
const MAX_MODEL_REQUESTS: usize = 8;

fn main() -> Result<(), Box<dyn Error>> {
    // 自动读取当前目录或父目录中的 .env；已有环境变量优先。
    if let Err(error) = dotenvy::dotenv()
        && !error.not_found()
    {
        return Err("无法读取或解析 .env，请检查文件格式和权限".into());
    }
    let api_url = required_env("LLM_API_URL")?;
    let api_key = required_env("LLM_API_KEY")?;
    let model = required_env("LLM_MODEL")?;

    // 工具说明随请求发给模型，让模型知道能调用什么、参数该怎么写。
    let tools = json!([{
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
    }]);
    // 历史暂时只放在内存中，程序退出后就会消失。
    let mut messages: Vec<Value> = Vec::new();
    let client = reqwest::blocking::Client::new();
    let initial_prompt = env::args().skip(1).collect::<Vec<_>>().join(" ");
    let mut next_prompt = (!initial_prompt.trim().is_empty()).then_some(initial_prompt);

    loop {
        // 命令行参数作为第一个问题；之后继续等待输入。
        let prompt = if let Some(prompt) = next_prompt.take() {
            prompt
        } else {
            print!("你：");
            io::stdout().flush()?;
            let mut input = String::new();
            // EOF（例如终端中的 Ctrl+D）也是正常的退出方式。
            if io::stdin().read_line(&mut input)? == 0 {
                break;
            }
            input.trim().to_owned()
        };
        if prompt.trim().is_empty() {
            continue;
        }
        if matches!(prompt.trim(), "exit" | "quit") {
            break;
        }
        messages.push(json!({ "role": "user", "content": prompt }));

        for request_index in 0..MAX_MODEL_REQUESTS {
            // 一次提问可能需要多次请求：模型先调用工具，再根据结果回答。
            let response: Value = client
                .post(&api_url)
                .bearer_auth(&api_key)
                .json(&json!({ "model": model, "messages": messages, "tools": tools }))
                .send()
                .and_then(|response| response.error_for_status())
                .map_err(|error| format!("模型请求失败：{}", error.without_url()))?
                .json()
                .map_err(|error| format!("无法解析模型响应：{error}"))?;
            let choice = &response["choices"][0];
            let message = choice["message"]
                .as_object()
                .ok_or("模型响应中没有找到消息")?;
            if choice["finish_reason"].as_str() == Some("length") {
                return Err("模型响应因长度限制被截断，请重试".into());
            }
            let calls = match message.get("tool_calls") {
                None | Some(Value::Null) => Vec::new(),
                Some(Value::Array(calls)) => calls.clone(),
                Some(_) => return Err("模型返回的工具调用格式无效".into()),
            };
            if calls.is_empty() {
                let answer = message
                    .get("content")
                    .and_then(Value::as_str)
                    .ok_or("模型响应中没有找到文本回答")?;
                println!("助手：{answer}");
                // 下一轮需要完整的对话历史，包括这次的最终回答。
                messages.push(Value::Object(message.clone()));
                break;
            }
            if request_index + 1 == MAX_MODEL_REQUESTS {
                return Err("模型请求次数已达到上限，无法继续工具调用".into());
            }

            // 先保存模型的调用请求，再将每个工具结果通过 id 与请求对应起来。
            // 一条回复可能要求调用多个工具，不能只处理第一个。
            messages.push(Value::Object(message.clone()));
            for call in calls {
                let id = call["id"]
                    .as_str()
                    .filter(|id| !id.is_empty())
                    .ok_or("模型返回的工具调用缺少 id，无法关联结果")?;
                let function = &call["function"];
                let result = match (function["name"].as_str(), function["arguments"].as_str()) {
                    (Some(name), Some(arguments)) => execute_tool(name, arguments),
                    _ => json!({ "error": "工具名称或参数缺失" }).to_string(),
                };
                messages.push(json!({ "role": "tool", "tool_call_id": id, "content": result }));
            }
            // 下一次请求带上完整历史，让模型看见工具结果。
        }
    }
    Ok(())
}

fn execute_tool(name: &str, arguments: &str) -> String {
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

fn required_env(name: &str) -> Result<String, io::Error> {
    // 只检查变量是否存在且非空；不会在错误中输出变量值。
    match env::var(name) {
        Ok(value) if !value.trim().is_empty() => Ok(value),
        _ => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("请设置环境变量 {name}"),
        )),
    }
}
