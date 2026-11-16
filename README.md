# SelfHarness

## 准备

准备一个兼容 OpenAI Chat Completions 的模型服务及 API Key。按需要安装 Rust 和 Cargo、Python 3.9+ 或 Node.js 24+。

## 配置

在项目根目录复制配置示例并填写自己的配置：

```sh
cp .env.example .env
```

`LLM_API_URL` 填写模型接口的完整地址，`LLM_API_KEY` 填写密钥，`LLM_MODEL` 填写模型名称。程序会从当前目录向上查找 `.env`；已有环境变量优先。不要提交自己的 `.env`。

## 安装依赖

在项目根目录安装 Python 依赖：

```sh
python3 -m venv .venv
source .venv/bin/activate
python -m pip install -r python/requirements.txt
```

Rust 依赖会在运行时由 Cargo 安装；TypeScript 不需要安装额外依赖。

## 运行

在项目根目录，选择一种语言运行：

```sh
cargo run --manifest-path rust/Cargo.toml -- "你好"
python python/main.py "你好"
npm --prefix ts start -- "你好"
```

也可以不传问题，启动后再输入。输入 `exit` 或 `quit` 退出，也可以按 Ctrl+D 结束输入。

## 切换节点

通过 jj 的历史选择想运行的阶段。先在项目根目录查看节点：

```sh
jj log -r 'all()'
```

从想学习的节点创建工作副本，把下面的 `<节点 ID>` 换成 `jj log` 显示的 change ID：

```sh
jj new -r <节点 ID>
```

切换后，按当前节点的 README 配置和运行。若你刚用 Git 克隆且还没有 `.jj`，先在仓库根目录运行 `jj git init --colocate`。

## 文档

[初识 Agent Loop](docs/01-agent-loop/README.md)
