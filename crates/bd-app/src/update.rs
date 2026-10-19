//! Updates: the app looks for a newer build on the site every few hours,
//! downloads it in the background, checks it (size, SHA-256 and the release
//! signature), and installs it the next time it starts, or at once from
//! "Restart to update". Nothing runs mid-dictation: installing means a
//! restart, which only happens at start or when asked.
//!
//! The list of builds is the download page's own `releases.json`
//! (`site/publish.sh`); `BD_UPDATE_URL` points the app at another site
//! (testing). Off in Settings › General, it checks only when asked.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use crossbeam_channel::{Receiver, RecvTimeoutError, Sender, unbounded};
use rinch::prelude::Signal;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub mod feed;
mod install;
mod verify;

use feed::Build;
use install::Install;

pub const CURRENT: &str = env!("CARGO_PKG_VERSION");
const SITE: &str = "https://beardictate.jkbase.app";
/// The first check a minute after start, then every six hours.
const FIRST_CHECK: Duration = Duration::from_secs(60);
const EVERY: Duration = Duration::from_secs(6 * 3600);
/// When what the last update left behind is cleared.
const LEFTOVERS: Duration = Duration::from_secs(30);

/// Where updates stand, for the window and the tray.
#[derive(Debug, Clone, PartialEq)]
pub enum Phase {
    /// Not checked yet this run.
    Idle,
    Checking,
    UpToDate,
    Downloading {
        version: String,
        done: u64,
        total: u64,
    },
    /// Downloaded and checked: installs at the next start, or now.
    Ready {
        version: String,
    },
    /// Newer, but this copy can't install it itself (a portable or
    /// development build): from the download page.
    Manual {
        version: String,
    },
    Failed(String),
}

/// The download page, for updates this copy can't install itself.
pub fn page() -> String {
    format!("{}/download.html", site())
}

fn site() -> String {
    std::env::var("BD_UPDATE_URL")
        .map(|u| u.trim_end_matches('/').to_string())
        .unwrap_or_else(|_| SITE.into())
}

/// What was downloaded and whether installing it has been tried.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Staged {
    version: String,
    file: String,
    size: u64,
    sha256: String,
    signature: String,
    #[serde(default)]
    attempted: bool,
    /// Installing it failed: don't try again at start (the button still can).
    #[serde(default)]
    failed: bool,
}

fn staged_path(dir: &Path) -> PathBuf {
    dir.join("staged.json")
}

fn read_staged(dir: &Path) -> Option<Staged> {
    serde_json::from_str(&fs::read_to_string(staged_path(dir)).ok()?).ok()
}

fn write_staged(dir: &Path, staged: &Staged) -> Result<()> {
    let path = staged_path(dir);
    let tmp = path.with_extension("json.part");
    fs::write(&tmp, serde_json::to_string_pretty(staged)?)
        .with_context(|| format!("writing {}", tmp.display()))?;
    fs::rename(&tmp, &path).with_context(|| format!("writing {}", path.display()))
}

fn log_path(dir: &Path) -> PathBuf {
    dir.join("install.log")
}

/// Removes old downloads, keeping the last install's log.
fn clear(dir: &Path) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        if e.file_name() == "install.log" {
            continue;
        }
        let path = e.path();
        let _ = if path.is_dir() {
            fs::remove_dir_all(&path)
        } else {
            fs::remove_file(&path)
        };
    }
}

/// What the app does at start.
pub enum Start {
    /// Carry on; with a line for the About page ("Updated to 0.1.9").
    Run(Option<String>),
    /// Installing an update: quit now.
    Quit,
}

/// At start, before anything else: installs a downloaded update (when
/// `auto`), or notes how the last one went.
pub fn at_start(dir: &Path, auto: bool) -> Start {
    let Some(mut staged) = read_staged(dir) else {
        return Start::Run(None);
    };
    if !feed::newer(&staged.version, CURRENT) {
        let note = staged.attempted.then(|| format!("Updated to {CURRENT}."));
        if let Some(n) = &note {
            tracing::info!("{n}");
        }
        clear(dir);
        return Start::Run(note);
    }
    if staged.attempted {
        tracing::warn!(
            "installing {} failed; see {}",
            staged.version,
            log_path(dir).display()
        );
        staged.attempted = false;
        staged.failed = true;
        let _ = write_staged(dir, &staged);
        return Start::Run(Some(format!(
            "Couldn't install {}. What happened is in {}.",
            staged.version,
            log_path(dir).display()
        )));
    }
    if !auto || staged.failed {
        return Start::Run(None);
    }
    match install_now(dir, &mut staged) {
        Ok(()) => Start::Quit,
        Err(e) => {
            tracing::warn!("installing {}: {e:#}", staged.version);
            Start::Run(None)
        }
    }
}

