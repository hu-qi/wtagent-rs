# Contributing / 贡献指南

## 中文

欢迎提交 Provider 适配、跨平台修复、协议/工具安全改进和文档。请遵循：

1. 不加入 CAPTCHA 绕过、stealth/fingerprint spoofing、代理池/账号轮换等规避机制。
2. Provider DOM 修改必须尽量使用稳定语义属性，并说明验证过的页面状态。
3. 本地工具必须经过 `PolicyEngine`；不要从 Browser Adapter 直接执行命令或写文件。
4. 新增副作用工具需要考虑崩溃后的 at-most-once 恢复语义。
5. 尽量减少 Provider 消息数；只读操作优先支持聚合而不是新增网页往返。
6. 提交前运行：

```bash
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets --all-features
cargo doc --no-deps
```

PR 请写清：问题、设计取舍、验证方式、对 Provider 请求数/限制风险的影响。

## English

Contributions are welcome for provider adapters, cross-platform behavior, protocol/tool safety, and documentation.

1. Do not add CAPTCHA bypass, stealth/fingerprint spoofing, proxy/account rotation, or similar evasion mechanisms.
2. Provider DOM changes should prefer stable semantic attributes and document which page states were verified.
3. Local tools must remain behind `PolicyEngine`; browser adapters must not execute commands or write files.
4. New side-effect tools must define crash/replay behavior consistent with at-most-once recovery.
5. Prefer fewer provider messages; batch read-only local work instead of adding web round trips.
6. Run the commands above before opening a PR.

Describe the problem, trade-offs, verification, and expected effect on provider-message volume and limit risk.
