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
#[derive(serde::Serialize)]
struct Message {
    role: &'static str,
    content: String,
}

// ***************************************请求 LLM***************************************
fn ask_llm(
    client: &Client,
    config: &Config,
    messages: &Vec<Message>,
) -> Result<String, Box<dyn Error>> {
    let body = serde_json::json!({
        "model": &config.model_name,
        "messages": &messages,
        "stream": false
    });

    let response: serde_json::Value = client
        .post(&config.model_url)
        .bearer_auth(&config.api_key)
        .json(&body)
        .send()?
        .error_for_status()?
        .json()?;

    let reply = response["choices"][0]["message"]["content"]
        .as_str()
        .ok_or("无法解析 DeepSeek API 响应")?
        .to_string();

    Ok(reply.to_string())
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

        messages.push(Message {
            role: "user",
            content: user_input.to_string(),
        });

        match ask_llm(&client, &config, &messages) {
            Ok(reply) => {
                println!("DeepSeek 回复: {}", reply);
                messages.push(Message {
                    role: "assistant",
                    content: reply,
                });
            }
            Err(e) => {
                eprintln!("请求 DeepSeek API 失败: {}", e);
            }
        }
    }
    Ok(())
}
