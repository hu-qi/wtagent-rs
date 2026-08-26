use clap::ValueEnum;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, ValueEnum)]
#[serde(rename_all = "lowercase")]
#[derive(Default)]
pub enum ProviderId {
    #[default]
    Chatgpt,
    Claude,
    Deepseek,
    Gemini,
    Kimi,
    Glm,
}

#[derive(Debug, Clone)]
pub struct ProviderConfig {
    pub id: ProviderId,
    pub label: &'static str,
    pub base_url: &'static str,
    pub conversation_path_prefix: &'static str,
    pub composer_selectors: &'static [&'static str],
    pub send_selectors: &'static [&'static str],
    pub stop_selectors: &'static [&'static str],
    pub assistant_selector: &'static str,
    pub user_selector: &'static str,
    pub message_selector: &'static str,
    pub upload_selector: Option<&'static str>,
    pub auth_path_prefixes: &'static [&'static str],
    pub auth_text_markers: &'static [&'static str],
    pub default_mode: Option<&'static str>,
    pub reliable_completion_signal: bool,
    pub long_thinking: bool,
}

impl ProviderId {
    pub fn config(self) -> ProviderConfig {
        match self {
            Self::Chatgpt => ProviderConfig {
                id: self,
                label: "ChatGPT",
                base_url: "https://chatgpt.com/",
                conversation_path_prefix: "/c/",
                composer_selectors: &[
                    "#prompt-textarea",
                    "textarea[placeholder*=\"Message\" i]",
                    "textarea[placeholder*=\"消息\"]",
                    "div[contenteditable=\"true\"][data-lexical-editor=\"true\"]",
                    "main div[contenteditable=\"true\"]",
                ],
                send_selectors: &[
                    "[data-testid=\"send-button\"]",
                    "button[aria-label*=\"send\" i]",
                    "button[aria-label*=\"发送\"]",
                ],
                stop_selectors: &[
                    "[data-testid=\"stop-button\"]",
                    "button[aria-label*=\"stop\" i]",
                    "button[aria-label*=\"停止\"]",
                ],
                assistant_selector:
                    "[data-message-author-role=\"assistant\"]:not([id^=\"request-placeholder-\"])",
                user_selector: "[data-message-author-role=\"user\"]",
                message_selector:
                    "[data-message-author-role=\"user\"], [data-message-author-role=\"assistant\"]",
                upload_selector: Some("#upload-files"),
                auth_path_prefixes: &["/auth/"],
                auth_text_markers: &[
                    "log in to get answers",
                    "log in or sign up",
                    "sign up for free",
                    "登录或注册",
                    "登录以",
                ],
                default_mode: None,
                reliable_completion_signal: true,
                long_thinking: false,
            },
            Self::Claude => ProviderConfig {
                id: self,
                label: "Claude",
                base_url: "https://claude.ai/",
                conversation_path_prefix: "/chat/",
                composer_selectors: &[
                    "[data-testid=\"chat-input\"]",
                    ".ProseMirror[contenteditable=\"true\"]",
                    "main div[contenteditable=\"true\"]",
                    "main textarea",
                ],
                send_selectors: &[
                    "[data-testid=\"chat-input-send\"]",
                    "main button[aria-label*=\"send\" i]",
                ],
                stop_selectors: &[
                    "[data-testid=\"chat-input-stop\"]",
                    "main button[aria-label*=\"stop\" i]",
                ],
                assistant_selector:
                    "[data-testid=\"transcript-row\"] [data-is-streaming]",
                user_selector: "[data-testid=\"user-message\"]",
                message_selector:
                    "[data-testid=\"user-message\"], [data-testid=\"transcript-row\"] [data-is-streaming]",
                upload_selector: Some("[data-testid=\"file-upload\"]"),
                auth_path_prefixes: &["/login", "/oauth", "/sso"],
                auth_text_markers: &[
                    "continue with google",
                    "continue with email",
                    "enter your email",
                    "sign in to claude",
                    "log in to claude",
                    "登录 claude",
                ],
                default_mode: None,
                reliable_completion_signal: true,
                long_thinking: true,
            },
            Self::Deepseek => ProviderConfig {
                id: self,
                label: "DeepSeek",
                base_url: "https://chat.deepseek.com/",
                conversation_path_prefix: "/a/chat/s/",
                composer_selectors: &[
                    "textarea[name=\"search\"]",
                    "textarea[placeholder*=\"发送消息\"]",
                    "textarea[placeholder*=\"Message\" i]",
                    "main textarea",
                ],
                send_selectors: &[
                    "button[aria-label*=\"send\" i]",
                    "button[aria-label*=\"发送\"]",
                ],
                stop_selectors: &[
                    "button[aria-label*=\"stop\" i]",
                    "button[aria-label*=\"停止\"]",
                ],
                assistant_selector:
                    "[data-virtual-list-item-key]:has(.ds-assistant-message-main-content), [data-virtual-list-item-key]:has(.ds-think-content)",
                user_selector:
                    "[data-virtual-list-item-key]:not(:has(.ds-assistant-message-main-content)):not(:has(.ds-think-content))",
                message_selector: "[data-virtual-list-item-key]",
                upload_selector: Some("input[type=\"file\"]"),
                auth_path_prefixes: &["/sign_in"],
                auth_text_markers: &[
                    "发送验证码",
                    "密码登录",
                    "微信扫码登录",
                    "sign in with apple",
                    "password login",
                ],
                default_mode: Some("expert-thinking"),
                reliable_completion_signal: false,
                long_thinking: true,
            },
            Self::Gemini => ProviderConfig {
                id: self,
                label: "Gemini",
                base_url: "https://gemini.google.com/app",
                conversation_path_prefix: "/app/",
                composer_selectors: &[
                    "[data-test-id=\"textarea-wrapper\"] .ql-editor[contenteditable=\"true\"][role=\"textbox\"]",
                    "div.ql-editor[contenteditable=\"true\"][role=\"textbox\"]",
                    "div[contenteditable=\"true\"][role=\"textbox\"]",
                ],
                send_selectors: &[
                    "button[aria-label=\"发送\"]",
                    "button[aria-label=\"Send\"]",
                    "button[aria-label=\"Send message\"]",
                ],
                stop_selectors: &[
                    "button[aria-label=\"停止回答\"]",
                    "button[aria-label=\"Stop response\"]",
                ],
                assistant_selector: "model-response",
                user_selector: "user-query",
                message_selector: "user-query, model-response",
                upload_selector: Some("input[type=\"file\"]"),
                auth_path_prefixes: &[
                    "/signin",
                    "/ServiceLogin",
                    "/o/oauth2/",
                ],
                auth_text_markers: &[
                    "sign in",
                    "登录",
                    "choose an account",
                    "选择账号",
                ],
                default_mode: None,
                reliable_completion_signal: true,
                long_thinking: false,
            },
            Self::Kimi => ProviderConfig {
                id: self,
                label: "Kimi",
                base_url: "https://www.kimi.com/",
                conversation_path_prefix: "/chat/",
                composer_selectors: &[
                    ".chat-input-editor",
                    "div[contenteditable=\"true\"][data-lexical-editor=\"true\"]",
                    "main div[contenteditable=\"true\"]",
                    "textarea",
                ],
                send_selectors: &[
                    ".send-button-container:not(.disabled)",
                    "button[aria-label*=\"send\" i]",
                    "button[aria-label*=\"发送\"]",
                ],
                stop_selectors: &[
                    "button[aria-label*=\"stop\" i]",
                    "button[aria-label*=\"停止\"]",
                    "button[aria-label*=\"中断\"]",
                ],
                assistant_selector: ".chat-content-item-assistant",
                user_selector: ".chat-content-item-user",
                message_selector: ".chat-content-item",
                upload_selector: Some("input[type=\"file\"]"),
                auth_path_prefixes: &[],
                auth_text_markers: &[
                    "手机号快捷登录",
                    "手机号登录",
                    "发送验证码",
                    "log in to sync",
                    "sign in",
                ],
                default_mode: Some("k3"),
                reliable_completion_signal: true,
                long_thinking: true,
            },
            Self::Glm => ProviderConfig {
                id: self,
                label: "GLM",
                base_url: "https://chat.z.ai/",
                conversation_path_prefix: "/c/",
                composer_selectors: &[
                    "#chat-input",
                    "textarea[placeholder*=\"帮您\"]",
                    "main textarea",
                ],
                send_selectors: &[
                    "#send-message-button",
                    "button[aria-label*=\"send\" i]",
                    "button[aria-label*=\"发送\"]",
                ],
                stop_selectors: &[
                    "#stop-response-button",
                    "button[aria-label*=\"stop\" i]",
                    "button[aria-label*=\"停止\"]",
                    "button[aria-label*=\"中断\"]",
                ],
                assistant_selector:
                    "#messages-container [id^=\"message-\"]:not([id$=\"-start\"]):not(.user-message)",
                user_selector:
                    "#messages-container [id^=\"message-\"]:not([id$=\"-start\"]).user-message",
                message_selector:
                    "#messages-container [id^=\"message-\"]:not([id$=\"-start\"])",
                upload_selector: Some("input[type=\"file\"]"),
                auth_path_prefixes: &["/auth"],
                auth_text_markers: &[
                    "手机号登录",
                    "发送验证码",
                    "欢迎回来",
                    "sign in",
                    "log in",
                ],
                default_mode: Some("latest"),
                reliable_completion_signal: true,
                long_thinking: true,
            },
        }
    }

    pub fn label(self) -> &'static str {
        self.config().label
    }

    pub fn profile_basename(self) -> &'static str {
        match self {
            Self::Chatgpt => "chrome-profile",
            Self::Claude => "claude-profile",
            Self::Deepseek => "deepseek-profile",
            Self::Gemini => "gemini-profile",
            Self::Kimi => "kimi-profile",
            Self::Glm => "glm-profile",
        }
    }
}

impl ProviderConfig {
    pub fn all() -> Vec<Self> {
        [
            ProviderId::Chatgpt,
            ProviderId::Claude,
            ProviderId::Deepseek,
            ProviderId::Gemini,
            ProviderId::Kimi,
            ProviderId::Glm,
        ]
        .into_iter()
        .map(ProviderId::config)
        .collect()
    }
}
