# DogOHarness

一个用 Rust 写的终端对话程序，用来学习如何调用模型 API 和管理多轮对话。程序通过 `reqwest` 调用 DeepSeek 的 Chat Completions 接口，把用户消息和模型回复保存在内存里。每次请求都会带上当前对话历史，退出后开启新会话。

## 运行

准备支持 Rust 2024 edition 的 Rust 和 Cargo，以及 DeepSeek API Key。在项目根目录创建 `.env`：

```dotenv
LLM_API_URL=https://api.deepseek.com/chat/completions
LLM_API_KEY=your_api_key
LLM_MODEL=deepseek-flash
```

把 `your_api_key` 换成自己的密钥。在同一个终端加载配置并运行：

```sh
set -a
source .env
set +a
cargo run
```

输入问题并按回车，收到回复后可以继续追问。输入 `/exit` 或按 Ctrl-D 结束对话。

入口在 [src/main.rs](src/main.rs)，可以顺着 `main()` 看消息怎么进入历史，再看 `ask_llm()` 怎么发送请求和取出回复。
