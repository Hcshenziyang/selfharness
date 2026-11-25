use serde::Deserialize;
use std::{collections::BTreeMap, error::Error, path::PathBuf};

// ******************************** 服务连接配置 ********************************

/// 网页提交的 MCP 服务配置。并发选项与传输字段位于同一层 JSON。
#[derive(Clone, Deserialize)]
pub(crate) struct ServerConfig {
    /// 显式允许整个服务的工具并发调用，缺省为 false。
    #[serde(default)]
    pub(super) supports_parallel_tool_calls: bool,
    #[serde(flatten)]
    pub(super) transport: TransportConfig,
}

/// Serde 根据 type 选择 stdio 或 http，并拒绝该传输方式无法识别的字段。
#[derive(Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum TransportConfig {
    /// 启动本地子进程。args 和 env 缺省为空，cwd 缺省时沿用当前目录。
    Stdio {
        command: String,
        #[serde(default)]
        args: Vec<String>,
        #[serde(default)]
        env: BTreeMap<String, String>,
        cwd: Option<PathBuf>,
    },
    /// 连接远端 HTTP 服务。streamable_http 是 http 的别名，headers 缺省为空。
    #[serde(alias = "streamable_http")]
    Http {
        url: String,
        #[serde(default)]
        headers: BTreeMap<String, String>,
    },
}

/// 检查服务名非空且只含 ASCII 字母、数字、下划线或连字符，以便用作工具名前缀。
pub(super) fn validate_name(name: &str) -> Result<(), Box<dyn Error + Send + Sync>> {
    if name.is_empty()
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    {
        return Err(format!(
            "MCP name {name:?} only allows alphanumeric characters, underscores, and hyphens "
        )
        .into());
    }
    Ok(())
}
