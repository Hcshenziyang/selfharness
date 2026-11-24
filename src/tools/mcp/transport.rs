use super::config::ServerConfig;
use reqwest::header::{HeaderName, HeaderValue};
use rmcp::{
    RoleClient, ServiceExt,
    service::RunningService,
    transport::{
        TokioChildProcess,
        streamable_http_client::{
            StreamableHttpClientTransport, StreamableHttpClientTransportConfig,
        },
    },
};
use std::{collections::HashMap, env, error::Error};
use tokio::process::Command;

pub(super) async fn connect(
    config: ServerConfig,
) -> Result<RunningService<RoleClient, ()>, Box<dyn Error>> {
    match config {
        ServerConfig::Stdio {
            command,
            args,
            env: extra_env,
            cwd,
        } => {
            if command.trim().is_empty() {
                return Err("stdio 服务缺少启动命令".into());
            }
            let mut child = Command::new(command);
            // 不自动把模型 API Key 传给子进程；服务需要的环境变量显式配置。
            child.args(args).env_clear().kill_on_drop(true);
            for key in ["PATH", "HOME", "TMPDIR"] {
                if let Some(value) = env::var_os(key) {
                    child.env(key, value);
                }
            }
            child.envs(extra_env);
            if let Some(cwd) = cwd {
                child.current_dir(cwd);
            }
            Ok(().serve(TokioChildProcess::new(child)?).await?)
        }
        ServerConfig::Http { url, headers } => {
            let parsed = reqwest::Url::parse(&url).map_err(|_| "MCP HTTP 地址无效")?;
            if !matches!(parsed.scheme(), "http" | "https") {
                return Err("MCP HTTP 地址必须以 http 或 https 开头".into());
            }
            let mut values = HashMap::new();
            for (name, value) in headers {
                add_header(&mut values, &name, &value)?;
            }
            let config = StreamableHttpClientTransportConfig::with_uri(url).custom_headers(values);
            // 不自动跟随重定向，避免自定义认证头被转发到其他地址。
            let client = reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .build()?;
            let transport = StreamableHttpClientTransport::with_client(client, config);
            Ok(().serve(transport).await?)
        }
    }
}

fn add_header(
    headers: &mut HashMap<HeaderName, HeaderValue>,
    name: &str,
    value: &str,
) -> Result<(), Box<dyn Error>> {
    let name = HeaderName::from_bytes(name.as_bytes()).map_err(|_| "MCP HTTP 请求头名称无效")?;
    let mut value =
        HeaderValue::from_str(value).map_err(|_| format!("MCP 请求头 {name} 的值无效"))?;
    value.set_sensitive(true);
    if headers.insert(name.clone(), value).is_some() {
        return Err(format!("MCP 请求头 {name} 配置重复").into());
    }
    Ok(())
}
