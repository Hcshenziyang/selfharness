import { createInterface } from "node:readline";
type ToolCall = { id?: string; function?: { name?: string; arguments?: string } };
type Message = {
  role?: string;
  content?: string | null;
  tool_calls?: ToolCall[] | null;
  tool_call_id?: string;
  [key: string]: unknown;
};
type ModelResponse = {
  choices?: Array<{ finish_reason?: string; message?: Message }>;
};
import { existsSync } from "node:fs";
import { dirname, join, resolve } from "node:path";

// 像 Rust 版一样，从当前目录向上找 .env；已有环境变量优先。
function loadEnv(): void {
  let directory = resolve(process.cwd());
  while (true) {
    const file = join(directory, ".env");
    if (existsSync(file)) {
      process.loadEnvFile(file);
      return;
    }
    const parent = dirname(directory);
    if (parent === directory) return;
    directory = parent;
  }
}

function requiredEnv(name: string): string {
  const value = process.env[name];
  if (!value?.trim()) throw new Error(`请设置环境变量 ${name}`);
  return value;
}

loadEnv();
const apiUrl = requiredEnv("LLM_API_URL");
const apiKey = requiredEnv("LLM_API_KEY");
const model = requiredEnv("LLM_MODEL");

// 工具说明随请求发给模型，执行函数由我们的程序负责。
const tools = [{
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

function executeTool(name: string, argumentsText: string): string {
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

// 对话历史暂存在内存中；进程结束后就会消失。
const messages: Message[] = [];
const input = createInterface({ input: process.stdin, crlfDelay: Infinity });
const lines = input[Symbol.asyncIterator]();
const firstPrompt = process.argv.slice(2).join(" ");
let nextPrompt: string | undefined = firstPrompt.trim() ? firstPrompt : undefined;

try {
  while (true) {
    // 命令行参数作为第一个问题；之后从终端逐行读取。
    let prompt = nextPrompt;
    nextPrompt = undefined;
    if (prompt === undefined) {
      process.stdout.write("你：");
      const line = await lines.next();
      if (line.done) break; // Ctrl+D / EOF 也可以退出。
      prompt = line.value.trim();
    }
    if (!prompt.trim()) continue;
    if (["exit", "quit"].includes(prompt.trim())) break;
    messages.push({ role: "user", content: prompt });

    // 每个用户问题各自有上限，模型可在一轮里多次调用工具。
    for (let requestIndex = 0; requestIndex < 8; requestIndex++) {
      const response = await fetch(apiUrl, {
        method: "POST",
        headers: { "Content-Type": "application/json", Authorization: `Bearer ${apiKey}` },
        body: JSON.stringify({ model, messages, tools }),
      }).catch(() => {
        // 请求地址可能携带敏感信息，出错时不回显原始 URL。
        throw new Error("模型请求失败，请检查模型服务地址或网络连接");
      });
      if (!response.ok) throw new Error(`模型请求失败：HTTP ${response.status}`);
      const result = await response.json() as ModelResponse;
      const choice = result.choices?.[0];
      const message = choice?.message;
      if (!message || typeof message !== "object" || Array.isArray(message)) {
        throw new Error("模型响应中没有找到消息");
      }
      if (choice?.finish_reason === "length") throw new Error("模型响应因长度限制被截断，请重试");
      const calls = message.tool_calls ?? [];
      if (!Array.isArray(calls)) throw new Error("模型返回的工具调用格式无效");
      if (calls.length === 0) {
        if (typeof message.content !== "string") throw new Error("模型响应中没有找到文本回答");
        console.log(`助手：${message.content}`);
        messages.push(message);
        break;
      }
      if (requestIndex === 7) throw new Error("模型请求次数已达到上限，无法继续工具调用");

      // 必须先保留完整的调用请求，再给每个调用 ID 追加对应结果。
      messages.push(message);
      for (const call of calls) {
        if (typeof call?.id !== "string" || !call.id) throw new Error("模型返回的工具调用缺少 id，无法关联结果");
        const name = call.function?.name;
        const args = call.function?.arguments;
        const result = typeof name === "string" && typeof args === "string"
          ? executeTool(name, args)
          : JSON.stringify({ error: "工具名称或参数缺失" });
        messages.push({ role: "tool", tool_call_id: call.id, content: result });
      }
      // 下一次模型请求会收到工具结果，然后决定继续调用还是回答。
    }
  }
} finally {
  input.close();
}
