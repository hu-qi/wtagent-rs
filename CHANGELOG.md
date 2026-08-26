# Changelog

All notable changes to WTAgent-RS are documented here.

## [0.1.0] - 2026-08-26

### Added

- Initial independent Rust reimplementation of WTAgent.
- Direct Chrome DevTools Protocol browser driver with dedicated provider profiles.
- ChatGPT, Claude, DeepSeek, Gemini, Kimi, and GLM/Z.ai provider definitions.
- XML agent protocol with JSON args and compatibility parsing for nested XML args.
- Read-only tool batching (up to four calls) and aggregate tool-result feedback.
- Filesystem, terminal, and managed-process local tools.
- Project-root policy and ask/auto/read-only approvals.
- Crash-aware side-effect journal to avoid automatic replay of uncertain operations.
- Provider pacing, rolling send ceiling, explicit rate/usage limit handling, and manual challenge policy.
- Persistent sessions and JSONL event logs.
- Chinese and English documentation.
- Multi-platform CI, MSRV checks, security audit, dependency updates, and tagged release workflow.
