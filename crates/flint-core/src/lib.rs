mod log;
mod parse;

use std::collections::{HashMap, VecDeque};
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use chrono::{DateTime, Duration, Utc};
use fs2::FileExt;
use serde::{de, Deserialize, Deserializer, Serialize};
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
    #[error("unsupported snapshot format version: {0}")]
    UnsupportedSnapshot(u32),
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

#[derive(Debug, Clone, Serialize)]
pub struct LimitConfig {
    pub key: String,
    pub rate: u64,
    pub per_millis: u64,
    pub algorithm: Algorithm,
}

impl<'de> Deserialize<'de> for LimitConfig {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct LimitConfigWire {
            key: String,
            rate: u64,
            #[serde(default)]
            per_millis: Option<u64>,
            #[serde(default)]
            per_seconds: Option<u64>,
            algorithm: Algorithm,
        }

        let wire = LimitConfigWire::deserialize(deserializer)?;
        let per_millis = match (wire.per_millis, wire.per_seconds) {
            (Some(per_millis), _) => per_millis,
            (None, Some(per_seconds)) => per_seconds
                .checked_mul(1000)
                .ok_or_else(|| de::Error::custom("per_seconds overflows milliseconds"))?,
            (None, None) => {
                return Err(de::Error::missing_field("per_millis"));
            }
        };

