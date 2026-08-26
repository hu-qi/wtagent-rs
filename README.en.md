# WTAgent-RS English Documentation

WTAgent-RS is a Rust reimplementation of [XiXian42/wtagent](https://github.com/XiXian42/wtagent). A web AI product (ChatGPT, Claude, DeepSeek, Gemini, Kimi, or GLM) supplies reasoning; the local Rust runtime owns filesystem access, commands, long-running processes, policy, sessions, and recovery.

This is not a mechanical JavaScript-to-Rust translation. The rewrite targets the operational issues that matter most in long coding tasks: **too many web turns, provider rate/usage limits, side-effect replay after crashes, cross-platform packaging/CI, and provider DOM churn leaking into core agent logic**.

## Installation

```bash
cargo install --git https://github.com/hu-qi/wtagent-rs
```

Choose one browser path:

- on macOS, install [ego-lite](https://github.com/citrolabs/ego-lite), finish onboarding, and make sure `ego-browser` is available. WTAgent-RS will prefer it automatically and reuse existing browser login state;
- or install Chrome/Chromium. Linux and Windows currently use Chrome/Chromium by default.

A legitimate account and available quota for the selected provider are still required. No model API key is required, and using ego-lite does not require the user to install a separate Node.js runtime for WTAgent-RS.

Start with:

```bash
wtagent doctor
```

It reports whether the resolved browser backend is `ego` or `chrome`.

## Quick usage

```bash
mkdir wtagent-demo && cd wtagent-demo
wtagent "Create hello.txt, read it back, and verify the content"
```

Provider selection:

```bash
wtagent --model claude "summarize the architecture"
wtagent --model deepseek "find and fix the failing tests"
wtagent --model gemini "review this repository for engineering risks"
```

Login, diagnostics, and resume:

```bash
wtagent login --model chatgpt
wtagent doctor
wtagent providers
wtagent sessions
wtagent resume <SESSION_ID> "continue and run the complete test suite"
```

## Browser backends

### ego-lite

On macOS, when `ego-browser` is visible on `PATH` or at `~/.local/bin/ego-browser`, WTAgent-RS prefers ego-lite automatically. It does not pretend ego-lite is a Chrome executable; it invokes the official `ego-browser nodejs` runtime and uses Task Spaces plus raw `cdp()` calls.

Each provider gets a stable Task Space, for example:

```text
wtagent-rs-chatgpt
wtagent-rs-claude
```

Task Spaces are isolated from normal user tabs while inheriting login state available to ego-lite.

### Chrome / Chromium

When ego-lite is unavailable, WTAgent-RS keeps the original CDP backend with a dedicated provider profile, `DevToolsActivePort`, and a direct WebSocket CDP connection. Passing `--chrome-path` explicitly forces Chrome:

```bash
wtagent --chrome-path /path/to/chrome doctor
```

See [Browser Backends](./docs/en/browser-backends.md) for details.

## Key changes from the Node.js upstream

### Fewer provider turns

A coding task often needs `list -> read -> search -> read`. Sending one web message after every read amplifies provider traffic. WTAgent-RS allows up to **four read-only local tool calls in one assistant turn**, executes them locally, and sends one aggregate result message back.

Side effects — file writes, commands, process start/stop — remain single-call turns so approval and recovery semantics stay unambiguous.

### Bounded results

Tool output is byte-budgeted. Large output is deterministically compacted rather than dumping unlimited logs, dependency trees, or whole files into web chat.

### Conservative limit handling

Defaults:

- at least 4 seconds between outbound provider messages;
- small bounded jitter to avoid local-tool completion creating a burst pattern;
- no more than 6 outbound messages in a rolling minute;
- transient rate-limit notices stop the current run rather than triggering automatic retry storms;
- explicit plan/account usage limits are hard stops;
- Cloudflare/CAPTCHA/security challenges require manual takeover.

These controls are intended to reduce request density and respect provider limits. They are not anti-detection features.

### Crash-aware side effects

Write/execute operations are persisted as `Started` before execution and `Completed` after the result is durable. If WTAgent-RS crashes in between, resume refuses to automatically replay that operation and asks the model to inspect local state first.

## Approval modes

```bash
wtagent --approval ask "..."
wtagent --approval auto "..."
wtagent --approval read-only "..."
```

Path policy always rejects parent traversal and project-external absolute paths.

## Traffic controls

```bash
wtagent \
  --min-send-interval-ms 6000 \
  --max-sends-per-minute 4 \
  "analyze and repair this repository"
```

If an account has a small web quota, slow the pacing down. Web accounts should not be treated like bulk APIs.

## Attachments

```bash
wtagent run -f ./requirements.pdf -f ./notes.txt "analyze these requirements"
```

Attachment upload is best-effort and depends on the provider's current DOM.

## Session storage

Chrome mode stores dedicated provider profiles under the system application-data directory. ego-lite owns its browser data and Task Spaces. WTAgent-RS session state remains separate:

```text
wtagent-rs/
  profiles/<provider-profile>/
  sessions/<session-id>/
    state.json
    events.jsonl
```

Provider browser profiles live outside the project so cookies and login state are not accidentally committed.

## Platforms and CI

Target platforms:

- macOS: ego-lite or Chrome/Chromium;
- Linux: Chrome/Chromium;
- Windows 10/11: Chrome/Chromium;
- WSL2: best-effort because GUI Chrome availability depends on WSLg and the distribution.

CI covers native Ubuntu/macOS/Windows test jobs, rustfmt, Clippy, docs, MSRV, package checks, and security auditing. Release automation produces native binaries on version tags.

## More documentation

- [Browser Backends](./docs/en/browser-backends.md)
- [Architecture](./docs/en/architecture.md)
- [Limits & Reliability](./docs/en/limits-and-reliability.md)
- [Provider Adapter Maintenance](./docs/en/provider-adapters.md)
- [Security](./SECURITY.md)
- [Contributing](./CONTRIBUTING.md)
