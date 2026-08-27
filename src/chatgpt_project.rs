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
const PROJECT_NAVIGATION_TIMEOUT: Duration = Duration::from_secs(8);

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

    fn from_navigation_url(value: &str, name: String) -> Result<Self> {
        let url = Url::parse(value).map_err(|e| {
            WtError::Browser(format!(
                "ChatGPT Project {name:?} navigated to an invalid URL {value:?}: {e}"
            ))
        })?;
        if url.scheme() != "https" || url.host_str() != Some("chatgpt.com") {
            return Err(WtError::Browser(format!(
                "ChatGPT Project {name:?} navigated outside chatgpt.com: {value}"
            )));
        }
        let segments: Vec<_> = url
            .path_segments()
            .map(|segments| segments.filter(|s| !s.is_empty()).collect())
            .unwrap_or_default();
        if segments.len() < 2 || segments[0] != "g" || !segments[1].starts_with("g-p-") {
            return Err(WtError::Browser(format!(
                "ChatGPT Project {name:?} did not navigate to a Project route: {value}"
            )));
        }
        Self::from_project_id(segments[1], Some(name))
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

    wait_for_project_directory(&page).await?;
    let current_url = page.cdp.current_url().await.unwrap_or_default();
    debug!(url = %current_url, "ChatGPT Project discovery page ready");

    // Keep the cheap href/HTML path for versions of ChatGPT that expose Project
    // routes directly. The current Projects UI renders project rows as SPA click
    // targets without hrefs, so native row navigation is the authoritative fallback.
    let direct = discover_projects_from_routes(&page).await?;
    debug!(
        count = direct.len(),
        "ChatGPT direct Project route discovery completed"
    );
    if !direct.is_empty() {
        return Ok(direct);
    }

    let projects = discover_projects_interactively(&page).await?;
    debug!(
        count = projects.len(),
        "ChatGPT interactive Project discovery completed"
    );
    if !projects.is_empty() {
        return Ok(projects);
    }

    let body = page.cdp.body_text().await.unwrap_or_default();
    let lower = body.to_ascii_lowercase();
    if lower.contains("log in") || lower.contains("sign up") || body.contains("登录") {
        return Err(WtError::Authentication(
            "ChatGPT is not signed in; run `wtagent login --model chatgpt` first".into(),
        ));
    }
    Ok(projects)
}

async fn wait_for_project_directory(page: &ChromePage) -> Result<()> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(12);
    while tokio::time::Instant::now() < deadline {
        let names = project_row_names(page).await?;
        if !names.is_empty() {
            debug!(
                count = names.len(),
                "ChatGPT Project directory rows are ready"
            );
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }

    let body = page.cdp.body_text().await.unwrap_or_default();
    let lower = body.to_ascii_lowercase();
    if lower.contains("log in") || lower.contains("sign up") || body.contains("登录") {
        return Err(WtError::Authentication(
            "ChatGPT is not signed in; run `wtagent login --model chatgpt` first".into(),
        ));
    }

    Err(WtError::Browser(
        "ChatGPT Projects page loaded, but no project directory rows became available".into(),
    ))
}

async fn project_row_names(page: &ChromePage) -> Result<Vec<String>> {
    let value = page
        .cdp
        .evaluate(
            r#"(() => {
                const prefix = 'Open project options for ';
                const names = [];
                for (const row of document.querySelectorAll('[role="row"]')) {
                    const button = [...row.querySelectorAll('button[aria-label]')]
                        .find((el) => (el.getAttribute('aria-label') || '').startsWith(prefix));
                    if (!button) continue;
                    const name = (button.getAttribute('aria-label') || '').slice(prefix.length).trim();
                    if (name && !names.includes(name)) names.push(name);
                }
                return names;
            })()"#,
        )
        .await?;
    let array = value.as_array().ok_or_else(|| {
        WtError::Browser("ChatGPT Project row discovery returned a non-array value".into())
    })?;
    Ok(array
        .iter()
        .filter_map(Value::as_str)
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(ToOwned::to_owned)
        .collect())
}

async fn discover_projects_interactively(page: &ChromePage) -> Result<Vec<ChatGptProjectBinding>> {
    let names = project_row_names(page).await?;
    if names.is_empty() {
        return Ok(Vec::new());
    }

    let mut projects = Vec::with_capacity(names.len());
    for name in names {
        click_project_row(page, &name).await?;
        let binding = wait_for_project_navigation(page, &name).await?;
        debug!(
            name = %name,
            project_id = %binding.project_id,
            url = %binding.url,
            "ChatGPT Project route resolved through native navigation"
        );
        projects.push(binding);

        page.cdp.navigate(PROJECTS_URL).await?;
        wait_for_project_directory(page).await?;
    }

    projects.sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.url.cmp(&b.url)));
    projects.dedup_by(|a, b| a.project_id == b.project_id);
    Ok(projects)
}

async fn click_project_row(page: &ChromePage, name: &str) -> Result<()> {
    let wanted = serde_json::to_string(name)?;
    let expression = format!(
        r#"(() => {{
            const wanted = {wanted};
            const prefix = 'Open project options for ';
            for (const row of document.querySelectorAll('[role="row"]')) {{
                const button = [...row.querySelectorAll('button[aria-label]')]
                    .find((el) => (el.getAttribute('aria-label') || '').startsWith(prefix));
                if (!button) continue;
                const candidate = (button.getAttribute('aria-label') || '').slice(prefix.length).trim();
                if (candidate !== wanted) continue;
                if (!(row instanceof HTMLElement)) return false;
                row.click();
                return true;
            }}
            return false;
        }})()"#
    );
    let clicked = page.cdp.evaluate_bool(&expression).await?;
    if !clicked {
        return Err(WtError::Browser(format!(
            "ChatGPT Project row {name:?} disappeared before it could be opened"
        )));
    }
    Ok(())
}

async fn wait_for_project_navigation(
    page: &ChromePage,
    name: &str,
) -> Result<ChatGptProjectBinding> {
    let deadline = tokio::time::Instant::now() + PROJECT_NAVIGATION_TIMEOUT;
    let mut last_url = String::new();
    while tokio::time::Instant::now() < deadline {
        last_url = page.cdp.current_url().await.unwrap_or_default();
        if let Ok(binding) = ChatGptProjectBinding::from_navigation_url(&last_url, name.to_string())
        {
            return Ok(binding);
        }
        tokio::time::sleep(Duration::from_millis(150)).await;
    }

    Err(WtError::Browser(format!(
        "ChatGPT Project {name:?} did not navigate to a Project URL within {}s; last URL: {last_url}",
        PROJECT_NAVIGATION_TIMEOUT.as_secs()
    )))
}

async fn discover_projects_from_routes(page: &ChromePage) -> Result<Vec<ChatGptProjectBinding>> {
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
    fn resolves_project_binding_from_native_navigation() {
        let project = ChatGptProjectBinding::from_navigation_url(
            "https://chatgpt.com/g/g-p-123-opensource/c/abcdef?foo=bar",
            "OpenSource".into(),
        )
        .unwrap();
        assert_eq!(project.name.as_deref(), Some("OpenSource"));
        assert_eq!(project.project_id, "g-p-123-opensource");
        assert_eq!(
            project.url,
            "https://chatgpt.com/g/g-p-123-opensource/project"
        );
    }

    #[test]
    fn rejects_non_project_navigation() {
        let error = ChatGptProjectBinding::from_navigation_url(
            "https://chatgpt.com/projects",
            "OpenSource".into(),
        )
        .unwrap_err();
        assert!(error
            .to_string()
            .contains("did not navigate to a Project route"));
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
