"""第 1 节：一次大模型对话。"""

import os
import sys

import requests
from dotenv import find_dotenv, load_dotenv


def main():
    # 在当前目录和父目录寻找 .env；已经设置的环境变量优先。
    env_file = find_dotenv(usecwd=True)
    if env_file:
        load_dotenv(env_file)

    api_url = required_env("LLM_API_URL")
    api_key = required_env("LLM_API_KEY")
    model = required_env("LLM_MODEL")

    # 命令行里的文字组成一个问题；未提供时使用默认问题。
    prompt = " ".join(sys.argv[1:]) or "你好，请介绍一下你自己。"

    # 模型服务接收消息列表，接口地址需要包含完整路径。
    try:
        response = requests.post(
            api_url,
            headers={"Authorization": f"Bearer {api_key}"},
            json={"model": model, "messages": [{"role": "user", "content": prompt}]},
            timeout=60,
        )
        response.raise_for_status()
    except requests.RequestException as error:
        # 不打印 URL 或请求头，避免把配置中的敏感内容写到终端。
        raise RuntimeError("模型请求失败，请检查接口配置和网络") from None

    # 取出第一条回答。这一节只请求一次，也不保存对话历史。
    try:
        answer = response.json()["choices"][0]["message"]["content"]
    except (ValueError, KeyError, IndexError, TypeError):
        raise RuntimeError("模型响应中没有找到文本回答") from None
    if not isinstance(answer, str):
        raise RuntimeError("模型响应中没有找到文本回答")
    print(answer)


def required_env(name):
    value = os.getenv(name, "")
    if not value.strip():
        raise ValueError(f"请设置环境变量 {name}")
    return value


if __name__ == "__main__":
    try:
        main()
    except (ValueError, RuntimeError) as error:
        sys.exit(str(error))
