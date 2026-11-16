use std::{env, error::Error, io};

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

    // 第二步：收集命令行里的问题。
    let prompt = env::args().skip(1).collect::<Vec<_>>().join(" ");
    let prompt = if prompt.is_empty() {
        "你好，请介绍一下你自己。"
    } else {
        &prompt
    };

    // 第三步：向兼容 Chat Completions 的模型服务发送一次同步请求。
    let response: Value = reqwest::blocking::Client::new()
        .post(api_url)
        .bearer_auth(api_key)
        .json(&json!({
            "model": model,
            "messages": [{ "role": "user", "content": prompt }]
        }))
        .send()?
        .error_for_status()?
        .json()?;

    // 第四步：读取第一条回答的文本。
    let answer = response["choices"][0]["message"]["content"]
        .as_str()
        .ok_or("模型响应中没有找到文本回答")?;
    println!("{answer}");

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
