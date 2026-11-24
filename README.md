# DogOHarness

一个用 Rust 写的本地网页对话程序，用来学习模型调用、Agent Loop 和 MCP 工具。后端通过 `reqwest` 调用 OpenAI Responses API 格式的模型网关。

## 运行

准备支持 Rust 2024 edition 的 Rust 和 Cargo，以及网关的 API Key。在项目根目录运行 `cp .env.example .env`，再填写模型配置。下面用 DeepSeek 举例。

```dotenv
LLM_API_URL=https://api.deepseek.com/responses
LLM_API_KEY=your_api_key
LLM_MODEL=deepseek-flash
```

`LLM_API_URL` 填完整的 Responses 接口地址，例如自建网关的 `https://gateway.example/v1/responses`。`LLM_MODEL` 和 `LLM_API_KEY` 分别填这个网关的模型名和密钥。在同一个终端加载配置并运行：

```sh
set -a
source .env
set +a
cargo run
```

浏览器打开 <http://127.0.0.1:8787> 即可对话。网页服务仅监听本机地址，按 Ctrl-C 停止服务。

## 会话历史

点击「新建对话」开始另一段会话。左侧「历史会话」可以切换回来继续聊，标题取自第一次成功对话的提问。刷新页面或重启服务后都可以继续。

SQLite 文件默认保存在启动目录的 `dogoharness.sqlite3`，可以通过环境变量 `DOGO_DB_PATH` 指定文件路径。`conversations` 表每行是一段会话，`messages` 表按顺序保存用户消息、助手回复、工具调用和工具结果。一轮成功后统一写入。会话的消息构造和数据库读写在 [src/conversation/](src/conversation/)。

## MCP 服务

在网页点击「添加服务」可填写 MCP 配置并立即连接。

- **stdio**：填写命令、参数及可选环境变量。例如命令 `python3`，参数一行填写 `examples/mcp_demo.py`。
- **Streamable HTTP**：填写服务地址，以及可选的 Bearer Token 或请求头。可以先在另一个终端运行 `python3 examples/mcp_http_demo.py`，然后填写 `http://127.0.0.1:8765/mcp`。

连接后可以在网页上看到服务和工具，也可以断开服务。不同会话共享本次运行的 MCP 连接，重启服务后需要重新连接。工具列表由后端载入，Agent Loop 在下一次请求模型时使用。

例如将 stdio 演示服务命名为 `local` 后，可以输入「请调用 `mcp__local__demo_code` 查询测试暗号」。如果同时连接多个服务，工具名中的服务名前缀用于区分同名工具。

stdio 配置中的环境变量会传给启动的 MCP 子进程。未填写工作目录时使用 DogOHarness 的启动目录；填写后，子进程会在指定目录中解析相对路径。Streamable HTTP 支持 JSON 与 SSE 响应；旧版独立 HTTP+SSE 传输和 OAuth 登录流程目前尚未接入。
