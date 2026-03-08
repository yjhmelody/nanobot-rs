# 代码和文档搜索工具设计

**文档版本**: 1.0  
**创建日期**: 2026-03-08  
**目标**: 设计高效的代码和文档搜索工具，支持 agent 快速定位和修改

---

## 1. 主流 Agents 搜索方案调研

### 1.1 Cursor IDE

**搜索能力**:
- **Semantic Search**: 基于语义的代码搜索
- **Symbol Search**: 函数、类、变量快速定位
- **Full-text Search**: 传统文本搜索
- **Codebase Indexing**: 自动索引整个代码库

**技术栈**:
- Tree-sitter: 语法解析
- Embeddings: 语义向量化
- SQLite: 本地索引存储

**特点**:
- ✅ 快速（毫秒级响应）
- ✅ 准确（理解代码语义）
- ✅ 增量更新（文件变化时自动更新索引）

### 1.2 Claude Code (Anthropic)

**搜索能力**:
- **Contextual Search**: 上下文感知搜索
- **Cross-file References**: 跨文件引用追踪
- **Documentation Search**: 文档和注释搜索
- **Pattern Matching**: 代码模式匹配

**技术栈**:
- AST Analysis: 抽象语法树分析
- Semantic Embeddings: 语义嵌入
- Graph Database: 代码关系图

**特点**:
- ✅ 理解代码结构
- ✅ 追踪依赖关系
- ✅ 智能推荐

### 1.3 Windsurf (Codeium)

**搜索能力**:
- **Multi-file Search**: 多文件并行搜索
- **Regex Search**: 正则表达式搜索
- **Type-aware Search**: 类型感知搜索
- **Dependency Graph**: 依赖关系图

**技术栈**:
- Ripgrep: 高性能文本搜索
- Language Servers: LSP 集成
- Custom Indexer: 自定义索引器

**特点**:
- ✅ 极快速度
- ✅ 大规模代码库支持
- ✅ 低内存占用

### 1.4 Cline (开源)

**搜索能力**:
- **Grep-based Search**: 基于 grep 的搜索
- **File Pattern Search**: 文件模式搜索
- **Simple Indexing**: 简单索引

**技术栈**:
- ripgrep: 文本搜索
- fd: 文件查找
- Basic caching: 基础缓存

**特点**:
- ✅ 简单可靠
- ✅ 无需复杂依赖
- ✅ 易于定制

---

## 2. 搜索工具分类

### 2.1 按搜索类型

| 类型 | 用途 | 速度 | 准确度 |
|------|------|------|--------|
| **Full-text** | 文本内容搜索 | ⭐⭐⭐⭐⭐ | ⭐⭐⭐ |
| **Semantic** | 语义相似搜索 | ⭐⭐⭐ | ⭐⭐⭐⭐⭐ |
| **Symbol** | 符号定义查找 | ⭐⭐⭐⭐⭐ | ⭐⭐⭐⭐⭐ |
| **Regex** | 模式匹配 | ⭐⭐⭐⭐ | ⭐⭐⭐⭐ |
| **AST** | 结构化搜索 | ⭐⭐⭐ | ⭐⭐⭐⭐⭐ |

### 2.2 按实现复杂度

| 方案 | 复杂度 | 依赖 | 效果 |
|------|--------|------|------|
| **ripgrep** | 低 | 外部命令 | 快速文本搜索 |
| **Tree-sitter** | 中 | Rust crate | 语法感知搜索 |
| **Embeddings** | 高 | ML 模型 | 语义搜索 |
| **LSP** | 中 | Language Server | 符号搜索 |

---

## 3. nanobot-rs 搜索工具设计

### 3.1 设计原则

**1. 渐进式实现**
- Phase 1: 基础文本搜索（ripgrep）
- Phase 2: 语法感知搜索（tree-sitter）
- Phase 3: 语义搜索（embeddings）

**2. 性能优先**
- 使用 Rust 原生工具
- 增量索引
- 缓存结果

**3. 易用性**
- 简单的 API
- 清晰的结果格式
- 智能排序

### 3.2 工具设计

#### 3.2.1 search_code - 代码搜索

**功能**: 在代码库中搜索指定内容

**参数**:
```json
{
  "query": "string",           // 搜索查询
  "type": "text|symbol|regex", // 搜索类型
  "path": "string?",           // 限制路径（可选）
  "language": "string?",       // 限制语言（可选）
  "limit": "number?"           // 结果数量（默认 20）
}
```

**返回**:
```json
{
  "results": [
    {
      "file": "src/main.rs",
      "line": 42,
      "column": 10,
      "match": "fn main() {",
      "context": "...",
      "score": 0.95
    }
  ],
  "total": 100,
  "truncated": true
}
```

#### 3.2.2 find_symbol - 符号查找

**功能**: 查找函数、类、变量定义

