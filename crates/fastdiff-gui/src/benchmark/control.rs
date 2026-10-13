use serde_json::Value;
use std::{
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

/// Prints each distinct failure once; a success re-arms the report.
#[derive(Default)]
pub(super) struct FailureLog {
    last: Option<String>,
    /// Lines printed so far.
    pub(super) printed: u32,
}

impl FailureLog {
    /// Report `failure` unless it repeats the previous one; true when it was printed.
    pub(super) fn record(&mut self, failure: String) -> bool {
        if self.last.as_deref() == Some(failure.as_str()) {
            return false;
        }
        eprintln!("{failure}");
        self.last = Some(failure);
        self.printed += 1;
        true
    }

    pub(super) fn clear(&mut self) {
        self.last = None;
    }
}

/// Reads of the benchmark control file next to a report.
///
/// Per-frame consumers share one throttled read: a poll interval costs at most one file read
/// whoever asks, and a persistently unreadable file is reported once.
#[derive(Default)]
pub(super) struct ControlPoll {
    pub(super) last_poll: Option<Instant>,
    /// The most recent poll's content; `None` when the file was missing or unusable.
    pub(super) latest: Option<Value>,
    /// A poll that no edit-trigger evaluation has looked at yet.
    unseen: bool,
    pub(super) failures: FailureLog,
}

impl ControlPoll {
    /// Read the control file now, outside the poll schedule (an open request just arrived).
    pub(super) fn read(&mut self, report: &Path) -> Option<Value> {
        let path = control_path(report);
        let failure = match std::fs::read(&path) {
            Ok(bytes) => match serde_json::from_slice(&bytes) {
                Ok(value) => {
                    self.failures.clear();
                    return Some(value);
                }
                Err(error) => {
                    format!("Invalid benchmark control {}: {error}", path.display())
                }
            },
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                self.failures.clear();
                return None;
            }
            Err(error) => format!(
                "Could not read benchmark control {}: {error}",
                path.display()
            ),
        };
        self.failures.record(failure);
        None
    }

    /// Read the control file unless the last poll is younger than `interval`; true when read.
    pub(super) fn poll(&mut self, report: &Path, now: Instant, interval: Duration) -> bool {
        if self
            .last_poll
            .is_some_and(|last| now.saturating_duration_since(last) < interval)
        {
            return false;
        }
        self.last_poll = Some(now);
        self.latest = self.read(report);
        self.unseen = true;
        true
    }

    /// Whether a poll is waiting to be evaluated; asking consumes it.
    pub(super) fn take_unseen(&mut self) -> bool {
        std::mem::take(&mut self.unseen)
    }

    /// Time until a poll at `interval` is due, measured from the last one.
    pub(super) fn due_in(&self, now: Instant, interval: Duration) -> Duration {
        self.last_poll.map_or(interval, |last| {
            interval.saturating_sub(now.saturating_duration_since(last))
        })
    }
}

fn control_path(report: &Path) -> PathBuf {
    let mut path = report.as_os_str().to_os_string();
    path.push(".request.json");
    PathBuf::from(path)
}