/// Checks the staged build again and hands over to the installer.
fn install_now(dir: &Path, staged: &mut Staged) -> Result<()> {
    let install = Install::detect();
    let file = dir.join(&staged.file);
    verify::check(&file, staged.size, &staged.sha256, &staged.signature)?;
    staged.attempted = true;
    write_staged(dir, staged)?;
    tracing::info!("installing {} from {}", staged.version, file.display());
    if let Err(e) = install::start(&install, &file, dir, &log_path(dir)) {
        staged.attempted = false;
        let _ = write_staged(dir, staged);
        return Err(e);
    }
    Ok(())
}

static INSTALL_ON_EXIT: AtomicBool = AtomicBool::new(false);

/// "Restart to update": the app quits, and installs on the way out.
pub fn restart_to_update() {
    INSTALL_ON_EXIT.store(true, Ordering::SeqCst);
    rinch::prelude::close_current_window();
}

/// At the very end of `main`, once the app has let go of everything.
pub fn at_exit(dir: &Path) {
    if !INSTALL_ON_EXIT.load(Ordering::SeqCst) {
        return;
    }
    let Some(mut staged) = read_staged(dir) else {
        return;
    };
    if let Err(e) = install_now(dir, &mut staged) {
        tracing::warn!("installing {}: {e:#}", staged.version);
    }
}

enum Msg {
    /// Check now (and download what's found).
    Check,
    /// The automatic checks on or off.
    Auto(bool),
}

/// The background checker. Cheap to clone.
#[derive(Clone)]
pub struct Updates {
    pub phase: Signal<Phase>,
    /// How the last update went, from [`at_start`].
    pub note: Option<String>,
    tx: Sender<Msg>,
}

impl Updates {
    pub fn spawn(dir: PathBuf, auto: bool, note: Option<String>) -> Updates {
        let phase = Signal::new(Phase::Idle);
        let (tx, rx) = unbounded();
        let install = Install::detect();
        tracing::info!(?install, channel = feed::channel(), "updates");
        let checker = Checker {
            dir,
            phase,
            last: std::cell::RefCell::new(Phase::Idle),
            install,
        };
        thread::Builder::new()
            .name("bd-updates".into())
            .spawn(move || checker.run(rx, auto))
            .expect("spawning the update checker");
        Updates { phase, note, tx }
    }

    /// Checks now; on the UI thread, which shows it at once.
    pub fn check_now(&self) {
        self.phase.set(Phase::Checking);
        let _ = self.tx.send(Msg::Check);
    }

    pub fn set_auto(&self, auto: bool) {
        let _ = self.tx.send(Msg::Auto(auto));
    }
}

struct Checker {
    dir: PathBuf,
    phase: Signal<Phase>,
    /// What `phase` was last sent (signals are read on the UI thread).
    last: std::cell::RefCell<Phase>,
    install: Install,
}

impl Checker {
    fn show(&self, phase: Phase) {
        *self.last.borrow_mut() = phase.clone();
        self.phase.send(phase);
    }

