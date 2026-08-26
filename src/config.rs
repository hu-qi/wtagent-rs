use std::{path::PathBuf, time::Duration};

use clap::ValueEnum;

use crate::{
    browser::provider::ProviderId,
    error::{Result, WtError},
};

#[derive(Debug, Clone, Copy, ValueEnum, PartialEq, Eq)]
pub enum ApprovalMode {
    /// Ask before side effects; read-only tools are automatic.
    Ask,
    /// Automatically approve project-local writes and program execution.
    Auto,
    /// Only allow read-only tools.
    ReadOnly,
}

#[derive(Debug, Clone)]
pub struct RateConfig {
    pub min_send_interval: Duration,
    pub jitter_max: Duration,
    pub max_sends_per_minute: usize,
    pub base_backoff: Duration,
    pub max_backoff: Duration,
}

impl Default for RateConfig {
    fn default() -> Self {
        Self {
            min_send_interval: Duration::from_millis(4_000),
            jitter_max: Duration::from_millis(1_500),
            max_sends_per_minute: 6,
            base_backoff: Duration::from_secs(15),
            max_backoff: Duration::from_secs(15 * 60),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Limits {
    pub model_turn_timeout: Duration,
    pub stable_window: Duration,
    pub max_browser_message_bytes: usize,
    pub max_tool_result_bytes: usize,
    pub max_file_read_bytes: usize,
    pub max_tool_output_bytes: usize,
    pub max_steps: usize,
    pub max_batch_read_calls: usize,
    pub command_timeout: Duration,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            model_turn_timeout: Duration::from_secs(12 * 60),
            stable_window: Duration::from_millis(2_500),
            max_browser_message_bytes: 48 * 1024,
            max_tool_result_bytes: 28 * 1024,
            max_file_read_bytes: 16 * 1024,
            max_tool_output_bytes: 8 * 1024,
            max_steps: 64,
            max_batch_read_calls: 4,
            command_timeout: Duration::from_secs(120),
        }
    }
}

#[derive(Debug, Clone)]
pub struct AppConfig {
    pub provider: ProviderId,
    pub mode: Option<String>,
    pub project_root: PathBuf,
    pub chrome_path: Option<PathBuf>,
    pub minimized: bool,
    pub approval: ApprovalMode,
    pub rate: RateConfig,
    pub limits: Limits,
    pub app_data_dir: PathBuf,
}

impl AppConfig {
    pub fn new(provider: ProviderId, project_root: PathBuf) -> Result<Self> {
        let project_root = project_root
            .canonicalize()
            .map_err(|e| WtError::Config(format!("cannot resolve project root: {e}")))?;
        let app_data_dir = default_app_data_dir()?;

        Ok(Self {
            provider,
            mode: None,
            project_root,
            chrome_path: None,
            minimized: false,
            approval: ApprovalMode::Ask,
            rate: RateConfig::default(),
            limits: Limits::default(),
            app_data_dir,
        })
    }

    pub fn profile_dir(&self) -> PathBuf {
        self.app_data_dir
            .join("profiles")
            .join(self.provider.profile_basename())
    }
}


pub fn default_app_data_dir() -> Result<PathBuf> {
    Ok(dirs::data_dir()
        .or_else(dirs::home_dir)
        .ok_or_else(|| WtError::Config("cannot determine application data directory".into()))?
        .join("wtagent-rs"))
}
