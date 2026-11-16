"""读取配置和命令行首问。"""

import os
import sys

from dotenv import find_dotenv, load_dotenv


def load():
    # 只在启动时读取；已有环境变量优先于 .env。
    env_file = find_dotenv(usecwd=True)
    if env_file:
        load_dotenv(env_file)
    return {
        "api_url": required_env("LLM_API_URL"),
        "api_key": required_env("LLM_API_KEY"),
        "model": required_env("LLM_MODEL"),
    }


def initial_prompt():
    return " ".join(sys.argv[1:]).strip() or None


def required_env(name):
    value = os.getenv(name, "")
    if not value.strip():
        raise ValueError(f"请设置环境变量 {name}")
    return value
