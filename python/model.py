"""只负责向模型发送请求并接收结果。"""

import requests


def request(config, messages, tools):
    try:
        response = requests.post(
            config["api_url"],
            headers={"Authorization": f"Bearer {config['api_key']}"},
            json={"model": config["model"], "messages": messages, "tools": tools},
            timeout=60,
        )
        response.raise_for_status()
    except requests.RequestException:
        # 避免把 URL 或请求头中的敏感信息打印到终端。
        raise RuntimeError("模型请求失败，请检查接口配置和网络") from None
    try:
        return response.json()
    except ValueError:
        raise RuntimeError("无法解析模型响应") from None
