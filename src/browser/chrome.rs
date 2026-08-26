use std::{
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};

use reqwest::Client;
use serde::Deserialize;
use tokio::{process::Child, process::Command};
use tracing::{debug, info};

use crate::{
    browser::{
        backend::{resolve_browser_backend, BrowserBackend},
        cdp::CdpClient,
        client::BrowserClient,
        ego::EgoClient,
        provider::ProviderConfig,
    },
    error::{Result, WtError},
};

#[derive(Debug, Deserialize)]
struct DevtoolsTarget {
    #[serde(rename = "type")]
    target_type: String,
    url: String,
    #[serde(rename = "webSocketDebuggerUrl")]
    websocket_debugger_url: Option<String>,
}

pub struct ChromePage {
    pub cdp: BrowserClient,
    pub debug_port: Option<u16>,
    _child: Option<Child>,
}

impl ChromePage {
    pub async fn launch(
        provider: &ProviderConfig,
        profile_dir: &Path,
        chrome_override: Option<&Path>,
        minimized: bool,
        preferred_url: Option<&str>,
    ) -> Result<Self> {
        Self::launch_with_backend(
            provider,
            profile_dir,
            BrowserBackend::Auto,
            chrome_override,
            None,
            None,
            minimized,
            preferred_url,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn launch_with_backend(
        provider: &ProviderConfig,
        profile_dir: &Path,
        browser_backend: BrowserBackend,
        chrome_override: Option<&Path>,
        ego_override: Option<&Path>,
        ego_task_space: Option<&str>,
        minimized: bool,
        preferred_url: Option<&str>,
    ) -> Result<Self> {
        let backend = resolve_browser_backend(browser_backend, chrome_override, ego_override)?;
        match backend {
            BrowserBackend::Chrome => {
                Self::launch_chrome(
                    provider,
                    profile_dir,
                    chrome_override,
                    minimized,
                    preferred_url,
                )
                .await
            }
            BrowserBackend::Ego => {
                let task_space = ego_task_space.map(ToOwned::to_owned).unwrap_or_else(|| {
                    format!("wtagent-rs-{:?}", provider.id).to_ascii_lowercase()
                });
                let ego =
                    EgoClient::launch(provider, ego_override, task_space.clone(), preferred_url)
                        .await?;
                info!(
                    provider = provider.label,
                    task_space,
                    executable = %ego.executable().display(),
                    "using ego-lite browser backend"
                );
                Ok(Self {
                    cdp: BrowserClient::ego(ego),
                    debug_port: None,
                    _child: None,
                })
            }
            BrowserBackend::Auto => unreachable!("auto backend must be resolved before launch"),
        }
    }

    async fn launch_chrome(
        provider: &ProviderConfig,
        profile_dir: &Path,
        chrome_override: Option<&Path>,
        minimized: bool,
        preferred_url: Option<&str>,
    ) -> Result<Self> {
        tokio::fs::create_dir_all(profile_dir).await?;
        let active_port_file = profile_dir.join("DevToolsActivePort");
        let client = Client::builder().timeout(Duration::from_secs(5)).build()?;

        let (port, child) =
            if let Some(port) = probe_existing_port(&client, &active_port_file).await {
                debug!(port, "reusing provider Chrome CDP endpoint");
                (port, None)
            } else {
                let chrome = discover_chrome(chrome_override)?;
                info!(chrome = %chrome.display(), provider = provider.label, "launching Chrome");
                let mut command = Command::new(chrome);
                command
                    .arg("--remote-debugging-port=0")
                    .arg(format!("--user-data-dir={}", profile_dir.display()))
                    .arg("--no-first-run")
                    .arg("--no-default-browser-check")
                    .arg(provider.base_url)
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .kill_on_drop(false);
                if minimized {
                    command.arg("--start-minimized");
                }
                let child = command.spawn().map_err(|e| {
                    WtError::Browser(format!("failed to launch Chrome/Chromium: {e}"))
                })?;
                let port = wait_for_devtools_port(&client, &active_port_file).await?;
                (port, Some(child))
            };

        let target = choose_or_create_target(&client, port, provider, preferred_url).await?;
        let ws = target.websocket_debugger_url.ok_or_else(|| {
            WtError::Browser("selected Chrome target has no debugger websocket".into())
        })?;
        let cdp = CdpClient::connect(&ws).await?;

        Ok(Self {
            cdp: BrowserClient::chrome(cdp),
            debug_port: Some(port),
            _child: child,
        })
    }
}

async fn probe_existing_port(client: &Client, active_port_file: &Path) -> Option<u16> {
    let content = tokio::fs::read_to_string(active_port_file).await.ok()?;
    let port = content.lines().next()?.trim().parse::<u16>().ok()?;
    let url = format!("http://127.0.0.1:{port}/json/version");
    client.get(url).send().await.ok()?.error_for_status().ok()?;
    Some(port)
}

async fn wait_for_devtools_port(client: &Client, active_port_file: &Path) -> Result<u16> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    while tokio::time::Instant::now() < deadline {
        if let Some(port) = probe_existing_port(client, active_port_file).await {
            return Ok(port);
        }
        tokio::time::sleep(Duration::from_millis(150)).await;
    }
    Err(WtError::Browser(
        "Chrome did not expose DevToolsActivePort within 20 seconds".into(),
    ))
}

async fn list_targets(client: &Client, port: u16) -> Result<Vec<DevtoolsTarget>> {
    let targets = client
        .get(format!("http://127.0.0.1:{port}/json/list"))
        .send()
        .await?
        .error_for_status()?
        .json::<Vec<DevtoolsTarget>>()
        .await?;
    Ok(targets)
}

async fn choose_or_create_target(
    client: &Client,
    port: u16,
    provider: &ProviderConfig,
    preferred_url: Option<&str>,
) -> Result<DevtoolsTarget> {
    let targets = list_targets(client, port).await?;
    if let Some(preferred) = preferred_url {
        if let Some(target) = targets.into_iter().find(|target| {
            target.target_type == "page"
                && same_origin_and_path(&target.url, preferred)
                && target.websocket_debugger_url.is_some()
        }) {
            return Ok(target);
        }
    } else if let Some(target) = targets.into_iter().find(|target| {
        target.target_type == "page"
            && target.url.starts_with(provider.base_url)
            && target.websocket_debugger_url.is_some()
    }) {
        return Ok(target);
    }

    let encoded: String =
        url::form_urlencoded::byte_serialize(provider.base_url.as_bytes()).collect();
    let target = client
        .put(format!("http://127.0.0.1:{port}/json/new?{encoded}"))
        .send()
        .await?
        .error_for_status()?
        .json::<DevtoolsTarget>()
        .await?;
    Ok(target)
}

fn same_origin_and_path(left: &str, right: &str) -> bool {
    let (Ok(left), Ok(right)) = (url::Url::parse(left), url::Url::parse(right)) else {
        return false;
    };
    left.scheme() == right.scheme()
        && left.host_str() == right.host_str()
        && left.path() == right.path()
}

pub fn discover_chrome(override_path: Option<&Path>) -> Result<PathBuf> {
    if let Some(path) = override_path {
        if path.is_file() {
            return Ok(path.to_path_buf());
        }
        return Err(WtError::Config(format!(
            "configured Chrome path does not exist: {}",
            path.display()
        )));
    }

    let candidates: Vec<PathBuf> = if cfg!(target_os = "macos") {
        vec![
            "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome".into(),
            "/Applications/Chromium.app/Contents/MacOS/Chromium".into(),
            dirs::home_dir()
                .unwrap_or_default()
                .join("Applications/Google Chrome.app/Contents/MacOS/Google Chrome"),
        ]
    } else if cfg!(target_os = "windows") {
        let mut paths = Vec::new();
        for base in [
            std::env::var_os("PROGRAMFILES"),
            std::env::var_os("PROGRAMFILES(X86)"),
            std::env::var_os("LOCALAPPDATA"),
        ]
        .into_iter()
        .flatten()
        {
            let base = PathBuf::from(base);
            paths.push(base.join("Google/Chrome/Application/chrome.exe"));
            paths.push(base.join("Chromium/Application/chrome.exe"));
        }
        paths
    } else {
        vec![
            "/usr/bin/google-chrome".into(),
            "/usr/bin/google-chrome-stable".into(),
            "/usr/bin/chromium".into(),
            "/usr/bin/chromium-browser".into(),
            "/snap/bin/chromium".into(),
        ]
    };

    candidates
        .into_iter()
        .find(|path| path.is_file())
        .ok_or_else(|| {
            WtError::Config(
                "Chrome/Chromium was not found. Install Chrome, install ego-lite, or pass --chrome-path."
                    .into(),
            )
        })
}
