# ChatGPT Projects

WTAgent-RS 可以把**新建的 ChatGPT 会话直接创建在指定 ChatGPT Project 中**。Project 绑定只影响 ChatGPT Provider；Claude、Gemini 等 Provider 不使用该参数。

## 列出当前账号可见的 Projects

```bash
wtagent chatgpt projects
```

机器可读输出：

```bash
wtagent chatgpt projects --format json
```

该命令使用当前 WTAgent-RS 浏览器后端（macOS 上可自动使用 ego-lite），打开 `https://chatgpt.com/projects`，从已登录页面的 Project 链接中读取名称和稳定 URL。WTAgent-RS 不依赖未公开的 ChatGPT backend API。

## 按名称在 Project 中新建会话

```bash
wtagent --model chatgpt --chatgpt-project "OpenSource" \
  "分析当前项目并运行测试"
```

名称匹配为不区分 ASCII 大小写的精确匹配。如果存在重名 Project，WTAgent-RS 会拒绝猜测，并要求使用 URL。

## 按 URL 指定 Project

URL 是更稳定、推荐用于自动化或长期配置的方式：

```bash
wtagent --model chatgpt \
  --chatgpt-project "https://chatgpt.com/g/g-p-<project-id>-<slug>/project" \
  "继续实现当前功能"
```

只接受 `https://chatgpt.com` 且路径形态为：

```text
/g/g-p-.../project
```

不会接受普通 Chat URL、Share URL 或第三方域名。

## Session 行为

新任务创建时，Session State 会保存：

```json
{
  "chatgpt_project": {
    "name": "OpenSource",
    "url": "https://chatgpt.com/g/g-p-.../project",
    "project_id": "g-p-..."
  }
}
```

同时在第一次发送之前把 Project URL 作为初始 conversation URL。ChatGPT 在 Project 页面发送第一条消息后会建立该 Project 下的新 Chat；WTAgent-RS 随后保存实际 conversation URL。

因此：

```bash
wtagent session resume <SESSION_ID>
```

会继续已经创建的 Project Chat，**不会重新创建另一条会话**。

`--chatgpt-project` 只用于新 Session。对 `resume` 再传该参数会被拒绝，避免把一个已存在的 WTAgent Session 悄悄迁移到另一个 Project。

查看绑定：

```bash
wtagent session show <SESSION_ID>
```

会输出 `chatgpt_project`、`chatgpt_project_id`、`chatgpt_project_url` 和实际 `conversation`。

## 浏览器与安全边界

- 使用已有的 ego-lite Task Space 或 WTAgent-RS Chrome Profile；
- 不读取或保存账号密码；
- 不调用未公开的 Project 创建/修改 API；
- 不自动绕过 CAPTCHA、Cloudflare 或其他安全验证；
- Project 名称解析失败时不会自动退回普通 Chat，避免任务进入错误上下文。

ChatGPT 网页 DOM 会变化。如果 `wtagent chatgpt projects` 无法发现已有 Project，请使用 Project URL 直接指定，并用 `--debug` 收集诊断信息。
