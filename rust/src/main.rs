mod config;
mod model;
mod tools;

use std::{error::Error, io, io::Write};

use serde_json::{Value, json};

// 每个用户问题都有独立的上限，避免模型反复调用工具时一直运行。
const MAX_MODEL_REQUESTS: usize = 8;

fn main() -> Result<(), Box<dyn Error>> {
    let config = config::load()?;
    let tools = tools::definitions();
    // 历史暂时只放在内存中，程序退出后就会消失。
    let mut messages: Vec<Value> = Vec::new();
    let client = reqwest::blocking::Client::new();
    let mut next_prompt = config::initial_prompt();

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
            let response = model::request(&client, &config, &messages, &tools)?;
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
                    (Some(name), Some(arguments)) => tools::execute_tool(name, arguments),
                    _ => json!({ "error": "工具名称或参数缺失" }).to_string(),
                };
                messages.push(json!({ "role": "tool", "tool_call_id": id, "content": result }));
            }
            // 下一次请求带上完整历史，让模型看见工具结果。
        }
    }
    Ok(())
}
