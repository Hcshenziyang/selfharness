use crate::{
    conversation::{Conversation, assistant_text},
    llm::ModelClient,
    tools,
};
use serde::Deserialize;
use std::{collections::HashSet, error::Error};

// ******************************** 工具调用请求 ********************************

/// 模型返回的一条工具调用。`arguments` 是 JSON 字符串，`call_id` 用来匹配执行结果。
#[derive(Deserialize)]
struct ToolCall {
    call_id: String,
    name: String,
    arguments: String,
    status: Option<String>,
}

// ******************************** 单次用户回合 ********************************

/// 从一条用户输入开始，反复请求模型并执行工具，直到模型给出最终回复。
/// 返回的会话包含本回合的全部记录，由调用方保存。
pub(crate) async fn run_turn(
    client: &ModelClient,
    mut conversation: Conversation,
    user_input: &str,
    mcp: &tools::McpClient,
) -> Result<Conversation, Box<dyn Error>> {
    conversation.add_user(user_input);
    loop {
        // 拿到模型名额后才取工具快照。这次响应里的工具调用都使用这批连接，下一次请求再取。
        let request = client.request().await?;
        let snapshot = mcp.tools().await;
        let output = request
            .ask_llm(conversation.messages(), &tools::definitions(&snapshot))
            .await?;
        // ask_llm 返回时已释放名额，后续等待工具期间其他会话也能请求模型。
        let calls = output
            .iter()
            .filter(|item| item["type"] == "function_call")
            .map(ToolCall::deserialize)
            .collect::<Result<Vec<_>, _>>()?;
        if calls.is_empty() && output.iter().find_map(assistant_text).is_none() {
            return Err("模型没有返回正文或工具调用".into());
        }

        // 整批校验通过才交给工具层。调用 ID 留在这里，用来匹配写入会话的结果。
        let mut ids = HashSet::new();
        let mut requests = Vec::with_capacity(calls.len());
        for call in &calls {
            if call.call_id.trim().is_empty()
                || call.name.trim().is_empty()
                || !ids.insert(&call.call_id)
                || call
                    .status
                    .as_deref()
                    .is_some_and(|status| status != "completed")
            {
                return Err("模型返回了无效的工具调用".into());
            }
            requests.push(tools::ToolRequest {
                name: &call.name,
                arguments: &call.arguments,
            });
        }

        conversation.add_output(output);
        if calls.is_empty() {
            return Ok(conversation);
        }
        let results = tools::call_batch(&requests, &snapshot).await;
        for (call, content) in calls.iter().zip(results) {
            conversation.add_tool_result(&call.call_id, content);
        }
    }
}
