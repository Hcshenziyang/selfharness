use crate::{config::Config, conversation::Message};
use reqwest::Client;
use serde::Deserialize;
use serde_json::Value;
use std::error::Error;

#[derive(Deserialize)]
struct ModelResponse {
    status: Option<String>,
    error: Option<Value>,
    incomplete_details: Option<Value>,
    #[serde(default)]
    output: Vec<Message>,
}

// ***************************************请求 LLM***************************************
pub(crate) async fn ask_llm(
    client: &Client,
    config: &Config,
    input: &[Message],
    tool_definitions: &Value,
) -> Result<Vec<Message>, Box<dyn Error>> {
    let body = serde_json::json!({
        "model": &config.model_name,
        "input": input,
        "stream": false,
        // 客户端每次回传完整历史，由本地 SQLite 保存会话。
        "store": false,
        "tools": tool_definitions,
        "parallel_tool_calls": true
    });

    let response: ModelResponse = client
        .post(&config.model_url)
        .bearer_auth(&config.api_key)
        .json(&body)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;

    if let Some(error) = response.error {
        return Err(format!(
            "模型请求失败：{}",
            error["message"].as_str().unwrap_or("未知错误")
        )
        .into());
    }
    if let Some(status) = response.status
        && status != "completed"
    {
        let reason = response
            .incomplete_details
            .as_ref()
            .and_then(|details| details["reason"].as_str());
        return Err(format!("模型响应未完成：{}", reason.unwrap_or(&status)).into());
    }
    Ok(response.output)
}
