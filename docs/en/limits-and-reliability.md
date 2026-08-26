# Provider Limits and Reliability

## 1. What “triggers limits too easily” actually means

The right fix is not stealthier automation. Common engineering causes are:

1. agent tasks create far more chat turns than ordinary human usage because every local tool round-trip becomes another web message;
2. tool results are too large;
3. empty/server-error recovery immediately emits continuation messages and creates bursts;
4. sessions are constantly recreated or reauthenticated;
5. long-thinking providers are misclassified as dead requests;
6. explicit rate/usage limits are automatically retried.

WTAgent-RS targets those causes.

## 2. Explicit non-goals

WTAgent-RS does not implement CAPTCHA bypass, Cloudflare bypass, browser fingerprint spoofing/stealth patches, proxy or account rotation, multi-account request farming, automatic retries around plan usage limits, or other anti-detection techniques.

## 3. Send budget

Defaults:

```text
minimum interval     = 4000 ms
bounded jitter       = 0..1500 ms
rolling send ceiling = 6 / minute
base backoff          = 15 s
maximum backoff       = 15 min
```

Slow it down when appropriate:

```bash
wtagent --min-send-interval-ms 8000 --max-sends-per-minute 3 "..."
```

A web chat account should not be treated as a bulk API contract.

## 4. Read batching reduces round trips

Up to four read-only calls can be requested in one assistant turn and returned as one aggregate tool-result message. Writes and execution remain single operations. This reduces provider messages without weakening side-effect safety.

## 5. Byte budgets

Defaults:

- file read: 16 KiB;
- command/process tool output: 8 KiB;
- aggregate tool result: 28 KiB;
- outbound browser message: 48 KiB.

Oversized results are deterministically compacted. The model must ask for narrower reads/searches if it needs more detail.

## 6. Outcome handling

**Success:** lowers the in-memory penalty level.

**Generation failure:** may apply a soft backoff. WTAgent-RS avoids endless continuation retries.

**Rate limited:** phrases such as `too many requests`, `rate limit`, `try again later`, or localized equivalents stop the current run. Resume later.

**Usage limit:** account/plan usage-limit messages are hard stops. No account switching or automated attempts to get around the limit.

**Challenge:** when no usable composer exists and the page shows security/human-verification signals, manual takeover is required.

## 7. Long-thinking providers

Claude, DeepSeek, Kimi, and GLM may be quiet for a long time. An aggressive “dead request” heuristic can incorrectly send a continuation while the model is still working. WTAgent-RS therefore prefers structural completion signals, slower polling for long-thinking providers, and no aggressive automatic dead-request nudge by default.

## 8. Recommended metrics

Track at least:

- provider messages per completed task;
- local tool calls per provider message;
- average read-batch size;
- result compaction rate;
- rate-limit events per 100 provider messages;
- usage-limit and challenge events;
- protocol retries;
- task success rate;
- median and p95 task duration.

A good optimization reduces provider messages and limit events without reducing task success.
