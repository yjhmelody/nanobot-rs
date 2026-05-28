# AI Agent 开发踩坑记录

## 1. CLI 子进程卡死（最严重）

**现象**：Agent 执行外部 CLI 命令（如 `lark-cli`）后整个循环卡住，没有错误日志，没有超时，消息也不进了。

**根因**：`tokio::process::Command` 默认继承父进程的 stdin。当子进程尝试读取 stdin 时，由于无人写入，它会**永远阻塞**。Agent loop 的 `wait_with_output()` 也就永远不返回。

**解决**：`cmd.stdin(Stdio::null())` —— 一行代码。但排查花了很长时间，因为卡死不产生任何日志，没有任何错误信号。

**教训**：所有 spawn 的子进程**必须显式关闭 stdin**，除非你明确需要管道输入。

## 2. WebSocket 静默断开

**现象**：飞书/Lark WebSocket 连接在网络波动后断开，Agent 不再接收任何用户消息，且没有任何告警。

**根因**：`LarkWsClient::open()` 是一次性连接，断开后不自动重连。Agent 的 ws_task 直接退出，`payload_task` 也被 abort，所有入站流量丢失。

**解决**：外层套无限循环 + 指数退避重连（1s → 2s → 4s → ... → 60s cap）。每次重连重建 dispatcher 和 payload task。

**教训**：外部 SDK 的 WebSocket 客户端几乎**默认不重连**。接入时必须自己包装重连逻辑。

## 3. API 速率限制与静默错误

**现象**：飞书流式消息编辑到第 20 次时报错 `code 230072`（"消息已被编辑 20+ 次"），原因是每次流式更新都调用一次 edit API。

**根因**：飞书 API 对单条消息的编辑次数硬限制为 20 次。原来的实现每 500ms 编辑一次，一条长流式消息轻松突破限制。

**解决**：
- **批处理**：新内容 < 500 字 且 距上次编辑 < 2s → 跳过本次编辑
- **分片**：编辑次数 ≥ 18 次 或 内容长度 ≥ 24000 字 → 发新消息，切换到新消息继续编辑

**教训**：
- 不要假设 API 没有限制 —— 文档可能没写，但实际有限
- 流式更新的成本是 O(n) 编辑次数，长流必须分片

## 4. Streaming 协议的复杂状态

**现象**：Anthropic extended thinking API 报错 "content[].thinking in the thinking mode must be passed back to the API"。

**根因**：Anthropic 的 streaming SSE 事件中，`thinking` 内容分三个阶段到达：
1. `content_block_start` → 创建 thinking block
2. `content_block_delta` → 追加 thinking 文本 + 发送 `signature`
3. `content_block_stop` → 完成

`signature` 只在 delta 事件中出现一次，且必须在后续请求中原样传回。如果 accumulator 没正确保留 signature，下次请求就会报错。

**解决**：在 `ThinkingBlock` 中增加 `signature: Option<String>` 字段，accumulator 在收到 `SignatureDelta` 事件时设到当前 thinking block 上。

**教训**：流式协议的第一个和最后一个事件往往携带元数据，中间的事件携带增量。accumulator 必须正确处理这三个阶段的合并。

## 5. 序列化的双重陷阱

**现象**：`serde_json::Value` 作为中间表示导致双重序列化，且丢失类型信息。

**根因**：原来的代码是 `ChatRequest → serde_json::Value → API payload`，中间多了一层 `serde_json::to_value` 序列化和反序列化。不仅性能差，而且 `Value` 丢失了结构体字段的类型约束，容易拼错字段名。

**解决**：将 `send_request` / `send_request_with_proxy_fallback` 改为泛型 `T: Serialize`，直接传入强类型 payload struct。

**教训**：不要在泛型 API 层用 `Value` 做中转。强类型虽然有更多样板代码，但编译器能捕获拼写错误和类型不匹配。

## 6. 弱类型的字符串枚举

**现象**：`AnthropicInputMessage.role` 是 `&'static str`，值为 `"user"` 或 `"assistant"`。字符串拼错不会编译报错，只在运行时出错。

**解决**：改为 `AnthropicMessageRole` 枚举，`#[serde(rename_all = "snake_case")]` 自动序列化。

**教训**：API 层的枚举值必须用 Rust enum + serde rename，不用字符串字面量。

## 7. 进程清理

**现象**：Agent 超时后子进程变成孤儿进程继续运行。

**根因**：`tokio::process::Child` drop 时不会自动 kill 子进程。

**解决**：`cmd.kill_on_drop(true)` —— 确保 tokio Command 在 drop 时发送 SIGKILL。

**教训**：所有 spawn 的 Command 都要设置 `kill_on_drop(true)`。

## 总结

| # | 问题 | 严重程度 | 修复工作量 |
|---|------|---------|-----------|
| 1 | CLI 子进程卡死 | 致命（进程级 Hang） | 1 行 |
| 2 | WebSocket 静默断开 | 严重（消息丢失） | 50 行 |
| 3 | API 编辑次数限制 | 严重（流式中断） | 80 行 |
| 4 | Thinking signature 丢失 | 中等（API 400） | 30 行 |
| 5 | 双重序列化 | 中等（性能 & 安全性） | 20 行 |
| 6 | 字符串角色 | 低（运行时才能发现） | 10 行 |
| 7 | 进程清理 | 低（资源泄露） | 1 行 |

最棘手的是 #1 和 #2 —— 它们的特点是**不产生任何错误日志**，系统静默失效，排查极其困难。
