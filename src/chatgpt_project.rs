use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;
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
            "ChatGPT Project {wanted:?} was not found. Run `wtagent chatgpt projects` to list visible projects, or pass the project URL directly."
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

    let deadline = tokio::time::Instant::now() + Duration::from_secs(12);
    let mut last = Vec::new();
    while tokio::time::Instant::now() < deadline {
        last = discover_projects(&page).await?;
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
                const seen = new Set();
                const output = [];
                for (const a of document.querySelectorAll('a[href*="/g/g-p-"]')) {
                    let u;
                    try { u = new URL(a.href, location.origin); } catch { continue; }
                    if (u.origin !== 'https://chatgpt.com') continue;
                    if (!/^\/g\/g-p-[^/]+\/project\/?$/.test(u.pathname)) continue;
                    u.search = '';
                    u.hash = '';
                    const href = u.href.replace(/\/$/, '');
                    if (seen.has(href)) continue;
                    seen.add(href);
                    const raw = (
                        a.getAttribute('aria-label') ||
                        a.getAttribute('title') ||
                        a.textContent ||
                        ''
                    );
                    const name = raw.replace(/\s+/g, ' ').trim();
                    const parts = u.pathname.split('/').filter(Boolean);
                    output.push({
                        name: name || null,
                        url: href,
                        project_id: parts[1]
                    });
                }
                return output;
            })()"#,
        )
        .await?;
    projects_from_value(value)
}

fn projects_from_value(value: Value) -> Result<Vec<ChatGptProjectBinding>> {
    let array = value
        .as_array()
        .ok_or_else(|| WtError::Browser("ChatGPT Project discovery returned a non-array value".into()))?;
    let mut projects = Vec::with_capacity(array.len());
    for item in array {
        let url = item.get("url").and_then(Value::as_str).unwrap_or_default();
        let mut binding = ChatGptProjectBinding::from_url(url)?;
        binding.name = item
            .get("name")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .map(ToOwned::to_owned);
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
