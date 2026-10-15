//! The typing thread: types text and takes it back, off the engine thread
//! (setting up typing can wait on a desktop permission dialog).

use std::fs;
use std::sync::Arc;
use std::thread;

use bd_core::InjectOutcome;
use bd_platform::{Erased, InjectorConfig, PlatformInjector};
use crossbeam_channel::{Receiver, Sender, unbounded};

use crate::settings::Paths;

/// Work for the typing thread.
pub(super) enum TypeJob {
    Type(String),
    /// Take back text typed earlier in this dictation.
    Erase(String),
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
