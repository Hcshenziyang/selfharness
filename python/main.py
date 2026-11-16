"""第 2 节：把对话保存在内存中，继续提问。"""

import os
import sys

import requests
from dotenv import find_dotenv, load_dotenv


def main():
    # .env 只作为补充，外部已经设置的变量仍然优先。
    env_file = find_dotenv(usecwd=True)
    if env_file:
        load_dotenv(env_file)
    api_url = required_env("LLM_API_URL")
    api_key = required_env("LLM_API_KEY")
    model = required_env("LLM_MODEL")

    # 历史只存在当前进程中；每次请求都发送目前的完整消息列表。
    messages = []
    next_prompt = " ".join(sys.argv[1:]).strip() or None
    while True:
        if next_prompt is not None:
            prompt, next_prompt = next_prompt, None
        else:
            try:
                prompt = input("你：").strip()
            except EOFError:  # Ctrl+D 也可以正常退出。
                break
        if not prompt.strip():
            continue
        if prompt.strip() in ("exit", "quit"):
            break

        messages.append({"role": "user", "content": prompt})
        try:
            response = requests.post(
                api_url,
                headers={"Authorization": f"Bearer {api_key}"},
                json={"model": model, "messages": messages},
                timeout=60,
            )
            response.raise_for_status()
        except requests.RequestException:
            raise RuntimeError("模型请求失败，请检查接口配置和网络") from None
        try:
            answer = response.json()["choices"][0]["message"]["content"]
        except (ValueError, KeyError, IndexError, TypeError):
            raise RuntimeError("模型响应中没有找到文本回答") from None
        if not isinstance(answer, str):
            raise RuntimeError("模型响应中没有找到文本回答")
        print(f"助手：{answer}")
        # 模型不知道我们的上一轮回答，所以下次请求也要一起带上。
        messages.append({"role": "assistant", "content": answer})


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
