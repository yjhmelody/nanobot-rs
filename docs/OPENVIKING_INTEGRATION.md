# OpenViking 集成说明

`nanobot-rs` 当前**未实现** OpenViking 集成。

这份文档的作用仅是明确当前状态，避免把历史设计草案误认为现有能力。

## 当前状态

目前仓库中的记忆系统仍以本地文件实现为主，相关能力包括：

- 基于工作区的长期记忆文件
- 历史记录追加
- `FileMemoryProvider`
- `CompositeMemoryProvider`
- 会话压缩与摘要生成

当前代码中**没有稳定可用的 OpenViking backend、配置项、运行时接入流程或用户可用命令**。

## 这意味着什么

如果你正在阅读这份文档，请以以下结论为准：

- 不能在配置中直接启用 OpenViking
- 没有官方支持的 OpenViking memory provider
- 没有保证兼容的 OpenViking API 接入层
- `QUICK_START`、`ARCHITECTURE`、`DEVELOPMENT` 中也不会把 OpenViking 视为当前功能

## 为什么保留这份文档

此前仓库中存在较长的 OpenViking 设计/集成草案，但它们容易造成误解：  
看起来像“已经支持”，实际上并没有完成落地。

因此这里将其收敛为一份简短说明，用来表达：

- 当前未实现
- 当前不承诺接口
- 当前不建议围绕它编写配置或集成代码

## 如果未来要支持

如果后续真的引入 OpenViking，这份文档应当只在以下内容已经落地后再扩写：

1. 明确的配置结构
2. 实际可用的 memory provider 实现
3. 错误处理与认证方式
4. 检索 / 写入行为说明
5. 对 `ARCHITECTURE` 和 `QUICK_START` 的同步更新

在这些内容真正进入代码之前，OpenViking 仍应被视为**未实现功能**。

## 相关文档

- `docs/README.md`
- `docs/ARCHITECTURE.md`
- `docs/QUICK_START.md`
- `docs/DEVELOPMENT.md`
