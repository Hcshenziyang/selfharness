mod agent_loop;
mod config;
mod conversation;
mod llm;
mod tools;
mod web;

use config::Config;
use conversation::ConversationStore;
use reqwest::Client;
use std::{error::Error, time::Duration};
// ***************************************主函数***************************************
#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let config = Config::load()?;
    let store = ConversationStore::open(&config.database_path)?;
    let client = Client::builder().timeout(Duration::from_secs(60)).build()?;
    web::serve(config, client, store).await
}
