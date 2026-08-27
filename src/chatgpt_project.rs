use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tracing::debug;
use url::Url;

use crate::{
    browser::{chrome::ChromePage, provider::ProviderId},
    config::AppConfig,
    error::{Result, WtError},
};

const PROJECTS_URL: &str = "https://chatgpt.com/projects";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChatGptProjectBinding {
    pub name: Option<String>,
    pub url: String,
    pub project_id: String,
}

impl ChatGptProjectBinding {
    pub fn from_url(value: &str) -> Result<Self> {
        let mut url = Url::parse(value)
            .map_err(|e| WtError::Config(format!("invalid ChatGPT project URL: {e}")))?;
        if url.scheme() != "https" || url.host_str() != Some("chatgpt.com") {
            return Err(WtError::Config(
                "ChatGPT project URL must use https://chatgpt.com".into(),
            ));
        }
        url.set_query(None);
        url.set_fragment(None);
        let segments: Vec<_> = url
            .path_segments()
            .map(|segments| segments.filter(|s| !s.is_empty()).collect())
            .unwrap_or_default();
        if segments.len() != 3
            || segments[0] != "g"
            || !segments[1].starts_with("g-p-")
            || segments[2] != "project"
        {
            return Err(WtError::Config(
                "expected a ChatGPT Project URL like https://chatgpt.com/g/g-p-<project-id>-<slug>/project"
                    .into(),
            ));
        }
        Ok(Self {
            name: None,
            url: url.to_string(),
            project_id: segments[1].to_string(),
        })
    }

    fn from_project_id(project_id: &str, name: Option<String>) -> Result<Self> {
        if !project_id.starts_with("g-p-") || project_id.contains('/') {
            return Err(WtError::Browser(format!(
                "invalid ChatGPT Project id discovered in page: {project_id:?}"
            )));
        }
        Ok(Self {
            name,
            url: format!("https://chatgpt.com/g/{project_id}/project"),
            project_id: project_id.to_string(),
        })
    }
}

pub async fn resolve_chatgpt_project(
    config: &AppConfig,
    target: &str,
) -> Result<ChatGptProjectBinding> {
    ensure_chatgpt(config)?;
    if target.starts_with("https://") {
        return ChatGptProjectBinding::from_url(target);
    }

    let projects = list_chatgpt_projects(config).await?;
    let wanted = target.trim();
    let matches: Vec<_> = projects
        .into_iter()
        .filter(|project| {
            project
                .name
                .as_deref()
                .map(|name| name.eq_ignore_ascii_case(wanted))
                .unwrap_or(false)
        })
        .collect();

    match matches.as_slice() {
        [project] => Ok(project.clone()),
        [] => Err(WtError::Config(format!(
            "ChatGPT Project {wanted:?} was not found by exact name. Run `wtagent chatgpt projects` to inspect visible projects, or pass the project URL directly."
        ))),
        _ => Err(WtError::Config(format!(
            "multiple ChatGPT Projects are named {wanted:?}; pass the exact project URL instead"
        ))),
    }
}

