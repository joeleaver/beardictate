//! The dictation engine, off the UI thread: models, microphone, pipeline,
//! and typing into the focused window.
//!
//! One worker thread owns the microphone (a cpal stream may not move between
//! threads) and the pipeline, and waits on everything at once: commands from
//! the UI, audio, pipeline events, a model load finishing, injection reports,
//! and a two-second tick that notices edits to the settings and word files.
//! Typing has a thread of its own, so a slow target app never holds up audio.
//! The UI hears about state through [`Ui`].

use std::fs;
use std::path::Path;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime};

use anyhow::{Context, Result};
use bd_audio::{AudioError, CaptureConfig, CaptureEvent, StreamPolicy};
use bd_pipeline::{Event, Pipeline};
use crossbeam_channel::{Receiver, Sender, bounded, never, select, tick, unbounded};

use crate::mic::Mic;
use crate::notify::Notifier;
use crate::settings::{Paths, Settings};

mod inject;
mod standby;
use inject::{Injected, TypeJob, spawn_injector};

mod load;
use load::{load, pipeline_config};

/// After this long without dictating, the next dictation is a new passage:
/// the cleanup model gets no context from before (you have probably moved on,
/// or to another window).
const NEW_PASSAGE_AFTER: Duration = Duration::from_secs(60);

pub enum Command {
    /// Start dictating, or stop.
    Toggle,
    /// The settings changed (from the tray menu; already saved).
    Apply(Settings),
    /// A cleanup model was downloaded or deleted.
    ModelsChanged,
    Shutdown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Loading,
    Idle,
    /// Always listening: the microphone is open, waiting for "bear, wake up"
    /// or the hotkey.
    Asleep,
    /// Always listening, with the microphone turned off from the menu.
    MicOff,
    Listening,
    /// Stopped listening; the last sentences are still being cleaned.
    Cleaning,
    NeedsYou,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Status {
    pub state: State,
    /// The cleanup model's name, or what needs attention.
    pub detail: String,
    /// When listening started.
    pub since: Option<Instant>,
    /// Always listening: the hotkey wakes and sends to sleep.
    pub voice: bool,
}

/// What the engine tells the UI. Called on the engine's thread.
pub trait Ui: Send + 'static {
    fn status(&self, status: Status);
    /// The settings file was edited by hand.
    fn settings_changed(&self, settings: Settings);
}

pub struct Engine {
    commands: Sender<Command>,
    thread: Option<JoinHandle<()>>,
}

impl Engine {
    pub fn spawn(paths: Paths, settings: Settings, ui: impl Ui) -> Engine {
        let (commands, rx) = unbounded();
        let thread = thread::Builder::new()
            .name("bd-engine".into())
            .spawn(move || Worker::new(paths, settings, ui, rx).run())
            .expect("spawning the engine thread");
        Engine { commands, thread: Some(thread) }
    }

    pub fn send(&self, command: Command) {
        let _ = self.commands.send(command);
    }

