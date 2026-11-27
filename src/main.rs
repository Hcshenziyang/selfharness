mod agent;
mod agent_loop;
mod config;
mod conversation;
mod llm;
mod tools;
mod web;

use config::Config;
use conversation::ConversationStore;
use llm::ModelClient;
use std::error::Error;

// ******************************** 程序入口 ********************************

/// 读取配置，打开会话数据库，再用共享的模型客户端启动 Web 服务。
#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let config = Config::load()?;
    let store = ConversationStore::open(&config.database_path, &config.model_name)?;
    let max_turns = config.max_unfinished_turns;
    web::serve(ModelClient::new(config)?, store, max_turns).await
}
