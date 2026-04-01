# CLI 与工作流升级设计

## 背景

`claude-code` 的一个核心优势，不是单点功能更强，而是把“计划、执行、审阅、恢复、查看进度”做成了连续工作流。相比之下，`nanobot-rs` 当前更像一个底层 runtime：能力分散存在，但工作流入口和用户心智还没有建立起来。

## 当前不足

### 命令面太薄

- 缺少 `sessions`、`resume`、`tasks`、`plan` 这类一眼能看懂的工作流命令
- `provider` 命令仍偏单一，不利于后续扩展不同 provider 的诊断能力

### 状态不可见

- `status` 只展示少量静态配置
- 看不到当前 session 列表、最近活动、后台任务、MCP 状态、channel 健康状态

### 交互缺乏阶段感

- 交互模式以简单读入/打印为主
- 没有明确的“当前在计划、执行、等待审批、后台处理中”之类的用户反馈

### 缺少恢复能力

- 虽然底层有 session 持久化，但 CLI 没有把“恢复工作”做成一等能力

## 对标亮点

适合借鉴的不是复杂全屏 UI，而是以下机制：

- `plan mode`：规划期和执行期分开
- `resume`：明确恢复会话
- `background tasks`：让子任务成为显式对象
- `intent/progress`：让用户知道系统在做什么
- `status/config/usage` 这类按视角组织的信息展示

## 设计目标

- 用最少的新交互，建立完整工作流心智
- 复用现有 session、message bus、agent loop 能力
- 先做命令和状态模型，再逐步考虑 richer TUI

## 方案设计

## 1. 扩展命令体系

建议新增以下命令组：

### `nanobot sessions`

- `sessions list`
- `sessions show <session-id>`
- `sessions delete <session-id>`
- `sessions compact <session-id>`

作用：

- 把底层 session 能力直接暴露给用户
- 作为后续 `resume` 和 `plan` 的基础设施

### `nanobot resume`

- `resume`：恢复最近一次当前 workspace 的会话
- `resume --session <id>`：恢复指定会话
- `resume --all`：显示所有 workspace 的最近会话

### `nanobot plan`

- `plan start [topic]`
- `plan show`
- `plan approve`
- `plan discard`

设计思路：

- 引入 session 级模式 `ExecutionMode::{Normal, Planning}`
- planning 状态下默认禁用写工具和高风险 shell
- planning 输出集中保存为 `plan.md` 或 session 内结构化 plan 数据

### `nanobot tasks`

- `tasks list`
- `tasks show <task-id>`
- `tasks cancel <task-id>`

作用：

- 统一承载后台子代理、长时间工具调用、cron 派生任务等“异步工作项”

### `nanobot doctor`

- 检查配置、provider 凭证、workspace、MCP server 可用性、channel 配置

## 2. 统一状态模型

引入统一状态对象，供 CLI、gateway 和后续 TUI 共享：

- `RuntimeStatus`
- `SessionSummary`
- `TaskSummary`
- `ProviderHealth`
- `ToolPolicyState`

这层只负责“可展示状态”，不直接持有复杂业务逻辑。

## 3. 交互式 CLI 升级

第一阶段不做重型全屏界面，先补以下能力：

- 历史记录
- 多行输入
- slash commands 自动补全
- 系统事件的可读化展示
- 当前 intent / step / active task 提示

推荐方式：

- 保持标准终端兼容
- 将“结构化状态”和“输入体验”分离
- 后续若引入 TUI，可直接消费同一状态模型

## 4. Plan Mode 设计

### 行为约束

- 默认只允许只读工具
- shell 仅允许低风险读操作，或直接禁用
- 明确显示“当前为规划模式”
- 退出规划模式时要求显式确认

### 数据模型

- `SessionMode`
- `PlanState`
- `PlanApprovalState`

### 用户收益

- 降低 agent 在理解阶段就开始改文件的概率
- 让长任务先拆解，再执行

## 5. 事件与进度展示

把当前已有的“流式输出”和“工具提示”提升成统一事件：

- `IntentChanged`
- `ToolQueued`
- `ToolStarted`
- `ToolFinished`
- `TaskSpawned`
- `TaskCompleted`
- `ApprovalRequested`

CLI 层只负责把这些事件变成人类可读输出。

## 分阶段实施

## 第一阶段

- 增加 `sessions list/show`
- 增强 `status`
- 增加 `resume`
- 定义统一状态结构体

## 第二阶段

- 增加 `tasks list/show/cancel`
- 事件标准化
- CLI 历史、多行输入、slash command 补全

## 第三阶段

- 增加 plan mode
- 增加 `doctor`
- 视需求评估是否引入轻量 TUI 视图

## 风险与取舍

- 如果一开始就做复杂 TUI，会拖慢核心 workflow 落地
- 如果只做命令不做状态模型，后续又会重复造展示逻辑
- plan mode 过严会影响体验，过松又无法建立信任，需要以“默认安全、可显式放开”为原则

## 验收标准

- 用户能列出、查看、恢复历史会话
- 用户能看到后台任务和当前执行阶段
- 用户能显式进入/退出 planning 模式
- `status` 和 `doctor` 足以支持常见排障场景