    /// A handle for sending commands from other threads.
    pub fn sender(&self) -> Sender<Command> {
        self.commands.clone()
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        self.send(Command::Shutdown);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// A pipeline ready to go.
struct Loaded {
    pipeline: Pipeline,
    cleanup: String,
}

struct Worker<U> {
    ui: U,
    paths: Paths,
    settings: Settings,
    commands: Receiver<Command>,
    notifier: Notifier,
    pipeline: Option<Pipeline>,
    cleanup_name: String,
    loading: Option<Receiver<Result<Loaded, String>>>,
    /// Reload once the current dictation is finished.
    reload_when_idle: bool,
    load_error: Option<String>,
    capture: Option<Mic>,
    listening_since: Option<Instant>,
    /// Always listening, with the microphone running (see `standby.rs`).
    standby: bool,
    /// Turn dictation on when the models first load.
    start_on_load: bool,
    /// Always listening was awake when a reload started: wake it again.
    wake_after_load: bool,
    cleaning: bool,
    last_dictation: Option<Instant>,
    type_text: Sender<TypeJob>,
    injected: Receiver<Injected>,
    injector_error: Option<String>,
    settings_mtime: Option<SystemTime>,
}

impl<U: Ui> Worker<U> {
    fn new(paths: Paths, settings: Settings, ui: U, commands: Receiver<Command>) -> Self {
        let (type_text, injected) = spawn_injector(&paths);
        Worker {
            notifier: Notifier::new(settings.notifications),
            settings_mtime: mtime(&paths.settings),
            ui,
            paths,
            settings,
            commands,
            pipeline: None,
            cleanup_name: String::new(),
            loading: None,
            reload_when_idle: false,
            load_error: None,
            capture: None,
            listening_since: None,
            standby: false,
            start_on_load: true,
            wake_after_load: false,
            cleaning: false,
            last_dictation: None,
            type_text,
            injected,
            injector_error: None,
        }
    }

    fn run(mut self) {
        self.start_load();
        let ticks = tick(Duration::from_secs(2));
        loop {
            let commands = self.commands.clone();
            let audio = self.capture.as_ref().map_or_else(never, Mic::events);
            let events = self.pipeline.as_ref().map_or_else(never, |p| p.events().clone());
            let loading = self.loading.clone().unwrap_or_else(never);
            let injected = self.injected.clone();
            select! {
                recv(commands) -> cmd => match cmd {
                    Ok(Command::Toggle) => self.toggle(),
                    Ok(Command::Apply(settings)) => self.apply(settings),
                    Ok(Command::ModelsChanged) => {
                        // "auto" may now mean a different model; a failed load
                        // may now succeed.
                        if self.settings.cleanup == "auto" || self.load_error.is_some() {
                            self.request_reload();
                        }
                    }
                    Ok(Command::Shutdown) | Err(_) => break,
                },
                recv(audio) -> ev => if let Ok(ev) = ev { self.on_audio(ev) },
                recv(events) -> ev => if let Ok(ev) = ev { self.on_pipeline(ev) },
                recv(loading) -> loaded => self.on_loaded(loaded.unwrap_or_else(|_| Err("model loading stopped".into()))),
                recv(injected) -> report => if let Ok(report) = report { self.on_injected(report) },
                recv(ticks) -> _ => self.watch_files(),
            }
        }
        if let Some(capture) = self.capture.as_mut() {
            let _ = capture.stop();
        }
    }

    // ---- status -------------------------------------------------------------

    fn publish(&self) {
        let (state, detail) = if self.loading.is_some() {
            (State::Loading, "Loading models…".to_string())
        } else if let Some(e) = self.load_error.as_ref().or(self.injector_error.as_ref()) {
            (State::NeedsYou, e.clone())
        } else if self.listening_since.is_some() {
            (State::Listening, self.cleanup_name.clone())
        } else if self.cleaning {
            (State::Cleaning, self.cleanup_name.clone())
        } else if self.standby {
            (State::Asleep, self.cleanup_name.clone())
        } else if self.settings.voice_wake {
            (State::MicOff, self.cleanup_name.clone())
        } else {
            (State::Idle, self.cleanup_name.clone())
        };
        self.ui.status(Status { state, detail, since: self.listening_since, voice: self.settings.voice_wake });
    }

    // ---- loading ------------------------------------------------------------

    fn start_load(&mut self) {
        self.reload_when_idle = false;
        self.wake_after_load = self.standby && self.listening_since.is_some();
        self.load_error = None;
        // Dropping the old pipeline joins its threads, which are idle.
        self.pipeline = None;
        let settings = self.settings.clone();
        let (tx, rx) = bounded(1);
        thread::Builder::new()
            .name("bd-load".into())
            .spawn(move || {
                let _ = tx.send(load(&settings).map_err(|e| format!("{e:#}")));
            })
            .expect("spawning the loader");
        self.loading = Some(rx);
        self.publish();
    }

    fn on_loaded(&mut self, loaded: Result<Loaded, String>) {
        self.loading = None;
        match loaded {
            Ok(l) => {
                tracing::info!(cleanup = %l.cleanup, "models loaded");
                self.cleanup_name = format!("Cleanup: {}", l.cleanup);
                // Settings changed while loading reach it now.
                l.pipeline.reconfigure(pipeline_config(&self.settings));
                self.pipeline = Some(l.pipeline);
                // The first load after starting: dictation comes on, in either
                // mode. Later loads (a settings change) keep what was.
                let starting = std::mem::take(&mut self.start_on_load);
                if starting && self.settings.voice_wake {
                    self.record_mic(true);
                }
                if self.wants_standby() {
                    let wake = starting || std::mem::take(&mut self.wake_after_load);
                    self.stand_by(wake);
                } else if starting
                    && !self.settings.voice_wake
                    && let Err(e) = self.start()
                {
                    self.notifier.notify("mic", "Microphone problem", &format!("{e:#}"));
                    self.capture = None;
                }
            }
            Err(e) => {
                if e != load::NOT_DOWNLOADED {
                    self.notifier.notify("load", "beardictate can't start", &e);
                }
                self.load_error = Some(e);
            }
        }
        self.publish();
    }

    fn busy(&self) -> bool {
        self.listening_since.is_some() || self.cleaning
    }

    // ---- dictation ----------------------------------------------------------

    fn toggle(&mut self) {
        tracing::info!(listening = self.listening_since.is_some(), loading = self.loading.is_some(), "toggle");
        if self.settings.voice_wake && self.pipeline.is_some() {
            self.voice_toggle();
        } else if self.listening_since.is_some() {
            self.stop();
        } else if self.loading.is_some() {
            tracing::info!("hotkey while loading; ignored");
        } else if self.pipeline.is_none() {
            // Loading failed: the hotkey retries.
            self.start_load();
        } else if let Err(e) = self.start() {
            self.notifier.notify("mic", "Microphone problem", &format!("{e:#}"));
            self.capture = None;
            self.publish();
        }
    }

    fn open_mic(&mut self) -> Result<()> {
        if self.capture.as_ref().is_none_or(|c| !c.is_alive()) {
            let keep_open = self.settings.keep_mic_open || self.settings.voice_wake;
            let config = CaptureConfig {
                device: Some(self.settings.microphone.clone()).filter(|m| !m.is_empty()),
                policy: if keep_open { StreamPolicy::KeepOpen } else { StreamPolicy::OpenOnStart },
                ..CaptureConfig::default()
            };
            let capture = Mic::open(config).context("opening the microphone")?;
            tracing::info!(device = %capture.name(), "microphone open");
            self.capture = Some(capture);
        }
        Ok(())
    }

    fn start(&mut self) -> Result<()> {
        self.open_mic()?;
        let pipeline = self.pipeline.as_ref().expect("checked by toggle");
        if self.last_dictation.is_none_or(|t| t.elapsed() > NEW_PASSAGE_AFTER) {
            pipeline.new_passage();
        }
        self.capture.as_mut().expect("opened above").start().context("starting the microphone")?;
        self.listening_since = Some(Instant::now());
        self.publish();
        Ok(())
    }

    fn stop(&mut self) {
        if let Some(capture) = self.capture.as_mut()
            && let Err(e) = capture.stop()
        {
            tracing::warn!("stopping the microphone: {e}");
        }
        self.listening_since = None;
        self.cleaning = true;
        self.last_dictation = Some(Instant::now());
        self.publish();
    }

    fn on_audio(&mut self, event: CaptureEvent) {
        let Some(pipeline) = self.pipeline.as_ref() else { return };
        match event {
            CaptureEvent::Chunk(chunk) => pipeline.push_audio(chunk.samples),
            CaptureEvent::Stopped { dropped_frames } => {
                if dropped_frames > 0 {
                    tracing::warn!(dropped_frames, "audio dropped");
                }
                pipeline.end_session();
            }
            CaptureEvent::Error(e @ AudioError::DeviceLost(_)) => {
                self.notifier.notify("mic", "Microphone lost", &e.to_string());
                if self.standby {
                    self.standby = false;
                    pipeline.set_awake(false);
                    pipeline.end_session();
                    self.listening_since = None;
                    self.capture = None;
                    return self.record_mic(false);
                }
                if self.listening_since.is_some() {
                    self.listening_since = None;
                    self.cleaning = true;
                    pipeline.end_session();
                }
                self.capture = None;
                self.publish();
            }
            CaptureEvent::Error(e) => tracing::warn!("capture: {e}"),
        }
    }

    fn on_pipeline(&mut self, event: Event) {
        match event {
            Event::Text { id, text, .. } => {
                tracing::info!(segment = id, chars = text.chars().count(), "typing");
                let _ = self.type_text.send(TypeJob::Type(text));
            }
            Event::SessionEnded => {
                self.cleaning = false;
                if self.reload_when_idle && !self.busy() {
                    self.start_load();
                    return;
                }
                self.publish();
            }
            Event::Erase { id, text } => {
                tracing::info!(segment = id, chars = text.chars().count(), "erasing (scratch that)");
                let _ = self.type_text.send(TypeJob::Erase(text));
            }
            Event::ScratchTooLate => {
                self.notifier.notify(
                    "scratch",
                    "Nothing to scratch",
                    "\"Scratch that\" only takes back text from the current dictation; earlier text is left as is.",
                );
            }
            Event::Error { stage, message } => {
                self.notifier.notify("pipeline", &format!("{stage:?} failed on a sentence"), &message);
            }
            Event::Woke => {
                tracing::info!("voice wake: awake");
                self.listening_since = Some(Instant::now());
                self.publish();
            }
            Event::Slept => {
                tracing::info!("voice wake: asleep");
                self.listening_since = None;
                self.last_dictation = Some(Instant::now());
                if self.reload_when_idle {
                    self.start_load();
                    return;
                }
                self.publish();
            }
            // Counts only, never the words: enough to see in the log which
            // sentence went missing, and when.
            Event::Transcribed { id, text, .. } => {
                tracing::info!(segment = id, words = text.split_whitespace().count(), "heard");
            }
            Event::Scratched { id } => tracing::info!(segment = id, "scratched"),
        }
    }

    // ---- settings -----------------------------------------------------------

    fn apply(&mut self, new: Settings) {
        let old = std::mem::replace(&mut self.settings, new);
        let s = &self.settings;
        self.notifier.set_enabled(s.notifications);
        if (old.voice_wake, old.microphone_on) != (s.voice_wake, s.microphone_on) {
            self.apply_rest(&old);
            return self.switch_mic(old.microphone_on);
        }
        if self.standby && old.microphone != s.microphone {
            // A different microphone: reopen it, staying asleep or awake.
            self.leave_standby();
            self.stand_by(false);
        } else if old.microphone != s.microphone || old.keep_mic_open != s.keep_mic_open {
            if self.busy() {
                tracing::info!("microphone change applies after this dictation");
            } else {
                self.capture = None;
            }
        }
        self.apply_rest(&old);
    }

    /// The model and live pipeline settings.
    fn apply_rest(&mut self, old: &Settings) {
        let s = &self.settings;
        if old.models() != s.models() {
            self.request_reload();
        } else if old.live() != s.live()
            && let Some(pipeline) = &self.pipeline
        {
            tracing::info!("settings applied");
            pipeline.reconfigure(pipeline_config(s));
        }
    }

    fn request_reload(&mut self) {
        // Always listening is awake for hours; waiting for "go to sleep"
        // would keep the old model all that time. Reload now and wake again
        // after (speech during the few seconds of loading isn't typed).
        let waits = self.cleaning || (self.listening_since.is_some() && !self.standby);
        if waits {
            self.reload_when_idle = true;
        } else {
            self.start_load();
        }
    }

    fn watch_files(&mut self) {
        let now = mtime(&self.paths.settings);
        if now != self.settings_mtime {
            self.settings_mtime = now;
            match self.paths.load() {
                Ok(s) if s != self.settings => {
                    tracing::info!("settings file changed");
                    self.ui.settings_changed(s.clone());
                    self.apply(s);
                }
                Ok(_) => {}
                Err(e) => self.notifier.notify("settings", "Settings file has a mistake", &format!("{e:#}")),
            }
        }
    }
}

fn mtime(path: &Path) -> Option<SystemTime> {
    fs::metadata(path).and_then(|m| m.modified()).ok()
}
