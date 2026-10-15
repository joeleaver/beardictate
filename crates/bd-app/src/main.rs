//! beardictate: dictation for fiction writers, from the system tray.
//!
//! The tray icon is the whole UI (DESIGN.md "UI"): its snout and corner dot
//! show the state, a left click or the hotkey starts and stops dictation,
//! and the right-click menu holds the everyday settings. Everything else is
//! in the settings file, which the menu opens. The work happens in
//! [`engine`], off the UI thread.

mod control;
mod engine;
mod icon;
mod mic;
mod notify;
mod settings;
mod ui;
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
    let settings = match paths.load() {
        Ok(s) => s,
        Err(e) => {
            // Don't overwrite a settings file with a mistake in it; say where.
            eprintln!("beardictate: {e:#}");
            std::process::exit(1);
        }
    };
    let shared = Shared::new(paths, settings);
    SHARED.with(|s| *s.borrow_mut() = Some(shared.clone()));

    let starting = Status { state: State::Loading, detail: "Starting…".into(), since: None };
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
        visible: !has_tray,
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
    TRAY.with(|t| t.borrow_mut().take());
}

/// Logs to stderr and to `log` (started fresh each run), so a run started
/// from the app menu still leaves a trace.
fn init_logging(log: &std::path::Path) {
    use std::io::Write;
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
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_env("BD_LOG").unwrap_or_else(|_| EnvFilter::new("info,llama-cpp-2=warn")))
        .with_ansi(false)
        .with_writer(move || Tee(file.clone()))
        .init();
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
    window::view(__scope, &shared)
}
