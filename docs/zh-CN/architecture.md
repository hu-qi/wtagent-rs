# WTAgent-RS 架构设计

## 1. 设计结论

WTAgent-RS 把网页模型视为一个**非受信任、可变的文本推理端**。网页模型可以提出本地 Tool Call，但它没有本地权限；真正决定“能不能执行、执行什么、是否需要审批、是否允许重放”的永远是 Rust Runtime。

```text
Terminal / User
      |
      v
   CLI Controller
      |
      v
   AgentRuntime ---------------- SessionStore (state.json + events.jsonl)
      |   |  \
      |   |   +-------------- RateController
      |   +------------------ PolicyEngine / Approval
      +---------------------- ToolExecutor
      |
      v
   WebAdapter
      |
      v
 Chrome DevTools Protocol (CDP)
      |
      v
 Dedicated Chrome Profile -> Web AI Provider
```

## 2. 模块边界

### `src/browser/`

负责所有 Provider 与浏览器外部不确定性：

- `provider.rs`：Provider URL、Profile 名、DOM selector、默认模式、完成信号；
- `chrome.rs`：Chrome/Chromium 发现、专用 Profile、DevToolsActivePort、Tab 复用；
- `cdp.rs`：最小 CDP WebSocket Client；
- `adapter.rs`：登录、会话、输入、发送、本轮 assistant 识别、完成判断、限制/挑战检测；
- `throttle.rs`：发送节奏、滚动窗口、退避状态。

核心 Runtime 不依赖 ChatGPT/Claude 的 DOM 细节。

### `src/protocol.rs`

负责网页文本与结构化事件之间转换：

- `<agent_response>` 解析；
- JSON Tool Args；
- 兼容上游常见的嵌套 XML Args；
- Bootstrap Prompt；
- Tool Result 聚合序列化；
- 协议错误的短反馈。

### `src/tools.rs`

内置工具：

- `fs.list`
- `fs.read`
- `fs.search`
- `fs.write`
- `fs.edit`
- `terminal.exec`
- `process.start`
- `process.read`
- `process.list`
- `process.stop`

`terminal.exec` 使用 `program + argv`，不把模型输出直接拼成 Shell 字符串。默认命令环境会剔除常见 Token/Secret/Password/Cookie 等敏感变量，除非用户明确批准继承。

### `src/policy.rs`

处理项目根目录策略：

- 拒绝绝对路径；
- 拒绝 `..`；
- 对已存在路径做 canonicalize；
- 写新文件前检查最近的已存在祖先目录，阻止 symlink 把新路径导出项目；
- 审批模式：`ask` / `auto` / `read-only`。

### `src/session.rs`

Session 是长任务恢复的权威数据：

```json
{
  "session_id": "...",
  "provider": "chatgpt",
  "project_root": "...",
  "conversation_url": "...",
  "last_assistant_id": "...",
  "turn": 8,
  "phase": "waiting_model",
  "effects": {}
}
```

同时写 `events.jsonl`，用于排查：

- 什么时候发了网页消息；
- 什么时候收到 Provider 限制；
- Tool Call 是否提出/执行/完成；
- Session 在哪个 Turn 中断。

### `src/runtime.rs`

AgentRuntime 是唯一流程编排者：

1. 启动/复用 Provider Chrome；
2. 检查登录；
3. 创建或恢复网页会话；
4. 发送 Bootstrap / Follow-up；
5. 等待完整 assistant turn；
6. 解析 XML；
7. 校验批量 Tool Call 规则；
8. 权限与副作用去重；
9. 本地执行；
10. 压缩并聚合 Tool Result；
11. 继续直到 `done=true` 或把控制权还给用户。

## 3. 为什么不再依赖 Playwright

上游 Node.js 实现使用 `playwright-core`。Rust 生态没有完全等价且零额外 Browser Bundle 的官方 Playwright Rust 实现，因此本项目直接使用 Chrome DevTools Protocol：

- 只依赖用户本机 Chrome/Chromium；
- 连接专用 Profile 的 DevTools WebSocket；
- 通过 `Runtime.evaluate`、`DOM.setFileInputFiles`、`Input.insertText` 等原语完成操作；
- 避免引入 Node Sidecar。

代价是 Locator/自动等待能力需要自行维护。因此 Provider 适配器有严格边界，DOM 变化只修改 browser 层。

## 4. 本轮身份与完成判定

不同 Provider 的 DOM 身份不同：

- ChatGPT：`data-message-id` / conversation turn；
- Claude：assistant row count + `data-is-streaming`；
- DeepSeek：assistant row count；
- Gemini：`message-content-id-*`；
- Kimi：`data-archer-id`；
- GLM：`message-<uuid>`。

发送前保存 assistant baseline，发送后只接受：

- assistant 数量增长；或
- 稳定 ID 变化；或
- Provider 会在原节点继续写入时，baseline 文本发生增长/变化。

完整性还需要满足：

- 文本非空；
- Provider 结构性“仍在生成”信号消失；
- 文本稳定超过窗口；
- 如果已经出现 `<agent_response`，优先等待 `</agent_response>`；
- 页面不是 challenge / usage-limit / rate-limit 状态。

## 5. 批量只读协议

模型可以一次请求：

```xml
<tool_calls>
  <tool_call name="fs.list"><args>{"path":"src","depth":2}</args></tool_call>
  <tool_call name="fs.read"><args>{"path":"Cargo.toml"}</args></tool_call>
  <tool_call name="fs.search"><args>{"query":"TODO","path":"src"}</args></tool_call>
</tool_calls>
```

Runtime 约束：

- 最多 4 个；
- 全部必须是 `Read`；
- 任一 `Write/Execute` 出现在 batch 中，整批拒绝并要求模型重发；
- 执行完成后只发一条 `<tool_results>`。

这是本项目降低 Provider 请求数的核心手段。

## 6. 副作用的 At-most-once 恢复

Side effect 的 Key 由以下信息决定：

- assistant message identity（没有稳定 ID 时使用 turn + raw response hash）；
- tool call index；
- tool name；
- canonicalized args hash。

执行流程：

```text
Tool Proposed
   |
Policy / Approval
   |
Persist Effect = Started
   |
Execute local side effect
   |
Persist Result + Effect = Completed
   |
Return result to web model
```

恢复时：

- `Completed`：复用结果，不重新执行；
- `Started` 且无结果：标记完成状态未知，不自动重放。

这更接近真实开发 Agent 需要的 at-most-once 语义，而不是“失败就重试”。

## 7. 后续扩展点

- Provider Adapter fixture 与真实浏览器回归录制；
- SQLite Session Store；
- 更精确的 Windows `.cmd/.bat` 安全启动层；
- Provider capability negotiation（附件、模式、结构化完成信号）；
- OpenTelemetry；
- 可选 TUI；
- Provider 页面变更自动健康检查。
