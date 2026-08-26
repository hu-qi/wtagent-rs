use std::{path::PathBuf, time::Duration};

use async_trait::async_trait;
use serde_json::Value;
use tracing::{debug, info, warn};
use url::Url;

use crate::{
    browser::{
        chrome::ChromePage,
        provider::{ProviderConfig, ProviderId},
    },
    error::{Result, WtError},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthState {
    Authenticated,
    Unauthenticated,
    Unknown,
}

#[derive(Debug, Clone)]
struct AssistantSnapshot {
    count: usize,
    id: Option<String>,
    text: String,
}

#[derive(Debug, Clone)]
pub struct CompletedTurn {
    pub text: String,
    pub assistant_id: Option<String>,
}

#[async_trait]
pub trait WebAdapter: Send {
    fn provider_id(&self) -> ProviderId;
    fn provider_label(&self) -> &'static str;
    async fn launch(&mut self, preferred_url: Option<&str>) -> Result<()>;
    async fn auth_state(&self) -> Result<AuthState>;
    async fn wait_for_manual_login(&self, timeout: Duration) -> Result<()>;
    async fn start_conversation(&self, conversation_url: Option<&str>) -> Result<()>;
    async fn select_mode(&self, mode: Option<&str>) -> Result<Option<String>>;
    async fn send_message(&mut self, text: &str, files: &[PathBuf]) -> Result<()>;
    async fn wait_for_turn(
        &mut self,
        timeout: Duration,
        stable_window: Duration,
    ) -> Result<CompletedTurn>;
    async fn conversation_url(&self) -> Result<String>;
}

pub struct BrowserWebAdapter {
    provider: ProviderConfig,
    profile_dir: PathBuf,
    chrome_path: Option<PathBuf>,
    minimized: bool,
    page: Option<ChromePage>,
    baseline: Option<AssistantSnapshot>,
}

impl BrowserWebAdapter {
    pub fn new(
        provider: ProviderId,
        profile_dir: PathBuf,
        chrome_path: Option<PathBuf>,
        minimized: bool,
    ) -> Self {
        Self {
            provider: provider.config(),
            profile_dir,
            chrome_path,
            minimized,
            page: None,
            baseline: None,
        }
    }

    fn page(&self) -> Result<&ChromePage> {
        self.page
            .as_ref()
            .ok_or_else(|| WtError::Browser("browser has not been launched".into()))
    }

    async fn composer_visible(&self) -> Result<bool> {
        Ok(self
            .page()?
            .cdp
            .visible_selector(self.provider.composer_selectors)
            .await?
            .is_some())
    }

    async fn wait_for_composer(&self, timeout: Duration) -> Result<bool> {
        let deadline = tokio::time::Instant::now() + timeout;
        while tokio::time::Instant::now() < deadline {
            self.throw_if_challenge().await?;
            if self.composer_visible().await.unwrap_or(false) {
                return Ok(true);
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        Ok(false)
    }

    async fn throw_if_challenge(&self) -> Result<()> {
        let composer = self.composer_visible_without_challenge_check().await?;
        if composer {
            return Ok(());
        }
        let body = self.page()?.cdp.body_text().await.unwrap_or_default();
        let lower = body.to_lowercase();
        let challenge_markers = [
            "just a moment",
            "verify you are human",
            "checking your browser",
            "security check",
            "attention required",
            "请稍候",
            "安全验证",
            "正在验证",
            "人机验证",
            "少々お待ち",
            "セキュリティ",
            "보안 확인",
            "un instant",
            "vérification",
            "einen moment",
            "sicherheitsprüfung",
        ];
        if challenge_markers
            .iter()
            .any(|marker| lower.contains(marker))
        {
            return Err(WtError::Challenge(format!(
                "{} displayed an anti-bot/security challenge; complete it manually in Chrome",
                self.provider.label
            )));
        }
        Ok(())
    }

    async fn composer_visible_without_challenge_check(&self) -> Result<bool> {
        Ok(self
            .page()?
            .cdp
            .visible_selector(self.provider.composer_selectors)
            .await?
            .is_some())
    }

    async fn assistant_snapshot(&self) -> Result<AssistantSnapshot> {
        let cdp = &self.page()?.cdp;
        let count = cdp.count(self.provider.assistant_selector).await?;
        let text = self.last_assistant_text().await?;
        let id = self.last_assistant_id().await?;
        Ok(AssistantSnapshot { count, id, text })
    }

    async fn last_assistant_id(&self) -> Result<Option<String>> {
        let cdp = &self.page()?.cdp;
        let selector = self.provider.assistant_selector;
        match self.provider.id {
            ProviderId::Chatgpt => {
                if let Some(id) = cdp.last_attribute(selector, "data-message-id").await? {
                    return Ok(Some(id));
                }
                let selector_json = serde_json::to_string(selector)?;
                let value = cdp
                    .evaluate(format!(
                        r#"(() => {{
                            const nodes = [...document.querySelectorAll({selector_json})];
                            const el = nodes[nodes.length - 1];
                            return el?.closest('[data-testid^="conversation-turn-"]')
                                ?.getAttribute('data-testid') ?? null;
                        }})()"#
                    ))
                    .await?;
                Ok(value.as_str().map(ToOwned::to_owned))
            }
            ProviderId::Claude | ProviderId::Deepseek => {
                let count = cdp.count(selector).await?;
                Ok((count > 0).then(|| format!("count:{count}")))
            }
            ProviderId::Gemini => {
                let selector_json = serde_json::to_string(selector)?;
                let value = cdp
                    .evaluate(format!(
                        r#"(() => {{
                            const nodes = [...document.querySelectorAll({selector_json})];
                            const el = nodes[nodes.length - 1];
                            return el?.querySelector('message-content[id^="message-content-id-"]')?.id ?? null;
                        }})()"#
                    ))
                    .await?;
                Ok(value.as_str().map(ToOwned::to_owned))
            }
            ProviderId::Kimi => cdp.last_attribute(selector, "data-archer-id").await,
            ProviderId::Glm => Ok(cdp
                .last_attribute(selector, "id")
                .await?
                .map(|id| id.trim_start_matches("message-").to_string())),
        }
    }

    async fn last_assistant_text(&self) -> Result<String> {
        let cdp = &self.page()?.cdp;
        let selector = serde_json::to_string(self.provider.assistant_selector)?;
        let expression = match self.provider.id {
            ProviderId::Claude => format!(
                r#"(() => {{
                    const nodes = [...document.querySelectorAll({selector})];
                    const el = nodes[nodes.length - 1];
                    if (!el) return '';
                    const response = el.querySelector('.font-claude-response');
                    return (response || el).innerText || '';
                }})()"#
            ),
            ProviderId::Gemini => format!(
                r#"(() => {{
                    const nodes = [...document.querySelectorAll({selector})];
                    const el = nodes[nodes.length - 1];
                    if (!el) return '';
                    const response = el.querySelector('message-content .markdown') ||
                                     el.querySelector('message-content');
                    return (response || el).innerText || '';
                }})()"#
            ),
            ProviderId::Kimi => format!(
                r#"(() => {{
                    const nodes = [...document.querySelectorAll({selector})];
                    const el = nodes[nodes.length - 1];
                    if (!el) return '';
                    const answers = [...el.querySelectorAll('.markdown')]
                        .filter(x => !x.closest('.thinking-container') && !x.closest('.toolcall-container'))
                        .map(x => (x.innerText || '').trim())
                        .filter(Boolean);
                    return answers.length ? [...new Set(answers)].join('\n') : (el.innerText || '');
                }})()"#
            ),
            _ => format!(
                r#"(() => {{
                    const nodes = [...document.querySelectorAll({selector})];
                    const el = nodes[nodes.length - 1];
                    return el?.innerText || '';
                }})()"#
            ),
        };
        cdp.evaluate_string(expression).await
    }

    async fn assistant_generating(&self, has_new_assistant: bool) -> Result<bool> {
        let cdp = &self.page()?.cdp;
        if cdp
            .visible_selector(self.provider.stop_selectors)
            .await?
            .is_some()
        {
            return Ok(true);
        }
        if !has_new_assistant {
            return Ok(false);
        }

        let selector = serde_json::to_string(self.provider.assistant_selector)?;
        match self.provider.id {
            ProviderId::Claude => {
                cdp.evaluate_bool(format!(
                    r#"(() => {{
                        const n=[...document.querySelectorAll({selector})].at(-1);
                        return n?.getAttribute('data-is-streaming') === 'true';
                    }})()"#
                ))
                .await
            }
            ProviderId::Gemini => {
                cdp.evaluate_bool(format!(
                    r#"(() => {{
                        const n=[...document.querySelectorAll({selector})].at(-1);
                        if (!n) return false;
                        return !n.querySelector('[data-test-id="regenerate-button"], button[aria-label="Copy"], button[aria-label="复制"]');
                    }})()"#
                ))
                .await
            }
            ProviderId::Kimi => {
                cdp.evaluate_bool(format!(
                    r#"(() => {{
                        const n=[...document.querySelectorAll({selector})].at(-1);
                        return !!n && !n.querySelector('.segment-assistant-actions');
                    }})()"#
                ))
                .await
            }
            ProviderId::Glm => {
                cdp.evaluate_bool(format!(
                    r#"(() => {{
                        const n=[...document.querySelectorAll({selector})].at(-1);
                        return !!n && !n.querySelector('.copy-response-button, .regenerate-response-button');
                    }})()"#
                ))
                .await
            }
            ProviderId::Chatgpt | ProviderId::Deepseek => Ok(false),
        }
    }

    async fn click_new_chat_if_needed(&self) -> Result<()> {
        if self
            .page()?
            .cdp
            .count(self.provider.message_selector)
            .await?
            == 0
        {
            return Ok(());
        }
        let selectors = new_chat_selectors(self.provider.id);
        if self.page()?.cdp.click_first_visible(selectors).await? {
            tokio::time::sleep(Duration::from_millis(800)).await;
        }
        if self
            .page()?
            .cdp
            .count(self.provider.message_selector)
            .await?
            > 0
        {
            return Err(WtError::Browser(format!(
                "{} did not open a verified empty conversation; refusing to send a new task into existing history",
                self.provider.label
            )));
        }
        Ok(())
    }

    async fn wait_for_sent_user_message(&self, before_count: usize) -> Result<bool> {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(12);
        while tokio::time::Instant::now() < deadline {
            let count = self.page()?.cdp.count(self.provider.user_selector).await?;
            if count > before_count {
                return Ok(true);
            }
            tokio::time::sleep(Duration::from_millis(150)).await;
        }
        Ok(false)
    }

    async fn usage_limit_or_rate_limit(&self, text: &str) -> Option<WtError> {
        let lower = text.to_lowercase();
        let usage = [
            "reached your usage limit",
            "reached your current usage limit",
            "hit your usage limit",
            "usage limit reached",
            "usage limits reached",
            "limit reached",
            "已达到使用上限",
            "已达使用上限",
            "使用上限",
            "额度已用尽",
            "额度用尽",
        ];
        if usage.iter().any(|marker| lower.contains(marker)) {
            return Some(WtError::UsageLimit(text.chars().take(180).collect()));
        }

        let rate = [
            "too many requests",
            "rate limit",
            "try again later",
            "请求过于频繁",
            "操作过于频繁",
            "请求频繁",
        ];
        if rate.iter().any(|marker| lower.contains(marker)) {
            return Some(WtError::RateLimit(format!(
                "{}: {}",
                self.provider.label,
                text.chars().take(180).collect::<String>()
            )));
        }
        None
    }

    async fn apply_mode_chatgpt(&self, requested: &str) -> Result<Option<String>> {
        let requested = requested.to_lowercase();
        let cdp = &self.page()?.cdp;
        let script = format!(
            r#"(() => {{
                const requested = {};
                const button = document.querySelector('[data-testid="model-switcher-dropdown-button"]');
                if (!button) return {{status:'missing'}};
                button.click();
                return {{status:'opened'}};
            }})()"#,
            serde_json::to_string(&requested)?
        );
        cdp.evaluate(script).await?;
        tokio::time::sleep(Duration::from_millis(600)).await;

        let requested_json = serde_json::to_string(&requested)?;
        let value = cdp
            .evaluate(format!(
                r#"(() => {{
                    const token = {requested_json}.replace(/[^a-z0-9]+/g,'');
                    const items = [...document.querySelectorAll('[role="menuitem"],[role="menuitemradio"]')]
                        .filter(el => {{
                            const r=el.getBoundingClientRect();
                            return r.width>0 && r.height>0;
                        }});
                    const normalized = (s) => (s||'').toLowerCase().replace(/[^a-z0-9]+/g,'');
                    let index = items.findIndex(el => {{
                        const slot = [
                            el.getAttribute('data-testid'),
                            el.getAttribute('data-value'),
                            el.id,
                            el.innerText
                        ].filter(Boolean).join(' ');
                        return normalized(slot).includes(token);
                    }});
                    if (index < 0) return {{status:'not-found'}};
                    const disabled = (el) =>
                        el.getAttribute('aria-disabled') === 'true' ||
                        el.hasAttribute('data-disabled');
                    let chosen = index;
                    if (disabled(items[chosen])) {{
                        chosen = index - 1;
                        while (chosen >= 0 && disabled(items[chosen])) chosen--;
                    }}
                    if (chosen < 0) return {{status:'disabled'}};
                    const label = (items[chosen].innerText || '').trim();
                    items[chosen].click();
                    return {{status: chosen === index ? 'selected':'fallback', label}};
                }})()"#
            ))
            .await?;
        Ok(value
            .get("label")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned))
    }

    async fn apply_default_provider_mode(&self, mode: &str) -> Result<Option<String>> {
        let cdp = &self.page()?.cdp;
        match (self.provider.id, mode) {
            (ProviderId::Deepseek, "expert-thinking") => {
                let result = cdp
                    .evaluate(
                        r#"(() => {
                            const radios=[...document.querySelectorAll('[role="radio"]')];
                            const expert=radios.find(x => /专家|expert/i.test(x.innerText||'')) || radios[1];
                            if (!expert) return false;
                            if (expert.getAttribute('aria-checked') !== 'true') expert.click();
                            setTimeout(() => {
                                const toggles=[...document.querySelectorAll('.ds-toggle-button')];
                                if (toggles.length === 1 && !toggles[0].className.includes('selected')) {
                                    toggles[0].click();
                                }
                            }, 250);
                            return true;
                        })()"#,
                    )
                    .await?;
                Ok(result
                    .as_bool()
                    .unwrap_or(false)
                    .then(|| "expert-thinking".into()))
            }
            (ProviderId::Kimi, "k3") => {
                cdp.evaluate(
                    r#"(() => {
                        const switcher=document.querySelector('.current-model');
                        if (!switcher) return false;
                        switcher.click();
                        return true;
                    })()"#,
                )
                .await?;
                tokio::time::sleep(Duration::from_millis(500)).await;
                let value = cdp
                    .evaluate(
                        r#"(() => {
                            const items=[...document.querySelectorAll('.models-container .model-item')];
                            const item=items.find(x => /^K3(?:\s|$)/i.test((x.innerText||'').trim()) && !/集群/.test(x.innerText||''));
                            if (!item) return null;
                            const label=(item.innerText||'K3').trim();
                            item.click();
                            return label;
                        })()"#,
                    )
                    .await?;
                Ok(value.as_str().map(ToOwned::to_owned))
            }
            (ProviderId::Glm, "latest") => {
                cdp.evaluate(
                    r#"(() => {
                        const b=document.querySelector('button.modelSelectorButton');
                        if (!b) return false;
                        b.click();
                        return true;
                    })()"#,
                )
                .await?;
                tokio::time::sleep(Duration::from_millis(500)).await;
                let value = cdp
                    .evaluate(
                        r#"(() => {
                            const all=[...document.querySelectorAll('button,[role="option"],[role="menuitem"]')];
                            const labels=['GLM-5.3','GLM-5.2'];
                            for (const wanted of labels) {
                                const item=all.find(x => (x.innerText||'').trim() === wanted);
                                if (item) { item.click(); return wanted; }
                            }
                            return null;
                        })()"#,
                    )
                    .await?;
                Ok(value.as_str().map(ToOwned::to_owned))
            }
            _ => Ok(None),
        }
    }
}

