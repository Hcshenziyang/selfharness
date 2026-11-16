# 01 · 大模型对话

## 什么是大模型对话？

嗨，先来做一件简单的事：给模型发一个问题，拿到回答，再显示出来。这一节先不考虑多轮对话和工具调用，咱们先看看一次请求是怎么走完的。

## 看本项目的代码

选择你熟悉的语言打开入口：

- Rust：[`rust/src/main.rs`](../../rust/src/main.rs)
- Python：[`python/main.py`](../../python/main.py)
- TypeScript：[`ts/main.ts`](../../ts/main.ts)

顺着注释找找：配置和问题从哪里来？程序怎样发出请求，又是怎样取出模型回答的？运行方式见项目根目录的 README。

## 扩展：接口长什么样？

程序使用 Chat Completions 格式：告诉服务使用哪个模型，把用户的问题放进消息列表，再从响应中取出文本。

不同服务的接口地址和模型名称可能不同，配置时可以参考服务方文档，例如 [DeepSeek 的首次 API 调用](https://api-docs.deepseek.com/)。
