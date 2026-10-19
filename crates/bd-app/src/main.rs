//! beardictate: dictation for fiction writers, from the system tray.
//!
//! The tray icon is the whole UI (DESIGN.md "UI"): its snout and corner dot
//! show the state, a left click or the hotkey starts and stops dictation,
//! and the right-click menu holds the everyday settings. Everything else is
//! in the settings file, which the menu opens. The work happens in
//! [`engine`], off the UI thread.

// A tray app: no console window on Windows. (Output then goes only to the
// log file; `--toggle` reports through its exit code.)
#![cfg_attr(windows, windows_subsystem = "windows")]

#[cfg(windows)]
mod autostart;
mod bear;
mod bear_view;
mod control;
mod engine;
mod icon;
mod menu;
mod mic;
mod models;
mod notify;
mod pad_window;
mod settings;
mod telemetry;
mod ui;
mod update;
mod window;

use std::cell::RefCell;
use std::rc::Rc;

use rinch::prelude::*;
use rinch::tray::{TrayIcon, TrayIconBuilder};
use tracing_subscriber::EnvFilter;

use crate::engine::{Engine, State, Status};
use crate::settings::Paths;
use crate::ui::{Shared, tray_image};

thread_local! {
    /// The tray, kept on the main thread (a `TrayIcon` is `!Send`).
    static TRAY: RefCell<Option<Rc<TrayIcon>>> = const { RefCell::new(None) };
    /// The engine, started by the first render (it posts to the UI thread,
    /// which needs the running app).
    static ENGINE: RefCell<Option<Engine>> = const { RefCell::new(None) };
    static SHARED: RefCell<Option<Shared>> = const { RefCell::new(None) };
}

/// The app's shared state, on the UI thread.
pub(crate) fn shared() -> Option<Shared> {
    SHARED.with(|s| s.borrow().clone())
}

pub(crate) fn tray() -> Option<Rc<TrayIcon>> {
    TRAY.with(|t| t.borrow().clone())
}

pub(crate) fn engine_send(command: engine::Command) {
    ENGINE.with(|e| {
        if let Some(engine) = e.borrow().as_ref() {
            engine.send(command);
        }
    });
}

fn main() {
    match std::env::args().nth(1).as_deref() {
        None => {}
        // The Bear Pad window, started by the app (see pad_window.rs).
        Some("--pad") => return pad_window::run(),
        Some("--toggle") => match control::send(control::Request::Toggle) {
            Ok(()) => return,
            Err(e) => {
                eprintln!("beardictate isn't running ({e})");
                std::process::exit(1);
            }
        },
        Some(other) => {
            eprintln!("usage: beardictate [--toggle]\n  --toggle  start or stop dictating in the running app");
            std::process::exit(if other == "--help" || other == "-h" { 0 } else { 2 });
        }
    }
    if control::send(control::Request::Ping).is_ok() {
        eprintln!("beardictate is already running (its bear is in the tray)");
        return;
    }
    let paths = match Paths::locate() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("beardictate: {e:#}");
            std::process::exit(1);
        }
    };
    init_logging(&paths.log);
    // A "segments" folder next to the log turns on saving what the
    // recognizer hears, for diagnosing mishearings (bd_pipeline::dump).
    if let Some(dir) = paths.log.parent().map(|d| d.join("segments")).filter(|d| d.is_dir()) {
        tracing::info!("saving heard segments to {}", dir.display());
        bd_pipeline::dump::save_to(dir);
    }
    let settings = match paths.load() {
        Ok(s) => s,
        Err(e) => {
            // Don't overwrite a settings file with a mistake in it; say where.
            eprintln!("beardictate: {e:#}");
            std::process::exit(1);
        }
    };
    // A downloaded update installs now, before anything starts.
    let note = match update::at_start(&paths.updates, settings.auto_update) {
        update::Start::Quit => return,
        update::Start::Run(note) => note,
    };
    telemetry::start(&paths);
    let updates = update::Updates::spawn(paths.updates.clone(), settings.auto_update, note);
    let shared = Shared::new(paths, settings, updates);
    let first_run = shared.models.states.with(|s| !models::missing_speech(s).is_empty());
    // An alpha's first start (an update arrives silently, past the
    // installer's notice): the window opens on About, which says what's sent.
    let alpha_notice = telemetry::first_start(&shared.paths);
    if alpha_notice && !first_run {
        shared.section.set(ui::Section::About);
    }
    SHARED.with(|s| *s.borrow_mut() = Some(shared.clone()));

    let starting = Status { state: State::Loading, detail: "Starting…".into(), since: None, voice: false };
    let tray = TrayIconBuilder::new()
        .with_id("beardictate")
        .with_title("beardictate")
        .with_tooltip("Starting…")
        .with_icon(tray_image(&starting, shared.settings.get().tray_theme))
        .with_menu(ui::tray_menu(&shared))
        .on_activate(|| engine_send(engine::Command::Toggle))
        .build();
    // Without a tray (GNOME without an AppIndicator extension, a bare
    // compositor) the status window stands in for it.
    let has_tray = match tray {
        Ok(tray) => {
            TRAY.with(|t| *t.borrow_mut() = Some(Rc::new(tray)));
            true
        }
        Err(e) => {
            tracing::warn!("no system tray ({e}); showing the window instead");
            false
        }
    };

    let props = WindowProps {
        title: "beardictate settings".into(),
        width: 780,
        height: 600,
        // On first run the window opens to download the speech models.
        visible: !has_tray || first_run || alpha_notice,
        // With a tray, closing hides the window; without one it quits.
        on_close_requested: has_tray.then(|| {
            std::sync::Arc::new(|| {
                hide_current_window();
                false
            }) as _
        }),
        ..Default::default()
    };
    App::new(app).window_props(props).activation_policy(ActivationPolicy::Accessory).run();

    // Stop dictation and the model threads, then take the tray down.
    ENGINE.with(|e| e.borrow_mut().take());
    telemetry::stop();
    TRAY.with(|t| t.borrow_mut().take());
    // "Restart to update": the installer takes over from here.
    let updates = SHARED.with(|s| s.borrow().as_ref().map(|s| s.paths.updates.clone()));
    if let Some(dir) = updates {
        update::at_exit(&dir);
    }
}

