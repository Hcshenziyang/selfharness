# 04 · 程序拆解

## 为什么要拆分程序？

随着多轮对话和工具调用加进来，一个文件要管的事越来越多。想改工具，却得先在请求和输入代码之间找半天。怎么办？我们可以按职责把代码放进不同文件，再由入口把它们串起来。

这次只整理代码，程序的使用方式和行为没有变化。拆分不会让模型变得更聪明；它的好处是，让人更容易找到和修改代码。

## 看本项目的代码

先选一个语言，从入口开始，顺着函数调用去看看其他模块：

| 职责 | Rust | Python | TypeScript |
| --- | --- | --- | --- |
| 对话与工具循环 | [main.rs](../../rust/src/main.rs) | [main.py](../../python/main.py) | [main.ts](../../ts/main.ts) |
| 配置与启动参数 | [config.rs](../../rust/src/config.rs) | [config.py](../../python/config.py) | [config.ts](../../ts/config.ts) |
| 模型请求 | [model.rs](../../rust/src/model.rs) | [model.py](../../python/model.py) | [model.ts](../../ts/model.ts) |
| 工具定义与执行 | [tools.rs](../../rust/src/tools.rs) | [tools.py](../../python/tools.py) | [tools.ts](../../ts/tools.ts) |

为什么工具结果还要回到入口？因为入口掌握着这轮对话的历史，需要把结果放回消息列表，再发给模型。三种语言的写法可以不同，先试着找到它们承担的共同职责。

## 扩展：Pi 怎么组织？

Pi 也把 Agent 的循环与具体应用分开；它还包含会话管理、用户界面等更丰富的部分。想顺着这个方向看看，可以从 [Pi 的 Agent Loop](https://github.com/earendil-works/pi/blob/22fc026b872b4a7df22ffa94e8fa00b873128054/packages/agent/src/agent-loop.ts) 开始。我们的例子暂时只按眼前的职责拆分，不必急着搭建完整框架。
