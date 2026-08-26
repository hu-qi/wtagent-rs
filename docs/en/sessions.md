# Session Management

WTAgent-RS persists task state, events, browser conversation URLs, and side-effect journals under the application data directory. The `session` command group manages and resumes those sessions.

## Common commands

```bash
# List the 20 most recent sessions
wtagent session

# Explicit list with a limit
wtagent session list
wtagent session list -n 10

# Machine-readable output
wtagent session list --format json

# Inspect one session
wtagent session show <SESSION_ID>
wtagent session show <SESSION_ID> --format json

# Resume a specific session
wtagent session resume <SESSION_ID>
wtagent session resume <SESSION_ID> "continue and run the tests"

# Continue the latest session for the current project
wtagent session continue
wtagent session continue "continue investigating the previous failure"

# Delete local session state and event logs
wtagent session delete <SESSION_ID>
```

The old `wtagent sessions` command remains as a compatibility alias for now, but new usage should prefer `wtagent session list`.

## Why `wtagent session` no longer starts an agent task

Earlier CLI parsing supported the shorthand `wtagent "task"` by retrying unknown CLI input as `wtagent run ...`. As a result, typing `wtagent session` created a new agent task whose literal task text was `session`, then entered the browser runtime and appeared to hang.

`session` is now a real command group. Bare `wtagent session` only lists saved sessions and sends no provider message.

## Design relationship to OpenCode

The design follows the same lifecycle separation used by OpenCode: CLI commands manage persistent sessions while interactive entry points continue or resume an existing session. WTAgent-RS currently implements the high-value core of that model — `list`, `show`, `resume`, `continue`, and `delete`. A TUI session picker, fork, and abort can be added later without changing the persisted session schema.

## Storage

```text
<app-data>/wtagent-rs/
  sessions/
    <session-id>/
      state.json
      events.jsonl
```

`delete` removes the selected session directory. `continue` selects the most recently updated session whose persisted project root matches the resolved current `-C/--project`, preventing accidental continuation of an unrelated repository.
