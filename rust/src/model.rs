use std::error::Error;

use serde_json::{Value, json};

use crate::config::Config;

// 这里只负责和模型通信。调用者继续处理回答、工具调用和对话历史。
pub(crate) fn request(
    client: &reqwest::blocking::Client,
    config: &Config,
    messages: &[Value],
    tools: &Value,
) -> Result<Value, Box<dyn Error>> {
    let response = client
        .post(&config.api_url)
        .bearer_auth(&config.api_key)
        .json(&json!({ "model": config.model, "messages": messages, "tools": tools }))
        .send()
        .and_then(|response| response.error_for_status())
        // 请求地址可能带有敏感信息，不把它放进错误消息里。
        .map_err(|error| format!("模型请求失败：{}", error.without_url()))?
        .json()
        .map_err(|error| format!("无法解析模型响应：{error}"))?;
    Ok(response)
}
