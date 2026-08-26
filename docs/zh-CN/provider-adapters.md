# Provider Adapter 维护指南

网页 DOM 是 WTAgent-RS 最容易变化的外部依赖。修改 Provider 时应遵循“稳定属性优先、结构信号优先、文本只做辅助”的原则。

## ChatGPT

- Base URL: `https://chatgpt.com/`
- Conversation: `/c/`
- Composer: `#prompt-textarea` 优先
- Assistant: `[data-message-author-role="assistant"]`
- User: `[data-message-author-role="user"]`
- Stable ID: `data-message-id`
- Completion: stop button / stable text
- Upload: `#upload-files`

模式切换优先读取 `data-testid / data-value / id`，显示文字作为回退。目标模式 disabled 时只能选择邻近可用回退，不应反复点击受限项。

## Claude

- Base URL: `https://claude.ai/`
- Conversation: `/chat/`
- Composer: `[data-testid="chat-input"]`
- Assistant: transcript row 中 `[data-is-streaming]`
- Completion: `data-is-streaming=false`
- Upload: `[data-testid="file-upload"]`

## DeepSeek

- Base URL: `https://chat.deepseek.com/`
- Conversation: `/a/chat/s/`
- Composer: `textarea[name="search"]`
- Assistant: `.ds-assistant-message-main-content` / `.ds-think-content`
- Identity: count baseline，而不是把 virtual-list key 当永久 ID
- Long thinking: 是

## Gemini

- Base URL: `https://gemini.google.com/app`
- Assistant: `model-response`
- User: `user-query`
- ID: `message-content-id-*`
- Completion: response action controls

## Kimi

- Base URL: `https://www.kimi.com/`
- Conversation: `/chat/`
- Composer: `.chat-input-editor`
- Stable ID: `data-archer-id`
- Completion: `.segment-assistant-actions`

## GLM / Z.ai

- Base URL: `https://chat.z.ai/`
- Conversation: `/c/`
- Composer: `#chat-input`
- Message ID: `message-<uuid>`
- Completion: copy/regenerate response actions

## 修改流程

1. 用合法账号手动确认当前 DOM；
2. 先寻找语义属性、ARIA、data-testid、自定义元素；
3. 再考虑稳定结构；
4. hashed/Tailwind class 只能最后兜底；
5. 确认新对话、已有对话、登录页、生成中、完成、错误、限制、challenge；
6. 增加/更新 fixture 或单元测试；
7. 不要在 Provider Adapter 中执行本地工具；
8. 不要加入反检测脚本。
