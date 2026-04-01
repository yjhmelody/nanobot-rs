# Provider Fallback 实现说明

本文档描述 `nanobot-rs` 中 **当前已经实现** 的 provider fallback 行为，不再保留面向未来的设计讨论或过长的使用示例。

## 作用

fallback 的目标是提升 provider 调用可靠性：

- 当主 provider 返回可重试错误时，自动尝试下一个 provider
- 当错误不可重试时，立即终止，不继续 fallback
- 对上层保持统一的 `LLMProvider` 接口

当前实现同时支持：

- 非流式 `chat()`
- 流式 `chat_stream()`

## 当前接入方式

fallback 由 provider 工厂自动装配。

当配置中存在：

- `agents.defaults.provider`
- `agents.defaults.model`
- `agents.defaults.fallbackProviders`

运行时会：

1. 先创建主 provider
2. 再按顺序创建 fallback provider 列表
3. 最后包装成一个 `FallbackProvider`

如果没有配置 `fallbackProviders`，则直接使用单个 provider。

## 配置方式

示例：

```json
{
  "agents": {
    "defaults": {
      "model": "anthropic/claude-sonnet-4-5",
      "provider": "auto",
      "fallbackProviders": ["openai", "custom"]
    }
  },
  "providers": {
    "anthropic": {
      "apiKey": "sk-ant-..."
    },
    "openai": {
      "apiKey": "sk-..."
    },
    "custom": {
      "apiBase": "https://your-endpoint.example.com/v1",
      "apiKey": "token"
    }
  }
}
```

含义是：

- 主 provider：根据 `model` 和 `provider` 推断
- fallback 顺序：`openai` -> `custom`

## 当前行为

### 非流式调用

`FallbackProvider::chat()` 会按顺序尝试 provider：

- 成功则立即返回结果
- 遇到可重试错误则尝试下一个
- 遇到不可重试错误则立即返回该错误
- 如果全部失败，则返回最后一个错误

### 流式调用

`FallbackProvider::chat_stream()` 也采用同样的顺序尝试：

- 成功创建流后立即返回
- 如果创建流阶段失败，且错误可重试，则尝试下一个 provider
- 如果错误不可重试，则立即停止 fallback
- 如果全部失败，则返回最后一个流式错误

需要注意：

- fallback 发生在“创建流”阶段
- 一旦某个 provider 的流已经成功建立，后续流中断不会再自动切换到另一个 provider

## 可重试与不可重试错误

### 对 `chat()`

当前实现依赖 provider error 自身的 `is_retryable()` 判定。

因此：

- 网络问题
- 超时
- 限流
- 某些服务端临时错误

通常会触发 fallback。

而：

- 认证失败
- 明显配置错误
- 其他不可恢复错误

通常不会继续 fallback。

### 对 `chat_stream()`

当前流式 fallback 使用的是较简单的规则：

以下错误会被视为可重试：

- `StreamError::Network`
- `StreamError::Provider` 且错误消息中包含：
  - `rate limit`
  - `timeout`

其他流式错误默认不会继续 fallback。

这意味着当前流式判定是“实用型实现”，不是完整的统一错误分类系统。

## 与当前 provider 体系的关系

当前 provider 工厂会创建：

- `AnthropicProvider`
- `OpenAICompatProvider`
- 或由它们组成的 `FallbackProvider`

其中：

- `anthropic` 使用原生 Messages 协议
- `openai` / `custom` 走 `OpenAICompatProvider`
- `custom` 要求兼容 Responses API

fallback 只负责“失败后换下一个 provider”，不会改变各 provider 的协议适配逻辑。

## 边界与限制

当前实现有几个重要边界：

### 1. 不支持中途流切换

如果流已经开始输出，之后即使连接中断，也不会自动迁移到下一个 provider 继续同一条流。

### 2. 不做多次重试

每个 provider 只尝试一次。  
当前实现是“按 provider 顺序切换”，不是“对单 provider 做重试退避”。

### 3. 不保证输出一致

不同 provider 即使用相同输入，也可能返回不同结果。  
fallback 只保证可用性，不保证响应完全一致。

### 4. 依赖配置正确

fallback 链中的每个 provider 都必须能独立创建成功。  
如果某个 provider 缺少必要配置，运行时可能在构建阶段就失败，而不是等到真正 fallback 时才发现。

## 使用建议

- 给 fallback 链中的每个 provider 都配置有效凭证
- 尽量选择都支持你当前能力边界的 provider
  - 例如工具调用
  - 流式输出
  - Responses / Messages 协议兼容性
- 把 fallback 当作“高可用补偿”，不要当作“结果一致性保障”
- 如果你需要更复杂的重试策略，应在更高层增加重试、超时和观测能力

## 何时更新本文档

仅在以下变化时更新：

- fallback 判定规则发生变化
- fallback 支持范围变化（例如新增中途续流）
- provider 工厂装配方式变化
- 配置字段语义变化

如果只是内部实现重构、日志调整或测试补充，不必扩写本文档。

## 相关文档

- `docs/QUICK_START.md`
  - provider 配置与基础使用
- `docs/ARCHITECTURE.md`
  - runtime 与 provider 总体结构
- `docs/DEVELOPMENT.md`
  - 开发与调试流程

## 结论

当前 fallback 机制的定位很明确：

- 用统一接口封装多个 provider
- 在可重试失败时自动切换
- 同时覆盖非流式与流式入口
- 以较低复杂度换取更高可用性

因此，这份文档的重点不是介绍“所有可能策略”，而是说明：

**当前代码里 fallback 实际会怎样工作，以及它不会做什么。**