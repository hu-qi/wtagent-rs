# WTAgent-RS Architecture

## 1. Core model

WTAgent-RS treats the web model as an **untrusted, mutable text reasoning endpoint**. It may request local tools, but it never owns local authority. The Rust runtime decides whether a request is valid, allowed, approved, replayable, and executable.

```text
Terminal / User
      |
      v
   CLI Controller
      |
      v
   AgentRuntime ---------------- SessionStore (state.json + events.jsonl)
      |   |  \
      |   |   +-------------- RateController
      |   +------------------ PolicyEngine / Approval
      +---------------------- ToolExecutor
      |
      v
   WebAdapter
      |
      v
 Chrome DevTools Protocol (CDP)
      |
      v
 Dedicated Chrome Profile -> Web AI Provider
```

## 2. Module boundaries

`src/browser/` contains all external browser/provider uncertainty: provider metadata and selectors, Chrome discovery/profile management, the minimal CDP client, turn observation, auth/challenge/limit detection, and provider pacing.

`src/protocol.rs` converts between web text and structured events. It parses `<agent_response>`, supports JSON tool arguments plus common legacy nested XML arguments, builds bootstrap/follow-up prompts, and serializes aggregated tool results.

`src/tools.rs` implements filesystem, command, and managed-process tools. `terminal.exec` accepts a program plus argv instead of an arbitrary shell string. Sensitive environment variables are removed by default.

`src/policy.rs` enforces project-root boundaries, rejects absolute paths and parent traversal, canonicalizes existing targets, checks existing ancestors for new writes, and applies `ask`, `auto`, or `read-only` approval mode.

`src/session.rs` persists authoritative task state and an append-only event log. The state includes provider, project root, conversation URL, last assistant identity, current turn, phase, and side-effect journal.

`src/runtime.rs` is the sole orchestrator: browser lifecycle, auth, conversation start/resume, prompt sends, turn parsing, batch validation, policy, side-effect deduplication, local execution, result compaction, and completion.

## 3. Why direct CDP instead of Playwright

The Node.js upstream uses `playwright-core`. The Rust rewrite uses direct Chrome DevTools Protocol so the CLI remains a native Rust binary without a Node sidecar or bundled browser:

- reuse the user's installed Chrome/Chromium;
- connect to the dedicated profile's DevTools WebSocket;
- use `Runtime.evaluate`, `Input.insertText`, `DOM.setFileInputFiles`, and related CDP primitives;
- isolate provider DOM churn from the runtime.

The trade-off is that Locator/autowait behavior must be maintained locally. That is why provider DOM knowledge is kept behind the browser boundary.

## 4. Turn identity and completion

Provider message identity differs:

- ChatGPT: `data-message-id` / conversation turn;
- Claude: response row count + `data-is-streaming`;
- DeepSeek: assistant-row count;
- Gemini: `message-content-id-*`;
- Kimi: `data-archer-id`;
- GLM: `message-<uuid>`.

Before sending, WTAgent-RS captures an assistant baseline. A later node is accepted as the current response only if the count/identity/text boundary proves it is new. Completion also requires stable non-empty text, a finished structural signal where available, a complete protocol envelope when one has begun, and no provider challenge/limit state.

## 5. Read-only batching

One assistant turn may request up to four read-only tools. The runtime executes them locally and returns one aggregate result message. Any write/execute/process side effect must remain a single tool call.

This is the primary provider-load reduction mechanism: fewer web round trips without weakening side-effect approval or recovery semantics.

## 6. At-most-once side-effect recovery

A side-effect identity combines assistant message identity (or turn + raw response hash), call index, tool name, and canonical argument hash.

Before execution the session persists `Started`; after execution it persists `Completed` plus the result. Resume reuses completed results and refuses to automatically replay a merely-started operation whose completion is unknown.

## 7. Extension points

Future work includes provider DOM fixtures/live regression checks, SQLite sessions, a hardened Windows `.cmd/.bat` launcher, capability negotiation, OpenTelemetry, an optional TUI, and automated provider health checks.
