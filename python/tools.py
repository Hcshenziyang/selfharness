"""工具的说明、参数校验与执行放在一起。"""

import json
import math


def definitions():
    return [{
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
