import { createInterface } from "node:readline";
import { initialPrompt, loadConfig } from "./config.ts";
import { requestModel, type Message } from "./model.ts";
import { executeTool, tools } from "./tools.ts";

const config = loadConfig();

// 对话历史暂存在内存中；进程结束后就会消失。
const messages: Message[] = [];
const input = createInterface({ input: process.stdin, crlfDelay: Infinity });
const lines = input[Symbol.asyncIterator]();
let nextPrompt = initialPrompt();

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
      const result = await requestModel(config, messages, tools);
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
