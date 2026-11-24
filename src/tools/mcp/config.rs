use serde::Deserialize;
use std::{collections::BTreeMap, error::Error, path::PathBuf};

#[derive(Deserialize)]
pub(crate) struct ServerConfig {
    #[serde(default)]
    pub(super) supports_parallel_tool_calls: bool,
    #[serde(flatten)]
    pub(super) transport: TransportConfig,
}

// tells serde to generate code to deserialize this struct from a format like JSON or TOML
#[derive(Deserialize)]
// tag means that use the "type" field in the input to determine which variant to use
// snake_case, example: streamable_http" instead of "StreamableHttp"
// deny_unknown_fields means that if there are any fields in the input that are not in the struct, it will return an error
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum TransportConfig {
    Stdio {
        command: String,
        #[serde(default)]
        args: Vec<String>,
        #[serde(default)]
        env: BTreeMap<String, String>,
        cwd: Option<PathBuf>,
    },
    #[serde(alias = "streamable_http")]
    Http {
        url: String,
        #[serde(default)]
        headers: BTreeMap<String, String>,
    },
}

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
