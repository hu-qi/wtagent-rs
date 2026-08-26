use std::{path::Path, time::Duration};

use serde_json::{json, Value};
use tracing::debug;

use crate::error::{Result, WtError};

use super::{backend::BrowserBackend, cdp::CdpClient, ego::EgoClient};

pub enum BrowserClient {
    Chrome(CdpClient),
    Ego(EgoClient),
}

impl BrowserClient {
    pub fn chrome(client: CdpClient) -> Self {
        Self::Chrome(client)
    }

    pub fn ego(client: EgoClient) -> Self {
        Self::Ego(client)
    }

    pub fn backend(&self) -> BrowserBackend {
        match self {
            Self::Chrome(_) => BrowserBackend::Chrome,
            Self::Ego(_) => BrowserBackend::Ego,
        }
    }

    pub fn backend_label(&self) -> &'static str {
        match self {
            Self::Chrome(_) => "Chrome/Chromium",
            Self::Ego(_) => "ego-lite",
        }
    }

    pub fn ego_details(&self) -> Option<(&Path, &str)> {
        match self {
            Self::Chrome(_) => None,
            Self::Ego(client) => Some((client.executable(), client.task_space())),
        }
    }

    pub async fn handoff_for_manual_login(&self, timeout: Duration) -> Result<bool> {
        match self {
            Self::Chrome(_) => Ok(false),
            Self::Ego(client) => {
                client.handoff_for_manual_login(timeout).await?;
                Ok(true)
            }
        }
    }

    pub async fn call(&self, method: &str, params: Value) -> Result<Value> {
        match self {
            Self::Chrome(client) => client.call(method, params).await,
            Self::Ego(client) => client.call(method, params).await,
        }
    }

    pub async fn evaluate(&self, expression: impl Into<String>) -> Result<Value> {
        let result = self
            .call(
                "Runtime.evaluate",
                json!({
                    "expression": expression.into(),
                    "awaitPromise": true,
                    "returnByValue": true,
                    "userGesture": true,
                }),
            )
            .await?;
        if let Some(details) = result.get("exceptionDetails") {
            return Err(WtError::Browser(format!(
                "browser JavaScript evaluation failed: {details}"
            )));
        }
        Ok(result
            .pointer("/result/value")
            .cloned()
            .unwrap_or(Value::Null))
    }

    pub async fn evaluate_string(&self, expression: impl Into<String>) -> Result<String> {
        Ok(self
            .evaluate(expression)
            .await?
            .as_str()
            .unwrap_or_default()
            .to_string())
    }

    pub async fn evaluate_bool(&self, expression: impl Into<String>) -> Result<bool> {
        Ok(self.evaluate(expression).await?.as_bool().unwrap_or(false))
    }

    pub async fn navigate(&self, url: &str) -> Result<()> {
        self.call("Page.navigate", json!({ "url": url })).await?;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
        while tokio::time::Instant::now() < deadline {
            let ready = self
                .evaluate_string("document.readyState")
                .await
                .unwrap_or_default();
            if ready == "interactive" || ready == "complete" {
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
        Err(WtError::Browser(format!("navigation to {url} timed out")))
    }

    pub async fn current_url(&self) -> Result<String> {
        self.evaluate_string("location.href").await
    }

    pub async fn body_text(&self) -> Result<String> {
        self.evaluate_string("document.body?.innerText || ''").await
    }

    pub async fn visible_selector(&self, selectors: &[&str]) -> Result<Option<String>> {
        let selectors = serde_json::to_string(selectors)?;
        let expression = format!(
            r#"(() => {{
                const selectors = {selectors};
                const visible = (el) => {{
                    if (!el) return false;
                    const r = el.getBoundingClientRect();
                    const s = getComputedStyle(el);
                    return r.width > 0 && r.height > 0 &&
                           s.visibility !== 'hidden' && s.display !== 'none';
                }};
                for (const selector of selectors) {{
                    for (const el of document.querySelectorAll(selector)) {{
                        if (visible(el)) return selector;
                    }}
                }}
                return null;
            }})()"#
        );
        Ok(self
            .evaluate(expression)
            .await?
            .as_str()
            .map(ToOwned::to_owned))
    }

    pub async fn count(&self, selector: &str) -> Result<usize> {
        let selector = serde_json::to_string(selector)?;
        let value = self
            .evaluate(format!("document.querySelectorAll({selector}).length"))
            .await?;
        Ok(value.as_u64().unwrap_or(0) as usize)
    }

    pub async fn last_text(&self, selector: &str) -> Result<String> {
        let selector = serde_json::to_string(selector)?;
        self.evaluate_string(format!(
            r#"(() => {{
                const nodes = [...document.querySelectorAll({selector})];
                const el = nodes[nodes.length - 1];
                return el?.innerText || '';
            }})()"#
        ))
        .await
    }

    pub async fn last_attribute(&self, selector: &str, attribute: &str) -> Result<Option<String>> {
        let selector = serde_json::to_string(selector)?;
        let attribute = serde_json::to_string(attribute)?;
        Ok(self
            .evaluate(format!(
                r#"(() => {{
                    const nodes = [...document.querySelectorAll({selector})];
                    const el = nodes[nodes.length - 1];
                    return el?.getAttribute({attribute}) ?? null;
                }})()"#
            ))
            .await?
            .as_str()
            .map(ToOwned::to_owned))
    }

    pub async fn focus_and_clear(&self, selectors: &[&str]) -> Result<bool> {
        let selectors = serde_json::to_string(selectors)?;
        self.evaluate_bool(format!(
            r#"(() => {{
                const selectors = {selectors};
                const visible = (el) => {{
                    const r = el.getBoundingClientRect();
                    const s = getComputedStyle(el);
                    return r.width > 0 && r.height > 0 &&
                           s.visibility !== 'hidden' && s.display !== 'none';
                }};
                for (const selector of selectors) {{
                    for (const el of document.querySelectorAll(selector)) {{
                        if (!visible(el)) continue;
                        el.focus();
                        if ('value' in el) {{
                            const proto = el instanceof HTMLTextAreaElement
                                ? HTMLTextAreaElement.prototype
                                : HTMLInputElement.prototype;
                            const setter = Object.getOwnPropertyDescriptor(proto, 'value')?.set;
                            if (setter) setter.call(el, '');
                            else el.value = '';
                        }} else {{
                            el.textContent = '';
                        }}
                        el.dispatchEvent(new InputEvent('input', {{
                            bubbles: true,
                            inputType: 'deleteContentBackward',
                            data: null
                        }}));
                        return true;
                    }}
                }}
                return false;
            }})()"#
        ))
        .await
    }

    pub async fn insert_text(&self, text: &str) -> Result<()> {
        self.call("Input.insertText", json!({ "text": text }))
            .await?;
        Ok(())
    }

    pub async fn click_first_visible(&self, selectors: &[&str]) -> Result<bool> {
        let selectors = serde_json::to_string(selectors)?;
        self.evaluate_bool(format!(
            r#"(() => {{
                const selectors = {selectors};
                const visible = (el) => {{
                    const r = el.getBoundingClientRect();
                    const s = getComputedStyle(el);
                    return r.width > 0 && r.height > 0 &&
                           s.visibility !== 'hidden' && s.display !== 'none';
                }};
                for (const selector of selectors) {{
                    for (const el of document.querySelectorAll(selector)) {{
                        if (!visible(el)) continue;
                        if (el.disabled || el.getAttribute('aria-disabled') === 'true') continue;
                        el.click();
                        return true;
                    }}
                }}
                return false;
            }})()"#
        ))
        .await
    }

    pub async fn press_enter(&self) -> Result<()> {
        self.call(
            "Input.dispatchKeyEvent",
            json!({
                "type": "keyDown",
                "key": "Enter",
                "code": "Enter",
                "windowsVirtualKeyCode": 13,
                "nativeVirtualKeyCode": 13,
            }),
        )
        .await?;
        self.call(
            "Input.dispatchKeyEvent",
            json!({
                "type": "keyUp",
                "key": "Enter",
                "code": "Enter",
                "windowsVirtualKeyCode": 13,
                "nativeVirtualKeyCode": 13,
            }),
        )
        .await?;
        Ok(())
    }

    pub async fn set_file_input(
        &self,
        selector: &str,
        files: &[std::path::PathBuf],
    ) -> Result<bool> {
        if files.is_empty() {
            return Ok(true);
        }
        let document = self.call("DOM.getDocument", json!({ "depth": 0 })).await?;
        let node_id = document
            .pointer("/root/nodeId")
            .and_then(Value::as_u64)
            .ok_or_else(|| WtError::Browser("DOM.getDocument returned no root node".into()))?;
        let queried = self
            .call(
                "DOM.querySelector",
                json!({ "nodeId": node_id, "selector": selector }),
            )
            .await?;
        let input_node = queried.get("nodeId").and_then(Value::as_u64).unwrap_or(0);
        if input_node == 0 {
            return Ok(false);
        }
        let paths: Vec<String> = files
            .iter()
            .map(|p| {
                p.canonicalize()
                    .unwrap_or_else(|_| p.to_path_buf())
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        self.call(
            "DOM.setFileInputFiles",
            json!({ "nodeId": input_node, "files": paths }),
        )
        .await?;
        Ok(true)
    }

    pub async fn save_screenshot(&self, path: &Path) -> Result<()> {
        let result = self
            .call("Page.captureScreenshot", json!({ "format": "png" }))
            .await?;
        let data = result
            .get("data")
            .and_then(Value::as_str)
            .ok_or_else(|| WtError::Browser("screenshot response did not contain data".into()))?;
        tokio::fs::write(path.with_extension("png.base64"), data).await?;
        debug!(path = %path.display(), "saved base64 screenshot diagnostics");
        Ok(())
    }
}
