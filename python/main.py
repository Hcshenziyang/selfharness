"""第 4 节：把配置、模型请求和工具拆出去。"""

import json
import sys

import config
import model
import tools

# 每个用户问题最多请求八次，避免工具调用一直循环。
MAX_MODEL_REQUESTS = 8


def main():
    settings = config.load()
    definitions = tools.definitions()
    # 历史只存在当前进程中；每次请求都发送完整消息列表。
    messages = []
    next_prompt = config.initial_prompt()
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
            response = model.request(settings, messages, definitions)
            try:
                choice = response["choices"][0]
                message = choice["message"]
                if not isinstance(message, dict):
                    raise TypeError
            except (KeyError, IndexError, TypeError):
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

            # 原始 assistant 消息和每个工具结果都必须留在对话历史里。
            messages.append(message)
            for call in calls:
                if not isinstance(call, dict) or not isinstance(call.get("id"), str) or not call["id"]:
                    raise RuntimeError("模型返回的工具调用缺少 id，无法关联结果")
                function = call.get("function")
                if not isinstance(function, dict):
                    function = {}
                name, arguments = function.get("name"), function.get("arguments")
                if isinstance(name, str) and isinstance(arguments, str):
                    result = tools.execute_tool(name, arguments)
                else:
                    result = json.dumps({"error": "工具名称或参数缺失"}, ensure_ascii=False)
                messages.append({"role": "tool", "tool_call_id": call["id"], "content": result})


if __name__ == "__main__":
    try:
        main()
    except (ValueError, RuntimeError) as error:
        sys.exit(str(error))
