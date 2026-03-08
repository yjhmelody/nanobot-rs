# 代码搜索工具 Phase 1 实施计划

**文档版本**: 1.0  
**创建日期**: 2026-03-08  
**目标**: 实现基于 ripgrep 的快速文本搜索

---

## 1. Phase 1 目标

### 1.1 核心功能

**必须实现**:
1. ✅ `search_code` - 代码文本搜索
2. ✅ `search_docs` - 文档搜索
3. ✅ `grep_files` - 文件内容搜索（通用）

**可选功能**:
4. ⏳ `find_files` - 文件名搜索
5. ⏳ 结果缓存

### 1.2 技术选型

**搜索引擎**: ripgrep (rg)
- 速度极快（比 grep 快 10-100 倍）
- 支持 .gitignore
- JSON 输出格式
- Rust 编写，性能优秀

**集成方式**:
- 方案 1: 外部命令调用（简单，推荐）
- 方案 2: grep crate（纯 Rust，复杂）

---

## 2. 工具设计

### 2.1 search_code - 代码搜索

**功能**: 在代码库中搜索文本

**Tool Definition**:
```json
{
  "name": "search_code",
  "description": "Search for text in code files. Fast full-text search using ripgrep.",
  "parameters": {
    "query": {
      "type": "string",
      "description": "Search query (supports regex)"
    },
    "path": {
      "type": "string",
      "description": "Directory or file to search (optional, defaults to workspace)"
    },
    "case_sensitive": {
      "type": "boolean",
      "description": "Case sensitive search (default: false)"
    },
    "regex": {
      "type": "boolean",
      "description": "Treat query as regex (default: false)"
    },
    "file_pattern": {
      "type": "string",
      "description": "File pattern to include (e.g., '*.rs', '*.py')"
    },
    "limit": {
      "type": "number",
      "description": "Maximum results (default: 50)"
    }
  },
  "required": ["query"]
}
```

**返回格式**:
```json
{
  "results": [
    {
      "file": "src/main.rs",
      "line": 42,
      "column": 10,
      "match": "fn main() {",
      "context_before": ["use std::env;", ""],
      "context_after": ["    let args = env::args();", "}"]
    }
  ],
  "total": 15,
  "truncated": false,
  "search_time_ms": 45
}
```

### 2.2 search_docs - 文档搜索

**功能**: 搜索文档文件（.md, .txt, .rst 等）

**Tool Definition**:
```json
{
  "name": "search_docs",
  "description": "Search in documentation files (markdown, text, etc.)",
  "parameters": {
    "query": {
      "type": "string",
      "description": "Search query"
    },
    "path": {
      "type": "string",
      "description": "Directory to search (optional)"
    },
    "limit": {
      "type": "number",
      "description": "Maximum results (default: 30)"
    }
  },
  "required": ["query"]
}
```

**返回格式**:
```json
{
  "results": [
    {
      "file": "docs/README.md",
      "line": 10,
      "match": "## Installation",
      "context": "To install nanobot-rs, run:\n\n```bash\ncargo install nanobot-rs\n```"
    }
  ],
  "total": 5
}
```

### 2.3 grep_files - 通用文件搜索

**功能**: 通用的文件内容搜索（底层实现）

**Tool Definition**:
```json
{
  "name": "grep_files",
  "description": "Generic file content search with advanced options",
  "parameters": {
    "query": {
      "type": "string",
      "description": "Search query"
    },
    "path": {
      "type": "string",
      "description": "Path to search"
    },
    "include": {
      "type": "array",
      "items": {"type": "string"},
      "description": "File patterns to include"
    },
    "exclude": {
      "type": "array",
      "items": {"type": "string"},
      "description": "File patterns to exclude"
    },
    "case_sensitive": {
      "type": "boolean"
    },
    "regex": {
      "type": "boolean"
    },
    "context_lines": {
      "type": "number",
      "description": "Lines of context (default: 2)"
    },
    "limit": {
      "type": "number"
    }
  },
  "required": ["query"]
}
```

---

## 3. 实现方案

### 3.1 目录结构

```
src/tools/
├── search/
│   ├── mod.rs           # 模块导出
│   ├── code.rs          # SearchCodeTool
│   ├── docs.rs          # SearchDocsTool
│   ├── grep.rs          # GrepFilesTool (底层)
│   ├── ripgrep.rs       # ripgrep 集成
│   └── types.rs         # 类型定义
└── mod.rs               # 添加 search 模块
```

### 3.2 核心实现

#### 3.2.1 ripgrep 集成

