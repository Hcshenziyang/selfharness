mod tools;
use std::{
    env,
    error::Error,
    time::Duration,
    io::{self, Write},
};
use reqwest::blocking::Client;


// ***************************************变量载入***************************************
pub struct Config {
    pub model_url: String,
    pub api_key: String,
    pub model_name: String,
}
impl Config {
    pub fn load() -> Result<Self, Box<dyn Error>> {
        Ok(Self {
            // env::var only return string, if we need to other type， parse it
            // ? means if the parse fail, return the error
            // if no ?, the result is Result<Url, ParseError>, and we need to handle it ourselves
            // if we do not handle it, the compiler will complain that we are not using the result
            model_url: env::var("LLM_API_URL").map_err(|_| "请设置 LLM_API_URL")?,
            api_key: env::var("LLM_API_KEY").map_err(|_| "请设置 LLM_API_KEY")?,
            model_name: env::var("LLM_MODEL").map_err(|_| "请设置 LLM_MODEL")?,
        })
    }
}

// ***************************************对话列表***************************************
// 保留完整消息，包括工具调用和模型的 reasoning_content。
type Message = serde_json::Value;

// ***************************************请求 LLM***************************************
fn ask_llm(
    client: &Client,
    config: &Config,
    messages: &Vec<Message>,
) -> Result<serde_json::Value, Box<dyn Error>> {
    let body = serde_json::json!({
        "model": &config.model_name,
        "messages": &messages,
        "stream": false,
        "tools": tools::definitions()
    });

    let response: serde_json::Value = client
        .post(&config.model_url)
        .bearer_auth(&config.api_key)
        .json(&body)
        .send()?
        .error_for_status()?
        .json()?;

    let reply = response["choices"][0]["message"]
        .as_object()
        .ok_or("无法解析 DeepSeek API 响应")?;

    Ok(serde_json::Value::Object(reply.clone()))
}


// 工具结果写入历史后再请求模型，每次用户输入最多请求 5 次。
fn run_turn(
    client: &Client,
    config: &Config,
    messages: &mut Vec<Message>,
) -> Result<(), Box<dyn Error>> {
    for _ in 0..5 {
        let reply = ask_llm(client, config, messages)?;
        let calls = reply["tool_calls"].as_array().filter(|calls| !calls.is_empty());
        let Some(calls) = calls else {
            let content = reply["content"]
                .as_str()
                .ok_or("模型没有返回文字或工具调用")?;
            println!("DeepSeek 回复: {content}");
            messages.push(reply);
            return Ok(());
        };

        // 先验证整批调用，避免留下无法配对的工具请求。
        let mut requests = Vec::new();
        let mut ids = std::collections::HashSet::new();
        for call in calls {
            let id = call["id"].as_str().filter(|id| !id.is_empty()).ok_or("工具调用缺少 id")?;
            if !ids.insert(id) {
                return Err("工具调用 id 重复".into());
            }
            if call["type"].as_str() != Some("function") {
                return Err("不支持的工具调用类型".into());
            }
            let name = call["function"]["name"].as_str().ok_or("工具调用缺少名称")?;
            let arguments = call["function"]["arguments"].as_str().ok_or("工具调用缺少参数")?;
            requests.push((id.to_string(), name.to_string(), arguments.to_string()));
        }
        messages.push(reply);

        for (id, name, arguments) in requests {
            // API 中 arguments 是 JSON 字符串，需要先解析。
            let result = serde_json::from_str(&arguments)
                .map_err(|e| format!("工具参数不是有效 JSON: {e}"))
                .and_then(|arguments| tools::call(&name, &arguments));
            let content = match result {
                Ok(output) => output,
                Err(error) => serde_json::json!({"error": error}).to_string(),
            };
            println!("工具 {name}: {content}");
            messages.push(serde_json::json!({
                "role": "tool",
                "tool_call_id": id,
                "content": content
            }));
        }
    }
    Err("已达到本轮请求次数上限（5 次）".into())
}

// ***************************************主函数***************************************
fn main() -> Result<(), Box<dyn Error>> {
    let config = Config::load()?;

    let client = Client::builder()
        .timeout(Duration::from_secs(60))
        .build()?;
    // 构造消息列表
    let mut messages: Vec<Message> = Vec::new();

    loop {
        println!("请输入你的问题（输入 '/exit' 退出）：");   // print pintln?
        io::stdout().flush()?; // 确保提示信息立即显示
        let mut user_input = String::new();
        if io::stdin().read_line(&mut user_input)? == 0 {
            break; // 用户输入 EOF，退出循环
        }

        let user_input = user_input.trim();
        if user_input == "/exit" {
            break; // 用户输入 '/exit'，退出循环
        }
        if user_input.is_empty() {
            continue; // 用户输入为空，继续循环
        }

        messages.push(serde_json::json!({
            "role": "user",
            "content": user_input
        }));

        if let Err(e) = run_turn(&client, &config, &mut messages) {
            eprintln!("请求 DeepSeek API 失败: {e}");
        }
    }
    Ok(())
}
