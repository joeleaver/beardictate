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
use bd_pipeline::Pipeline;
use crossbeam_channel::{Receiver, Sender, never, select, tick, unbounded};

use crate::mic::Mic;
use crate::notify::Notifier;
use crate::settings::{Paths, Settings};

mod events;
mod notes;
mod inject;
mod mic_check;
mod pad;
mod standby;
use inject::{Injected, TypeJob, spawn_injector};

mod load;
use load::pipeline_config;

/// After this long without dictating, the next dictation is a new passage:
/// the cleanup model gets no context from before (you have probably moved on,
/// or to another window).
const NEW_PASSAGE_AFTER: Duration = Duration::from_secs(60);

pub enum Command {
    /// Start dictating, or stop.
    Toggle,
    /// Start or stop the Microphone page's test.
    MicTest(bool),
    /// The settings changed (from the tray menu; already saved).
    Apply(Settings),
    /// A cleanup model was downloaded or deleted.
    ModelsChanged,
    /// Bear Pad: a button in the pad window.
    Pad(bd_pipeline::PadAction),
    /// Bear Pad: the user closed the pad window.
    PadClosed,
    /// The bear's menu, for the bear on screen.
    BearMenu(Vec<crate::menu::Entry>),
    /// A choice from the bear's menu on screen, for the UI to carry out.
    Act(crate::menu::Act),
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
    /// The microphone's level, after gain (RMS, 0 to 1), ten times a second.
    fn meter(&self, _level: f32) {}
    /// The microphone gain now in use (a factor, 1 and up).
    fn gain(&self, _gain: f32) {}
    /// During a microphone test: what was heard, and how loud it came in.
    fn heard(&self, _text: String, _level: f32) {}
    /// A choice from the bear's menu on screen.
    fn act(&self, _act: crate::menu::Act) {}
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
            .spawn({
                let commands = commands.clone();
                move || Worker::new(paths, settings, ui, rx, commands).run()
            })
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
    /// Measuring a just-opened microphone (see `mic_check.rs`).
    mic_check: Option<mic_check::MicCheck>,
    /// A different microphone was chosen mid-dictation: reopen after.
    reopen_mic: bool,
    /// The settings changed while models were loading: load again.
    reload_after_load: bool,
    /// Quiet sentences in a row, and whether that was said this run.
    quiet_run: u8,
    quiet_warned: bool,
    /// Learned microphone gains by microphone, and the gain in use.
    gains: std::collections::BTreeMap<String, f32>,
    gain: f32,
    /// The level meter: loudest chunk since it last updated, and when.
    meter_peak: f32,
    meter_at: Instant,
    /// The Microphone page's test is on; it started a dictation to listen.
    testing: bool,
    test_dictation: bool,
    /// What the status says while models load: which, and whether a switch.
    loading_detail: String,
    /// Always listening was awake when a reload started: wake it again.
    wake_after_load: bool,
    cleaning: bool,
    last_dictation: Option<Instant>,
    type_text: Sender<TypeJob>,
    injected: Receiver<Injected>,
    injector_error: Option<String>,
    settings_mtime: Option<SystemTime>,
    pad: pad::PadState,
}

