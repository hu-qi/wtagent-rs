# ChatGPT Project 发现机制

ChatGPT 的 `/projects` 页面是 SPA。不同版本的页面不一定把 Project URL 暴露为 `<a href>`：2026-08 的实测页面中，Project 目录使用 `role="row"` 的可点击行，并通过 React 事件执行导航，DOM 和序列化 HTML 中都可能完全没有 `g-p-*` 字符串。

WTAgent-RS 因此使用两级发现策略：

1. **直接路由发现**：如果页面提供 `/g/g-p-.../project`、Project chat 路由或等价 href，直接解析并规范化。
2. **原生导航发现**：如果没有直接路由，从 Project 目录行读取项目名称，逐个触发 ChatGPT 自己的行点击导航，读取导航后的真实 Project URL/ID，再返回 `/projects` 继续下一项。

第二种方式不调用 ChatGPT 未公开的 backend API，也不会从认证信息中提取 ID；它只观察已经登录的网页自身公开呈现的目录和原生导航结果。

## Debug 诊断

```bash
wtagent --debug chatgpt projects
```

正常情况下可看到类似：

```text
ChatGPT Project directory rows are ready count=10
ChatGPT direct Project route discovery completed count=0
ChatGPT Project route resolved through native navigation name=OpenSource project_id=g-p-... url=https://chatgpt.com/g/g-p-.../project
ChatGPT interactive Project discovery completed count=10
```

如果 Project 行在点击前消失、导航没有进入 Project route，或 ego-lite Task Space 进入 user control，WTAgent-RS 会硬停止并给出明确错误，不会猜测 Project ID，也不会绕过 ego-lite 的 ownership 规则。
