use std::{
    collections::VecDeque,
    time::{Duration, Instant},
};

use rand::RngExt;
use tokio::sync::Mutex;
use tracing::debug;

use crate::config::RateConfig;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderOutcome {
    Success,
    GenerationFailure,
    RateLimited,
    Challenge,
    UsageLimit,
}

#[derive(Debug)]
struct RateState {
    last_send: Option<Instant>,
    recent_sends: VecDeque<Instant>,
    blocked_until: Option<Instant>,
    penalty_level: u32,
}

#[derive(Debug)]
pub struct RateController {
    config: RateConfig,
    state: Mutex<RateState>,
}

impl RateController {
    pub fn new(config: RateConfig) -> Self {
        Self {
            config,
            state: Mutex::new(RateState {
                last_send: None,
                recent_sends: VecDeque::new(),
                blocked_until: None,
                penalty_level: 0,
            }),
        }
    }

    pub async fn before_send(&self) {
        loop {
            let delay = {
                let mut state = self.state.lock().await;
                let now = Instant::now();
                Self::prune(&mut state, now);
                self.next_delay(&state, now)
            };
            if delay.is_zero() {
                let mut state = self.state.lock().await;
                let now = Instant::now();
                Self::prune(&mut state, now);
                if self.next_delay(&state, now).is_zero() {
                    state.last_send = Some(now);
                    state.recent_sends.push_back(now);
                    return;
                }
                continue;
            }
            debug!(?delay, "pacing web-provider message");
            tokio::time::sleep(delay).await;
        }
    }

    pub async fn record_outcome(&self, outcome: ProviderOutcome) {
        let mut state = self.state.lock().await;
        let now = Instant::now();
        match outcome {
            ProviderOutcome::Success => {
                state.penalty_level = state.penalty_level.saturating_sub(1);
                if state.blocked_until.is_some_and(|until| until <= now) {
                    state.blocked_until = None;
                }
            }
            ProviderOutcome::GenerationFailure => {
                let delay = self
                    .backoff_for(state.penalty_level)
                    .min(Duration::from_secs(60));
                state.penalty_level = state.penalty_level.saturating_add(1);
                state.blocked_until = Some(now + delay);
            }
            ProviderOutcome::RateLimited => {
                let delay = self.backoff_for(state.penalty_level.max(2));
                state.penalty_level = state.penalty_level.saturating_add(2);
                state.blocked_until = Some(now + delay);
            }
            ProviderOutcome::Challenge => {
                state.penalty_level = state.penalty_level.saturating_add(2);
                state.blocked_until = Some(now + Duration::from_secs(5 * 60));
            }
            ProviderOutcome::UsageLimit => {
                state.penalty_level = state.penalty_level.saturating_add(4);
                state.blocked_until = Some(now + Duration::from_secs(60 * 60));
            }
        }
    }

    fn next_delay(&self, state: &RateState, now: Instant) -> Duration {
        let mut delay = Duration::ZERO;

        if let Some(until) = state.blocked_until {
            delay = delay.max(until.saturating_duration_since(now));
        }

        if let Some(last) = state.last_send {
            let jitter_ms = if self.config.jitter_max.is_zero() {
                0
            } else {
                rand::rng().random_range(0..=self.config.jitter_max.as_millis() as u64)
            };
            let desired = self.config.min_send_interval + Duration::from_millis(jitter_ms);
            delay = delay.max(desired.saturating_sub(now.saturating_duration_since(last)));
        }

        if state.recent_sends.len() >= self.config.max_sends_per_minute {
            if let Some(oldest) = state.recent_sends.front() {
                let release_at = *oldest + Duration::from_secs(60);
                delay = delay.max(release_at.saturating_duration_since(now));
            }
        }

        delay
    }

    fn prune(state: &mut RateState, now: Instant) {
        while state
            .recent_sends
            .front()
            .is_some_and(|sent| now.saturating_duration_since(*sent) >= Duration::from_secs(60))
        {
            state.recent_sends.pop_front();
        }
    }

    fn backoff_for(&self, penalty_level: u32) -> Duration {
        let multiplier = 1u32.checked_shl(penalty_level.min(10)).unwrap_or(u32::MAX);
        self.config
            .base_backoff
            .saturating_mul(multiplier)
            .min(self.config.max_backoff)
    }
}
