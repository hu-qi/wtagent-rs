# ChatGPT Project discovery

ChatGPT's `/projects` page is an SPA. Different UI revisions do not necessarily expose Project URLs as `<a href>` links. In an August 2026 observed layout, the Project directory used clickable `role="row"` elements with React event handlers, while both the DOM and serialized HTML contained no `g-p-*` route strings.

WTAgent-RS therefore uses a two-stage discovery strategy:

1. **Direct route discovery**: if the page exposes `/g/g-p-.../project`, nested Project chat routes, or equivalent hrefs, parse and canonicalize them directly.
2. **Native navigation discovery**: if no direct routes are available, read project names from the Project directory rows, trigger ChatGPT's own row navigation, observe the resulting Project URL/ID, then navigate back to `/projects` and continue with the next row.

The fallback does not call undocumented ChatGPT backend APIs and does not extract IDs from authentication state. It only observes the visible authenticated directory and the native navigation result produced by the ChatGPT UI itself.

## Debug diagnostics

```bash
wtagent --debug chatgpt projects
```

A successful run may include messages such as:

```text
ChatGPT Project directory rows are ready count=10
ChatGPT direct Project route discovery completed count=0
ChatGPT Project route resolved through native navigation name=OpenSource project_id=g-p-... url=https://chatgpt.com/g/g-p-.../project
ChatGPT interactive Project discovery completed count=10
```

If a row disappears before it can be opened, navigation does not reach a Project route, or the ego-lite Task Space moves into user control, WTAgent-RS stops with an explicit error. It does not guess Project IDs or bypass ego-lite ownership rules.
