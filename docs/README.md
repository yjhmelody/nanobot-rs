# nanobot-rs 文档索引

本目录用于记录 `nanobot-rs`（Rust MVP）的源码级设计与实现边界。

## 对齐来源

以下文档是 Rust 侧设计的主要参考基线：

- Python 侧源码注释与实现
  - `nanobot/channels/manager.py`
  - `nanobot/agent/tools/base.py`
  - `nanobot/agent/tools/registry.py`
- 语言无关设计文档
  - `docs/source-level-design.md`
  - `docs/components/07-tools.md`
  - `docs/components/12-channels.md`
  - `docs/components/14-templates-and-bootstrap.md`

## 文档列表

- `nanobot-rs/docs/rust-mvp-design.md`
  - Rust 当前实现的组件划分、运行流程、能力边界、与 Python 对齐项
- `nanobot-rs/docs/channel-compatibility-roadmap.md`
  - Rust 对多渠道能力的现状、差距、最小可落地路线图

## 阅读建议

1. 先读 `rust-mvp-design.md`，明确当前 Rust 版本“已经实现到哪一层”。
2. 再读 `channel-compatibility-roadmap.md`，作为后续补齐 Telegram/Discord/Slack/Matrix 等渠道能力的执行清单。
