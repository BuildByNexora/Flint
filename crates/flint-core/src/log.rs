use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{Event, FlintError};

const LOG_FORMAT_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogEntry {
    #[serde(default)]
    pub version: Option<u32>,
    pub ts: DateTime<Utc>,
    pub event: Event,
    #[serde(default)]
    pub checksum: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
struct LogPayload<'a> {
    version: u32,
    ts: DateTime<Utc>,
    event: &'a Event,
}

pub struct AppendOnlyLog {
    path: PathBuf,
    file: File,
    sync_policy: SyncPolicy,
    pending_events: u64,
    last_sync: Instant,
}

impl AppendOnlyLog {
    pub fn open_with_sync(
        path: impl AsRef<Path>,
        sync_policy: SyncPolicy,
    ) -> Result<Self, FlintError> {
        let path = path.as_ref().to_path_buf();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        #[cfg(unix)]
        let file = {
            use std::os::unix::fs::OpenOptionsExt;
            OpenOptions::new()
                .create(true)
                .append(true)
                .mode(0o600)
                .open(&path)?
        };
        #[cfg(not(unix))]
        let file = OpenOptions::new().create(true).append(true).open(&path)?;
        Ok(Self {
            path,
            file,
            sync_policy,
            pending_events: 0,
            last_sync: Instant::now(),
        })
    }

    pub fn append(&mut self, event: &Event) -> Result<(), FlintError> {
        let ts = Utc::now();
        let checksum = checksum_payload(&LogPayload {
            version: LOG_FORMAT_VERSION,
            ts,
            event,
        })?;
        let entry = LogEntry {
            version: Some(LOG_FORMAT_VERSION),
            ts,
            event: event.clone(),
            checksum: Some(checksum),
        };
        let mut encoded = serde_json::to_string(&entry)?;
        encoded.push('\n');
        self.file.write_all(encoded.as_bytes())?;
        self.pending_events = self.pending_events.saturating_add(1);
        if self.should_sync() {
            self.flush()?;
        }
        Ok(())
    }

    pub fn flush(&mut self) -> Result<(), FlintError> {
        if self.pending_events == 0 {
            return Ok(());
        }
        self.file.flush()?;
        self.file.sync_data()?;
        self.pending_events = 0;
        self.last_sync = Instant::now();
        Ok(())
    }

    pub fn has_pending_events(&self) -> bool {
        self.pending_events > 0
    }

    pub fn len(&self) -> Result<u64, FlintError> {
        Ok(self.file.metadata()?.len())
    }

    pub fn replay_from(&self, offset: u64) -> Result<Vec<Event>, FlintError> {
        let file = File::open(&self.path)?;
        let mut file = file;
        file.seek(SeekFrom::Start(offset))?;
        let reader = BufReader::new(file);
        let lines = reader.lines().collect::<Result<Vec<_>, _>>()?;
        let last_non_empty = lines.iter().rposition(|line| !line.trim().is_empty());
        let mut events = Vec::new();
        for (line_no, line) in lines.iter().enumerate() {
            if line.trim().is_empty() {
                continue;
            }
            match serde_json::from_str::<LogEntry>(line) {
                Ok(entry) => {
                    verify_entry_checksum(&entry, line_no + 1)?;
                    events.push(entry.event);
                }
                Err(err) if Some(line_no) == last_non_empty && err.is_eof() => break,
                Err(source) => {
                    return Err(FlintError::CorruptLog {
                        line: line_no + 1,
                        source,
                    });
                }
            }
        }
        Ok(events)
    }

    pub fn truncate(&mut self) -> Result<(), FlintError> {
        self.file.set_len(0)?;
        self.file.seek(SeekFrom::Start(0))?;
        self.file.sync_all()?;
        self.pending_events = 0;
        self.last_sync = Instant::now();
        Ok(())
    }

    fn should_sync(&self) -> bool {
        match self.sync_policy {
            SyncPolicy::Always => true,
            SyncPolicy::Batch {
                flush_every_events,
                flush_every,
            } => {
                self.pending_events >= flush_every_events || self.last_sync.elapsed() >= flush_every
            }
        }
    }
}

impl Drop for AppendOnlyLog {
    fn drop(&mut self) {
        if self.pending_events > 0 {
            let _ = self.flush();
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub enum SyncPolicy {
    Always,
    Batch {
        flush_every_events: u64,
        flush_every: Duration,
    },
}

fn verify_entry_checksum(entry: &LogEntry, line: usize) -> Result<(), FlintError> {
    let Some(expected) = &entry.checksum else {
        return Ok(());
    };
    let Some(version) = entry.version else {
        return Err(FlintError::StorageIntegrity(format!(
            "AOF checksummed record at line {line} is missing format version"
        )));
    };
    if version != LOG_FORMAT_VERSION {
        return Err(FlintError::StorageIntegrity(format!(
            "unsupported AOF format version {version} at line {line}"
        )));
    }
    let actual = checksum_payload(&LogPayload {
        version,
        ts: entry.ts,
        event: &entry.event,
    })?;
    if !constant_time_eq(expected.as_bytes(), actual.as_bytes()) {
        return Err(FlintError::StorageIntegrity(format!(
            "AOF checksum mismatch at line {line}"
        )));
    }
    Ok(())
}

fn checksum_payload(payload: &LogPayload<'_>) -> Result<String, FlintError> {
    let bytes = serde_json::to_vec(payload)?;
    Ok(hex_sha256(&bytes))
}

fn hex_sha256(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        use std::fmt::Write as _;
        let _ = write!(out, "{byte:02x}");
    }
    out
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.iter()
        .zip(right)
        .fold(0_u8, |acc, (left, right)| acc | (left ^ right))
        == 0
}