```rust
// src/tools/search/ripgrep.rs

use std::path::Path;
use std::process::Stdio;
use tokio::process::Command;
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub struct RipgrepMatch {
    pub path: String,
    pub line_number: u32,
    pub column: u32,
    pub text: String,
}

pub struct RipgrepSearcher {
    rg_path: String,
}

impl RipgrepSearcher {
    pub fn new() -> Self {
        Self {
            rg_path: "rg".to_string(), // 假设 rg 在 PATH 中
        }
    }
    
    pub async fn search(
        &self,
        query: &str,
        path: &Path,
        options: &SearchOptions,
    ) -> Result<Vec<RipgrepMatch>> {
        let mut cmd = Command::new(&self.rg_path);
        
        // 基础参数
        cmd.arg("--json")           // JSON 输出
           .arg("--line-number")    // 显示行号
           .arg("--column")         // 显示列号
           .arg("--no-heading")     // 无标题
           .arg("--with-filename"); // 显示文件名
        
        // 上下文行数
        if options.context_lines > 0 {
            cmd.arg("-C").arg(options.context_lines.to_string());
        }
        
        // 大小写敏感
        if !options.case_sensitive {
            cmd.arg("--smart-case");
        }
        
        // 正则表达式
        if !options.regex {
            cmd.arg("--fixed-strings");
        }
        
        // 文件模式
        if let Some(pattern) = &options.file_pattern {
            cmd.arg("--glob").arg(pattern);
        }
        
        // 排除模式
        for exclude in &options.exclude_patterns {
            cmd.arg("--glob").arg(format!("!{}", exclude));
        }
        
        // 限制结果数
        if let Some(limit) = options.limit {
            cmd.arg("--max-count").arg(limit.to_string());
        }
        
        // 查询和路径
        cmd.arg(query).arg(path);
        
        // 执行
        let output = cmd
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .await?;
        
        // 解析 JSON 输出
        self.parse_output(&output.stdout)
    }
    
    fn parse_output(&self, output: &[u8]) -> Result<Vec<RipgrepMatch>> {
        let mut matches = Vec::new();
        
        for line in output.split(|&b| b == b'\n') {
            if line.is_empty() {
                continue;
            }
            
            // ripgrep JSON 格式
            #[derive(Deserialize)]
            struct RgLine {
                #[serde(rename = "type")]
                line_type: String,
                data: Option<RgData>,
            }
            
            #[derive(Deserialize)]
            struct RgData {
                path: Option<RgPath>,
                lines: Option<RgLines>,
                line_number: Option<u32>,
                absolute_offset: Option<u64>,
                submatches: Option<Vec<RgSubmatch>>,
            }
            
            #[derive(Deserialize)]
            struct RgPath {
                text: String,
            }
            
            #[derive(Deserialize)]
            struct RgLines {
                text: String,
            }
            
            #[derive(Deserialize)]
            struct RgSubmatch {
                #[serde(rename = "match")]
                match_text: RgMatchText,
                start: u32,
                end: u32,
            }
            
            #[derive(Deserialize)]
            struct RgMatchText {
                text: String,
            }
            
            let rg_line: RgLine = serde_json::from_slice(line)?;
            
            if rg_line.line_type == "match" {
                if let Some(data) = rg_line.data {
                    if let (Some(path), Some(lines), Some(line_num), Some(submatches)) = 
                        (data.path, data.lines, data.line_number, data.submatches) 
                    {
                        for submatch in submatches {
                            matches.push(RipgrepMatch {
                                path: path.text.clone(),
                                line_number: line_num,
                                column: submatch.start + 1, // 1-indexed
                                text: lines.text.clone(),
                            });
                        }
                    }
                }
            }
        }
        
        Ok(matches)
    }
}

#[derive(Debug, Default)]
pub struct SearchOptions {
    pub case_sensitive: bool,
    pub regex: bool,
    pub file_pattern: Option<String>,
    pub exclude_patterns: Vec<String>,
    pub context_lines: u32,
    pub limit: Option<u32>,
}
```

#### 3.2.2 SearchCodeTool 实现