pub async fn list_chatgpt_projects(config: &AppConfig) -> Result<Vec<ChatGptProjectBinding>> {
    ensure_chatgpt(config)?;
    let provider = ProviderId::Chatgpt.config();
    let page = ChromePage::launch(
        &provider,
        &config.profile_dir(),
        config.chrome_path.as_deref(),
        config.minimized,
        Some(PROJECTS_URL),
    )
    .await?;

    let current_url = page.cdp.current_url().await.unwrap_or_default();
    debug!(url = %current_url, "ChatGPT Project discovery page ready");

    let deadline = tokio::time::Instant::now() + Duration::from_secs(12);
    let mut last = Vec::new();
    while tokio::time::Instant::now() < deadline {
        last = discover_projects(&page).await?;
        debug!(
            count = last.len(),
            "ChatGPT Project discovery pass completed"
        );
        if !last.is_empty() {
            return Ok(last);
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }

    let body = page.cdp.body_text().await.unwrap_or_default();
    let lower = body.to_ascii_lowercase();
    if lower.contains("log in") || lower.contains("sign up") || body.contains("登录") {
        return Err(WtError::Authentication(
            "ChatGPT is not signed in; run `wtagent login --model chatgpt` first".into(),
        ));
    }
    Ok(last)
}

async fn discover_projects(page: &ChromePage) -> Result<Vec<ChatGptProjectBinding>> {
    let value = page
        .cdp
        .evaluate(
            r#"(() => {
                const candidates = new Map();
                const add = (rawUrl, rawName, score) => {
                    if (!rawUrl) return;
                    let u;
                    try { u = new URL(rawUrl, location.origin); } catch { return; }
                    if (u.origin !== 'https://chatgpt.com') return;
                    const parts = u.pathname.split('/').filter(Boolean);
                    if (parts.length < 2 || parts[0] !== 'g' || !parts[1].startsWith('g-p-')) return;
                    const projectId = parts[1];
                    const canonical = `https://chatgpt.com/g/${projectId}/project`;
                    const name = (rawName || '').replace(/\s+/g, ' ').trim() || null;
                    const previous = candidates.get(projectId);
                    if (!previous || score > previous.score || (score === previous.score && !previous.name && name)) {
                        candidates.set(projectId, { name, url: canonical, project_id: projectId, score });
                    }
                };

                for (const el of document.querySelectorAll('[href]')) {
                    const href = el.getAttribute('href');
                    if (!href || !href.includes('/g/g-p-')) continue;
                    const text =
                        el.getAttribute('aria-label') ||
                        el.getAttribute('title') ||
                        el.textContent ||
                        '';
                    let score = 1;
                    try {
                        const path = new URL(href, location.origin).pathname;
                        if (/^\/g\/g-p-[^/]+\/project\/?$/.test(path)) score = 3;
                        else if (el.tagName === 'A') score = 2;
                    } catch {}
                    add(href, text, score);
                }

                // React/Next may keep route hrefs in rendered data before they become
                // concrete anchors. Discover project ids from the live HTML as a
                // fallback; names are intentionally left unset in this path.
                const html = document.documentElement?.innerHTML || '';
                const patterns = [
                    /\/g\/(g-p-[A-Za-z0-9_-]+)\/(?:project|c\/[^\"'<>\\\s]+)/g,
                    /\\\/g\\\/(g-p-[A-Za-z0-9_-]+)\\\/(?:project|c\\\/[^\"'<>\\\s]+)/g
                ];
                for (const re of patterns) {
                    let match;
                    while ((match = re.exec(html)) !== null) {
                        add(`/g/${match[1]}/project`, null, 0);
                    }
                }

                return [...candidates.values()]
                    .map(({score, ...project}) => project)
                    .sort((a, b) => (a.name || '').localeCompare(b.name || '') || a.url.localeCompare(b.url));
            })()"#,
        )
        .await?;
    projects_from_value(value)
}

fn projects_from_value(value: Value) -> Result<Vec<ChatGptProjectBinding>> {
    let array = value.as_array().ok_or_else(|| {
        WtError::Browser("ChatGPT Project discovery returned a non-array value".into())
    })?;
    let mut projects = Vec::with_capacity(array.len());
    for item in array {
        let project_id = item
            .get("project_id")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let name = item
            .get("name")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .map(ToOwned::to_owned);
        let binding = ChatGptProjectBinding::from_project_id(project_id, name)?;
        projects.push(binding);
    }
    projects.sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.url.cmp(&b.url)));
    Ok(projects)
}

fn ensure_chatgpt(config: &AppConfig) -> Result<()> {
    if config.provider != ProviderId::Chatgpt {
        return Err(WtError::Config(
            "ChatGPT Project targeting is only available with --model chatgpt".into(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_project_url() {
        let project = ChatGptProjectBinding::from_url(
            "https://chatgpt.com/g/g-p-6a0be51c7d58819182a98476d1424347-demo/project?foo=bar#x",
        )
        .unwrap();
        assert_eq!(
            project.project_id,
            "g-p-6a0be51c7d58819182a98476d1424347-demo"
        );
        assert_eq!(
            project.url,
            "https://chatgpt.com/g/g-p-6a0be51c7d58819182a98476d1424347-demo/project"
        );
    }

    #[test]
    fn builds_canonical_project_from_nested_discovery() {
        let value = serde_json::json!([{
            "name": "OpenSource",
            "url": "https://chatgpt.com/g/g-p-123-opensource/project",
            "project_id": "g-p-123-opensource"
        }]);
        let projects = projects_from_value(value).unwrap();
        assert_eq!(projects[0].name.as_deref(), Some("OpenSource"));
        assert_eq!(
            projects[0].url,
            "https://chatgpt.com/g/g-p-123-opensource/project"
        );
    }

    #[test]
    fn rejects_non_project_chatgpt_url() {
        let error = ChatGptProjectBinding::from_url("https://chatgpt.com/c/abc").unwrap_err();
        assert!(error.to_string().contains("expected a ChatGPT Project URL"));
    }

    #[test]
    fn rejects_other_hosts() {
        let error = ChatGptProjectBinding::from_url(
            "https://example.com/g/g-p-6a0be51c7d58819182a98476d1424347-demo/project",
        )
        .unwrap_err();
        assert!(error.to_string().contains("https://chatgpt.com"));
    }
}
