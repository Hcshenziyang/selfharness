# 03 · 工具调用

## 什么是工具调用？

模型会说话，但生成一段文字不会自动运行程序。它想借助计算器，怎么办？

我们先告诉模型有哪些工具、需要什么参数。模型认为有必要时会提出调用请求；程序接住并执行，再把结果交给模型。模型拿到结果后，可以继续调用工具，也可以回答用户。这一来一回，就是一个最小的工具调用循环。

## 看本项目的代码

选择你熟悉的语言打开入口：

- Rust：[`rust/src/main.rs`](../../rust/src/main.rs)
- Python：[`python/main.py`](../../python/main.py)
- TypeScript：[`ts/main.ts`](../../ts/main.ts)

入口保留了工具循环；工具说明和执行逻辑已经分别放在 [`rust/src/tools.rs`](../../rust/src/tools.rs)、[`python/tools.py`](../../python/tools.py) 和 [`ts/tools.ts`](../../ts/tools.ts)。顺着注释看看，模型提出调用后，结果是怎样交回去的？

三个版本都提供 `add_numbers`，用两个数字做加法。留意一下：一次回复可能包含多个调用，每个结果都要带上对应的调用 ID。参数错误或工具不存在时，程序会把错误交回模型；每次提问也有请求次数上限，避免一直循环。

模型是否使用工具由它自己决定，并不是每次提问都会调用。当前示例使用兼容 Chat Completions 的工具调用格式。

## 扩展：Pi 怎么做

Pi 也在 Agent Loop 中识别模型提出的工具调用，再交给执行逻辑。它还要处理更多工具和运行状态，所以比我们的示例丰富得多。[Pi：识别和处理工具调用](https://github.com/earendil-works/pi/blob/22fc026b872b4a7df22ffa94e8fa00b873128054/packages/agent/src/agent-loop.ts#L268-L289)