    fn run(self, rx: Receiver<Msg>, mut auto: bool) {
        let mut next = Instant::now() + FIRST_CHECK;
        // What an update left: on Windows the installer is still running
        // when it starts the new version, so its file can't go at start.
        let mut leftovers = Some(Instant::now() + LEFTOVERS);
        loop {
            let check_at = if auto { next } else { Instant::now() + Duration::from_secs(24 * 3600) };
            let wait = leftovers.map_or(check_at, |at| at.min(check_at)).saturating_duration_since(Instant::now());
            if leftovers.is_some_and(|at| at <= Instant::now()) {
                leftovers = None;
                if read_staged(&self.dir).is_none() {
                    clear(&self.dir);
                }
                continue;
            }
            match rx.recv_timeout(wait) {
                Ok(Msg::Check) => {
                    self.check();
                    next = Instant::now() + EVERY;
                }
                Ok(Msg::Auto(on)) => {
                    if on && !auto {
                        next = Instant::now() + FIRST_CHECK;
                    }
                    auto = on;
                }
                Err(RecvTimeoutError::Timeout) if auto && Instant::now() >= next => {
                    self.check();
                    next = Instant::now() + EVERY;
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return,
            }
        }
    }

    fn check(&self) {
        // A download that's ready stays ready if checking again fails.
        let before = self.last.borrow().clone();
        self.show(Phase::Checking);
        let phase = match self.find_and_fetch() {
            Ok(p) => p,
            Err(e) => {
                tracing::warn!("checking for updates: {e:#}");
                match before {
                    Phase::Ready { .. } => before,
                    _ => Phase::Failed(format!("{e:#}")),
                }
            }
        };
        self.show(phase);
    }

    fn find_and_fetch(&self) -> Result<Phase> {
        let agent = agent();
        let url = format!("{}/downloads/releases.json", site());
        let mut response = agent
            .get(&url)
            .call()
            .with_context(|| format!("reading {url}"))?;
        anyhow::ensure!(
            response.status().is_success(),
            "{url}: HTTP {}",
            response.status()
        );
        let feed: feed::Feed = response
            .body_mut()
            .read_json()
            .with_context(|| format!("reading {url}"))?;
        let Some(build) = feed::newest(&feed, CURRENT, feed::platform(), feed::channel()).cloned()
        else {
            return Ok(Phase::UpToDate);
        };
        let version = build.version.clone();
        let Some(signature) = build
            .signature
            .clone()
            .filter(|_| self.install.can_update())
        else {
            return Ok(Phase::Manual { version });
        };
        fs::create_dir_all(&self.dir)
            .with_context(|| format!("creating {}", self.dir.display()))?;
        if let Some(staged) = read_staged(&self.dir)
            && staged.version == version
            && staged.sha256 == build.sha256
            && verify::check(
                &self.dir.join(&staged.file),
                staged.size,
                &staged.sha256,
                &staged.signature,
            )
            .is_ok()
        {
            return Ok(Phase::Ready { version });
        }
        clear(&self.dir);
        let file = self.download(&agent, &build)?;
        verify::check(&file, build.size, &build.sha256, &signature)?;
        write_staged(
            &self.dir,
            &Staged {
                version: version.clone(),
                file: build.file.clone(),
                size: build.size,
                sha256: build.sha256.clone(),
                signature,
                attempted: false,
                failed: false,
            },
        )?;
        tracing::info!("update {version} downloaded; it installs at the next start");
        Ok(Phase::Ready { version })
    }

    /// Downloads a build into the updates folder, hashing as it goes.
    fn download(&self, agent: &ureq::Agent, build: &Build) -> Result<PathBuf> {
        use std::io::{Read, Write};
        anyhow::ensure!(
            !build.file.contains(['/', '\\']) && !build.file.starts_with('.'),
            "odd file name {:?}",
            build.file
        );
        let url = format!("{}/downloads/{}", site(), build.file);
        let response = agent
            .get(&url)
            .call()
            .with_context(|| format!("downloading {url}"))?;
        anyhow::ensure!(
            response.status().is_success(),
            "{url}: HTTP {}",
            response.status()
        );
        let dest = self.dir.join(&build.file);
        let part = self.dir.join(format!("{}.part", build.file));
        let mut out =
            fs::File::create(&part).with_context(|| format!("creating {}", part.display()))?;
        let mut reader = response.into_body().into_reader();
        let mut hasher = Sha256::new();
        let mut buf = vec![0u8; 1 << 16];
        let (mut done, mut shown) = (0u64, 0u64);
        loop {
            let n = reader
                .read(&mut buf)
                .with_context(|| format!("downloading {url}"))?;
            if n == 0 {
                break;
            }
            out.write_all(&buf[..n])
                .with_context(|| format!("writing {}", part.display()))?;
            hasher.update(&buf[..n]);
            done += n as u64;
            anyhow::ensure!(
                done <= build.size,
                "{url} is bigger than the {} bytes listed",
                build.size
            );
            if done - shown > build.size / 100 {
                shown = done;
                self.show(Phase::Downloading {
                    version: build.version.clone(),
                    done,
                    total: build.size,
                });
            }
        }
        out.sync_all()?;
        drop(out);
        anyhow::ensure!(
            hex::encode(hasher.finalize()).eq_ignore_ascii_case(&build.sha256),
            "{url} doesn't match its SHA-256"
        );
        fs::rename(&part, &dest).with_context(|| format!("saving {}", dest.display()))?;
        Ok(dest)
    }
}

fn agent() -> ureq::Agent {
    let options = bd_models::HttpOptions {
        https_only: !site().starts_with("http://"),
        ..Default::default()
    };
    bd_models::http_agent(&options)
}
