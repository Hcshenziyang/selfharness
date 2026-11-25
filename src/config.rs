use std::{env, error::Error, path::PathBuf};

// ******************************** 启动配置 ********************************

/// 启动时从环境变量读取的模型连接、数据库位置和并发上限。
pub struct Config {
    pub model_url: String,
    pub api_key: String,
    pub model_name: String,
    pub database_path: PathBuf,
    /// 全部会话同时向上游模型发送请求的名额。
    pub max_model_concurrency: usize,
    /// 全部会话中，正在执行和排队等待的用户回合总数上限。
    pub max_unfinished_turns: usize,
}

impl Config {
    /// 读取必需的模型配置，并为数据库路径和两个上限提供默认值。
    pub fn load() -> Result<Self, Box<dyn Error>> {
        Ok(Self {
            // env::var 读出字符串。读取失败时，? 把 map_err 给出的错误返回给启动入口。
            model_url: env::var("LLM_API_URL").map_err(|_| "请设置 LLM_API_URL")?,
            api_key: env::var("LLM_API_KEY").map_err(|_| "请设置 LLM_API_KEY")?,
            model_name: env::var("LLM_MODEL").map_err(|_| "请设置 LLM_MODEL")?,
            database_path: env::var_os("DOGO_DB_PATH")
                .filter(|path| !path.is_empty())
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("dogoharness.sqlite3")),
            max_model_concurrency: positive_limit("DOGO_MAX_MODEL_CONCURRENCY", 2)?,
            max_unfinished_turns: positive_limit("DOGO_MAX_UNFINISHED_TURNS", 16)?,
        })
    }
}

/// 变量缺失时使用默认值，显式设置的值必须是 Tokio 信号量支持的正整数。
fn positive_limit(name: &str, default: usize) -> Result<usize, Box<dyn Error>> {
    let value = match env::var(name) {
        Ok(value) => value,
        Err(env::VarError::NotPresent) => return Ok(default),
        Err(error) => return Err(format!("{name}: {error}").into()),
    };
    let max = tokio::sync::Semaphore::MAX_PERMITS;
    value
        .parse::<usize>()
        .ok()
        .filter(|n| (1..=max).contains(n))
        .ok_or_else(|| format!("{name} 必须是 1 到 {max} 之间的整数").into())
}
