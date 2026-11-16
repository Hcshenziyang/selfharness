// 工具说明随请求发给模型，执行函数由我们的程序负责。
export const tools = [{
  type: "function",
  function: {
    name: "add_numbers",
    description: "计算两个数字的和",
    parameters: {
      type: "object",
      properties: {
        a: { type: "number", description: "第一个数字" },
        b: { type: "number", description: "第二个数字" },
      },
      required: ["a", "b"],
      additionalProperties: false,
    },
  },
}];

export function executeTool(name: string, argumentsText: string): string {
  // 模型传来的是字符串，解析前不能假设它一定是正确参数。
  if (name !== "add_numbers") return JSON.stringify({ error: `未知工具：${name}` });
  let args: unknown;
  try {
    args = JSON.parse(argumentsText);
  } catch {
    return JSON.stringify({ error: "工具参数不是有效的 JSON" });
  }
  if (typeof args !== "object" || args === null || Array.isArray(args)) {
    return JSON.stringify({ error: "工具参数必须是对象" });
  }
  const object = args as Record<string, unknown>;
  if (Object.keys(object).some((key) => key !== "a" && key !== "b")) {
    return JSON.stringify({ error: "工具参数只允许包含 a 和 b" });
  }
  if (typeof object.a !== "number" || typeof object.b !== "number") {
    return JSON.stringify({ error: "工具参数 a 和 b 都必须是数字" });
  }
  const sum = object.a + object.b;
  if (!Number.isFinite(sum)) return JSON.stringify({ error: "计算结果超出可表示范围" });
  return JSON.stringify({ result: sum });
}