**参数**:
```json
{
  "symbol": "string",          // 符号名称
  "kind": "function|class|variable|all", // 符号类型
  "path": "string?"            // 限制路径（可选）
}
```

**返回**:
```json
{
  "definitions": [
    {
      "file": "src/lib.rs",
      "line": 10,
      "kind": "function",
      "name": "parse_config",
      "signature": "fn parse_config(path: &Path) -> Result<Config>",
      "doc": "Parse configuration from file"
    }
  ]
}
```

#### 3.2.3 find_references - 引用查找

**功能**: 查找符号的所有引用

**参数**:
```json
{
  "symbol": "string",          // 符号名称
  "file": "string",            // 定义所在文件
  "line": "number"             // 定义所在行
}
```

**返回**:
```json
{
  "references": [
    {
      "file": "src/main.rs",
      "line": 20,
      "context": "let config = parse_config(&path)?;"
    }
  ]
}
```

#### 3.2.4 search_docs - 文档搜索

**功能**: 搜索文档和注释

**参数**:
```json
{
  "query": "string",           // 搜索查询
  "include_comments": "boolean?", // 包含代码注释
  "path": "string?"            // 限制路径（可选）
}
```

**返回**:
```json
{
  "results": [
    {
      "file": "docs/README.md",
      "line": 5,
      "match": "## Installation",
      "context": "...",
      "type": "markdown"
    }
  ]
}
```

#### 3.2.5 list_symbols - 符号列表

**功能**: 列出文件或目录中的所有符号

**参数**:
```json
{
  "path": "string",            // 文件或目录路径
  "kind": "function|class|variable|all", // 符号类型
  "recursive": "boolean?"      // 递归子目录
}
```

**返回**:
```json
{
  "symbols": [
    {
      "file": "src/lib.rs",
      "name": "Config",
      "kind": "struct",
      "line": 10,
      "visibility": "pub"
    }
  ]
}
```

---

## 4. 实施方案

### 4.1 Phase 1: 基础文本搜索（1 周）

**目标**: 实现快速的文本搜索

**工具**:
- `search_code` (text mode)
- `search_docs`

**技术栈**:
- ripgrep (通过 `grep` crate 或外部命令)
- 简单的结果解析

**实现**:
```rust
// src/tools/search.rs

pub struct SearchCodeTool {
    config: SharedToolConfig,
}

impl SearchCodeTool {
    async fn search_text(&self, query: &str, path: Option<&Path>) -> Result<Vec<SearchResult>> {
        // 使用 ripgrep 搜索
        let output = Command::new("rg")
            .arg("--json")
            .arg(query)
            .arg(path.unwrap_or(Path::new(".")))
            .output()
            .await?;
        
        // 解析结果
        parse_ripgrep_output(&output.stdout)
    }
}
```

### 4.2 Phase 2: 语法感知搜索（2 周）

**目标**: 实现符号搜索和引用查找

**工具**:
- `find_symbol`
- `find_references`
- `list_symbols`

**技术栈**:
- tree-sitter: 语法解析
- 简单的符号索引

**实现**:
```rust
// src/tools/search/symbol.rs

use tree_sitter::{Parser, Language};

pub struct SymbolIndexer {
    parser: Parser,
    index: HashMap<String, Vec<SymbolInfo>>,
}

impl SymbolIndexer {
    pub fn index_file(&mut self, path: &Path) -> Result<()> {
        let source = fs::read_to_string(path)?;
        let tree = self.parser.parse(&source, None)?;
        
        // 遍历 AST 提取符号
        self.extract_symbols(tree.root_node(), path);
        Ok(())
    }
    
    pub fn find_symbol(&self, name: &str) -> Vec<&SymbolInfo> {
        self.index.get(name).map(|v| v.iter().collect()).unwrap_or_default()
    }
}
```

### 4.3 Phase 3: 语义搜索（3 周）

**目标**: 实现基于语义的搜索

**工具**:
- `search_code` (semantic mode)

**技术栈**:
- Embeddings model (如 CodeBERT)
- Vector database (如 qdrant)

**实现**:
```rust
// src/tools/search/semantic.rs

pub struct SemanticSearcher {
    embedder: Embedder,
    index: VectorIndex,
}

impl SemanticSearcher {
    pub async fn search(&self, query: &str, limit: usize) -> Result<Vec<SearchResult>> {
        // 1. 生成查询向量
        let query_vec = self.embedder.embed(query).await?;
        
        // 2. 向量搜索
        let results = self.index.search(&query_vec, limit)?;
        
        // 3. 返回结果
        Ok(results)
    }
}
```

---

## 5. 工具集成

### 5.1 注册到 ToolRegistry

