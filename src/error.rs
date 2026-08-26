use thiserror::Error;

pub type Result<T> = std::result::Result<T, WtError>;

#[derive(Debug, Error)]
pub enum WtError {
    #[error("browser error: {0}")]
    Browser(String),
    #[error("provider usage limit reached: {0}")]
    UsageLimit(String),
    #[error("provider rate limit encountered: {0}")]
    RateLimit(String),
    #[error("provider challenge requires manual action: {0}")]
    Challenge(String),
    #[error("authentication required: {0}")]
    Authentication(String),
    #[error("protocol error: {0}")]
    Protocol(String),
    #[error("tool error: {0}")]
    Tool(String),
    #[error("policy denied: {0}")]
    Policy(String),
    #[error("session error: {0}")]
    Session(String),
    #[error("configuration error: {0}")]
    Config(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Http(#[from] reqwest::Error),
    #[error(transparent)]
    Url(#[from] url::ParseError),
    #[error(transparent)]
    WebSocket(#[from] tokio_tungstenite::tungstenite::Error),
}
