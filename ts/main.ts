// 第一步：读取模型地址、密钥和模型名，再收集要提问的内容。
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

const prompt = process.argv.slice(2).join(" ") || "你好，请介绍一下你自己。";

// 第二步：向兼容 Chat Completions 的接口发送一次请求。

const response = await fetch(apiUrl, {
  method: "POST",
  headers: { "Content-Type": "application/json", Authorization: `Bearer ${apiKey}` },
  body: JSON.stringify({ model, messages: [{ role: "user", content: prompt }] }),
}).catch(() => {
  // 请求地址可能携带敏感信息，出错时不回显原始 URL。
  throw new Error("模型请求失败，请检查模型服务地址或网络连接");
});
if (!response.ok) throw new Error(`模型请求失败：HTTP ${response.status}`);
const result = await response.json() as { choices?: Array<{ message?: { content?: unknown } }> };

// 第三步：取出模型的第一条文本回答。
const answer = result.choices?.[0]?.message?.content;
if (typeof answer !== "string") throw new Error("模型响应中没有找到文本回答");
console.log(answer);
