# Provider Adapter Maintenance

Web DOMs are WTAgent-RS's most volatile dependency. Prefer stable semantic attributes and structural completion signals; use localized text only as a secondary signal.

## ChatGPT

Base `https://chatgpt.com/`, conversations under `/c/`, composer `#prompt-textarea`, assistant/user role attributes, `data-message-id` identity, stop/stability completion, and `#upload-files` attachments. Model selection should prefer stable data attributes. A disabled requested mode must not be clicked repeatedly.

## Claude

Base `https://claude.ai/`, `/chat/`, composer `[data-testid="chat-input"]`, assistant transcript row with `[data-is-streaming]`, completion at `data-is-streaming=false`, upload `[data-testid="file-upload"]`.

## DeepSeek

Base `https://chat.deepseek.com/`, `/a/chat/s/`, composer `textarea[name="search"]`, assistant content under `.ds-assistant-message-main-content` / `.ds-think-content`. Use a count baseline rather than treating virtual-list keys as permanent message IDs. Long-thinking behavior is expected.

## Gemini

Base `https://gemini.google.com/app`, `model-response` / `user-query`, `message-content-id-*` identity, response action controls as completion signal.

## Kimi

Base `https://www.kimi.com/`, `/chat/`, composer `.chat-input-editor`, stable `data-archer-id`, `.segment-assistant-actions` completion.

## GLM / Z.ai

Base `https://chat.z.ai/`, `/c/`, composer `#chat-input`, message IDs `message-<uuid>`, copy/regenerate controls as completion signal.

## Change procedure

1. Verify the live DOM manually with a legitimate account.
2. Prefer semantic attributes, ARIA, data-testid, and custom elements.
3. Use stable structural relationships next.
4. Hashed/generated CSS classes are last-resort fallbacks.
5. Test new chat, existing chat, logged-out state, generating, completed, error, usage/rate limit, and challenge states.
6. Add/update fixtures or unit tests.
7. Never execute local tools in a provider adapter.
8. Do not add anti-detection scripts.
