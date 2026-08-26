# Security Policy / 安全策略

## Supported versions / 支持版本

Security fixes target the latest released minor version and the current `main` branch.

## Reporting / 漏洞报告

Please use GitHub's private vulnerability reporting for this repository when available. Do not publish credentials, cookies, browser-profile data, or exploit details in a public issue before a fix is available.

如发现目录穿越、命令注入、敏感环境变量泄漏、浏览器 Profile 泄漏、Tool Call 未经审批执行、副作用重复执行等问题，请优先通过 GitHub Private Vulnerability Reporting 私下提交，不要把 Cookie、Token、浏览器 Profile 或可直接利用的细节发到公开 Issue。

## Security model / 安全模型

The web model is not a trusted principal. Its tool XML is a request only. Local authority remains with the Rust runtime.

网页模型不是受信任主体，网页回复里的 Tool Call 只是请求。安全边界包括：

- project-root path guard；
- no absolute/project-external tool paths；
- side-effect approval modes；
- `program + argv` command model instead of arbitrary shell text；
- sensitive environment filtering by default；
- bounded file/command/web-message sizes；
- crash-aware side-effect journal；
- dedicated browser profiles stored outside the project；
- manual handling of provider security challenges。

## Out of scope / 非目标

WTAgent-RS is not an anti-bot bypass product. CAPTCHA bypass, fingerprint spoofing, account rotation, proxy pools, and automation intended to defeat provider controls are out of scope.
