# 统一流式响应实现说明

本文档只描述 `nanobot-rs` 中 **当前已经存在的统一流式响应实现**，不再保留早期的完整设计草案、分阶段计划或大段伪代码。

## 目标

当前流式层的目标很简单：

- 为上层提供统一的流式事件接口
- 屏蔽不同 provider 的流协议差异
- 允许 provider 在“真实流式输出”和“非流式回退”之间共用同一抽象
- 让上层在需要时可以消费增量事件，也可以累积为完整响应

## 当前实现结构

统一流式实现位于 `crates/nanobot-provider/src/streaming/`，包含：

- `events.rs`
  - 定义统一事件与错误类型
- `accumulator.rs`
  - 负责把流式事件累积为完整响应
- `adapter.rs`
  - 定义 provider 适配器接口
- `sse_adapter.rs`
  - 处理 Anthropic 的 SSE 流
- `openai_adapter.rs`
  - 处理 OpenAI Responses 风格的流

模块导出入口在：

- `crates/nanobot-provider/src/streaming/mod.rs`

Provider trait 位于：

- `crates/nanobot-provider/src/traits.rs`

## 统一抽象

### 1. `LLMProvider::chat`

所有 provider 都必须实现非流式接口：

- 输入：`ChatRequest`
- 输出：`LLMResponse`

这是最基础、最稳定的调用方式。

### 2. `LLMProvider::chat_stream`

流式接口也已经纳入统一 trait。

其职责是返回统一的事件流，而不是把 provider 原始协议直接暴露给上层。

默认行为不是报错，而是：

- 先调用 `chat()`
- 再把完整结果包装成一个单事件流返回

这意味着：

- 即使某个 provider 还没有真实流式能力，上层也可以通过同一个接口消费结果
- 上层无需区分“真流式 provider”和“非流式 fallback provider”

## 当前事件模型

统一流式事件用于表达文本增量、工具调用增量、usage 更新、完成状态和错误。

这层抽象的重点不是完整复刻每家 provider 的原始事件，而是保留对 Agent 有意义的稳定语义，例如：

- 文本内容增量
- thinking / reasoning 增量
- 工具调用开始、参数增量、结束
- token usage 更新
- finish reason 更新
- 完整响应完成事件
- 流错误

因此，当前设计更偏向“Agent 可消费的统一事件”，而不是“协议透传”。

## 累积器职责

`StreamAccumulator` 的职责是把事件流重建成一个标准 `LLMResponse`。

它会维护：

- 内容块
- thinking 块
- 工具调用参数拼装
- usage 信息
- finish reason

这样可以支持两种消费方式：

- **增量消费**：边收到边处理
- **最终消费**：把流事件累积成完整响应后再统一处理

这也是统一流式层存在的核心价值之一：  
上层不必自己维护每个 provider 的拼装状态。

## Provider 适配方式

### Anthropic

Anthropic provider 使用独立适配器处理 SSE 流。

特点：

- 底层协议是 SSE
- 适配器负责从字节流中解析事件
- 再映射为统一 `StreamEvent`

### OpenAI / OpenAI-compatible

OpenAI 兼容 provider 使用独立适配器处理 Responses 风格流。

特点：

- 解析 OpenAI 风格的数据块
- 统一转换为标准事件流
- 对上层隐藏具体 provider 名称与协议差异

### 非流式回退

如果某 provider 没有覆盖 `chat_stream()`，则自动回退为：

- `chat()` 一次性拿到完整响应
- 包装成单个完成事件返回

这保证了统一接口始终可用。

## 与当前 provider 结构的关系

当前 provider 工厂会根据配置创建：

- `AnthropicProvider`
- `OpenAICompatProvider`
- 或它们的 fallback 组合

从流式角度看：

- `AnthropicProvider` 对应 Anthropic 的消息流协议
- `OpenAICompatProvider` 对应 OpenAI Responses 兼容协议
- fallback 机制不改变统一流式接口本身

也就是说，**provider 选择** 和 **流式事件抽象** 是两层独立职责：

- provider 决定“请求发到哪里”
- streaming 层决定“如何把结果统一表示给上层”

## 当前文档边界

本文档不再包含以下内容：

- 超长类型草案
- 分阶段实施计划
- 尚未落地的未来架构
- 大段与源码重复的伪代码
- Agent Loop 的假想接入方式说明

这些内容维护成本高，而且容易与实际代码脱节。

## 什么时候更新本文档

仅在以下情况变化时更新：

- 统一事件模型发生实质变化
- `chat_stream()` 的行为边界改变
- 新增或移除 provider 适配方式
- 累积器的职责发生明显调整

如果只是某个字段、解析细节或内部实现调整，优先以源码和测试为准，不必扩写本文档。

## 相关文档

- `docs/README.md`
  - 文档索引与维护原则
- `docs/ARCHITECTURE.md`
  - 当前系统总体架构
- `docs/QUICK_START.md`
  - 安装、配置与使用方式
- `docs/DEVELOPMENT.md`
  - 开发、测试与调试流程

## 结论

当前 `nanobot-rs` 的统一流式层已经具备稳定职责：

- 用统一 trait 暴露流式能力
- 用 adapter 吸收 provider 协议差异
- 用 accumulator 把事件还原为标准响应
- 用默认回退机制保证接口一致性

因此，这份文档的定位不是“设计提案”，而是：

**说明当前实现边界，帮助维护者理解流式抽象在系统中的角色。**
