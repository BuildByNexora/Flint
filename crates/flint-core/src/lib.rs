mod log;
mod parse;

use std::collections::{HashMap, VecDeque};
use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use chrono::{DateTime, Duration, Utc};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use thiserror::Error;

pub use parse::parse_duration;

#[derive(Debug, Error)]
pub enum FlintError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("invalid duration: {0}")]
    InvalidDuration(String),
    #[error("unsupported algorithm: {0}")]
    UnsupportedAlgorithm(String),
    #[error("data directory is already locked: {path}")]
    DataDirLocked { path: String },
    #[error("limit is not configured: {0}")]
    LimitNotConfigured(String),
    #[error("corrupt log at line {line}: {source}")]
    CorruptLog {
        line: usize,
        source: serde_json::Error,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Algorithm {
    TokenBucket,
    SlidingWindowLog,
    FixedWindowCounter,
}

impl Algorithm {
    pub fn parse(value: &str) -> Result<Self, FlintError> {
        match value {
            "token_bucket" => Ok(Self::TokenBucket),
            "sliding_window_log" => Ok(Self::SlidingWindowLog),
            "fixed_window_counter" => Ok(Self::FixedWindowCounter),
            other => Err(FlintError::UnsupportedAlgorithm(other.to_string())),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LimitConfig {
    pub key: String,
    pub rate: u64,
    pub per_seconds: u64,
    pub algorithm: Algorithm,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckResult {
    pub key: String,
    pub allowed: bool,
    pub remaining: u64,
    pub reset_at: DateTime<Utc>,
    pub algorithm: Algorithm,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LimitSummary {
    pub key: String,
    pub rate: u64,
    pub per_seconds: u64,
    pub algorithm: Algorithm,
    pub remaining: u64,
    pub reset_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Event {
    LimitConfigured { config: LimitConfig },
    Allow { key: String, at: DateTime<Utc> },
    Deny { key: String, at: DateTime<Utc> },
    Reset { key: String, at: DateTime<Utc> },
}

#[derive(Debug, Clone)]
struct BucketState {
    tokens: f64,
    last_refill: DateTime<Utc>,
}

#[derive(Debug, Clone)]
struct FixedWindowState {
    window_start: DateTime<Utc>,
    count: u64,
}

#[derive(Debug, Default)]
struct State {
    configs: HashMap<String, LimitConfig>,
    buckets: HashMap<String, BucketState>,
    fixed_windows: HashMap<String, FixedWindowState>,
    sliding_windows: HashMap<String, VecDeque<DateTime<Utc>>>,
    history: Vec<Event>,
}

pub struct Limiter {
    data_dir: PathBuf,
    log: Mutex<log::AppendOnlyLog>,
    state: Mutex<State>,
    _lock_file: File,
}

impl Limiter {
    pub fn open(data_dir: impl AsRef<Path>) -> Result<Self, FlintError> {
        let data_dir = data_dir.as_ref().to_path_buf();
        std::fs::create_dir_all(&data_dir)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&data_dir, std::fs::Permissions::from_mode(0o700))?;
        }

        let lock_path = data_dir.join("flint.lock");
        #[cfg(unix)]
        let lock_file = {
            use std::os::unix::fs::OpenOptionsExt;
            OpenOptions::new()
                .create(true)
                .read(true)
                .write(true)
                .truncate(false)
                .mode(0o600)
                .open(&lock_path)?
        };
        #[cfg(not(unix))]
        let lock_file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(&lock_path)?;
        lock_file
            .try_lock_exclusive()
            .map_err(|_| FlintError::DataDirLocked {
                path: lock_path.display().to_string(),
            })?;

        let log = log::AppendOnlyLog::open(data_dir.join("flint.aof"))?;
        let events = log.replay()?;
        let mut state = State::default();
        for event in &events {
            apply_event(&mut state, event.clone());
        }

        Ok(Self {
            data_dir,
            log: Mutex::new(log),
            state: Mutex::new(state),
            _lock_file: lock_file,
        })
    }

    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    pub fn limit(
        &self,
        key: impl Into<String>,
        rate: u64,
        per: impl AsRef<str>,
        algorithm: Algorithm,
    ) -> Result<(), FlintError> {
        let config = LimitConfig {
            key: key.into(),
            rate,
            per_seconds: parse_duration(per.as_ref())?,
            algorithm,
        };
        if config.rate == 0 {
            return Err(FlintError::InvalidDuration(
                "rate must be greater than zero".into(),
            ));
        }
        if config.per_seconds == 0 {
            return Err(FlintError::InvalidDuration(
                "duration must be greater than zero".into(),
            ));
        }
        self.append(Event::LimitConfigured { config })
    }

    pub fn allow(&self, key: &str) -> Result<bool, FlintError> {
        Ok(self.check(key)?.allowed)
    }

    pub fn check(&self, key: &str) -> Result<CheckResult, FlintError> {
        let now = Utc::now();
        let mut log = self.log.lock().expect("limiter log lock poisoned");
        let mut state = self.state.lock().expect("limiter state lock poisoned");
        let config = state
            .configs
            .get(key)
            .cloned()
            .ok_or_else(|| FlintError::LimitNotConfigured(key.to_string()))?;

        let result = match config.algorithm {
            Algorithm::TokenBucket => check_token_bucket_preview(&mut state, &config, now),
            Algorithm::SlidingWindowLog => check_sliding_window_preview(&mut state, &config, now),
            Algorithm::FixedWindowCounter => check_fixed_window_preview(&mut state, &config, now),
        };
        let event = if result.allowed {
            Event::Allow {
                key: key.to_string(),
                at: now,
            }
        } else {
            Event::Deny {
                key: key.to_string(),
                at: now,
            }
        };
        log.append(&event)?;
        apply_event(&mut state, event);
        Ok(result)
    }

    pub fn reset(&self, key: &str) -> Result<(), FlintError> {
        let mut log = self.log.lock().expect("limiter log lock poisoned");
        let mut state = self.state.lock().expect("limiter state lock poisoned");
        if !state.configs.contains_key(key) {
            return Err(FlintError::LimitNotConfigured(key.to_string()));
        }
        let event = Event::Reset {
            key: key.to_string(),
            at: Utc::now(),
        };
        log.append(&event)?;
        apply_event(&mut state, event);
        Ok(())
    }

    pub fn status(&self, key: &str) -> Result<Option<LimitSummary>, FlintError> {
        let mut state = self.state.lock().expect("limiter state lock poisoned");
        let Some(config) = state.configs.get(key).cloned() else {
            return Ok(None);
        };
        Ok(Some(summary_for(&mut state, &config, Utc::now())))
    }

    pub fn list(&self) -> Result<Vec<LimitSummary>, FlintError> {
        let mut state = self.state.lock().expect("limiter state lock poisoned");
        let configs = state.configs.values().cloned().collect::<Vec<_>>();
        Ok(configs
            .iter()
            .map(|config| summary_for(&mut state, config, Utc::now()))
            .collect())
    }

    pub fn history(&self, key: &str) -> Result<Vec<Event>, FlintError> {
        let state = self.state.lock().expect("limiter state lock poisoned");
        Ok(state
            .history
            .iter()
            .filter(|event| event_key(event).is_some_and(|candidate| candidate == key))
            .cloned()
            .collect())
    }

    fn append(&self, event: Event) -> Result<(), FlintError> {
        let mut log = self.log.lock().expect("limiter log lock poisoned");
        log.append(&event)?;
        let mut state = self.state.lock().expect("limiter state lock poisoned");
        apply_event(&mut state, event);
        Ok(())
    }
}

fn check_token_bucket_preview(
    state: &mut State,
    config: &LimitConfig,
    now: DateTime<Utc>,
) -> CheckResult {
    let mut bucket = state
        .buckets
        .get(&config.key)
        .cloned()
        .unwrap_or(BucketState {
            tokens: config.rate as f64,
            last_refill: now,
        });
    refill_bucket(&mut bucket, config, now);
    let allowed = bucket.tokens >= 1.0;
    if allowed {
        bucket.tokens -= 1.0;
    }
    token_bucket_result(config, &bucket, allowed, now)
}

fn summary_for(state: &mut State, config: &LimitConfig, now: DateTime<Utc>) -> LimitSummary {
    match config.algorithm {
        Algorithm::TokenBucket => {
            let bucket = state
                .buckets
                .entry(config.key.clone())
                .or_insert(BucketState {
                    tokens: config.rate as f64,
                    last_refill: now,
                });
            refill_bucket(bucket, config, now);
            let result = token_bucket_result(config, bucket, true, now);
            LimitSummary {
                key: config.key.clone(),
                rate: config.rate,
                per_seconds: config.per_seconds,
                algorithm: config.algorithm,
                remaining: result.remaining,
                reset_at: result.reset_at,
            }
        }
        Algorithm::SlidingWindowLog => {
            let result = check_sliding_window_preview(state, config, now);
            LimitSummary {
                key: config.key.clone(),
                rate: config.rate,
                per_seconds: config.per_seconds,
                algorithm: config.algorithm,
                remaining: result.remaining,
                reset_at: result.reset_at,
            }
        }
        Algorithm::FixedWindowCounter => {
            let result = check_fixed_window_preview(state, config, now);
            LimitSummary {
                key: config.key.clone(),
                rate: config.rate,
                per_seconds: config.per_seconds,
                algorithm: config.algorithm,
                remaining: result.remaining,
                reset_at: result.reset_at,
            }
        }
    }
}

fn check_sliding_window_preview(
    state: &mut State,
    config: &LimitConfig,
    now: DateTime<Utc>,
) -> CheckResult {
    let cutoff = now - Duration::seconds(config.per_seconds as i64);
    let mut entries = state
        .sliding_windows
        .get(&config.key)
        .cloned()
        .unwrap_or_default();
    while entries.front().is_some_and(|value| *value <= cutoff) {
        entries.pop_front();
    }
    let reset_at = entries
        .front()
        .map(|first| *first + Duration::seconds(config.per_seconds as i64))
        .unwrap_or(now + Duration::seconds(config.per_seconds as i64));
    CheckResult {
        key: config.key.clone(),
        allowed: entries.len() < config.rate as usize,
        remaining: config.rate.saturating_sub(entries.len() as u64),
        reset_at,
        algorithm: config.algorithm,
    }
}

fn check_fixed_window_preview(
    state: &mut State,
    config: &LimitConfig,
    now: DateTime<Utc>,
) -> CheckResult {
    let per = Duration::seconds(config.per_seconds as i64);
    let mut window = state
        .fixed_windows
        .get(&config.key)
        .cloned()
        .unwrap_or(FixedWindowState {
            window_start: now,
            count: 0,
        });
    if now >= window.window_start + per {
        window.window_start = now;
        window.count = 0;
    }
    CheckResult {
        key: config.key.clone(),
        allowed: window.count < config.rate,
        remaining: config.rate.saturating_sub(window.count),
        reset_at: window.window_start + per,
        algorithm: config.algorithm,
    }
}

fn refill_bucket(bucket: &mut BucketState, config: &LimitConfig, now: DateTime<Utc>) {
    if now <= bucket.last_refill {
        return;
    }
    let elapsed = (now - bucket.last_refill).num_milliseconds().max(0) as f64 / 1000.0;
    let refill = elapsed * (config.rate as f64 / config.per_seconds as f64);
    bucket.tokens = (bucket.tokens + refill).min(config.rate as f64);
    bucket.last_refill = now;
}

fn token_bucket_result(
    config: &LimitConfig,
    bucket: &BucketState,
    allowed: bool,
    now: DateTime<Utc>,
) -> CheckResult {
    let missing = (config.rate as f64 - bucket.tokens).max(0.0);
    let seconds_to_full =
        (missing / (config.rate as f64 / config.per_seconds as f64)).ceil() as i64;
    CheckResult {
        key: config.key.clone(),
        allowed,
        remaining: bucket.tokens.floor() as u64,
        reset_at: now + Duration::seconds(seconds_to_full.max(0)),
        algorithm: config.algorithm,
    }
}

fn apply_event(state: &mut State, event: Event) {
    match event.clone() {
        Event::LimitConfigured { config } => {
            state.configs.insert(config.key.clone(), config);
        }
        Event::Allow { key, at } => apply_consumption(state, &key, at),
        Event::Deny { .. } => {}
        Event::Reset { key, at } => {
            state.buckets.insert(
                key.clone(),
                BucketState {
                    tokens: state.configs.get(&key).map(|c| c.rate).unwrap_or(0) as f64,
                    last_refill: at,
                },
            );
            state.fixed_windows.remove(&key);
            state.sliding_windows.remove(&key);
        }
    }
    state.history.push(event);
}

fn apply_consumption(state: &mut State, key: &str, at: DateTime<Utc>) {
    let Some(config) = state.configs.get(key).cloned() else {
        return;
    };
    match config.algorithm {
        Algorithm::TokenBucket => {
            let bucket = state.buckets.entry(key.to_string()).or_insert(BucketState {
                tokens: config.rate as f64,
                last_refill: at,
            });
            refill_bucket(bucket, &config, at);
            if bucket.tokens >= 1.0 {
                bucket.tokens -= 1.0;
            }
        }
        Algorithm::SlidingWindowLog => {
            let cutoff = at - Duration::seconds(config.per_seconds as i64);
            let entries = state.sliding_windows.entry(key.to_string()).or_default();
            while entries.front().is_some_and(|value| *value <= cutoff) {
                entries.pop_front();
            }
            entries.push_back(at);
        }
        Algorithm::FixedWindowCounter => {
            let per = Duration::seconds(config.per_seconds as i64);
            let window = state
                .fixed_windows
                .entry(key.to_string())
                .or_insert(FixedWindowState {
                    window_start: at,
                    count: 0,
                });
            if at >= window.window_start + per {
                window.window_start = at;
                window.count = 0;
            }
            window.count += 1;
        }
    }
}

fn event_key(event: &Event) -> Option<&str> {
    match event {
        Event::LimitConfigured { config } => Some(&config.key),
        Event::Allow { key, .. } | Event::Deny { key, .. } | Event::Reset { key, .. } => Some(key),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::thread;
    use tempfile::TempDir;

    #[test]
    fn token_bucket_denies_after_capacity_and_recovers_from_log() {
        let dir = TempDir::new().unwrap();
        let limiter = Limiter::open(dir.path()).unwrap();
        limiter
            .limit("api:user-42", 2, "1m", Algorithm::TokenBucket)
            .unwrap();
        assert!(limiter.allow("api:user-42").unwrap());
        assert!(limiter.allow("api:user-42").unwrap());
        assert!(!limiter.allow("api:user-42").unwrap());
        drop(limiter);

        let limiter = Limiter::open(dir.path()).unwrap();
        assert!(!limiter.allow("api:user-42").unwrap());
        assert_eq!(limiter.history("api:user-42").unwrap().len(), 5);
    }

    #[test]
    fn data_dir_lock_is_exclusive() {
        let dir = TempDir::new().unwrap();
        let limiter = Limiter::open(dir.path()).unwrap();
        match Limiter::open(dir.path()) {
            Err(FlintError::DataDirLocked { .. }) => {}
            Ok(_) => panic!("second limiter unexpectedly acquired the data dir lock"),
            Err(err) => panic!("unexpected error: {err}"),
        }
        drop(limiter);
        Limiter::open(dir.path()).unwrap();
    }

    #[test]
    fn concurrent_checks_do_not_over_allow() {
        let dir = TempDir::new().unwrap();
        let limiter = Arc::new(Limiter::open(dir.path()).unwrap());
        limiter
            .limit("api", 1, "1m", Algorithm::TokenBucket)
            .unwrap();

        let mut handles = Vec::new();
        for _ in 0..16 {
            let limiter = Arc::clone(&limiter);
            handles.push(thread::spawn(move || limiter.allow("api").unwrap()));
        }

        let allowed = handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .filter(|allowed| *allowed)
            .count();
        assert_eq!(allowed, 1);
    }

    #[test]
    fn reset_unknown_key_returns_error() {
        let dir = TempDir::new().unwrap();
        let limiter = Limiter::open(dir.path()).unwrap();
        let err = limiter.reset("missing").unwrap_err();
        assert!(matches!(err, FlintError::LimitNotConfigured(_)));
    }

    #[test]
    fn fixed_window_resets_after_period() {
        let dir = TempDir::new().unwrap();
        let limiter = Limiter::open(dir.path()).unwrap();
        limiter
            .limit("x", 1, "1s", Algorithm::FixedWindowCounter)
            .unwrap();
        assert!(limiter.allow("x").unwrap());
        assert!(!limiter.allow("x").unwrap());
        thread::sleep(std::time::Duration::from_millis(1100));
        assert!(limiter.allow("x").unwrap());
    }
}
