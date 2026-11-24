use crate::{
    config::Config,
    conversation::{Conversation, assistant_text},
    llm::ask_llm,
    tools,
};
use futures_util::future::join_all;
use reqwest::Client;
use serde::Deserialize;
use serde_json::json;
use std::{collections::HashSet, error::Error};

#[derive(Deserialize)]
struct ToolCall {
    call_id: String,
    name: String,
    arguments: String,
    status: Option<String>,
}

impl ToolCall {
    async fn execute(&self, mcp: &tools::McpTools) -> String {
        let result = match serde_json::from_str(&self.arguments) {
            Ok(arguments) => tools::call(&self.name, &arguments, mcp).await,
            Err(error) => Err(format!("工具参数不是有效 JSON：{error}")),
        };
        result.unwrap_or_else(|error| json!({ "error": error }).to_string())
    }
}

pub(crate) async fn run_turn(
    client: &Client,
    config: &Config,
    mut conversation: Conversation,
    user_input: &str,
    mcp: &tools::McpTools,
) -> Result<Conversation, Box<dyn Error>> {
    conversation.add_user(user_input);
    let tool_definitions = tools::definitions(mcp);
    loop {
        let output = ask_llm(client, config, conversation.messages(), &tool_definitions).await?;
        let calls = output
            .iter()
            .filter(|item| item["type"] == "function_call")
            .map(ToolCall::deserialize)
            .collect::<Result<Vec<_>, _>>()?;
        if calls.is_empty() && output.iter().find_map(assistant_text).is_none() {
            return Err("模型没有返回正文或工具调用".into());
        }

        // 先检查整批调用，保证每个工具结果都有对应的调用 ID。
        let mut ids = HashSet::new();
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
        }

        conversation.add_output(output);
        if calls.is_empty() {
            return Ok(conversation);
        }
        // 相邻的可并发调用一起跑。每个独占调用单独成组，等前一组结束才开始。
        for group in calls.chunk_by(|left, right| {
            tools::supports_parallel(&left.name, mcp) && tools::supports_parallel(&right.name, mcp)
        }) {
            let results = join_all(group.iter().map(|call| call.execute(mcp))).await;
            for (call, content) in group.iter().zip(results) {
                conversation.add_tool_result(&call.call_id, content);
            }
        }
    }
}
