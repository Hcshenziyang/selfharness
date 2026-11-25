use crate::{config::Config, conversation::Message};
use reqwest::Client;
use serde::Deserialize;
use serde_json::Value;
use std::{error::Error, time::Duration};
use tokio::sync::{AcquireError, Semaphore, SemaphorePermit};

// ******************************** 模型客户端 ********************************

/// 所有会话共用的模型客户端，负责复用 HTTP 连接并限制同时进行的模型请求数。
pub(crate) struct ModelClient {
    http: Client,
    url: String,
    api_key: String,
    model: String,
    slots: Semaphore,
}

impl ModelClient {
    /// 创建一个新的 ModelClient 实例，使用给定的配置。
    pub(crate) fn new(config: Config) -> Result<Self, reqwest::Error> {
        Ok(Self {
            http: Client::builder().timeout(Duration::from_secs(60)).build()?,
            url: config.model_url,
            api_key: config.api_key,
            model: config.model_name,
            slots: Semaphore::new(config.max_model_concurrency),
        })
    }

    /// 获取一个请求对象，使用完后会自动释放名额。
    /// 名额用满时在这里等待，拿到名额后由 [`ModelRequest::ask_llm`] 发送请求。
    pub(crate) async fn request(&self) -> Result<ModelRequest<'_>, AcquireError> {
        Ok(ModelRequest {
            client: self,
            _permit: self.slots.acquire().await?,
        })
    }
}

// ********************************* 模型请求 *********************************

/// 已取得一个并发名额的请求对象，借用共享客户端来完成一次模型调用。
pub(crate) struct ModelRequest<'a> {
    client: &'a ModelClient,
    // 请求对象持有这份许可，直到对象释放时由 Tokio 归还名额。
    _permit: SemaphorePermit<'a>,
}

impl ModelRequest<'_> {
    /// 将对话历史和本次可用的工具定义发给模型，返回输出项供 Agent Loop 处理。
    /// 一次请求消耗一个名额，读取完响应或发生错误后即释放。
    pub(crate) async fn ask_llm(
        self,
        input: &[Message],
        tool_definitions: &Value,
    ) -> Result<Vec<Message>, Box<dyn Error>> {
        let body = serde_json::json!({
            "model": &self.client.model,
            "input": input,
            "stream": false,
            // 客户端每次回传完整历史，由本地 SQLite 保存会话。
            "store": false,
            "tools": tool_definitions,
            "parallel_tool_calls": true
        });

        let response: ModelResponse = self
            .client
            .http
            .post(&self.client.url)
            .bearer_auth(&self.client.api_key)
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
}

// ********************************* 模型响应 *********************************

/// 从 Responses API 的 JSON 回包中读取生成状态、错误和模型输出。
#[derive(Deserialize)]
struct ModelResponse {
    status: Option<String>,
    error: Option<Value>,
    /// 生成未完成时的详细原因，例如达到输出长度上限。
    incomplete_details: Option<Value>,
    /// 助手消息、工具调用等输出项。回包省略此字段时按空列表处理。
    #[serde(default)]
    output: Vec<Message>,
}