/// Logs to stderr and to `log` (started fresh each run), so a run started
/// from the app menu still leaves a trace.
fn init_logging(log: &std::path::Path) {
    use std::io::Write;
    // Keep the previous run's log: a restart shouldn't erase what went wrong.
    let _ = std::fs::rename(log, log.with_extension("log.1"));
    let file = log.parent().and_then(|d| std::fs::create_dir_all(d).ok()).and_then(|()| std::fs::File::create(log).ok());
    let file = std::sync::Arc::new(std::sync::Mutex::new(file));
    struct Tee(std::sync::Arc<std::sync::Mutex<Option<std::fs::File>>>);
    impl Write for Tee {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            if let Some(f) = self.0.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
                let _ = f.write_all(buf);
            }
            std::io::stderr().write(buf)
        }
        fn flush(&mut self) -> std::io::Result<()> {
            std::io::stderr().flush()
        }
    }
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;
    use tracing_subscriber::Layer;
    let filter = EnvFilter::try_from_env("BD_LOG").unwrap_or_else(|_| EnvFilter::new("info,llama-cpp-2=warn"));
    let fmt = tracing_subscriber::fmt::layer().with_ansi(false).with_writer(move || Tee(file.clone())).with_filter(filter);
    let registry = tracing_subscriber::registry().with(fmt);
    #[cfg(feature = "alpha")]
    let registry = registry.with(telemetry::Layer);
    registry.init();
}

/// The settings window, hidden until the tray's "Settings…" opens it.
/// Mounted at startup, so it also starts the engine and the hotkey and keeps
/// the tray in step with the state.
#[component]
fn app() -> NodeHandle {
    let shared = SHARED.with(|s| s.borrow().clone()).expect("made in main");
    ENGINE.with(|e| {
        if e.borrow().is_none() {
            let engine = Engine::spawn(shared.paths.clone(), shared.settings.get(), shared.bridge());
            if let Err(err) = control::listen(engine.sender()) {
                tracing::warn!("`beardictate --toggle` won't work: {err}");
            }
            *e.borrow_mut() = Some(engine);
        }
    });
    ui::wire(&shared);
    bear::wire(&shared);
    window::view(__scope, &shared)
}
