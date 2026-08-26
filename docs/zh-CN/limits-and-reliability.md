# Provider 限制与稳定性设计

## 1. 问题定义

“容易触发限制”不能简单理解为“需要更隐蔽地自动化”。真实问题通常来自：

1. Agent 一次任务需要大量工具往返，导致网页消息数量远高于普通人工聊天；
2. Tool Result 过长，单轮输入量和页面压力过大；
3. 空回复/服务异常后立即连续发送 continuation，形成突发请求；
4. 每次任务频繁新开对话或重新登录；
5. 长思考 Provider 被错误判断为“死请求”，随后多发一条消息；
6. 遇到显式 rate/usage limit 后仍自动 retry。

WTAgent-RS 优先解决这些工程问题。

## 2. 不做什么

明确不实现：

- CAPTCHA 自动识别/绕过；
- Cloudflare challenge 绕过；
- 浏览器指纹伪装、`navigator.webdriver` stealth patch；
- 代理池、账号轮换、多账号并发刷请求；
- 检测到套餐用量上限后继续自动撞限；
- 隐藏 WTAgent 的真实本地执行行为来规避 Provider 管控。

这些做法既不稳定，也会把“可靠性优化”变成“规避平台控制”。

## 3. 发送预算

默认：

```text
minimum interval     = 4000 ms
bounded jitter       = 0..1500 ms
rolling send ceiling = 6 / minute
base backoff          = 15 s
maximum backoff       = 15 min
```

CLI 可以调慢：

```bash
wtagent --min-send-interval-ms 8000 --max-sends-per-minute 3 "..."
```

不建议调快。网页产品的交互额度和 API 吞吐不是同一类产品契约。

## 4. 批量只读降低轮次

假设一次任务需要：

```text
1. list src
2. read Cargo.toml
3. search TODO
4. read src/main.rs
5. edit src/main.rs
6. cargo test
```

传统单工具轮次大致需要 6 次 tool request + 6 次 result feedback。

WTAgent-RS 可以把前 4 个只读操作合并：

```text
Turn A: 4 read-only tool calls
Turn B: one aggregated result
Turn C: one edit request
Turn D: edit result
Turn E: cargo test request
Turn F: test result
```

网页往返显著下降，而且写入/执行仍保持单独审批。

## 5. Tool Result 字节预算

默认：

- 单文件 read：16 KiB；
- Tool Output：8 KiB；
- 聚合 Tool Result：28 KiB；
- 浏览器消息：48 KiB。

超过预算时做确定性压缩：保留 `name / ok / message`，把大 `data` 变成 bounded preview，并明确标记 `compacted=true`。模型需要更多内容时，应继续发起更窄的 `fs.read(offset=...)` 或 `fs.search`。

## 6. Provider 响应状态

### Success

正常完成，逐步降低内存中的 penalty level。

### Generation failure

用于服务端生成失败等短暂异常，可设置软 backoff。当前 Runtime 不做无限自动续发；失败会尽快交还用户。

### Rate limited

检测到：

- `too many requests`
- `rate limit`
- `try again later`
- `请求过于频繁`
- `操作过于频繁`

当前 Run 立即停止，不自动连发重试。用户稍后通过 `wtagent resume` 继续。

### Usage limit

检测到账号/套餐用量上限后硬停止。Runtime 不尝试自动切账号、自动刷新套餐或绕过限制。

### Challenge

页面没有 Composer 且出现安全验证/人机验证等特征时，要求人工完成。Challenge 文本只有在“没有可用 Composer”时才作为安全页面信号，避免模型正常回复里提到“verify you are human”造成误判。

## 7. 长思考与错误 continuation

DeepSeek、Claude、Kimi、GLM 可能长时间没有新 token。原方案最危险的误判之一是：模型其实还在思考，但 Agent 认为请求已死，于是主动发一条“continue”。

Rust 版默认更保守：

- 有可靠 structural signal 的 Provider 等待 signal 完成；
- long-thinking Provider 使用较低轮询频率；
- 不默认做 aggressive dead-request continuation；
- 只有完整、稳定的新 assistant turn 才进入协议解析。

这会让少量真实“死请求”更慢暴露，但能显著减少误发消息。

## 8. 如何评估优化是否有效

不要只看“有没有被限制”。建议记录：

- `provider_messages / completed_task`
- `local_tool_calls / provider_message`
- `aggregated_read_batch_size`
- `tool_result_compaction_rate`
- `rate_limit_events / 100 provider messages`
- `usage_limit_events`
- `challenge_events`
- `protocol_retry_count`
- `task_success_rate`
- `median/95p task duration`

理想优化应该同时做到：**每个任务网页消息更少、任务成功率不下降、限制事件降低**。