```rust
// src/tools/search/code.rs

use std::collections::BTreeMap;
use async_trait::async_trait;
use serde::Deserialize;

use crate::error::{NanobotError, Result};
use crate::tools::base::{Tool, ToolContext, ToolDefinition, JsonSchema, JsonSchemaType};
use crate::tools::config::SharedToolConfig;
use crate::tools::search::ripgrep::{RipgrepSearcher, SearchOptions};

#[derive(Debug, Deserialize)]
struct SearchCodeRequest {
    query: String,
    path: Option<String>,
    case_sensitive: Option<bool>,
    regex: Option<bool>,
    file_pattern: Option<String>,
    limit: Option<u32>,
}

pub struct SearchCodeTool {
    config: SharedToolConfig,
    searcher: RipgrepSearcher,
}

impl SearchCodeTool {
    pub fn new(config: SharedToolConfig) -> Self {
        Self {
            config,
            searcher: RipgrepSearcher::new(),
        }
    }
}

#[async_trait]
impl Tool for SearchCodeTool {
    fn name(&self) -> &str {
        "search_code"
    }
    
    fn definition(&self) -> ToolDefinition {
        let mut properties = BTreeMap::new();
        
        properties.insert(
            "query".to_string(),
            JsonSchema {
                schema_type: JsonSchemaType::String,
                description: Some("Search query (supports regex if regex=true)".to_string()),
                properties: BTreeMap::new(),
                required: Vec::new(),
                enum_values: None,
                items: None,
                minimum: None,
                maximum: None,
            },
        );
        
        properties.insert(
            "path".to_string(),
            JsonSchema {
                schema_type: JsonSchemaType::String,
                description: Some("Directory or file to search (optional, defaults to workspace)".to_string()),
                properties: BTreeMap::new(),
                required: Vec::new(),
                enum_values: None,
                items: None,
                minimum: None,
                maximum: None,
            },
        );
        
        properties.insert(
            "case_sensitive".to_string(),
            JsonSchema {
                schema_type: JsonSchemaType::Boolean,
                description: Some("Case sensitive search (default: false)".to_string()),
                properties: BTreeMap::new(),
                required: Vec::new(),
                enum_values: None,
                items: None,
                minimum: None,
                maximum: None,
            },
        );
        
        properties.insert(
            "regex".to_string(),
            JsonSchema {
                schema_type: JsonSchemaType::Boolean,
                description: Some("Treat query as regex (default: false)".to_string()),
                properties: BTreeMap::new(),
                required: Vec::new(),
                enum_values: None,
                items: None,
                minimum: None,
                maximum: None,
            },
        );
        
        properties.insert(
            "file_pattern".to_string(),
            JsonSchema {
                schema_type: JsonSchemaType::String,
                description: Some("File pattern to include (e.g., '*.rs', '*.py')".to_string()),
                properties: BTreeMap::new(),
                required: Vec::new(),
                enum_values: None,
                items: None,
                minimum: None,
                maximum: None,
            },
        );
        
        properties.insert(
            "limit".to_string(),
            JsonSchema {
                schema_type: JsonSchemaType::Number,
                description: Some("Maximum results (default: 50)".to_string()),
                properties: BTreeMap::new(),
                required: Vec::new(),
                enum_values: None,
                items: None,
                minimum: Some(1.0),
                maximum: Some(1000.0),
            },
        );
        
        ToolDefinition::function(
            self.name(),
            "Search for text in code files. Fast full-text search using ripgrep. \
             Use this to find code patterns, function calls, variable usage, etc.",
            JsonSchema {
                schema_type: JsonSchemaType::Object,
                description: None,
                properties,
                required: vec!["query".to_string()],
                enum_values: None,
                items: None,
                minimum: None,
                maximum: None,
            },
        )
    }
    
    async fn execute(&self, args: &str, _context: &ToolContext) -> Result<String> {
        let req: SearchCodeRequest = serde_json::from_str(args)
            .map_err(|e| NanobotError::invalid_tool_args(
                self.name(),
                format!("Failed to parse arguments: {}", e)
            ))?;
        
        // 获取搜索路径
        let snapshot = self.config.snapshot().await;
        let search_path = if let Some(path) = req.path {
            snapshot.workspace.join(path)
        } else {
            snapshot.workspace.clone()
        };
        
        // 构建搜索选项
        let mut options = SearchOptions {
            case_sensitive: req.case_sensitive.unwrap_or(false),
            regex: req.regex.unwrap_or(false),
            file_pattern: req.file_pattern,
            context_lines: 2,
            limit: Some(req.limit.unwrap_or(50)),
            ..Default::default()
        };
        
        // 默认排除模式
        options.exclude_patterns = vec![
            "node_modules".to_string(),
            "target".to_string(),
            ".git".to_string(),
            "*.lock".to_string(),
        ];
        
        // 执行搜索
        let matches = self.searcher.search(&req.query, &search_path, &options)
            .await
            .map_err(|e| NanobotError::tool_execution(self.name(), e))?;
        
        // 格式化结果
        let result = serde_json::json!({
            "results": matches.iter().map(|m| {
                serde_json::json!({
                    "file": m.path,
                    "line": m.line_number,
                    "column": m.column,
                    "match": m.text,
                })
            }).collect::<Vec<_>>(),
            "total": matches.len(),
            "truncated": matches.len() >= options.limit.unwrap_or(50) as usize,
        });
        
        Ok(serde_json::to_string_pretty(&result)?)
    }
}
```

