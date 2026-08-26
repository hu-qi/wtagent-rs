# ChatGPT Projects

WTAgent-RS can create a **new ChatGPT conversation directly inside a selected ChatGPT Project**. Project targeting applies only to the ChatGPT provider.

## List visible Projects

```bash
wtagent chatgpt projects
```

Machine-readable output:

```bash
wtagent chatgpt projects --format json
```

The command uses the currently resolved WTAgent-RS browser backend, opens `https://chatgpt.com/projects`, and reads Project links from the authenticated page. WTAgent-RS does not depend on undocumented ChatGPT backend APIs for Project discovery.

## Create a new session by Project name

```bash
wtagent --model chatgpt --chatgpt-project "OpenSource" \
  "analyze this repository and run the tests"
```

Name matching is an exact ASCII-case-insensitive match. If multiple Projects have the same name, WTAgent-RS refuses to guess and asks for the URL instead.

## Target a Project URL

A Project URL is the more stable option for automation and long-lived configuration:

```bash
wtagent --model chatgpt \
  --chatgpt-project "https://chatgpt.com/g/g-p-<project-id>-<slug>/project" \
  "continue implementing the feature"
```

Only HTTPS `chatgpt.com` URLs matching this shape are accepted:

```text
/g/g-p-.../project
```

Ordinary chat URLs, share URLs, and third-party hosts are rejected.

## Session behavior

For a newly targeted task, Session State persists the normalized Project binding:

```json
{
  "chatgpt_project": {
    "name": "OpenSource",
    "url": "https://chatgpt.com/g/g-p-.../project",
    "project_id": "g-p-..."
  }
}
```

Before the first message, the Project URL is used as the initial conversation URL. Sending the first message from the Project page creates a new chat in that Project; WTAgent-RS then persists the actual conversation URL.

Therefore:

```bash
wtagent session resume <SESSION_ID>
```

continues the existing Project chat instead of creating another conversation.

`--chatgpt-project` is only valid when creating a new session. Passing it during resume is rejected so an existing WTAgent session cannot be silently migrated to another Project.

Inspect the binding with:

```bash
wtagent session show <SESSION_ID>
```

The output includes `chatgpt_project`, `chatgpt_project_id`, `chatgpt_project_url`, and the actual `conversation` URL.

## Browser and safety boundaries

- Uses the existing ego-lite Task Space or WTAgent-RS Chrome profile.
- Does not read or store provider passwords.
- Does not call undocumented APIs to create or mutate Projects.
- Does not bypass CAPTCHA, Cloudflare, or other security challenges.
- If name resolution fails, it does not silently fall back to a plain chat, avoiding accidental execution in the wrong context.

ChatGPT DOM structure can change. If `wtagent chatgpt projects` cannot discover an existing Project, pass its Project URL directly and use `--debug` for diagnostics.