        Ok(Self {
            key: wire.key,
            rate: wire.rate,
            per_millis,
            algorithm: wire.algorithm,
        })
    }
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
    pub per_millis: u64,
    pub algorithm: Algorithm,
    pub remaining: u64,
    pub reset_at: DateTime<Utc>,
    pub total_allowed: u64,
    pub total_denied: u64,
    pub last_allowed_at: Option<DateTime<Utc>>,
    pub last_denied_at: Option<DateTime<Utc>>,
    pub last_reset_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Event {
    LimitConfigured { config: LimitConfig },
    Allow { key: String, at: DateTime<Utc> },
    Deny { key: String, at: DateTime<Utc> },
    Reset { key: String, at: DateTime<Utc> },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct BucketState {
    tokens: f64,
    last_refill: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct FixedWindowState {
    window_start: DateTime<Utc>,
    count: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LimitMetrics {
    pub total_allowed: u64,
    pub total_denied: u64,
    pub last_allowed_at: Option<DateTime<Utc>>,
    pub last_denied_at: Option<DateTime<Utc>>,
    pub last_reset_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct State {
    configs: HashMap<String, LimitConfig>,
    buckets: HashMap<String, BucketState>,
    fixed_windows: HashMap<String, FixedWindowState>,
    sliding_windows: HashMap<String, VecDeque<DateTime<Utc>>>,
    metrics: HashMap<String, LimitMetrics>,
    history: Vec<Event>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DoctorReport {
    pub ok: bool,
    pub limits: usize,
    pub history_events: usize,
    pub aof_bytes: u64,
    pub snapshot_exists: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TopEntry {
    pub key: String,
    pub total_allowed: u64,
    pub total_denied: u64,
}

#[derive(Debug, Clone, Copy)]
pub enum TopBy {
    Allowed,
    Denied,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Snapshot {
    format_version: u32,
    created_at: DateTime<Utc>,
    aof_offset: u64,
    state: State,
}

const SNAPSHOT_FORMAT_VERSION: u32 = 1;

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
        let (mut state, offset) = read_snapshot(&data_dir)?.unwrap_or_default();
        for event in log.replay_from(offset)? {
            apply_event(&mut state, event);
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
            per_millis: parse_duration(per.as_ref())?,
            algorithm,
        };
        if config.rate == 0 {
            return Err(FlintError::InvalidDuration(
                "rate must be greater than zero".into(),
            ));
        }
        if config.per_millis == 0 {
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

    pub fn compact(&self) -> Result<(), FlintError> {
        let mut log = self.log.lock().expect("limiter log lock poisoned");
        let mut state = self.state.lock().expect("limiter state lock poisoned");
        refresh_all_summaries(&mut state);
        let snapshot = Snapshot {
            format_version: SNAPSHOT_FORMAT_VERSION,
            created_at: Utc::now(),
            aof_offset: log.len()?,
            state: state.clone(),
        };
        write_snapshot(&self.data_dir, &snapshot)?;
        log.truncate()?;
        write_snapshot(
            &self.data_dir,
            &Snapshot {
                aof_offset: 0,
                ..snapshot
            },
        )
    }

    pub fn doctor(&self) -> Result<DoctorReport, FlintError> {
        let snapshot_exists = self.data_dir.join("flint.snapshot").exists();
        let _ = read_snapshot(&self.data_dir)?;
        let aof_bytes = {
            let log = self.log.lock().expect("limiter log lock poisoned");
            let _ = log.replay_from(0)?;
            log.len()?
        };
        let state = self.state.lock().expect("limiter state lock poisoned");
        Ok(DoctorReport {
            ok: true,
            limits: state.configs.len(),
            history_events: state.history.len(),
            aof_bytes,
            snapshot_exists,
        })
    }

    pub fn top(&self, by: TopBy, limit: usize) -> Result<Vec<TopEntry>, FlintError> {
        let state = self.state.lock().expect("limiter state lock poisoned");
        let mut entries = state
            .metrics
            .iter()
            .map(|(key, metrics)| TopEntry {
                key: key.clone(),
                total_allowed: metrics.total_allowed,
                total_denied: metrics.total_denied,
            })
            .collect::<Vec<_>>();
        entries.sort_by_key(|entry| {
            std::cmp::Reverse(match by {
                TopBy::Allowed => entry.total_allowed,
                TopBy::Denied => entry.total_denied,
            })
        });
        entries.truncate(limit);
        Ok(entries)
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
    let result = match config.algorithm {
        Algorithm::TokenBucket => {
            let bucket = state
                .buckets
                .entry(config.key.clone())
                .or_insert(BucketState {
                    tokens: config.rate as f64,
                    last_refill: now,
                });
            refill_bucket(bucket, config, now);
            token_bucket_result(config, bucket, true, now)
        }
        Algorithm::SlidingWindowLog => check_sliding_window_preview(state, config, now),
        Algorithm::FixedWindowCounter => check_fixed_window_preview(state, config, now),
    };
    let metrics = state.metrics.get(&config.key).cloned().unwrap_or_default();
    LimitSummary {
        key: config.key.clone(),
        rate: config.rate,
        per_millis: config.per_millis,
        algorithm: config.algorithm,
        remaining: result.remaining,
        reset_at: result.reset_at,
        total_allowed: metrics.total_allowed,
        total_denied: metrics.total_denied,
        last_allowed_at: metrics.last_allowed_at,
        last_denied_at: metrics.last_denied_at,
        last_reset_at: metrics.last_reset_at,
    }
}

fn check_sliding_window_preview(
    state: &mut State,
    config: &LimitConfig,
    now: DateTime<Utc>,
) -> CheckResult {
    let cutoff = now - duration_ms(config.per_millis);
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
        .map(|first| *first + duration_ms(config.per_millis))
        .unwrap_or(now + duration_ms(config.per_millis));
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
    let per = duration_ms(config.per_millis);
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
    let elapsed_ms = (now - bucket.last_refill).num_milliseconds().max(0) as f64;
    let refill = elapsed_ms * (config.rate as f64 / config.per_millis as f64);
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
    let millis_to_full = (missing / (config.rate as f64 / config.per_millis as f64)).ceil() as i64;
    CheckResult {
        key: config.key.clone(),
        allowed,
        remaining: bucket.tokens.floor() as u64,
        reset_at: now + Duration::milliseconds(millis_to_full.max(0)),
        algorithm: config.algorithm,
    }
}

fn apply_event(state: &mut State, event: Event) {
    match event.clone() {
        Event::LimitConfigured { config } => {
            state.metrics.entry(config.key.clone()).or_default();
            state.configs.insert(config.key.clone(), config);
        }
        Event::Allow { key, at } => {
            let metrics = state.metrics.entry(key.clone()).or_default();
            metrics.total_allowed += 1;
            metrics.last_allowed_at = Some(at);
            apply_consumption(state, &key, at);
        }
        Event::Deny { key, at } => {
            let metrics = state.metrics.entry(key).or_default();
            metrics.total_denied += 1;
            metrics.last_denied_at = Some(at);
        }
        Event::Reset { key, at } => {
            let metrics = state.metrics.entry(key.clone()).or_default();
            metrics.last_reset_at = Some(at);
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
            let cutoff = at - duration_ms(config.per_millis);
            let entries = state.sliding_windows.entry(key.to_string()).or_default();
            while entries.front().is_some_and(|value| *value <= cutoff) {
                entries.pop_front();
            }
            entries.push_back(at);
        }
        Algorithm::FixedWindowCounter => {
            let per = duration_ms(config.per_millis);
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

fn duration_ms(ms: u64) -> Duration {
    Duration::milliseconds(ms.min(i64::MAX as u64) as i64)
}

fn refresh_all_summaries(state: &mut State) {
    let configs = state.configs.values().cloned().collect::<Vec<_>>();
    for config in configs {
        let _ = summary_for(state, &config, Utc::now());
    }
}

fn snapshot_path(data_dir: &Path) -> PathBuf {
    data_dir.join("flint.snapshot")
}

fn read_snapshot(data_dir: &Path) -> Result<Option<(State, u64)>, FlintError> {
    let path = snapshot_path(data_dir);
    if !path.exists() {
        return Ok(None);
    }
    let snapshot: Snapshot = serde_json::from_slice(&std::fs::read(path)?)?;
    if snapshot.format_version != SNAPSHOT_FORMAT_VERSION {
        return Err(FlintError::UnsupportedSnapshot(snapshot.format_version));
    }
    Ok(Some((snapshot.state, snapshot.aof_offset)))
}

fn write_snapshot(data_dir: &Path, snapshot: &Snapshot) -> Result<(), FlintError> {
    let tmp = data_dir.join("flint.snapshot.tmp");
    let mut file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(&tmp)?;
    serde_json::to_writer_pretty(&mut file, snapshot)?;
    file.write_all(b"\n")?;
    file.sync_all()?;
    std::fs::rename(&tmp, snapshot_path(data_dir))?;
    File::open(data_dir)?.sync_all()?;
    Ok(())
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
        assert_eq!(
            limiter.status("api:user-42").unwrap().unwrap().total_denied,
            2
        );
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
        for _ in 0..100 {
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
            .limit("x", 1, "100ms", Algorithm::FixedWindowCounter)
            .unwrap();
        assert!(limiter.allow("x").unwrap());
        assert!(!limiter.allow("x").unwrap());
        thread::sleep(std::time::Duration::from_millis(130));
        assert!(limiter.allow("x").unwrap());
    }

    #[test]
    fn compaction_preserves_status_and_metrics() {
        let dir = TempDir::new().unwrap();
        let limiter = Limiter::open(dir.path()).unwrap();
        limiter.limit("x", 2, "1m", Algorithm::TokenBucket).unwrap();
        assert!(limiter.allow("x").unwrap());
        assert!(limiter.allow("x").unwrap());
        assert!(!limiter.allow("x").unwrap());
        limiter.compact().unwrap();
        drop(limiter);
        let limiter = Limiter::open(dir.path()).unwrap();
        let status = limiter.status("x").unwrap().unwrap();
        assert_eq!(status.remaining, 0);
        assert_eq!(status.total_allowed, 2);
        assert_eq!(status.total_denied, 1);
    }

    #[test]
    fn legacy_per_seconds_log_entries_are_migrated_to_millis() {
        let dir = TempDir::new().unwrap();
        std::fs::create_dir_all(dir.path()).unwrap();
        let legacy_entry = r#"{"ts":"2026-06-10T08:00:00Z","event":{"type":"LIMIT_CONFIGURED","config":{"key":"legacy","rate":7,"per_seconds":60,"algorithm":"token_bucket"}}}"#;
        std::fs::write(dir.path().join("flint.aof"), format!("{legacy_entry}\n")).unwrap();

        let limiter = Limiter::open(dir.path()).unwrap();
        let status = limiter.status("legacy").unwrap().unwrap();
        assert_eq!(status.rate, 7);
        assert_eq!(status.per_millis, 60_000);
    }

    #[test]
    fn doctor_fails_on_corrupt_middle_log_record() {
        let dir = TempDir::new().unwrap();
        let limiter = Limiter::open(dir.path()).unwrap();
        limiter.limit("x", 1, "1m", Algorithm::TokenBucket).unwrap();
        drop(limiter);

        let path = dir.path().join("flint.aof");
        let mut file = OpenOptions::new().append(true).open(&path).unwrap();
        writeln!(file, "{{bad json").unwrap();
        let valid_tail = r#"{"ts":"2026-06-10T08:00:00Z","event":{"type":"DENY","key":"x","at":"2026-06-10T08:00:00Z"}}"#;
        writeln!(file, "{valid_tail}").unwrap();

        match Limiter::open(dir.path()) {
            Err(FlintError::CorruptLog { .. }) => {}
            Ok(_) => panic!("corrupt log unexpectedly opened"),
            Err(err) => panic!("unexpected error: {err}"),
        }
    }

    #[test]
    fn ten_thousand_keys_can_be_configured() {
        let dir = TempDir::new().unwrap();
        let limiter = Limiter::open(dir.path()).unwrap();
        for idx in 0..10_000 {
            limiter
                .limit(format!("k{idx}"), 1, "1m", Algorithm::TokenBucket)
                .unwrap();
        }
        assert_eq!(limiter.list().unwrap().len(), 10_000);
    }
}
