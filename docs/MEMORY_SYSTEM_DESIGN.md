# Memory 系统当前实现说明

本文档描述 `nanobot-rs` 中 **当前已经实现** 的 memory 相关结构与职责，不再维护面向未来的增强设计、外部向量数据库集成草案或长篇实施计划。

## 目标

当前 memory 系统的目标是：

- 为 Agent 提供可注入提示词的长期上下文
- 为运行过程保留可追加的历史记录
- 保持实现简单、可本地运行、低维护成本
- 通过 trait 保留后续扩展空间，但不提前承诺未落地能力

## 当前实现位置

memory 相关实现主要位于 `crates/nanobot-session/`：

- `traits.rs`
  - 定义 `MemoryProvider` trait
- `memory_store.rs`
  - 提供基于工作区文件的 memory 存储
- `memory_provider.rs`
  - 提供 `FileMemoryProvider` 和 `CompositeMemoryProvider`
- `manager.rs`
  - `SessionManager` 在保存/读取流程中组合 memory 能力
- `consolidation_strategy.rs`
  - 会话压缩与 summarization，和 memory 相邻但职责不同

## 当前能力边界

当前 memory 系统不是一个通用知识库，也不是语义检索系统。它主要提供两类能力：

1. **长期记忆上下文**
   - 从工作区中的 memory 文件读取内容
   - 在构建 Agent 上下文时作为附加信息使用

2. **历史追加记录**
   - 将运行过程中的条目追加到历史文件
   - 用于保留可追溯的本地历史信息

当前不包含：

- 向量 embedding
- 语义召回
- relevance ranking
- metadata filtering
- 外部 memory backend 自动切换
- 分布式同步
- OpenViking 原生集成

这些都不属于当前真实实现。

## `MemoryProvider` trait

当前 `MemoryProvider` 是一个轻量抽象，重点在于“上下文补充”和“历史写入”，而不是复杂检索。

它提供的核心职责是：

- `get_context(query, session_key)`
  - 返回可注入到提示词或上下文中的 memory 文本
- `store(content, session_key, metadata)`
  - 写入长期 memory
- `append_history(entry)`
  - 追加历史记录

这里的 `query` 和 `metadata` 目前更多是为扩展保留接口形状，当前文件实现并没有把它们用作复杂检索条件。

## 文件型 memory 实现

### `FileMemoryProvider`

`FileMemoryProvider` 是当前默认、也是实际接入运行时的 memory provider。

它基于工作区目录中的文件进行读写，特点是：

- 本地优先
- 无外部依赖
- 行为稳定、易调试
- 适合当前项目的 workspace 模式

当前行为大致如下：

- `get_context(...)`
  - 返回长期 memory 内容
- `store(...)`
  - 写入长期 memory
- `append_history(...)`
  - 向历史记录追加条目

从职责上看，它更接近“文件化长期记忆适配器”，而不是“智能 memory 引擎”。

### `MemoryStore`

`MemoryStore` 负责底层文件组织与读写，是 `FileMemoryProvider` 的存储基础。

当前文档层面只需要理解：

- memory 内容保存在 workspace 下的 `memory/` 目录
- 长期 memory 与 history 以文件方式组织
- 上层通过 provider trait 访问，不直接依赖底层文件细节

具体文件格式与读写细节以源码为准。

## 组合型 memory 实现

### `CompositeMemoryProvider`

当前代码中还提供了 `CompositeMemoryProvider`，用于把多个 `MemoryProvider` 组合起来。

它的当前行为是：

- `get_context(...)`
  - 依次从多个 provider 获取上下文
  - 忽略空结果
  - 以分隔符拼接成单个字符串返回
- `store(...)`
  - 向所有 provider 写入
- `append_history(...)`
  - 向所有 provider 追加历史

这说明当前 memory 层已经具备**简单组合能力**，但仍然是非常直接的 fan-out / concat 语义，不包含：

- 去重
- 排序
- 置信度评分
- 权重合并
- 失败恢复策略

因此，它更适合作为“多来源上下文拼接器”，而不是复杂检索框架。

## 与 Session 系统的关系

memory 系统和 session 系统是相关但不同的两层：

### Session

Session 负责：

- 会话消息存储
- 会话列表与删除
- 会话压缩（consolidation）
- 生命周期 hook 与消息历史管理

### Memory

Memory 负责：

- 提供额外长期上下文
- 保存长期记忆内容
- 维护历史记录文件

可以把两者理解为：

- `session` 处理“对话主记录”
- `memory` 处理“补充上下文与长期侧写”

## 与 consolidation 的关系

`consolidation` 经常会被误认为 memory 系统的一部分，但在当前实现里，它更适合作为 **session 压缩策略** 理解。

### consolidation 当前职责

- 当消息数量达到阈值时触发
- 保留最近若干条消息
- 将更早的会话消息摘要为一条系统消息
- 降低后续上下文长度

### memory 当前职责

- 存储长期 memory 文件
- 返回 memory 上下文
- 记录 history

也就是说：

- consolidation 解决的是“会话太长”
- memory 解决的是“长期信息放哪里”

这两者会一起影响最终提示词上下文，但职责并不相同。

## 当前运行时接入方式

在当前运行时构建过程中，`AgentLoopBuilder` 会创建并注入：

- `JsonlSessionStore`
- `LlmConsolidationStrategy`
- `FileMemoryProvider`

这意味着当前默认运行时里：

- session 是 JSONL 持久化
- consolidation 是 LLM 驱动摘要
- memory 是文件型实现

因此，当你从系统角度看上下文构建链路时，可以把 memory 看成默认启用的本地能力，而不是可选实验模块。

## 当前设计取舍

当前 memory 设计明显偏向“够用、稳定、本地优先”：

### 优点

- 依赖少
- 本地可读可写
- 容易排查问题
- 不需要额外服务
- 能与 workspace 模式自然结合

### 限制

- 检索能力弱
- 没有语义搜索
- 无结构化排名
- 不适合大规模 memory 数据集
- 主要依赖文件内容质量

这是一种有意的取舍：当前项目更重视运行时可维护性，而不是过早引入复杂 memory 基础设施。

## 不再维护的内容

本文档不再保留以下内容：

- OpenViking 集成设计
- 向量数据库 schema 草案
- embedding / semantic search 架构设想
- migration plan
- plugin memory backend 长篇规划
- 与当前 trait 不一致的大型增强接口设计

如果未来这些能力真正落地，应以新实现为准，直接更新本文档，而不是继续累积“未来设计草案”。

## 什么时候更新本文档

仅在以下情况变化时更新：

- `MemoryProvider` trait 职责发生变化
- 默认 memory backend 改变
- memory 文件组织方式发生明显调整
- 运行时不再默认接入 `FileMemoryProvider`
- memory 与 session / context builder 的边界有实质变化

如果只是小范围内部实现变动，优先以源码和测试为准。

## 相关文档

- `docs/README.md`
  - 文档维护原则
- `docs/ARCHITECTURE.md`
  - 系统整体架构
- `docs/DEVELOPMENT.md`
  - 开发与测试流程
- `docs/QUICK_START.md`
  - 使用方式与配置入口

## 结论

当前 `nanobot-rs` 的 memory 系统是一个**轻量、本地优先、文件驱动**的实现。

它的真实定位不是高级知识检索平台，而是：

- 为 Agent 提供长期上下文补充
- 为运行过程保留本地历史
- 与 session 压缩协作，共同控制上下文质量和长度

如果后续需要引入更复杂的 memory backend，也应该在保持这层职责清晰的前提下扩展，而不是把所有“长期信息处理”都混进当前 memory 抽象中。