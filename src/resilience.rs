use anyhow::{Context, Result};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::Write;
use std::path::PathBuf;
use tracing::warn;

pub const POWER_CYCLE_THRESHOLD: u32 = 5;
pub const POWER_CYCLE_WINDOW: Duration = Duration::minutes(10);
pub const POWER_CYCLE_RATE_LIMIT: Duration = Duration::minutes(15);
pub const REBOOT_EXTRA_FAILURES: u32 = 10;
pub const REBOOT_RATE_LIMIT: Duration = Duration::minutes(30);
pub const REBOOT_MAX_24H: usize = 3;
pub const REBOOT_24H_WINDOW: Duration = Duration::hours(24);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LadderAction {
    None,
    PowerCycle,
    Reboot,
    KeepRetrying,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ResilienceState {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_power_cycle_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub reboot_timestamps: Vec<DateTime<Utc>>,
}

impl ResilienceState {
    pub fn prune_reboots(&mut self, now: DateTime<Utc>) {
        self.reboot_timestamps
            .retain(|t| now.signed_duration_since(*t) < REBOOT_24H_WINDOW);
    }
}

pub struct ResilienceLadder {
    consecutive_failures: u32,
    failure_timestamps: Vec<DateTime<Utc>>,
    state: ResilienceState,
}

impl ResilienceLadder {
    pub fn new() -> Self {
        Self {
            consecutive_failures: 0,
            failure_timestamps: Vec::new(),
            state: ResilienceState::default(),
        }
    }

    pub fn from_state(state: ResilienceState) -> Self {
        Self {
            consecutive_failures: 0,
            failure_timestamps: Vec::new(),
            state,
        }
    }

    pub fn state(&self) -> &ResilienceState {
        &self.state
    }

    pub fn consecutive_failures(&self) -> u32 {
        self.consecutive_failures
    }

    pub fn on_connection_success(&mut self) {
        self.consecutive_failures = 0;
        self.failure_timestamps.clear();
    }

    pub fn on_connection_failure(&mut self, now: DateTime<Utc>) -> LadderAction {
        self.consecutive_failures += 1;
        self.failure_timestamps.push(now);
        self.prune_failure_window(now);
        self.state.prune_reboots(now);

        // Rung 2: 5 consecutive failures within a rolling 10-minute window -> PowerCycle.
        if self.consecutive_failures >= POWER_CYCLE_THRESHOLD
            && self.failures_within_window(now) >= POWER_CYCLE_THRESHOLD
        {
            let rate_limited = self
                .state
                .last_power_cycle_at
                .map(|last| now.signed_duration_since(last) < POWER_CYCLE_RATE_LIMIT)
                .unwrap_or(false);
            if rate_limited {
                warn!(
                    "Power-cycle skipped due to rate limit (last at {:?}); continuing to reboot rung",
                    self.state.last_power_cycle_at
                );
            } else {
                self.state.last_power_cycle_at = Some(now);
                return LadderAction::PowerCycle;
            }
        }

        // Rung 3: 10 further consecutive failures after a power-cycle has been attempted -> Reboot.
        if self.state.last_power_cycle_at.is_some()
            && self.consecutive_failures >= POWER_CYCLE_THRESHOLD + REBOOT_EXTRA_FAILURES
        {
            let within_30min = self
                .state
                .reboot_timestamps
                .iter()
                .any(|t| now.signed_duration_since(*t) < REBOOT_RATE_LIMIT);
            let within_24h = self
                .state
                .reboot_timestamps
                .iter()
                .filter(|t| now.signed_duration_since(**t) < REBOOT_24H_WINDOW)
                .count();
            if within_30min || within_24h >= REBOOT_MAX_24H {
                warn!(
                    "Reboot skipped due to rate limit (within_30min={}, within_24h={})",
                    within_30min, within_24h
                );
            } else {
                self.state.reboot_timestamps.push(now);
                return LadderAction::Reboot;
            }
        }

        // Rung 4: once the 24h reboot cap is reached, stop escalating.
        let within_24h = self
            .state
            .reboot_timestamps
            .iter()
            .filter(|t| now.signed_duration_since(**t) < REBOOT_24H_WINDOW)
            .count();
        if within_24h >= REBOOT_MAX_24H {
            return LadderAction::KeepRetrying;
        }

        LadderAction::None
    }

    fn failures_within_window(&self, now: DateTime<Utc>) -> u32 {
        self.failure_timestamps
            .iter()
            .filter(|t| now.signed_duration_since(**t) < POWER_CYCLE_WINDOW)
            .count() as u32
    }

    fn prune_failure_window(&mut self, now: DateTime<Utc>) {
        self.failure_timestamps
            .retain(|t| now.signed_duration_since(*t) < POWER_CYCLE_WINDOW);
    }
}

impl Default for ResilienceLadder {
    fn default() -> Self {
        Self::new()
    }
}

pub struct ResilienceStateStore {
    path: PathBuf,
}

impl ResilienceStateStore {
    pub fn new(device_name: &str) -> Self {
        Self {
            path: PathBuf::from(format!(
                "/var/lib/rshunterbtt/{}-resilience-state.json",
                device_name
            )),
        }
    }

    pub fn with_path(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn load(&self) -> Result<ResilienceState> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("creating state dir {}", parent.display()))?;
        }
        match fs::read(&self.path) {
            Ok(bytes) => {
                let mut state: ResilienceState = serde_json::from_slice(&bytes)
                    .with_context(|| format!("parsing state file {}", self.path.display()))?;
                state.prune_reboots(Utc::now());
                Ok(state)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(ResilienceState::default()),
            Err(e) => Err(e).with_context(|| format!("reading state file {}", self.path.display())),
        }
    }

    pub fn save(&self, state: &ResilienceState) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("creating state dir {}", parent.display()))?;
        }
        let json = serde_json::to_vec_pretty(state)?;
        let mut file = fs::File::create(&self.path)
            .with_context(|| format!("creating state file {}", self.path.display()))?;
        file.write_all(&json)?;
        file.flush()?;
        file.sync_all()?;
        if let Some(parent) = self.path.parent() {
            let dir = fs::File::open(parent)
                .with_context(|| format!("opening state dir {}", parent.display()))?;
            dir.sync_all()?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(secs: i64) -> DateTime<Utc> {
        DateTime::from_timestamp(secs, 0).unwrap()
    }

    #[test]
    fn no_action_on_first_failures() {
        let mut ladder = ResilienceLadder::new();
        let now = t(1_000);
        for i in 0..4 {
            assert_eq!(
                ladder.on_connection_failure(now + Duration::seconds(i)),
                LadderAction::None
            );
        }
    }

    #[test]
    fn five_failures_within_window_power_cycles() {
        let mut ladder = ResilienceLadder::new();
        let now = t(1_000);
        for i in 0..4 {
            ladder.on_connection_failure(now + Duration::seconds(i));
        }
        assert_eq!(
            ladder.on_connection_failure(now + Duration::seconds(4)),
            LadderAction::PowerCycle
        );
    }

    #[test]
    fn sixth_failure_within_window_is_rate_limited() {
        let mut ladder = ResilienceLadder::new();
        let now = t(1_000);
        for i in 0..5 {
            ladder.on_connection_failure(now + Duration::seconds(i));
        }
        assert_eq!(
            ladder.on_connection_failure(now + Duration::seconds(5)),
            LadderAction::None
        );
    }

    #[test]
    fn success_resets_counter() {
        let mut ladder = ResilienceLadder::new();
        let now = t(1_000);
        for i in 0..5 {
            ladder.on_connection_failure(now + Duration::seconds(i));
        }
        ladder.on_connection_success();
        assert_eq!(ladder.consecutive_failures(), 0);
        assert_eq!(
            ladder.on_connection_failure(now + Duration::seconds(100)),
            LadderAction::None
        );
    }

    #[test]
    fn fifteen_failures_reboots() {
        let mut ladder = ResilienceLadder::new();
        let now = t(1_000);
        for i in 0..15 {
            ladder.on_connection_failure(now + Duration::seconds(i));
        }
        // 15th failure (index 14) triggers reboot
        assert_eq!(ladder.state().reboot_timestamps.len(), 1);
    }

    #[test]
    fn reboot_rate_limited_within_30min() {
        let mut ladder = ResilienceLadder::new();
        let now = t(1_000);
        for i in 0..15 {
            ladder.on_connection_failure(now + Duration::seconds(i));
        }
        // Next failure within 30 min of reboot -> rate limited, no new reboot
        let action = ladder.on_connection_failure(now + Duration::minutes(5));
        assert_eq!(action, LadderAction::None);
        assert_eq!(ladder.state().reboot_timestamps.len(), 1);
    }

    #[test]
    fn reboot_cap_reaches_keep_retrying() {
        let mut ladder = ResilienceLadder::new();
        let mut now = t(1_000);
        // First reboot
        for i in 0..15 {
            ladder.on_connection_failure(now + Duration::seconds(i));
        }
        // Advance past 30-min reboot rate limit, do 2nd reboot
        now += Duration::minutes(31);
        for i in 0..15 {
            ladder.on_connection_failure(now + Duration::seconds(i));
        }
        // Advance past 30-min, do 3rd reboot
        now += Duration::minutes(31);
        for i in 0..15 {
            ladder.on_connection_failure(now + Duration::seconds(i));
        }
        assert_eq!(ladder.state().reboot_timestamps.len(), 3);
        // Advance past 30-min; 4th would exceed 24h cap -> KeepRetrying
        now += Duration::minutes(31);
        let action = ladder.on_connection_failure(now + Duration::seconds(0));
        assert_eq!(action, LadderAction::KeepRetrying);
    }

    #[test]
    fn state_round_trip_and_prune() {
        let dir = std::env::temp_dir().join(format!("rshunterbtt-test-{}", std::process::id()));
        let path = dir.join("dev-resilience-state.json");
        let store = ResilienceStateStore::with_path(&path);
        let now = Utc::now();
        let state = ResilienceState {
            last_power_cycle_at: Some(now),
            reboot_timestamps: vec![now, now - Duration::hours(25)],
        };
        store.save(&state).unwrap();
        let loaded = store.load().unwrap();
        // 25h-old reboot pruned on load
        assert_eq!(loaded.reboot_timestamps.len(), 1);
        assert_eq!(loaded.last_power_cycle_at, Some(now));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
