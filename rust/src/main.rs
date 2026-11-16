use std::{env, error::Error, io, io::Write};

use serde_json::{Value, json};

fn main() -> Result<(), Box<dyn Error>> {
    // 第一步：自动读取当前目录或父目录中的 .env；已有环境变量优先。
    if let Err(error) = dotenvy::dotenv()
        && !error.not_found()
    {
        return Err("无法读取或解析 .env，请检查文件格式和权限".into());
    }
    let api_url = required_env("LLM_API_URL")?;
    let api_key = required_env("LLM_API_KEY")?;
    let model = required_env("LLM_MODEL")?;

    // 对话历史暂时只放在内存中，程序退出后就会消失。
    let mut messages: Vec<Value> = Vec::new();
    let client = reqwest::blocking::Client::new();

    // 命令行参数可以作为第一个问题；没有参数时，等待用户输入。
    let initial_prompt = env::args().skip(1).collect::<Vec<_>>().join(" ");
    let mut next_prompt = (!initial_prompt.trim().is_empty()).then_some(initial_prompt);

    loop {
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

        // 把新问题接在历史消息后面，再把整个列表一起交给模型。
        messages.push(json!({ "role": "user", "content": prompt }));
        let response: Value = client
            .post(&api_url)
            .bearer_auth(&api_key)
            .json(&json!({ "model": model, "messages": messages }))
            .send()
            .and_then(|response| response.error_for_status())
            .map_err(|error| format!("模型请求失败：{}", error.without_url()))?
            .json()
            .map_err(|error| format!("无法解析模型响应：{error}"))?;

        let answer = response["choices"][0]["message"]["content"]
            .as_str()
            .ok_or("模型响应中没有找到文本回答")?;
        println!("助手：{answer}");

        // 别忘了保存助手的回答：下一次请求需要知道双方之前说过什么。
        messages.push(json!({ "role": "assistant", "content": answer }));
    }

    Ok(())
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
