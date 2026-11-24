use std::{env, error::Error, path::PathBuf};

// ***************************************变量载入***************************************
pub struct Config {
    pub model_url: String,
    pub api_key: String,
    pub model_name: String,
    pub database_path: PathBuf,
}

impl Config {
    pub fn load() -> Result<Self, Box<dyn Error>> {
        Ok(Self {
            // env::var only return string, if we need to other type， parse it
            // ? means if the parse fail, return the error
            // if no ?, the result is Result<Url, ParseError>, and we need to handle it ourselves
            // if we do not handle it, the compiler will complain that we are not using the result
            model_url: env::var("LLM_API_URL").map_err(|_| "请设置 LLM_API_URL")?,
            api_key: env::var("LLM_API_KEY").map_err(|_| "请设置 LLM_API_KEY")?,
            model_name: env::var("LLM_MODEL").map_err(|_| "请设置 LLM_MODEL")?,
            database_path: env::var_os("DOGO_DB_PATH")
                .filter(|path| !path.is_empty())
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("dogoharness.sqlite3")),
        })
    }
}
