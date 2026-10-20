//! The typing thread: types text and takes it back, off the engine thread
//! (setting up typing can wait on a desktop permission dialog).

use std::fs;
use std::sync::Arc;
use std::thread;
use std::time::Instant;

use bd_core::InjectOutcome;
use bd_platform::{Erased, InjectorConfig, PlatformInjector};
use crossbeam_channel::{Receiver, Sender, unbounded};
use serde_json::json;

use super::{Ui, Worker};
use crate::settings::Paths;

/// Work for the typing thread.
pub(super) enum TypeJob {
    Type(String, Typed),
    /// Take back text typed earlier in this dictation.
    Erase(String),
}

/// What a `typed` telemetry event says besides how typing went
/// (docs/telemetry.md).
pub(super) struct Typed {
    pub segment: Option<u64>,
    /// Characters taken back before it (a space before a break or mark).
    pub take_back: usize,
    /// Typed as it was spoken: your app with tidy off.
    pub live: bool,
    /// The Bear Pad's text, sent.
    pub pad: bool,
}

/// What the typing thread reports.
pub(super) enum Injected {
    /// Typing works; the backend's name.
    Ready(&'static str),
    LeftOnClipboard(String),
    /// "Scratch that" couldn't take the text back.
    NotErased(String),
    Failed(String),
}

/// Starts the typing thread. It sets up the platform injector first, which
/// on Wayland asks the desktop for permission (once; the approval is kept in
/// [`Paths::portal_token`]).
pub(super) fn spawn_injector(paths: &Paths) -> (Sender<TypeJob>, Receiver<Injected>) {
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
            let backend = injector.backend_name();
            let _ = report_tx.send(Injected::Ready(backend));
            for job in text_rx {
                let report = match job {
                    TypeJob::Type(text, typed) => {
                        let started = Instant::now();
                        let outcome = injector.inject_text(&text);
                        note_typed(&text, &typed, backend, &outcome, started);
                        match outcome {
                            Ok(InjectOutcome::LeftOnClipboard { reason }) => Some(Injected::LeftOnClipboard(reason)),
                            Ok(_) => None,
                            Err(e) => Some(Injected::Failed(e.to_string())),
                        }
                    }
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

/// The `typed` event, once typing has gone one way or the other.
fn note_typed<E: std::fmt::Display>(text: &str, typed: &Typed, backend: &str, outcome: &Result<InjectOutcome, E>, started: Instant) {
    let inject_ms = crate::telemetry::ms(started.elapsed());
    crate::telemetry::record("typed", typed.segment, || {
        let (outcome, restored, reason) = match outcome {
            Ok(InjectOutcome::Typed) => ("typed", None, None),
            Ok(InjectOutcome::Pasted { restored }) => ("pasted", Some(*restored), None),
            Ok(InjectOutcome::LeftOnClipboard { reason }) => ("clipboard", None, Some(reason.clone())),
            Err(e) => ("failed", None, Some(e.to_string())),
        };
        json!({
            "text": text,
            "take_back": typed.take_back,
            "live": typed.live,
            "pad": typed.pad,
            "backend": backend,
            "outcome": outcome,
            "restored": restored,
            "reason": reason,
            "inject_ms": inject_ms,
        })
    });
}

impl<U: Ui> Worker<U> {
    pub(super) fn on_injected(&mut self, report: Injected) {
        // Text that didn't reach the page: what was typed before it can't be
        // found by counting back from the cursor any more, so nothing typed
        // earlier is tidied in place or scratched across it.
        if matches!(report, Injected::LeftOnClipboard(_) | Injected::NotErased(_) | Injected::Failed(_))
            && let Some(p) = &self.pipeline
        {
            p.new_passage();
        }
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
}