impl<U: Ui> Worker<U> {
    fn new(paths: Paths, settings: Settings, ui: U, commands: Receiver<Command>, to_self: Sender<Command>) -> Self {
        let (type_text, injected) = spawn_injector(&paths);
        let gains = paths.load_gains();
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
            mic_check: None,
            reopen_mic: false,
            reload_after_load: false,
            quiet_run: 0,
            quiet_warned: false,
            gains,
            gain: 1.0,
            meter_peak: 0.0,
            meter_at: Instant::now(),
            testing: false,
            test_dictation: false,
            loading_detail: String::new(),
            wake_after_load: false,
            cleaning: false,
            last_dictation: None,
            type_text,
            injected,
            injector_error: None,
            pad: pad::PadState::new(to_self),
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
                    Ok(Command::MicTest(on)) => self.mic_test(on),
                    Ok(Command::Apply(settings)) => self.apply(settings),
                    Ok(Command::ModelsChanged) => {
                        // "auto" may now mean a different model; a failed load
                        // may now succeed.
                        if self.settings.cleanup == "auto" || self.load_error.is_some() {
                            self.request_reload();
                        }
                    }
                    Ok(Command::Pad(action)) => self.on_pad_command(action),
                    Ok(Command::PadClosed) => self.on_pad_closed(),
                    Ok(Command::BearMenu(entries)) => {
                        self.pad.menu = entries;
                        self.sync_pad();
                    }
                    Ok(Command::Act(act)) => self.ui.act(act),
                    Ok(Command::Shutdown) | Err(_) => break,
                },
                recv(audio) -> ev => if let Ok(ev) = ev { self.on_audio(ev) },
                recv(events) -> ev => if let Ok(ev) = ev { self.on_pipeline(ev) },
                recv(loading) -> loaded => self.on_loaded(loaded.unwrap_or_else(|_| Err("model loading stopped".into()))),
                recv(injected) -> report => if let Ok(report) = report { self.on_injected(report) },
                recv(ticks) -> _ => {
                    self.watch_files();
                    self.check_mic_level();
                }
            }
        }
        if let Some(capture) = self.capture.as_mut() {
            let _ = capture.stop();
        }
    }

    // ---- status -------------------------------------------------------------

    fn publish(&self) {
        let (state, detail) = if self.loading.is_some() {
            (State::Loading, self.loading_detail.clone())
        } else if let Some(e) = self.load_error.as_ref().or(self.injector_error.as_ref()) {
            (State::NeedsYou, e.clone())
        } else if self.listening_since.is_some() {
            let detail = match (self.reload_when_idle, load::cleanup_label(&self.settings)) {
                (true, Some(next)) => format!("{} · {next} loads when you stop dictating", self.cleanup_name),
                (true, None) => format!("{} · post-processing turns off when you stop dictating", self.cleanup_name),
                _ => self.cleanup_name.clone(),
            };
            (State::Listening, detail)
        } else if self.cleaning {
            (State::Cleaning, self.cleanup_name.clone())
        } else if self.standby {
            (State::Asleep, self.cleanup_name.clone())
        } else if self.settings.voice_wake {
            (State::MicOff, self.cleanup_name.clone())
        } else {
            (State::Idle, self.cleanup_name.clone())
        };
        crate::telemetry::set_dictating(matches!(state, State::Listening | State::Cleaning));
        self.ui.status(Status { state, detail, since: self.listening_since, voice: self.settings.voice_wake });
        self.pad.look.set(state);
        self.sync_pad();
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
            self.mic_check = Some(mic_check::MicCheck::new(capture.name()));
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
        self.pad.dismissed = false;
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
            CaptureEvent::Chunk(chunk) => {
                pipeline.push_audio(chunk.samples);
                self.note_level(chunk.rms);
                self.note_meter(chunk.rms);
            }
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

    fn apply(&mut self, new: Settings) {
        let old = std::mem::replace(&mut self.settings, new);
        let s = &self.settings;
        self.notifier.set_enabled(s.notifications);
        if (old.voice_wake, old.microphone_on) != (s.voice_wake, s.microphone_on) {
            self.apply_rest(&old);
            return self.switch_mic(old.microphone_on);
        }
        if old.microphone != s.microphone
            && let Some(pipeline) = &self.pipeline
        {
            // Each microphone has its own learned gain.
            pipeline.reconfigure(pipeline_config(s, self.starting_gain()));
            self.gain = self.starting_gain();
        }
        let s = &self.settings;
        if self.standby && old.microphone != s.microphone {
            // A different microphone: reopen it, staying asleep or awake.
            let awake = self.listening_since.is_some();
            self.leave_standby();
            self.stand_by(awake);
        } else if old.microphone != s.microphone || old.keep_mic_open != s.keep_mic_open {
            if self.busy() {
                tracing::info!("microphone change applies after this dictation");
                self.reopen_mic = true;
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
            pipeline.reconfigure(pipeline_config(s, self.starting_gain()));
        }
    }

    fn request_reload(&mut self) {
        if self.loading.is_some() {
            // One load at a time: two at once hold two copies of the model.
            // The one running is stale; load again when it lands.
            self.reload_after_load = true;
            return;
        }
        // Always listening is awake for hours; waiting for "go to sleep"
        // would keep the old model all that time. Reload now and wake again
        // after (speech during the few seconds of loading isn't typed).
        let waits = self.cleaning || (self.listening_since.is_some() && !self.standby);
        if waits {
            self.reload_when_idle = true;
            self.publish();
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