```rust
// src/tools/mod.rs
pub mod search;

// src/tools/registry.rs
impl ToolRegistry {
    pub(crate) fn new(...) -> Self {
        // ... 现有工具 ...
        
        // 搜索工具
        let search_code_tool = Arc::new(SearchCodeTool::new(config.clone()));
        tools.insert(search_code_tool.name().to_string(), search_code_tool);
        
        let find_symbol_tool = Arc::new(FindSymbolTool::new(config.clone()));
        tools.insert(find_symbol_tool.name().to_string(), find_symbol_tool);
        
        // ...
    }
}
```

### 5.2 配置

```toml
# config.toml

[tools.search]
enabled = true
indexing = true           # 启用索引
indexPath = ".nanobot/index"  # 索引存储路径
maxResults = 50           # 最大结果数
excludePatterns = [       # 排除模式
  "node_modules",
  "target",
  ".git"
]

[tools.search.ripgrep]
path = "rg"               # ripgrep 路径
args = ["--json", "--smart-case"]

[tools.search.treesitter]
enabled = true
languages = ["rust", "python", "javascript", "typescript"]
```

---

## 6. 使用示例

### 6.1 文本搜索

```
用户: "在项目中搜索所有使用 'parse_config' 的地方"

Agent:
  ↓ 调用 search_code
  ↓ {
      "query": "parse_config",
      "type": "text",
      "limit": 20
    }
  ↓ 返回 15 个匹配结果
  ↓ 分析结果并回复用户
```

### 6.2 符号查找

```
用户: "找到 Config 结构体的定义"

Agent:
  ↓ 调用 find_symbol
  ↓ {
      "symbol": "Config",
      "kind": "class"
    }
  ↓ 返回定义位置
  ↓ 显示代码片段
```

### 6.3 引用查找

```
用户: "找到所有调用 execute 方法的地方"

Agent:
  ↓ 调用 find_references
  ↓ {
      "symbol": "execute",
      "file": "src/tools/base.rs",
      "line": 42
    }
  ↓ 返回所有引用
  ↓ 按文件分组显示
```

---

## 7. 性能优化

### 7.1 索引策略

**增量索引**:
- 监听文件变化
- 只更新修改的文件
- 后台异步索引

**缓存策略**:
- LRU 缓存搜索结果
- 缓存符号索引
- 定期清理过期缓存

### 7.2 并行处理

```rust
use rayon::prelude::*;

pub fn search_parallel(&self, query: &str, paths: &[PathBuf]) -> Result<Vec<SearchResult>> {
    let results: Vec<_> = paths
        .par_iter()
        .filter_map(|path| self.search_file(query, path).ok())
        .flatten()
        .collect();
    
    Ok(results)
}
```

---

## 8. 对比分析

### 8.1 方案对比

| 方案 | 速度 | 准确度 | 复杂度 | 推荐 |
|------|------|--------|--------|------|
| **ripgrep** | ⭐⭐⭐⭐⭐ | ⭐⭐⭐ | 低 | Phase 1 ✅ |
| **tree-sitter** | ⭐⭐⭐⭐ | ⭐⭐⭐⭐⭐ | 中 | Phase 2 ✅ |
| **embeddings** | ⭐⭐⭐ | ⭐⭐⭐⭐⭐ | 高 | Phase 3 ⏳ |

### 8.2 推荐实施顺序

1. **Phase 1** (1 周): ripgrep 文本搜索
   - 快速实现
   - 立即可用
   - 覆盖 80% 需求

2. **Phase 2** (2 周): tree-sitter 符号搜索
   - 提升准确度
   - 支持重构
   - 覆盖 95% 需求

3. **Phase 3** (3 周): 语义搜索
   - 最佳体验
   - 智能推荐
   - 覆盖 100% 需求

---

## 9. 总结

### 9.1 核心工具

**必须实现**（Phase 1）:
1. ✅ `search_code` - 代码搜索
2. ✅ `search_docs` - 文档搜索

**推荐实现**（Phase 2）:
3. ✅ `find_symbol` - 符号查找
4. ✅ `find_references` - 引用查找
5. ✅ `list_symbols` - 符号列表

**可选实现**（Phase 3）:
6. ⏳ 语义搜索
7. ⏳ 智能推荐

### 9.2 技术选型

**Phase 1**:
- ripgrep: 文本搜索
- 简单解析

**Phase 2**:
- tree-sitter: 语法解析
- 符号索引

**Phase 3**:
- Embeddings: 语义向量
- Vector DB: 向量存储

### 9.3 预期效果

**搜索速度**:
- 小项目（< 10K 文件）: < 100ms
- 中项目（< 100K 文件）: < 500ms
- 大项目（> 100K 文件）: < 2s

**准确度**:
- 文本搜索: 95%
- 符号搜索: 99%
- 语义搜索: 90%

---

**状态**: ✅ 设计完成  
**预计时间**: 6 周（Phase 1-3）  
**优先级**: Phase 1 (高), Phase 2 (中), Phase 3 (低)
