# Session 会话管理

WTAgent-RS 将每次任务的状态、事件、网页会话 URL 和副作用日志持久化到应用数据目录。`session` 命令组用于管理和恢复这些会话。

## 常用命令

```bash
# 默认列出最近 20 个 session
wtagent session

# 显式列出，可限制数量
wtagent session list
wtagent session list -n 10

# 机器可读输出
wtagent session list --format json

# 查看单个 session
wtagent session show <SESSION_ID>
wtagent session show <SESSION_ID> --format json

# 恢复指定 session
wtagent session resume <SESSION_ID>
wtagent session resume <SESSION_ID> "继续，并运行测试"

# 恢复当前项目最近更新的 session
wtagent session continue
wtagent session continue "继续排查刚才的问题"

# 删除本地 session 状态和事件日志
wtagent session delete <SESSION_ID>
```

旧的 `wtagent sessions` 暂时保留兼容，但推荐迁移到 `wtagent session list`。

## 为什么 `wtagent session` 不再启动 Agent 任务

早期 CLI 允许 `wtagent "task"` 省略 `run`。未知子命令会被兜底解释成任务文本，因此输入 `wtagent session` 时实际上会创建一个任务内容为 `session` 的新会话，然后进入网页 Agent runtime，看起来像命令卡住。

现在 `session` 是正式命令组，裸 `wtagent session` 只做会话管理，不会发送任何网页模型消息。

## 与 OpenCode 的设计关系

实现参考了 OpenCode 的会话生命周期分层：CLI 负责 list/delete 等持久会话管理，交互入口负责 continue/resume。WTAgent-RS 当前优先实现最实用的 `list/show/resume/continue/delete`；TUI session picker、fork、abort 等能力后续可独立扩展，而不需要改变现有 session state 格式。

## 数据位置

```text
<app-data>/wtagent-rs/
  sessions/
    <session-id>/
      state.json
      events.jsonl
```

`delete` 会删除对应 session 目录。`continue` 只在当前 `-C/--project` 解析后的项目根目录中选择最近更新的 session，避免误恢复其他项目的会话。
