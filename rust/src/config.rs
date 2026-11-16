use std::{env, error::Error, io};

// 配置在程序启动时读取一次，其他模块只使用读取后的结果。
pub(crate) struct Config {
    pub(crate) api_url: String,
    pub(crate) api_key: String,
    pub(crate) model: String,
}

pub(crate) fn load() -> Result<Config, Box<dyn Error>> {
    // 自动读取当前目录或父目录中的 .env；已有环境变量优先。
    if let Err(error) = dotenvy::dotenv()
        && !error.not_found()
    {
        return Err("无法读取或解析 .env，请检查文件格式和权限".into());
    }
    Ok(Config {
        api_url: required_env("LLM_API_URL")?,
        api_key: required_env("LLM_API_KEY")?,
        model: required_env("LLM_MODEL")?,
    })
}

// 命令行参数可作为第一个问题；没有内容时留给 main 从终端读取。
pub(crate) fn initial_prompt() -> Option<String> {
    let prompt = env::args().skip(1).collect::<Vec<_>>().join(" ");
    (!prompt.trim().is_empty()).then_some(prompt)
}

fn required_env(name: &str) -> Result<String, io::Error> {
    // 只检查变量是否存在且非空；不会在错误中输出变量值。
    match env::var(name) {
        Ok(value) if !value.trim().is_empty() => Ok(value),
        _ => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("请设置环境变量 {name}"),
        )),
    }
}