#[async_trait]
impl WebAdapter for BrowserWebAdapter {
    fn provider_id(&self) -> ProviderId {
        self.provider.id
    }

    fn provider_label(&self) -> &'static str {
        self.provider.label
    }

    async fn launch(&mut self, preferred_url: Option<&str>) -> Result<()> {
        if self.page.is_some() {
            return Ok(());
        }
        self.page = Some(
            ChromePage::launch(
                &self.provider,
                &self.profile_dir,
                self.chrome_path.as_deref(),
                self.minimized,
                preferred_url,
            )
            .await?,
        );
        Ok(())
    }

    async fn auth_state(&self) -> Result<AuthState> {
        let url = self.page()?.cdp.current_url().await?;
        let parsed = Url::parse(&url).ok();
        if let Some(path) = parsed.as_ref().map(Url::path) {
            if self
                .provider
                .auth_path_prefixes
                .iter()
                .any(|prefix| path.starts_with(prefix))
            {
                return Ok(AuthState::Unauthenticated);
            }
        }
        if self.composer_visible().await.unwrap_or(false) {
            return Ok(AuthState::Authenticated);
        }

        let body = self.page()?.cdp.body_text().await.unwrap_or_default();
        let lower = body.to_lowercase();
        if self
            .provider
            .auth_text_markers
            .iter()
            .any(|marker| lower.contains(&marker.to_lowercase()))
        {
            return Ok(AuthState::Unauthenticated);
        }
        Ok(AuthState::Unknown)
    }

    async fn wait_for_manual_login(&self, timeout: Duration) -> Result<()> {
        info!(
            provider = self.provider.label,
            "waiting for manual login in the dedicated Chrome profile"
        );
        let deadline = tokio::time::Instant::now() + timeout;
        let mut consecutive = 0usize;
        while tokio::time::Instant::now() < deadline {
            self.throw_if_challenge()
                .await
                .or_else(|error| match error {
                    WtError::Challenge(_) => Ok(()),
                    other => Err(other),
                })?;
            if self.auth_state().await? == AuthState::Authenticated {
                consecutive += 1;
                if consecutive >= 3 {
                    return Ok(());
                }
            } else {
                consecutive = 0;
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
        Err(WtError::Authentication(format!(
            "login to {} was not detected within {} seconds",
            self.provider.label,
            timeout.as_secs()
        )))
    }

    async fn start_conversation(&self, conversation_url: Option<&str>) -> Result<()> {
        let target = conversation_url.unwrap_or(self.provider.base_url);
        if let Some(url) = conversation_url {
            let parsed = Url::parse(url)?;
            let base = Url::parse(self.provider.base_url)?;
            if parsed.scheme() != "https" || parsed.host_str() != base.host_str() {
                return Err(WtError::Browser(format!(
                    "refusing to open conversation outside {}",
                    base.host_str().unwrap_or_default()
                )));
            }
        }

        let current = self.page()?.cdp.current_url().await.unwrap_or_default();
        if !same_origin_and_path(&current, target) {
            self.page()?.cdp.navigate(target).await?;
        }
        if !self.wait_for_composer(Duration::from_secs(30)).await? {
            return Err(WtError::Browser(format!(
                "{} composer was not found",
                self.provider.label
            )));
        }

        if conversation_url.is_none() {
            self.click_new_chat_if_needed().await?;
        }
        Ok(())
    }

    async fn select_mode(&self, mode: Option<&str>) -> Result<Option<String>> {
        let requested = mode.or(self.provider.default_mode);
        let Some(requested) = requested else {
            return Ok(None);
        };
        match self.provider.id {
            ProviderId::Chatgpt => self.apply_mode_chatgpt(requested).await,
            ProviderId::Deepseek | ProviderId::Kimi | ProviderId::Glm => {
                self.apply_default_provider_mode(requested).await
            }
            ProviderId::Claude | ProviderId::Gemini => Ok(None),
        }
    }

    async fn send_message(&mut self, text: &str, files: &[PathBuf]) -> Result<()> {
        self.throw_if_challenge().await?;
        if !self.wait_for_composer(Duration::from_secs(30)).await? {
            return Err(WtError::Browser(format!(
                "{} composer is unavailable",
                self.provider.label
            )));
        }

        let baseline = self.assistant_snapshot().await?;
        let user_before = self.page()?.cdp.count(self.provider.user_selector).await?;

        if !files.is_empty() {
            if let Some(selector) = self.provider.upload_selector {
                match self.page()?.cdp.set_file_input(selector, files).await {
                    Ok(true) => tokio::time::sleep(Duration::from_millis(800)).await,
                    Ok(false) => warn!(
                        provider = self.provider.label,
                        "file input was not found; continuing without attachments"
                    ),
                    Err(error) => warn!(%error, "attachment upload failed; continuing"),
                }
            }
        }

        if !self
            .page()?
            .cdp
            .focus_and_clear(self.provider.composer_selectors)
            .await?
        {
            return Err(WtError::Browser(format!(
                "{} composer could not be focused",
                self.provider.label
            )));
        }
        self.page()?.cdp.insert_text(text).await?;

        let clicked = self
            .page()?
            .cdp
            .click_first_visible(self.provider.send_selectors)
            .await?;
        if !clicked {
            self.page()?.cdp.press_enter().await?;
        }

        if !self.wait_for_sent_user_message(user_before).await? {
            // One conservative retry for a UI click that did not register. We do
            // not loop: repeated retries are exactly the kind of traffic burst
            // that can trigger provider limits.
            tokio::time::sleep(Duration::from_secs(2)).await;
            let clicked = self
                .page()?
                .cdp
                .click_first_visible(self.provider.send_selectors)
                .await?;
            if !clicked {
                self.page()?.cdp.press_enter().await?;
            }
            if !self.wait_for_sent_user_message(user_before).await? {
                return Err(WtError::Browser(format!(
                    "{} did not render the sent user message",
                    self.provider.label
                )));
            }
        }

        self.baseline = Some(baseline);
        Ok(())
    }

    async fn wait_for_turn(
        &mut self,
        timeout: Duration,
        stable_window: Duration,
    ) -> Result<CompletedTurn> {
        let baseline = self
            .baseline
            .clone()
            .ok_or_else(|| WtError::Browser("wait_for_turn called before send_message".into()))?;
        let deadline = tokio::time::Instant::now() + timeout;
        let mut last_text = String::new();
        let mut stable_since = tokio::time::Instant::now();

        while tokio::time::Instant::now() < deadline {
            self.throw_if_challenge().await?;
            let current = self.assistant_snapshot().await?;
            let has_new = current.count > baseline.count
                || (current.id.is_some() && current.id != baseline.id)
                || (!baseline.text.is_empty() && current.text != baseline.text);

            if has_new {
                if let Some(error) = self.usage_limit_or_rate_limit(&current.text).await {
                    return Err(error);
                }
                if current.text != last_text {
                    last_text = current.text.clone();
                    stable_since = tokio::time::Instant::now();
                    debug!(
                        provider = self.provider.label,
                        bytes = current.text.len(),
                        "assistant response changed"
                    );
                }

                let generating = self.assistant_generating(true).await?;
                let stable_for = tokio::time::Instant::now().duration_since(stable_since);
                let looks_protocol = current.text.contains("<agent_response");
                let complete_protocol =
                    !looks_protocol || current.text.contains("</agent_response>");
                let grace = if self.provider.reliable_completion_signal {
                    Duration::from_secs(10)
                } else {
                    Duration::from_secs(60)
                };

                if !current.text.trim().is_empty()
                    && !generating
                    && stable_for >= stable_window
                    && (complete_protocol || stable_for >= grace)
                {
                    self.baseline = None;
                    return Ok(CompletedTurn {
                        text: current.text.trim().to_string(),
                        assistant_id: current.id,
                    });
                }
            }

            tokio::time::sleep(if has_new {
                Duration::from_millis(300)
            } else if self.provider.long_thinking {
                Duration::from_millis(750)
            } else {
                Duration::from_millis(500)
            })
            .await;
        }

        Err(WtError::Browser(format!(
            "{} turn did not complete within {} seconds",
            self.provider.label,
            timeout.as_secs()
        )))
    }

    async fn conversation_url(&self) -> Result<String> {
        self.page()?.cdp.current_url().await
    }
}

fn new_chat_selectors(provider: ProviderId) -> &'static [&'static str] {
    match provider {
        ProviderId::Chatgpt => &["[data-testid=\"create-new-chat-button\"]", "a[href=\"/\"]"],
        ProviderId::Claude => &["a[href=\"/new\"]"],
        ProviderId::Deepseek => &["[data-testid=\"new-chat\"]", "a[href=\"/\"]"],
        ProviderId::Gemini => &["a[href=\"/app\"]"],
        ProviderId::Kimi => &["a[href=\"/\"]"],
        ProviderId::Glm => &["#sidebar-new-chat-button"],
    }
}

fn same_origin_and_path(left: &str, right: &str) -> bool {
    let (Ok(left), Ok(right)) = (Url::parse(left), Url::parse(right)) else {
        return false;
    };
    left.origin() == right.origin() && left.path() == right.path()
}
