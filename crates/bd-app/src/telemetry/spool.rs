//! The local queue: events appended to rotating NDJSON files, each
//! segment's audio as FLAC, both under size caps. The uploader sends the
//! closed files and deletes them.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crossbeam_channel::{Receiver, RecvTimeoutError, Sender, bounded, unbounded};
use serde_json::{Value, json};

use super::upload::Uploader;
use crate::settings::Paths;

/// A file of events is closed (and can be sent) at this size, or this old.
const EVENTS_FILE_BYTES: u64 = 256 * 1024;
const EVENTS_FILE_AGE: Duration = Duration::from_secs(60);
/// What may wait to be sent; past it the oldest goes.
const MAX_EVENTS_BYTES: u64 = 20 << 20;
const MAX_AUDIO_BYTES: u64 = 200 << 20;

pub(super) struct Recorder {
    tx: Sender<Msg>,
    pub install: String,
    pub session: String,
    seq: AtomicU64,
    app: String,
}

enum Msg {
    Line(String),
    Audio(u64, Vec<f32>),
    /// Close the events file, send what's waiting, then answer.
    Flush(Sender<()>),
}

static RECORDER: OnceLock<Recorder> = OnceLock::new();

pub(super) fn recorder() -> Option<&'static Recorder> {
    RECORDER.get()
}

pub fn start(paths: &Paths) {
    let key = std::env::var("BD_TELEMETRY_KEY").ok().or_else(|| option_env!("BD_TELEMETRY_KEY").map(str::to_string));
    let Some(key) = key.filter(|k| !k.is_empty()) else {
        tracing::warn!("alpha build without BD_TELEMETRY_KEY: telemetry is off");
        return;
    };
    let dir = super::dir(paths);
    if let Err(e) = fs::create_dir_all(dir.join("audio")) {
        tracing::warn!("telemetry folder {}: {e}", dir.display());
        return;
    }
    let install = install_id(&dir.join("install-id"));
    let session = random_hex(16);
    let (tx, rx) = unbounded();
    let rec = Recorder { tx, install: install.clone(), session: session.clone(), seq: AtomicU64::new(0), app: format!("{}-alpha", env!("CARGO_PKG_VERSION")) };
    if RECORDER.set(rec).is_err() {
        return;
    }
    let uploader = Uploader::new(key, install, dir.clone());
    let writer = Writer { dir, session, file: None, opened: Instant::now(), uploader };
    thread::Builder::new().name("bd-telemetry".into()).spawn(move || writer.run(rx)).expect("spawning the telemetry thread");
    bd_pipeline::dump::audio_to(Box::new(|id, samples| {
        if let Some(r) = recorder().filter(|_| super::dictating()) {
            let _ = r.tx.send(Msg::Audio(id, samples.to_vec()));
        }
    }));
    tracing::info!("alpha telemetry on (install {})", recorder().map_or("", |r| r.install.as_str()));
    if let Some(r) = recorder() {
        super::crash::start(&super::dir(paths), &r.session);
    }
    super::system::record_system();
}

pub fn record(kind: &'static str, segment: Option<u64>, data: Value) {
    let Some(r) = recorder() else { return };
    let line = json!({
        "v": 1,
        "install": r.install,
        "session": r.session,
        "seq": r.seq.fetch_add(1, Ordering::Relaxed),
        "ts": rfc3339(SystemTime::now()),
        "app": r.app,
        "os": std::env::consts::OS,
        "kind": kind,
        "segment": segment,
        "data": data,
    });
    let _ = r.tx.send(Msg::Line(line.to_string()));
}

/// Closes the events file and tries to send everything, for at most a few
/// seconds; what's left goes at the next start.
pub fn stop() {
    let Some(r) = recorder() else { return };
    let (done, wait) = bounded(1);
    if r.tx.send(Msg::Flush(done)).is_ok() {
        let _ = wait.recv_timeout(Duration::from_secs(5));
    }
}

struct Writer {
    dir: PathBuf,
    session: String,
    /// The open events file.
    file: Option<PathBuf>,
    opened: Instant,
    uploader: Uploader,
}

impl Writer {
    fn run(mut self, rx: Receiver<Msg>) {
        loop {
            match rx.recv_timeout(Duration::from_secs(5)) {
                Ok(Msg::Line(line)) => self.append(&line),
                Ok(Msg::Audio(id, samples)) => self.audio(id, &samples),
                Ok(Msg::Flush(done)) => {
                    self.close();
                    self.uploader.send_waiting(Some(Duration::from_secs(4)));
                    let _ = done.send(());
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return,
            }
            if self.file.is_some() && self.opened.elapsed() >= EVENTS_FILE_AGE {
                self.close();
            }
            if self.uploader.due() {
                self.uploader.send_waiting(None);
            }
        }
    }

    fn append(&mut self, line: &str) {
        let path = self.file.get_or_insert_with(|| {
            self.opened = Instant::now();
            self.dir.join("events.open")
        });
        let written = OpenOptions::new().create(true).append(true).open(&*path).and_then(|mut f| writeln!(f, "{line}"));
        if written.is_err() {
            return;
        }
        if fs::metadata(&*path).is_ok_and(|m| m.len() >= EVENTS_FILE_BYTES) {
            self.close();
        }
    }

    /// Makes the open events file one the uploader sends.
    fn close(&mut self) {
        let Some(open) = self.file.take() else { return };
        let millis = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis());
        let _ = fs::rename(&open, self.dir.join(format!("events-{millis}.ndjson")));
        cap(&self.dir, "events-", MAX_EVENTS_BYTES);
    }