---

## 4. 测试计划

### 4.1 单元测试

```rust
#[cfg(test)]
mod tests {
    use super::*;
    
    #[tokio::test]
    async fn test_search_code_basic() {
        let config = SharedToolConfig::default();
        let tool = SearchCodeTool::new(config);
        
        let args = r#"{"query": "fn main"}"#;
        let result = tool.execute(args, &ToolContext::default()).await;
        
        assert!(result.is_ok());
    }
    
    #[tokio::test]
    async fn test_search_code_with_pattern() {
        let config = SharedToolConfig::default();
        let tool = SearchCodeTool::new(config);
        
        let args = r#"{
            "query": "struct",
            "file_pattern": "*.rs",
            "limit": 10
        }"#;
        
        let result = tool.execute(args, &ToolContext::default()).await;
        assert!(result.is_ok());
    }
}
```

### 4.2 集成测试

```bash
# 测试 1: 基础搜索
nanobot-rs agent -m "搜索项目中所有包含 'Config' 的代码"

# 测试 2: 正则搜索
nanobot-rs agent -m "用正则表达式搜索所有 'fn.*main' 的函数"

# 测试 3: 文件模式
nanobot-rs agent -m "在所有 Rust 文件中搜索 'async fn'"

# 测试 4: 文档搜索
nanobot-rs agent -m "在文档中搜索 'installation' 相关内容"
```

---

## 5. 实施步骤

### Step 1: 创建模块结构（30 分钟）

```bash
mkdir -p src/tools/search
touch src/tools/search/{mod.rs,code.rs,docs.rs,grep.rs,ripgrep.rs,types.rs}
```

### Step 2: 实现 ripgrep 集成（2 小时）

```bash
# 实现 src/tools/search/ripgrep.rs
# 实现 src/tools/search/types.rs
```

### Step 3: 实现 SearchCodeTool（2 小时）

```bash
# 实现 src/tools/search/code.rs
```

### Step 4: 实现 SearchDocsTool（1 小时）

```bash
# 实现 src/tools/search/docs.rs
```

### Step 5: 注册工具（30 分钟）

```bash
# 修改 src/tools/mod.rs
# 修改 src/tools/registry.rs
```

### Step 6: 测试（2 小时）

```bash
cargo test --lib search
cargo check
```

### Step 7: 文档（1 小时）

```bash
# 更新 README.md
# 添加使用示例
```

**总计**: 约 9 小时（1 天）

---

## 6. 依赖检查

### 6.1 外部依赖

**ripgrep**:
```bash
# 检查是否安装
which rg

# 如果未安装
# macOS
brew install ripgrep

# Linux
apt install ripgrep  # Debian/Ubuntu
dnf install ripgrep  # Fedora

# Windows
choco install ripgrep
```

### 6.2 Rust 依赖

```toml
# Cargo.toml

[dependencies]
# 已有依赖
tokio = { version = "1", features = ["process"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
```

无需新增依赖 ✅

---

## 7. 配置

### 7.1 工具配置

```toml
# config.toml

[tools.search]
enabled = true

[tools.search.ripgrep]
path = "rg"  # ripgrep 路径
defaultLimit = 50
defaultContextLines = 2
excludePatterns = [
  "node_modules",
  "target",
  ".git",
  "*.lock",
  "dist",
  "build"
]
```

---

## 8. 预期效果

### 8.1 性能指标

| 项目规模 | 文件数 | 搜索时间 |
|----------|--------|----------|
| 小 | < 1K | < 50ms |
| 中 | < 10K | < 200ms |
| 大 | < 100K | < 1s |
| 超大 | > 100K | < 3s |

### 8.2 功能覆盖

- ✅ 文本搜索
- ✅ 正则搜索
- ✅ 文件模式过滤
- ✅ 大小写控制
- ✅ 结果限制
- ✅ 上下文显示

---

## 9. 总结

### 9.1 Phase 1 交付物

**代码**:
- `src/tools/search/` 模块（约 500 行）
- 3 个工具：search_code, search_docs, grep_files

**文档**:
- 设计文档
- 实施计划
- 使用示例

**测试**:
- 单元测试
- 集成测试

### 9.2 下一步

**Phase 2** (2 周后):
- tree-sitter 集成
- 符号搜索
- 引用查找

---

**状态**: ✅ 计划完成  
**预计时间**: 1 天（9 小时）  
**难度**: 中等  
**优先级**: 高
