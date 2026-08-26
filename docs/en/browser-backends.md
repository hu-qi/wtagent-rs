# Browser backends: ego-lite and Chrome/Chromium

WTAgent-RS separates provider DOM logic from browser transport. It can drive pages through the ego-lite `ego-browser` Task Space runtime or through the standard Chrome/Chromium DevTools Protocol.

## Automatic selection

The default path is automatic:

1. an explicit `--chrome-path` selects Chrome/Chromium;
2. on macOS, WTAgent-RS prefers ego-lite when `ego-browser` is available on `PATH` or at `~/.local/bin/ego-browser`;
3. otherwise it falls back to Chrome/Chromium;
4. Linux and Windows currently use Chrome/Chromium by default because ego-lite's desktop distribution is currently macOS-focused.

Run `wtagent doctor` to see the backend and executable that were actually resolved.

## ego-lite backend

WTAgent-RS does not treat ego-lite as a Chrome executable and does not wait for `DevToolsActivePort`. It invokes the official `ego-browser nodejs` runtime and uses its preloaded `cdp()` and Task Space helpers.

Each provider gets a stable Task Space such as:

```text
wtagent-rs-chatgpt
wtagent-rs-claude
wtagent-rs-gemini
```

The Task Space is isolated from the user's normal tabs while inheriting login state available to ego-lite. In normal use this avoids maintaining another provider Cookie/profile solely for WTAgent-RS.

After installing ego-lite, finish the GUI onboarding once so `ego-browser` is registered, commonly at:

```text
~/.local/bin/ego-browser
```

Check it with:

```bash
command -v ego-browser
```

## Chrome/Chromium backend

When ego-lite is unavailable, WTAgent-RS keeps its original native CDP path:

- a dedicated `--user-data-dir` for every provider;
- an ephemeral remote-debugging port;
- `DevToolsActivePort` discovery;
- direct WebSocket CDP connection.

To force a particular Chrome/Chromium binary:

```bash
wtagent --chrome-path /path/to/chrome doctor
```

An explicit `--chrome-path` overrides macOS ego-lite auto-preference.

## Authentication, challenges, and control

ego-lite's key benefit is reuse of the browser identity already present on the machine. For the most predictable flow, sign into the provider in ego-lite before starting WTAgent-RS. The Chrome backend uses WTAgent-RS's dedicated provider profile and manual sign-in.

Neither backend automates account credentials, bypasses CAPTCHAs, spoofs fingerprints, or rotates proxies/accounts. Security challenges remain a manual browser action.

## Troubleshooting

### `Chrome/Chromium was not found`

On macOS with ego-lite installed, make sure onboarding has completed and `command -v ego-browser` returns a path. Reopen the terminal if necessary so `~/.local/bin` is present on `PATH`, then run `wtagent doctor`.

### `ego-browser was not found`

Usually the app is installed but onboarding has not registered the command yet, or the current shell cannot see `~/.local/bin`.

### Task Space is user-controlled

ego-lite explicitly separates agent control from user control. If the user takes over a Task Space in the GUI, WTAgent-RS does not silently seize it back. Finish the manual action and return control before continuing.

## Architectural boundary

The browser backend only changes how CDP/page operations reach the browser. Provider selectors, message sending, response detection, pacing, the tool protocol, policy enforcement, and session recovery continue to use the same Rust implementation. ego-lite therefore does not create a second parallel set of ChatGPT/Claude/Gemini adapters.
