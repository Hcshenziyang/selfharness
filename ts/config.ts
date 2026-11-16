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

export function loadConfig(): { apiUrl: string; apiKey: string; model: string } {
  loadEnv();
  return { apiUrl: requiredEnv("LLM_API_URL"), apiKey: requiredEnv("LLM_API_KEY"), model: requiredEnv("LLM_MODEL") };
}

export function initialPrompt(): string | undefined {
  const prompt = process.argv.slice(2).join(" ");
  return prompt.trim() ? prompt : undefined;
}
