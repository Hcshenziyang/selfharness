use serde::{Deserialize, Serialize};

/// 首次创建的助手接收原有会话，也供未指定助手的客户端使用。
pub(crate) const DEFAULT_AGENT_ID: i64 = 1;
pub(crate) const MEMORY_LIMIT: usize = 4000;

#[derive(Serialize)]
pub(crate) struct AgentSummary {
    pub id: i64,
    pub name: String,
    pub model: String,
}

/// 一轮对话开始时从数据库读取，供这一轮的每次模型请求使用。
#[derive(Deserialize, Serialize)]
pub(crate) struct AgentSettings {
    pub name: String,
    pub model: String,
    #[serde(default)]
    pub instructions: String,
    #[serde(default)]
    pub memory: String,
}

impl AgentSettings {
    pub fn validate(mut self) -> Result<Self, &'static str> {
        self.name = self.name.trim().to_owned();
        self.model = self.model.trim().to_owned();
        if self.name.is_empty() || self.model.is_empty() {
            return Err("请填写助手名称和模型 ID");
        }
        if self.memory.chars().count() > MEMORY_LIMIT {
            return Err("私有记忆最多可保存 4000 个字符");
        }
        Ok(self)
    }

    pub fn model_instructions(&self) -> String {
        let instructions = self.instructions.trim();
        let memory = self.memory.trim();
        if memory.is_empty() {
            return instructions.to_owned();
        }
        let prefix = if instructions.is_empty() { "" } else { "\n\n" };
        format!("{instructions}{prefix}助手私有笔记（供参考）\n{memory}")
    }
}

#[derive(Serialize)]
pub(crate) struct Agent {
    pub id: i64,
    pub revision: i64,
    #[serde(flatten)]
    pub settings: AgentSettings,
}
