# WTAgent-RS

> A Rust reimplementation of [XiXian42/wtagent](https://github.com/XiXian42/wtagent): use your own Web AI session as the reasoning layer of a local coding agent, while a local Rust runtime owns tools, permissions, state, recovery, and execution.
>
> `WTAgent-RS` is an independent Rust implementation. It preserves the MIT attribution of the upstream project and focuses on reliability, lower web-turn volume, cross-platform binaries, and conservative handling of provider limits.

[中文文档](./README.zh-CN.md) · [English Documentation](./README.en.md) · [架构 / Architecture](./docs/zh-CN/architecture.md) · [限制与稳定性 / Limits & Reliability](./docs/zh-CN/limits-and-reliability.md)

## Why Rust / 为什么用 Rust

- Single native CLI binary; no Node.js runtime required.
- Strongly typed runtime boundaries for browser, protocol, policy, tools, and session state.
- Cross-platform CI for Linux, macOS, and Windows, plus MSRV, Clippy, rustfmt, docs, packaging, and security audit workflows.
- Crash-aware side-effect journal: a write/execute operation marked as started but not completed is **not replayed automatically** after interruption.
- Lower provider traffic: up to **4 read-only local tools** may be batched in one model turn and their results are returned in one web message.
- Conservative provider pacing and explicit limit handling. WTAgent-RS **does not** bypass CAPTCHAs, spoof browser fingerprints, rotate accounts, or retry around explicit plan/usage limits.

## Supported web providers / 支持的网页模型

| Provider | Web endpoint | Dedicated profile | Default behavior |
| --- | --- | --- | --- |
| ChatGPT | `chatgpt.com` | Yes | Keep current mode unless requested |
| Claude | `claude.ai` | Yes | Keep site-selected model |
| DeepSeek | `chat.deepseek.com` | Yes | Prefer Expert + Deep Thinking |
| Gemini | `gemini.google.com` | Yes | Keep site-selected model |
| Kimi | `kimi.com` | Yes | Prefer K3 |
| GLM / Z.ai | `chat.z.ai` | Yes | Prefer newest available GLM |

Web UIs change frequently. Provider DOM selectors are intentionally isolated in `src/browser/provider.rs` and `src/browser/adapter.rs` so a site change does not spread into the runtime or local tools.

## Quick start / 快速开始

```bash
cargo install --git https://github.com/hu-qi/wtagent-rs

mkdir demo && cd demo
wtagent "Create hello.txt, verify it, and summarize what changed"
```

Choose a provider:

```bash
wtagent --model claude "inspect this repository and summarize its architecture"
wtagent --model gemini "find the failing tests and explain the root cause"
```

Useful commands:

```bash
wtagent doctor
wtagent providers
wtagent login --model chatgpt
wtagent sessions
wtagent resume <SESSION_ID> "continue and run the tests"
```

The first run opens a dedicated visible Chrome/Chromium profile. Sign in manually. Credentials stay in the browser profile; WTAgent-RS does not ask for or store your provider password.

## Limit-safety design / 限制友好设计

The main optimization is **fewer and better web turns**, not evasion:

1. Read-only batching: max 4 local reads/searches/list/process reads per assistant turn.
2. One aggregate tool-result message per batch.
3. Deterministic result compaction under a byte budget.
4. Persistent provider Chrome profiles and resumable conversations; fewer new-chat/login cycles.
5. Default 4s + bounded jitter pacing and a rolling 6 sends/minute ceiling; both are configurable.
6. Transient rate-limit signals stop the current run instead of generating a retry burst.
7. Explicit account/plan usage-limit messages are a hard stop. Resume only after the provider resets or the user changes mode/plan manually.
8. Security challenges require manual takeover. No CAPTCHA bypass, stealth patches, fingerprint spoofing, proxy/account rotation, or hidden automation tricks.

See [中文：限制与稳定性](./docs/zh-CN/limits-and-reliability.md) or [English: Limits & Reliability](./docs/en/limits-and-reliability.md).

## Local tools / 本地工具

`fs.list`, `fs.read`, `fs.search`, `fs.write`, `fs.edit`, `terminal.exec`, `process.start`, `process.read`, `process.list`, `process.stop`.

Local tool requests from the web model are **requests, not authority**. The Rust runtime validates paths, tool shape, batch rules, approval mode, side-effect replay state, command environment, output budgets, and timeouts before execution.

## Status

This repository is a Rust rewrite rather than a drop-in byte-for-byte port. The architecture and interaction model are compatible with the upstream intent, while the browser automation implementation is intentionally smaller and more conservative. Because web-provider DOMs are external and mutable, live-provider compatibility should be treated as continuously maintained integration behavior rather than a permanent guarantee.

## License and attribution

MIT. See [LICENSE](./LICENSE) and [NOTICE.md](./NOTICE.md). The project is derived conceptually and behaviorally from `XiXian42/wtagent`; upstream MIT attribution is preserved.
