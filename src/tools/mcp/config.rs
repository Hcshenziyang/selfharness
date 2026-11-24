use serde::Deserialize;
use std::{collections::BTreeMap, error::Error, path::PathBuf};

// tells serde to generate code to deserialize this struct from a format like JSON or TOML
#[derive(Deserialize)]
// tag means that use the "type" field in the input to determine which variant to use
// snake_case, example: streamable_http" instead of "StreamableHttp"
// deny_unknown_fields means that if there are any fields in the input that are not in the struct, it will return an error
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum ServerConfig {
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

pub(super) fn validate_name(name: &str) -> Result<(), Box<dyn Error>> {
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
