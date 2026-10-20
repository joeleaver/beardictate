//! Crashes, for alpha telemetry. A panic on any thread is recorded with
//! where it happened and sent at once: one on the engine thread leaves the
//! app looking alive while nothing works. A crash no panic hook sees (in
//! llama.cpp, onnxruntime or a GPU driver) shows at the next start: a
//! `running` file, removed when the app quits, says the last run never did.
//! A tester's reports of crashes (2026-10-20) left nothing to go on.

use std::fs;
use std::path::PathBuf;
use std::sync::OnceLock;

use serde_json::json;

/// The `running` file of this run.
static RUNNING: OnceLock<PathBuf> = OnceLock::new();

/// At start, once telemetry is on: reports a last run that ended without
/// quitting, marks this one as running, and catches panics.
pub(super) fn start(dir: &std::path::Path, session: &str) {
    let running = dir.join("running");
    if let Ok(last) = fs::read_to_string(&running) {
        super::record("error", None, || json!({"stage": "previous_run", "message": "the last run ended without quitting (a crash, a kill, or the computer going off)", "session": last.trim()}));
    }
    if let Err(e) = fs::write(&running, session) {
        tracing::warn!("marking the run: {e}");
    }
    let _ = RUNNING.set(running);
    let earlier = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let thread = std::thread::current().name().unwrap_or("unnamed").to_string();
        let location = info.location().map(|l| format!("{}:{}", l.file(), l.line()));
        let message = info
            .payload()
            .downcast_ref::<&str>()
            .map(|s| s.to_string())
            .or_else(|| info.payload().downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "(not a string)".into());
        let backtrace = std::backtrace::Backtrace::force_capture().to_string();
        let backtrace: String = backtrace.chars().take(6000).collect();
        super::record("error", None, || json!({"stage": "panic", "message": message, "thread": thread, "location": location, "backtrace": backtrace}));
        // Sent before the process can go; not from the thread that sends.
        if thread != "bd-telemetry" {
            super::stop();
        }
        earlier(info);
    }));
}

/// At a clean quit.
pub(super) fn quit() {
    if let Some(running) = RUNNING.get() {
        let _ = fs::remove_file(running);
    }
}
