use super::config::TransportConfig;
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

// ******************************** 建立 MCP 连接 ********************************

/// 按配置启动 stdio 子进程或连接 HTTP 服务，完成 MCP 握手并返回运行中的连接。
pub(super) async fn connect(
    config: TransportConfig,
) -> Result<RunningService<RoleClient, ()>, Box<dyn Error + Send + Sync>> {
    match config {
        TransportConfig::Stdio {
            command,
            args,
            env: extra_env,
            cwd,
        } => {
            if command.trim().is_empty() {
                return Err("stdio 服务缺少启动命令".into());
            }
            let mut child = Command::new(command);
            // 子进程只继承下面三个基础变量，服务凭据通过 extra_env 显式传入。
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
        TransportConfig::Http { url, headers } => {
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

/// 校验自定义 HTTP 请求头，按大小写无关的名称检查重复，并把值标记为敏感信息。
fn add_header(
    headers: &mut HashMap<HeaderName, HeaderValue>,
    name: &str,
    value: &str,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    let name = HeaderName::from_bytes(name.as_bytes()).map_err(|_| "MCP HTTP 请求头名称无效")?;
    let mut value =
        HeaderValue::from_str(value).map_err(|_| format!("MCP 请求头 {name} 的值无效"))?;
    value.set_sensitive(true);
    if headers.insert(name.clone(), value).is_some() {
        return Err(format!("MCP 请求头 {name} 配置重复").into());
    }
    Ok(())
}