    fn audio(&mut self, id: u64, samples: &[f32]) {
        let Some(flac) = encode_flac(samples) else { return };
        let path = self.dir.join("audio").join(format!("{}-{id:06}.flac", self.session));
        if fs::write(&path, flac).is_ok() {
            cap(&self.dir.join("audio"), "", MAX_AUDIO_BYTES);
        }
    }
}

/// Deletes the oldest files starting with `prefix` in `dir` until they
/// take at most `max` bytes.
fn cap(dir: &Path, prefix: &str, max: u64) {
    let Ok(entries) = fs::read_dir(dir) else { return };
    let mut files: Vec<(SystemTime, u64, PathBuf)> = entries
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with(prefix) && e.path().is_file())
        .filter_map(|e| e.metadata().ok().map(|m| (m.modified().unwrap_or(UNIX_EPOCH), m.len(), e.path())))
        .collect();
    let mut total: u64 = files.iter().map(|f| f.1).sum();
    files.sort();
    for (_, len, path) in files {
        if total <= max {
            break;
        }
        if fs::remove_file(&path).is_ok() {
            total -= len;
        }
    }
}

/// 16 kHz mono 16-bit FLAC.
fn encode_flac(samples: &[f32]) -> Option<Vec<u8>> {
    use flacenc::component::BitRepr;
    use flacenc::error::Verify;
    if samples.is_empty() {
        return None;
    }
    let ints: Vec<i32> = samples.iter().map(|s| (s.clamp(-1.0, 1.0) * 32767.0) as i32).collect();
    let config = flacenc::config::Encoder::default().into_verified().ok()?;
    let source = flacenc::source::MemSource::from_samples(&ints, 1, 16, bd_core::SAMPLE_RATE as usize);
    let stream = flacenc::encode_with_fixed_block_size(&config, source, config.block_size).ok()?;
    let mut sink = flacenc::bitsink::ByteSink::new();
    stream.write(&mut sink).ok()?;
    Some(sink.as_slice().to_vec())
}

/// This install's id, made on first use.
fn install_id(path: &Path) -> String {
    if let Ok(id) = fs::read_to_string(path).map(|s| s.trim().to_string())
        && id.len() == 32
    {
        return id;
    }
    let id = random_hex(16);
    let _ = fs::write(path, &id);
    id
}

fn random_hex(bytes: usize) -> String {
    let mut buf = vec![0u8; bytes];
    if getrandom::fill(&mut buf).is_err() {
        // A clock-based fallback; collisions only merge two testers' logs.
        let n = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_nanos());
        buf.iter_mut().enumerate().for_each(|(i, b)| *b = (n >> (i % 16 * 8)) as u8);
    }
    buf.iter().map(|b| format!("{b:02x}")).collect()
}

/// `2026-10-18T16:02:11.482Z`.
pub(super) fn rfc3339(t: SystemTime) -> String {
    let d = t.duration_since(UNIX_EPOCH).unwrap_or_default();
    let secs = d.as_secs();
    let (days, rem) = ((secs / 86_400) as i64, secs % 86_400);
    // Civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{:03}Z",
        rem / 3600,
        rem / 60 % 60,
        rem % 60,
        d.subsec_millis()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timestamps_are_rfc3339_utc() {
        let t = UNIX_EPOCH + Duration::from_millis(1_792_339_331_482);
        assert_eq!(rfc3339(t), "2026-10-18T16:02:11.482Z");
        assert_eq!(rfc3339(UNIX_EPOCH), "1970-01-01T00:00:00.000Z");
        assert_eq!(rfc3339(UNIX_EPOCH + Duration::from_secs(951_782_400)), "2000-02-29T00:00:00.000Z");
    }

    #[test]
    fn flac_is_a_flac_stream_and_smaller() {
        let samples: Vec<f32> = (0..16_000).map(|i| (i as f32 * 0.05).sin() * 0.3).collect();
        let flac = encode_flac(&samples).unwrap();
        assert_eq!(&flac[..4], b"fLaC");
        assert!(flac.len() < samples.len() * 2);
    }

    #[test]
    fn the_cap_drops_the_oldest_first() {
        let tmp = std::env::temp_dir().join(format!("bd-cap-{}", random_hex(4)));
        fs::create_dir_all(&tmp).unwrap();
        for (i, name) in ["events-1", "events-2", "events-3"].iter().enumerate() {
            fs::write(tmp.join(name), vec![0u8; 100]).unwrap();
            let t = UNIX_EPOCH + Duration::from_secs(1000 + i as u64);
            fs::File::options().write(true).open(tmp.join(name)).unwrap().set_modified(t).unwrap();
        }
        cap(&tmp, "events-", 200);
        assert!(!tmp.join("events-1").exists() && tmp.join("events-2").exists() && tmp.join("events-3").exists());
        fs::remove_dir_all(&tmp).unwrap();
    }
}
