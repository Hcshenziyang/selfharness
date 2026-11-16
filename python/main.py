"""第 3 节：让模型请求工具，再把结果交回模型。"""

import json
import math
import os
import sys

import requests
from dotenv import find_dotenv, load_dotenv

# 每个用户问题最多请求八次，避免工具调用一直循环。
MAX_MODEL_REQUESTS = 8


def main():
    env_file = find_dotenv(usecwd=True)
    if env_file:
        load_dotenv(env_file)
    api_url = required_env("LLM_API_URL")
    api_key = required_env("LLM_API_KEY")
    model = required_env("LLM_MODEL")

    # 工具说明随消息一起交给模型；真正执行代码在 execute_tool 里。
    tools = [{
        "type": "function",
        "function": {
            "name": "add_numbers",
            "description": "计算两个数字的和",
            "parameters": {
                "type": "object",
                "properties": {
                    "a": {"type": "number", "description": "第一个数字"},
                    "b": {"type": "number", "description": "第二个数字"},
                },
                "required": ["a", "b"],
                "additionalProperties": False,
            },
        },
    }]
    messages = []
    next_prompt = " ".join(sys.argv[1:]).strip() or None
    while True:
        if next_prompt is not None:
            prompt, next_prompt = next_prompt, None
        else:
            try:
                prompt = input("你：").strip()
            except EOFError:
                break
        if not prompt.strip():
            continue
        if prompt.strip() in ("exit", "quit"):
            break
        messages.append({"role": "user", "content": prompt})

        # 内层是工具循环：模型请求工具 -> 程序执行 -> 再问模型。
        for request_index in range(MAX_MODEL_REQUESTS):
            try:
                response = requests.post(
                    api_url,
                    headers={"Authorization": f"Bearer {api_key}"},
                    json={"model": model, "messages": messages, "tools": tools},
                    timeout=60,
                )
                response.raise_for_status()
            except requests.RequestException:
                raise RuntimeError("模型请求失败，请检查接口配置和网络") from None
            try:
                choice = response.json()["choices"][0]
                message = choice["message"]
                if not isinstance(message, dict):
                    raise TypeError
            except (ValueError, KeyError, IndexError, TypeError):
                raise RuntimeError("模型响应中没有找到消息") from None
            if choice.get("finish_reason") == "length":
                raise RuntimeError("模型响应因长度限制被截断，请重试")
            calls = message.get("tool_calls")
            if calls is None:
                calls = []
            if not isinstance(calls, list):
                raise RuntimeError("模型返回的工具调用格式无效")
            if not calls:
                answer = message.get("content")
                if not isinstance(answer, str):
                    raise RuntimeError("模型响应中没有找到文本回答")
                print(f"助手：{answer}")
                messages.append(message)
                break
            if request_index + 1 == MAX_MODEL_REQUESTS:
                raise RuntimeError("模型请求次数已达到上限，无法继续工具调用")

            # 保留完整的调用请求，并用 ID 将每个结果与调用对应起来。
            messages.append(message)
            for call in calls:
                if not isinstance(call, dict) or not isinstance(call.get("id"), str) or not call["id"]:
                    raise RuntimeError("模型返回的工具调用缺少 id，无法关联结果")
                function = call.get("function")
                if not isinstance(function, dict):
                    function = {}
                name, arguments = function.get("name"), function.get("arguments")
                if isinstance(name, str) and isinstance(arguments, str):
                    result = execute_tool(name, arguments)
                else:
                    result = json.dumps({"error": "工具名称或参数缺失"}, ensure_ascii=False)
                messages.append({"role": "tool", "tool_call_id": call["id"], "content": result})


def execute_tool(name, arguments):
    # 模型给的是字符串；参数缺失、格式不对等错误也要作为工具结果交回。
    if name != "add_numbers":
        return json.dumps({"error": f"未知工具：{name}"}, ensure_ascii=False)
    try:
        values = json.loads(arguments, parse_constant=reject_constant)
    except (ValueError, TypeError):
        return json.dumps({"error": "工具参数不是有效的 JSON"}, ensure_ascii=False)
    if not isinstance(values, dict):
        return json.dumps({"error": "工具参数必须是对象"}, ensure_ascii=False)
    if any(key not in ("a", "b") for key in values):
        return json.dumps({"error": "工具参数只允许包含 a 和 b"}, ensure_ascii=False)
    a, b = values.get("a"), values.get("b")
    if any(isinstance(x, bool) or not isinstance(x, (int, float)) for x in (a, b)):
        return json.dumps({"error": "工具参数 a 和 b 都必须是数字"}, ensure_ascii=False)
    try:
        result = float(a) + float(b)
    except (OverflowError, ValueError):
        result = math.inf
    if not math.isfinite(result):
        return json.dumps({"error": "计算结果超出可表示范围"}, ensure_ascii=False)
    return json.dumps({"result": result}, ensure_ascii=False)


def reject_constant(value):
    # Python 默认接受 NaN 和 Infinity，工具参数需要遵守标准 JSON。
    raise ValueError(f"无效的数字：{value}")


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
