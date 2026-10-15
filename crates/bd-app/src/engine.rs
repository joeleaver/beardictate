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
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime};

use anyhow::{Context, Result};
use bd_audio::{AudioError, CaptureConfig, CaptureEvent, StreamPolicy};
use bd_core::InjectOutcome;
use bd_models::Store;
use bd_pipeline::setup::{self, CleanupOptions, CleanupSource};
use bd_pipeline::{Event, Parts, Pipeline, PipelineConfig};
use bd_platform::{Erased, InjectorConfig, PlatformInjector};
use crossbeam_channel::{Receiver, Sender, bounded, never, select, tick, unbounded};

use crate::mic::Mic;
use crate::notify::Notifier;
use crate::settings::{Paths, Settings, vocabulary};

/// After this long without dictating, the next dictation is a new passage:
/// the cleanup model gets no context from before (you have probably moved on,
/// or to another window).
const NEW_PASSAGE_AFTER: Duration = Duration::from_secs(60);

pub enum Command {
    /// Start dictating, or stop.
    Toggle,
    /// The settings changed (from the tray menu; already saved).
    Apply(Settings),
    Shutdown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Loading,
    Idle,
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

/// Work for the typing thread.
enum TypeJob {
    Type(String),
    /// Take back text typed earlier in this dictation.
    Erase(String),
}

/// What the typing thread reports.
enum Injected {
    /// Typing works; the backend's name.
    Ready(&'static str),
    LeftOnClipboard(String),
    /// "Scratch that" couldn't take the text back.
    NotErased(String),
    Failed(String),
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
        } else {
            (State::Idle, self.cleanup_name.clone())
        };
        self.ui.status(Status { state, detail, since: self.listening_since });
    }

    // ---- loading ------------------------------------------------------------

    fn start_load(&mut self) {
        self.reload_when_idle = false;
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
                self.pipeline = Some(l.pipeline);
            }
            Err(e) => {
                self.notifier.notify("load", "beardictate can't start", &e);
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
        if self.listening_since.is_some() {
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

    fn start(&mut self) -> Result<()> {
        if self.capture.as_ref().is_none_or(|c| !c.is_alive()) {
            let config = CaptureConfig {
                device: Some(self.settings.microphone.clone()).filter(|m| !m.is_empty()),
                policy: if self.settings.keep_mic_open { StreamPolicy::KeepOpen } else { StreamPolicy::OpenOnStart },
                ..CaptureConfig::default()
            };
            let capture = Mic::open(config).context("opening the microphone")?;
            tracing::info!(device = %capture.name(), "microphone open");
            self.capture = Some(capture);
        }
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
            Event::Transcribed { .. } | Event::Scratched { .. } => {}
        }
    }

    fn on_injected(&mut self, report: Injected) {
        match report {
            Injected::Ready(backend) => {
                tracing::info!(backend, "typing ready");
                if self.injector_error.take().is_some() {
                    self.publish();
                }
            }
            Injected::LeftOnClipboard(reason) => {
                self.notifier.notify("clipboard", "Text is on the clipboard", &format!("Couldn't type it ({reason}). Press Ctrl+V to paste."));
            }
            Injected::NotErased(reason) => {
                self.notifier.notify("erase", "Couldn't scratch that", &format!("The text is still there ({reason})."));
            }
            Injected::Failed(e) => {
                self.notifier.notify("inject", "Couldn't type the text", &e);
                self.injector_error = Some(format!("Can't type into other windows: {e}"));
                self.publish();
            }
        }
    }

    // ---- settings -----------------------------------------------------------

    fn apply(&mut self, new: Settings) {
        let old = std::mem::replace(&mut self.settings, new);
        let s = &self.settings;
        self.notifier.set_enabled(s.notifications);
        if old.microphone != s.microphone || old.keep_mic_open != s.keep_mic_open {
            if self.busy() {
                tracing::info!("microphone change applies after this dictation");
            } else {
                self.capture = None;
            }
        }
        if old.pipeline() != s.pipeline() {
            self.request_reload();
        }
    }

    fn request_reload(&mut self) {
        if self.busy() {
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

/// Loads every model and starts a pipeline. Runs on its own thread.
fn load(settings: &Settings) -> Result<Loaded> {
    let store = match settings.models_dir.trim() {
        "" => Store::from_env()?,
        dir => Store::at(dir),
    };
    let words = vocabulary(&settings.special_words);
    let segmenter = setup::load_segmenter(&store)?;
    let asr = setup::load_asr(&store, &words.hotwords(), settings.hotword_boost)?;
    let source = CleanupSource::parse(&settings.cleanup);
    let cleanup = setup::load_cleanup(&source, &CleanupOptions::default(), &store)?;
    let name = cleanup.as_ref().map_or_else(|| "off".to_string(), |c| c.name.clone());
    if let Some(c) = &cleanup {
        tracing::info!("{}", c.description);
    }
    let parts = Parts {
        segmenter: Box::new(segmenter),
        asr: Box::new(asr),
        cleanup: cleanup.map(|c| Box::new(c.engine) as _),
    };
    let config = PipelineConfig {
        style: settings.style(),
        commands: settings.commands(),
        vocabulary: words,
        pause_paragraph: settings.pause_paragraph(),
        ..PipelineConfig::default()
    };
    let pipeline = Pipeline::spawn(parts, config).map_err(|e| anyhow::anyhow!(e))?;
    Ok(Loaded { pipeline, cleanup: name })
}

/// Starts the typing thread. It sets up the platform injector first, which
/// on Wayland asks the desktop for permission (once; the approval is kept in
/// [`Paths::portal_token`]).
fn spawn_injector(paths: &Paths) -> (Sender<TypeJob>, Receiver<Injected>) {
    let (text_tx, text_rx) = unbounded::<TypeJob>();
    let (report_tx, report_rx) = unbounded();
    let token_path = paths.portal_token.clone();
    let token = paths.load_portal_token();
    thread::Builder::new()
        .name("bd-type".into())
        .spawn(move || {
            let save = Arc::new(move |token: &str| {
                if let Some(dir) = token_path.parent() {
                    let _ = fs::create_dir_all(dir);
                }
                if let Err(e) = fs::write(&token_path, token) {
                    tracing::warn!("saving the input permission: {e}");
                }
            });
            let config = InjectorConfig {
                portal_restore_token: token,
                on_portal_restore_token: Some(save),
                ..InjectorConfig::default()
            };
            let mut injector = match PlatformInjector::new(config).and_then(|mut i| i.prepare().map(|()| i)) {
                Ok(i) => i,
                Err(e) => {
                    let _ = report_tx.send(Injected::Failed(e.to_string()));
                    return;
                }
            };
            let _ = report_tx.send(Injected::Ready(injector.backend_name()));
            for job in text_rx {
                let report = match job {
                    TypeJob::Type(text) => match injector.inject_text(&text) {
                        Ok(InjectOutcome::LeftOnClipboard { reason }) => Some(Injected::LeftOnClipboard(reason)),
                        Ok(_) => None,
                        Err(e) => Some(Injected::Failed(e.to_string())),
                    },
                    TypeJob::Erase(text) => match injector.erase(&text) {
                        Ok(Erased::Done) => None,
                        Ok(Erased::Refused { reason }) => Some(Injected::NotErased(reason)),
                        Err(e) => Some(Injected::NotErased(e.to_string())),
                    },
                };
                if let Some(r) = report {
                    let _ = report_tx.send(r);
                }
            }
        })
        .expect("spawning the typing thread");
    (text_tx, report_rx)
}
