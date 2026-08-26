use std::{fmt, path::Path};

use clap::ValueEnum;

use crate::error::{Result, WtError};

use super::{chrome::discover_chrome, ego::discover_ego};

#[derive(Debug, Clone, Copy, Default, ValueEnum, PartialEq, Eq)]
pub enum BrowserBackend {
    /// Prefer ego-lite on macOS when available, otherwise use Chrome/Chromium.
    #[default]
    Auto,
    /// Use a Chrome/Chromium executable and its DevTools websocket transport.
    Chrome,
    /// Use ego-lite through the `ego-browser` task-space runtime.
    Ego,
}

impl fmt::Display for BrowserBackend {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Auto => f.write_str("auto"),
            Self::Chrome => f.write_str("chrome"),
            Self::Ego => f.write_str("ego"),
        }
    }
}

pub fn resolve_browser_backend(
    requested: BrowserBackend,
    chrome_override: Option<&Path>,
    ego_override: Option<&Path>,
) -> Result<BrowserBackend> {
    match requested {
        BrowserBackend::Chrome => {
            discover_chrome(chrome_override)?;
            Ok(BrowserBackend::Chrome)
        }
        BrowserBackend::Ego => {
            discover_ego(ego_override)?;
            Ok(BrowserBackend::Ego)
        }
        BrowserBackend::Auto => {
            if chrome_override.is_some() && ego_override.is_some() {
                return Err(WtError::Config(
                    "--chrome-path and --ego-path cannot both be used with --browser auto; choose --browser chrome or --browser ego"
                        .into(),
                ));
            }
            if chrome_override.is_some() {
                discover_chrome(chrome_override)?;
                return Ok(BrowserBackend::Chrome);
            }
            if ego_override.is_some() {
                discover_ego(ego_override)?;
                return Ok(BrowserBackend::Ego);
            }
            if cfg!(target_os = "macos") && discover_ego(None).is_ok() {
                return Ok(BrowserBackend::Ego);
            }
            discover_chrome(None)?;
            Ok(BrowserBackend::Chrome)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_is_cli_stable() {
        assert_eq!(BrowserBackend::Auto.to_string(), "auto");
        assert_eq!(BrowserBackend::Chrome.to_string(), "chrome");
        assert_eq!(BrowserBackend::Ego.to_string(), "ego");
    }
}
