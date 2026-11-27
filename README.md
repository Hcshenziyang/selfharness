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

## 助手和会话历史

侧栏可以新建、切换和编辑助手。每个助手有自己的模型 ID、指令和可编辑的私有记忆（最多 4000 个字符）。同一助手的多个会话共用这些设置，各自保留聊天历史。所有助手使用同一个 Responses 网关与全局并发额度。首次启动或升级时，程序用 `LLM_MODEL` 初始化「默认助手」，已有会话自动归入该助手。

后台开始处理一条消息时读取助手设置。该回合的工具循环沿用相同的模型、指令和私有记忆，已经排队的下一条消息在开始时读取最新配置。指令和记忆通过 Responses 请求的 `instructions` 传入，SQLite 中的 `messages` 仅保存对话及工具记录。

`GET /api/agents` 列出助手，`POST /api/agents` 创建助手。`GET /api/agents/{id}` 返回完整设置，保存时用 `PUT` 提交设置及读取时的 `revision`。其他标签页保存了该助手，或助手自己更新了记忆，编辑窗口会提示版本变化并保留当前输入。提交旧版本返回 409。按助手列出会话用 `GET /api/conversations?agent_id=…`，创建会话用 `POST /api/conversations` 提交 `agent_id`。省略助手 ID 时使用默认助手。

点击「新建对话」打开空白页，发送第一条消息时才创建会话。左侧「历史会话」可以切换回来继续聊，标题默认取自第一次成功对话的提问。标题旁的「⋯」可以重命名或删除会话。

不同会话可以同时进行。在同一标签页可以连续发送多条消息，后端按会话顺序执行。可以撤回排队消息，也可以停止正在回复的回合。停止后会继续下一条，已经发出的外部工具操作可能仍会生效。关闭标签页后，已受理的消息仍会执行。重新打开会话可以看到进度，进程重启后可以恢复已写入 SQLite 的历史。

最近一次失败的原因和原文可在接下来的 5 分钟内从多个页面查看。该会话下次成功入队会清除这条提示，进程最多保留 64 个会话的失败记录。

可用 `DOGO_MAX_MODEL_CONCURRENCY` 设置同时请求模型的上限，默认 2。等待工具时会释放模型名额。`DOGO_MAX_UNFINISHED_TURNS` 控制所有会话合计可以受理的回合数，包含运行和排队中的回合，默认 16。达到上限时，新提交返回 HTTP 503，网页保留输入供稍后重试。

SQLite 文件默认保存在启动目录的 `dogoharness.sqlite3`，可以通过环境变量 `DOGO_DB_PATH` 指定文件路径。`conversations` 表每行是一段会话，`messages` 表按顺序保存用户消息、助手回复、工具调用和工具结果。一轮成功后统一写入。会话的消息构造和数据库读写在 [src/conversation/](src/conversation/)。

## 内置工具与工作目录

在侧栏选定助手，打开「内置工具与工作目录」。每个助手初始只开启 `echo`。其余三项分别授权。

| 工具 | 开启后的能力 |
| --- | --- |
| `memory_update` | 更新本助手的完整私有笔记，最多 4000 字符。后续回合读取新笔记。如果执行期间其他会话或用户先改了笔记，本次写入返回冲突。 |
| `read_file` | 读取工作目录里的 UTF-8 文本，单文件最多 64 KiB。 |
| `write_file` | 在已有子目录中创建或原子替换 UTF-8 文本，单次最多 64 KiB。 |

启用文件工具时，在设置中填入本机已有目录的绝对路径。模型使用相对路径，例如 `docs/note.txt`。文件工具逐级打开授权目录内的路径，拒绝绝对路径、上级目录、符号链接及多重硬链接文件。越界请求会作为工具错误回传给模型。这个目录约束内置文件工具，外部 MCP 使用其进程权限或远端凭据。

工具调用产生的记忆或文件写入在工具成功时生效。停止回合或后续模型请求失败时，这些已完成的操作仍会保留。

## MCP 服务

在当前助手的侧栏点击「添加服务」。每个助手独立管理绑定、活动连接和工具授权。两个助手可以用相同的服务名连接不同的进程或远端账号。

- **stdio** 使用命令、参数及可选工作目录和环境变量。可以用 `python3` 启动 `examples/mcp_demo.py`。
- **Streamable HTTP** 使用服务地址、可选 Bearer Token 或自定义请求头。运行 `python3 examples/mcp_http_demo.py` 可在本机测试 `http://127.0.0.1:8765/mcp`。

切换助手或刷新页面时，已有服务的工具授权区默认折叠。首次连接或发现新增工具时，对应服务会自动展开。勾选需要的工具，或点「全选」。例如把演示服务命名为 `local`，勾选 `demo_code` 后可在对话中要求助手调用 `mcp__local__demo_code`。重新发现的同名工具沿用已保存的授权，新增工具默认关闭。远端暂时撤下的已授权工具显示「暂未提供」，仍可取消授权。

| 卡片操作 | 效果 |
| --- | --- |
| 断开 | 关闭活动连接、释放连接配置，保留绑定及授权。 |
| 配置并连接 | 在待连接状态补填命令或凭据，连接成功后恢复已勾选的工具。 |
| 重连 / 编辑 | 使用进程内现有配置重新握手，或提交一份新配置。新连接成功后才替换原连接。 |
| 移除 | 关闭连接并删除当前助手的这条绑定和授权。 |

SQLite 保存服务名、传输类型、工具清单和授权。命令参数、环境变量、URL、Token、请求头只用于连接，进程重启后卡片显示「待连接」。标签页之间按助手同步连接和勾选状态。

Agent Loop 在每次请求模型前生成一次工具快照。模型收到的定义和该次返回的整批调用使用同一份权限及连接。更改勾选从下一次模型请求起生效。同一回合中途重连时，早先发出的调用仍指向原连接。等待回包时断开，模型会收到「执行结果未知」。内置 `echo`、`read_file` 和声明了 `readOnlyHint: true` 的 MCP 工具可并行，服务设置 `supports_parallel_tool_calls` 可显式允许整个服务并行。每批结果按调用顺序回传，单次 MCP 调用的超时为 30 秒。

工具接口以 `/api/agents/{id}` 为前缀。

| 路由 | 操作 |
| --- | --- |
| `GET /tools`、`PUT /tools` | 读取或保存内置工具与工作目录。保存时携带读取到的 `revision`。 |
| `GET /mcp/servers`、`POST /mcp/servers` | 查看绑定，或提交服务名与连接参数进行首次连接。 |
| `PUT /mcp/servers/{binding}`、`DELETE /mcp/servers/{binding}` | 按绑定 ID 重新配置并连接，或移除绑定。 |
| `POST /mcp/servers/{binding}/disconnect`、`POST /mcp/servers/{binding}/reconnect` | 断开，或复用进程内已有的配置重连。 |
| `PATCH /mcp/servers/{binding}/tools` | 用 `{"revision":1,"tools":{"mcp__local__demo_code":true}}` 修改指定项。空更新或重复勾选保留当前版本，提交旧版本返回 409。 |

stdio 子进程继承基础环境和显式填写的变量，使用配置的工作目录解析相对路径。Streamable HTTP 支持 JSON 与 SSE 响应。

## 测试

`cargo test` 使用临时 SQLite 和本机模拟服务测试完整对话流程。要同时运行双标签页回归，在 Node.js 可加载 `playwright` 且已安装 Chromium 的环境运行 `DOGO_BROWSER_TESTS=1 cargo test`。也可用 `PLAYWRIGHT_CHROMIUM_EXECUTABLE` 指定本机 Chromium 或 Edge 的可执行文件。
