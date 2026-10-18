//! Sends the queue's closed files to the telemetry server and deletes each
//! one the server took. Waits longer after each failure.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use flate2::Compression;
use flate2::write::GzEncoder;

/// How often the queue is sent, and the longest wait after failures.
const EVERY: Duration = Duration::from_secs(60);
const MAX_BACKOFF: Duration = Duration::from_secs(15 * 60);

pub(super) struct Uploader {
    agent: ureq::Agent,
    base: String,
    key: String,
    install: String,
    dir: PathBuf,
    next: Instant,
    backoff: Duration,
}

/// What the server made of one file.
enum Sent {
    /// Taken, or refused for good (malformed, too big): delete it.
    Done,
    /// Try again later.
    Later,
}

impl Uploader {
    pub fn new(key: String, install: String, dir: PathBuf) -> Self {
        let base = std::env::var("BD_TELEMETRY_URL").unwrap_or_else(|_| "https://beardictate-telemetry.jkbase.app".into());
        let agent = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(30)))
            .http_status_as_error(false)
            .build()
            .new_agent();
        // The first send soon after start, for what an earlier run left.
        Self { agent, base, key, install, dir, next: Instant::now() + Duration::from_secs(10), backoff: EVERY }
    }

    pub fn due(&self) -> bool {
        Instant::now() >= self.next
    }

    /// Sends the closed files, oldest first: events, then audio. With a
    /// `budget`, stops when it runs out (at quit).
    pub fn send_waiting(&mut self, budget: Option<Duration>) {
        let started = Instant::now();
        let over = || budget.is_some_and(|b| started.elapsed() >= b);
        let mut ok = true;
        for path in waiting(&self.dir, "events-", ".ndjson") {
            if over() {
                return;
            }
            match self.send_events(&path) {
                Sent::Done => remove(&path),
                Sent::Later => {
                    ok = false;
                    break;
                }
            }
        }
        if ok {
            for path in waiting(&self.dir.join("audio"), "", ".flac") {
                if over() {
                    return;
                }
                match self.send_audio(&path) {
                    Sent::Done => remove(&path),
                    Sent::Later => {
                        ok = false;
                        break;
                    }
                }
            }
        }
        self.backoff = if ok { EVERY } else { (self.backoff * 2).min(MAX_BACKOFF) };
        self.next = Instant::now() + self.backoff;
    }

    fn send_events(&self, path: &Path) -> Sent {
        let Ok(body) = fs::read(path) else { return Sent::Done };
        let mut gz = GzEncoder::new(Vec::new(), Compression::default());
        if gz.write_all(&body).is_err() {
            return Sent::Done;
        }
        let Ok(gz) = gz.finish() else { return Sent::Done };
        let response = self
            .agent
            .post(format!("{}/v1/events", self.base))
            .header("Authorization", format!("Bearer {}", self.key))
            .header("Content-Type", "application/x-ndjson")
            .header("Content-Encoding", "gzip")
            .send(&gz[..]);
        outcome(response.map(|r| r.status().as_u16()))
    }

    fn send_audio(&self, path: &Path) -> Sent {
        // `<session>-<segment>.flac`.
        let name = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        let Some((session, segment)) = name.rsplit_once('-') else { return Sent::Done };
        let Ok(body) = fs::read(path) else { return Sent::Done };
        let response = self
            .agent
            .put(format!("{}/v1/audio/{}/{session}/{segment}.flac", self.base, self.install))
            .header("Authorization", format!("Bearer {}", self.key))
            .header("Content-Type", "audio/flac")
            .send(&body[..]);
        outcome(response.map(|r| r.status().as_u16()))
    }
}

fn outcome(status: Result<u16, ureq::Error>) -> Sent {
    match status {
        Ok(200..=299 | 400 | 413) => Sent::Done,
        Ok(status) => {
            tracing::debug!(status, "telemetry upload refused; trying later");
            Sent::Later
        }
        Err(e) => {
            tracing::debug!("telemetry upload failed: {e}");
            Sent::Later
        }
    }
}

/// Files in `dir` named `<prefix>…<suffix>`, oldest name first.
fn waiting(dir: &Path, prefix: &str, suffix: &str) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(dir) else { return Vec::new() };
    let mut files: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.file_name().is_some_and(|n| n.to_string_lossy().starts_with(prefix) && n.to_string_lossy().ends_with(suffix)))
        .collect();
    files.sort();
    files
}

fn remove(path: &Path) {
    let _ = fs::remove_file(path);
}
