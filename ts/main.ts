import { createInterface } from "node:readline";
type Message = { role: string; content: string };
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

    // 每次请求都把之前的消息带上，模型才能理解上下文。

    const response = await fetch(apiUrl, {
      method: "POST",
      headers: { "Content-Type": "application/json", Authorization: `Bearer ${apiKey}` },
      body: JSON.stringify({ model, messages }),
    }).catch(() => {
      // 请求地址可能携带敏感信息，出错时不回显原始 URL。
      throw new Error("模型请求失败，请检查模型服务地址或网络连接");
    });
    if (!response.ok) throw new Error(`模型请求失败：HTTP ${response.status}`);
    const result = await response.json() as { choices?: Array<{ message?: { content?: unknown } }> };
    
    const answer = result.choices?.[0]?.message?.content;
    if (typeof answer !== "string") throw new Error("模型响应中没有找到文本回答");
    console.log(`助手：${answer}`);
    // 保存助手回答，下一轮就能看到双方的对话。
    messages.push({ role: "assistant", content: answer });
  }
} finally {
  input.close();
}
