# 浏览器后端：ego-lite 与 Chrome/Chromium

WTAgent-RS 的网页 Provider 逻辑与浏览器传输层分离。当前支持两条路径：ego-lite 的 `ego-browser` Task Space Runtime，以及标准 Chrome/Chromium CDP。

## 自动选择

默认使用自动选择：

1. 如果显式传入 `--chrome-path`，使用 Chrome/Chromium；
2. macOS 上如果能在 `PATH` 或 `~/.local/bin/ego-browser` 找到 `ego-browser`，优先使用 ego-lite；
3. 否则回退到 Chrome/Chromium；
4. Linux/Windows 当前默认使用 Chrome/Chromium。ego-lite 的桌面发行目前以 macOS 为主。

运行 `wtagent doctor` 可以看到最终解析出的浏览器后端和可执行文件路径。

## ego-lite 模式

WTAgent-RS 不会把 ego-lite 当成 `chrome-path`，也不会等待 `DevToolsActivePort`。它直接调用官方 `ego-browser nodejs` Runtime，并通过其预加载的 `cdp()` / Task Space helper 与页面交互。

每个 Provider 使用一个稳定的 Task Space 名称，例如：

```text
wtagent-rs-chatgpt
wtagent-rs-claude
wtagent-rs-gemini
```

Task Space 与用户正常浏览窗口隔离，但可以继承 ego-lite 中已有的登录状态，因此通常不需要为 WTAgent-RS 再维护一套独立 Cookie/Profile。

安装 ego-lite 后需要先完成一次 GUI onboarding，使 `ego-browser` 注册到 PATH。常见位置是：

```text
~/.local/bin/ego-browser
```

如果 `wtagent doctor` 没有发现 ego-lite，可先检查：

```bash
command -v ego-browser
```

## Chrome/Chromium 模式

没有 ego-lite 时，WTAgent-RS 继续使用原生 Chrome DevTools Protocol：

- 为每个 Provider 建立独立的 `--user-data-dir`；
- 使用随机 remote-debugging port；
- 读取 `DevToolsActivePort`；
- 连接页面 WebSocket CDP endpoint。

需要强制使用某个 Chrome/Chromium 时可以传：

```bash
wtagent --chrome-path /path/to/chrome doctor
```

显式 `--chrome-path` 会覆盖 macOS 上的 ego-lite 自动优先策略。

## 登录、验证码与控制权

ego-lite 的主要优势是复用现有浏览器身份。建议先在 ego-lite 中完成对应 Provider 登录，再运行 WTAgent-RS。Chrome backend 则使用 WTAgent-RS 专用 Profile 手动登录。

无论使用哪种后端，WTAgent-RS 都不会自动填写账号密码、绕过 CAPTCHA、伪造浏览器指纹、轮换代理或账号。Provider 出现安全验证时，应由用户在真实浏览器界面中处理。

## 故障排查

### `Chrome/Chromium was not found`

macOS 上如果已经安装 ego-lite，请确认 onboarding 已完成，并且：

```bash
command -v ego-browser
```

能够返回路径。也可以重新打开终端，使 `~/.local/bin` 的 PATH 配置生效，然后运行：

```bash
wtagent doctor
```

### `ego-browser was not found`

这通常意味着 ego-lite 仅安装了 App，但 onboarding 尚未完成，或 `~/.local/bin` 不在当前 shell 的 PATH 中。

### Task Space 被用户控制

ego-lite 明确区分 Agent 与用户的控制权。用户在 GUI 中接管 Task Space 后，WTAgent-RS 不会偷偷夺回控制权；应先在界面中完成需要的人工操作并把控制权交回，再继续任务。

## 设计边界

Browser backend 只负责“如何把 CDP/页面操作送到浏览器”。Provider DOM selector、消息发送、回复等待、Rate Controller、Tool Protocol、权限策略和 Session 恢复仍然复用同一套 Rust 上层逻辑。因此增加 ego-lite 不会复制出第二套 ChatGPT/Claude/Gemini adapter。
