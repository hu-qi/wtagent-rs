# WTAgent-RS 中文文档

WTAgent-RS 是 [XiXian42/wtagent](https://github.com/XiXian42/wtagent) 的 Rust 重实现：让 ChatGPT、Claude、DeepSeek、Gemini、Kimi、GLM 等网页模型负责推理，本地 Rust Runtime 负责文件、命令、进程、权限、会话和恢复。

它不是简单把 JavaScript 翻译成 Rust，而是针对原方案最影响实际使用的几个问题重新设计：**网页轮次过多、限制/风控触发概率、长任务副作用重放、跨平台安装与 CI、浏览器适配器与 Agent 核心耦合**。

## 1. 安装

当前推荐从 GitHub 源码安装：

```bash
cargo install --git https://github.com/hu-qi/wtagent-rs
```

运行依赖：

- Chrome 或 Chromium；
- 对应网页模型的合法账号与可用额度；
- 首次使用时在 WTAgent-RS 专用浏览器 Profile 中手动登录。

运行时不要求 Node.js，也不要求模型 API Key。

## 2. 快速使用

```bash
mkdir wtagent-demo
cd wtagent-demo
wtagent "创建 hello.txt，写入 Hello from WTAgent-RS，然后读取并确认内容"
```

选择 Provider：

```bash
wtagent --model claude "分析当前项目架构"
wtagent --model deepseek "定位失败测试并修复"
wtagent --model gemini "检查这个项目有哪些明显的工程问题"
```

首次登录：

```bash
wtagent login --model chatgpt
```

诊断：

```bash
wtagent doctor
wtagent providers
wtagent sessions
```

恢复历史会话：

```bash
wtagent resume <SESSION_ID> "继续，并运行完整测试"
```

## 3. 与上游 Node.js 版本相比的核心变化

### 3.1 减少网页消息，而不是“绕限制”

复杂编码任务经常需要连续执行 `list -> read -> search -> read`。如果每个只读工具都单独发一次网页 Tool Result，网页模型请求数会快速膨胀。

WTAgent-RS 允许模型在**同一 assistant turn 中批量请求最多 4 个只读工具**，本地执行后把结果聚合成**一条**网页消息回传。写文件、执行命令、启动/停止进程等副作用操作仍然必须单独请求，避免审批和恢复语义变复杂。

### 3.2 结果压缩

网页 Tool Result 有明确字节预算。超出预算时，Runtime 做确定性的结构压缩和预览截断，不再把无界日志、大文件或完整依赖树直接塞回网页聊天。

### 3.3 限流与风控处理

默认策略：

- 两次发送至少间隔 4 秒；
- 附加有限随机抖动，避免本地工具瞬间完成时形成规则性突发；
- 滚动窗口默认最多 6 条网页消息/分钟；
- 识别 `Too many requests / rate limit / 请求过于频繁` 后停止当前 Run，不做自动连续重试；
- 识别账号/套餐 `usage limit` 后硬停止；
- 出现 Cloudflare、验证码、人机验证等 challenge 时交还人工处理。

这套机制的目标是降低请求密度、尊重 Provider 限制和减少误触发，不是规避平台安全机制。

### 3.4 副作用日志

文件写入、命令执行、进程启动/停止等操作在执行前会写入 Session State：

- `Started`：已经准备开始；
- `Completed`：结果已经持久化。

如果程序在 `Started` 后异常退出，下次恢复时同一副作用**不会自动重放**，而是返回“完成状态未知”，要求模型先检查本地状态。这比简单“重试上一次 Tool Call”更安全。

## 4. 审批模式

```bash
# 默认：只读自动，写入/执行询问
wtagent --approval ask "..."

# 自动批准项目内写入和命令执行
wtagent --approval auto "..."

# 严格只读
wtagent --approval read-only "..."
```

无论哪种模式，项目路径策略仍会阻止 `..` 穿越和项目外绝对路径。

## 5. 发送节奏配置

```bash
wtagent \
  --min-send-interval-ms 6000 \
  --max-sends-per-minute 4 \
  "分析并修复这个项目"
```

如果账号本身额度较小，建议主动把节奏调慢。不要把这些参数调成极低值去追求吞吐；网页账号不是批量 API。

## 6. 文件附件

新任务可附带文件：

```bash
wtagent run -f ./requirements.pdf -f ./notes.txt "结合附件分析需求"
```

附件上传依赖 Provider 当前页面是否暴露可控的 `input[type=file]`。上传失败是软失败；文本任务仍可继续。

## 7. Session 数据

默认数据目录遵循系统应用数据目录，内部结构类似：

```text
wtagent-rs/
  profiles/
    chrome-profile/
    claude-profile/
    ...
  sessions/
    <session-id>/
      state.json
      events.jsonl
```

浏览器 Profile 不放在项目目录，避免误提交 Cookie、登录态和浏览器缓存。

## 8. 跨平台

目标平台：

- macOS；
- Linux；
- Windows 10/11；
- WSL2 可运行，但 GUI Chrome 环境取决于发行版与 WSLg，属于 best-effort。

CI 在 Ubuntu、macOS、Windows 上运行 Rust build/test；发布工作流分别产出平台原生二进制。

## 9. 更多文档

- [架构设计](./docs/zh-CN/architecture.md)
- [限制与稳定性设计](./docs/zh-CN/limits-and-reliability.md)
- [Provider Adapter 维护](./docs/zh-CN/provider-adapters.md)
- [安全策略](./SECURITY.md)
- [贡献指南](./CONTRIBUTING.md)
