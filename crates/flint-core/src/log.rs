use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::{Event, FlintError};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogEntry {
    pub ts: DateTime<Utc>,
    pub event: Event,
}

pub struct AppendOnlyLog {
    path: PathBuf,
    file: File,
}

impl AppendOnlyLog {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, FlintError> {
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
        Ok(Self { path, file })
    }

    pub fn append(&mut self, event: &Event) -> Result<(), FlintError> {
        let entry = LogEntry {
            ts: Utc::now(),
            event: event.clone(),
        };
        let mut encoded = serde_json::to_string(&entry)?;
        encoded.push('\n');
        self.file.write_all(encoded.as_bytes())?;
        self.file.sync_data()?;
        Ok(())
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
                Ok(entry) => events.push(entry.event),
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
        Ok(())
    }
}
