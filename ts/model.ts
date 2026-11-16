type ToolCall = { id?: string; function?: { name?: string; arguments?: string } };
export type Message = {
  role?: string;
  content?: string | null;
  tool_calls?: ToolCall[] | null;
  tool_call_id?: string;
  [key: string]: unknown;
};
type ModelResponse = {
  choices?: Array<{ finish_reason?: string; message?: Message }>;
};

import type { loadConfig } from "./config.ts";

type Config = ReturnType<typeof loadConfig>;

// 这个模块只负责与模型通信，循环和工具处理留给入口。
export async function requestModel(config: Config, messages: Message[], tools: unknown): Promise<ModelResponse> {
  const response = await fetch(config.apiUrl, {
    method: "POST",
    headers: { "Content-Type": "application/json", Authorization: `Bearer ${config.apiKey}` },
    body: JSON.stringify({ model: config.model, messages, tools }),
  }).catch(() => {
    // 请求地址可能携带敏感信息，出错时不回显原始 URL。
    throw new Error("模型请求失败，请检查模型服务地址或网络连接");
  });
  if (!response.ok) throw new Error(`模型请求失败：HTTP ${response.status}`);
  return await response.json() as ModelResponse;
}
