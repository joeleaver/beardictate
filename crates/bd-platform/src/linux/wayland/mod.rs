//! Wayland backend: libei through the RemoteDesktop portal.
//!
//! Wayland has no global synthetic-input API for ordinary clients. The
//! sanctioned route on KDE Plasma 6 and GNOME 45+ is the RemoteDesktop
//! portal, which (after a one-time permission dialog) hands us a libei
//! socket. The libei connection must answer the compositor's pings, so it
//! lives on a worker thread; the injector sends it commands.
//!
//! Limits: the focused app can't be identified (no per-app overrides), and
//! characters missing from the active layout can't be typed unless the
//! compositor offers `ei_text` (libei 1.6+), so `Strategy::Auto` pastes them.

mod keymap;
#[cfg(test)]
mod live_test;
mod portal;
mod session;

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use self::session::{EiSession, wait_readable};
use crate::config::{InjectorConfig, PasteChord, RestoreTokenSink, Settings};
use crate::driver::{Chord, Typed};
use crate::error::PlatformError;
use crate::linux::keysym::{Stroke, chord_stroke};
use crate::text::Token;

pub(crate) use self::portal::Connected;

/// Obtains a libei socket, given the last restore token. The portal in
/// production; a nested compositor in the live test.
pub(crate) type Connector =
    Arc<dyn Fn(Option<&str>) -> Result<Connected, PlatformError> + Send + Sync>;

enum Command {
    WaitModifiers(Duration, Sender<Result<bool, PlatformError>>),
    Type(Vec<Token>, Settings, Sender<Result<Typed, PlatformError>>),
    Stroke(Stroke, Sender<Result<Option<String>, PlatformError>>),
}

struct Worker {
    commands: Sender<Command>,
    wake: UnixStream,
    thread: Option<JoinHandle<()>>,
}

impl Worker {
    fn ask<T>(
        &mut self,
        make: impl FnOnce(Sender<Result<T, PlatformError>>) -> Command,
    ) -> Result<T, PlatformError> {
        let (tx, rx) = mpsc::channel();
        let dead = || PlatformError::Ei("the libei worker stopped".into());
        self.commands.send(make(tx)).map_err(|_| dead())?;
        self.wake.write_all(&[1]).map_err(|_| dead())?;
        rx.recv().map_err(|_| dead())?
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        // Closing the wake socket ends the worker loop.
        let _ = self.wake.shutdown(std::net::Shutdown::Both);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

pub(crate) struct WaylandBackend {
    worker: Option<Worker>,
    connector: Connector,
    /// The latest restore token, shared with the worker that receives it.
    token: Arc<Mutex<Option<String>>>,
    sink: Option<RestoreTokenSink>,
    /// Why the last portal attempt failed; we don't re-prompt until
    /// `prepare` is called again.
    refused: Option<String>,
}

impl WaylandBackend {
    pub(crate) fn new(config: &InjectorConfig) -> Self {
        Self::with_connector(config, Arc::new(portal::start))
    }

    pub(crate) fn with_connector(config: &InjectorConfig, connector: Connector) -> Self {
        Self {
            worker: None,
            connector,
            token: Arc::new(Mutex::new(config.portal_restore_token.clone())),
            sink: config.on_portal_restore_token.clone(),
            refused: None,
        }
    }

    pub(crate) fn prepare(&mut self, config: &InjectorConfig) -> Result<(), PlatformError> {
        self.sink = config.on_portal_restore_token.clone();
        self.refused = None;
        self.worker = None;
        self.ensure().map(|_| ())
    }

    /// The running worker, starting one (portal + libei) if needed.
    fn ensure(&mut self) -> Result<&mut Worker, PlatformError> {
        if let Some(why) = &self.refused {
            return Err(PlatformError::Portal(why.clone()));
        }
        if self
            .worker
            .as_ref()
            .is_some_and(|w| w.thread.as_ref().is_some_and(|t| t.is_finished()))
        {
            self.worker = None;
        }
        if self.worker.is_none() {
            match spawn(
                self.connector.clone(),
                self.token.clone(),
                self.sink.clone(),
            ) {
                Ok(w) => self.worker = Some(w),
                Err(e) => {
                    if matches!(e, PlatformError::Portal(_)) {
                        self.refused = Some(e.to_string());
                    }
                    return Err(e);
                }
            }
        }
        Ok(self.worker.as_mut().expect("just set"))
    }

    /// Runs a command, restarting the connection once if it had died (the
    /// restore token makes that silent).
    fn ask<T>(
        &mut self,
        make: impl Fn(Sender<Result<T, PlatformError>>) -> Command,
    ) -> Result<T, PlatformError> {
        match self.ensure()?.ask(&make) {
            Err(PlatformError::Ei(e)) => {
                tracing::info!(error = %e, "libei connection lost; reconnecting");
                self.worker = None;
                self.ensure()?.ask(&make)
            }
            other => other,
        }
    }

    pub(crate) fn wait_modifiers_released(
        &mut self,
        timeout: Duration,
    ) -> Result<bool, PlatformError> {
        match self.ask(|tx| Command::WaitModifiers(timeout, tx)) {
            Ok(released) => Ok(released),
            // No connection means we can't type either; type_tokens reports why.
            Err(_) => Ok(true),
        }
    }

    pub(crate) fn type_tokens(
        &mut self,
        tokens: &[Token],
        settings: &Settings,
    ) -> Result<Typed, PlatformError> {
        match self.ask(|tx| Command::Type(tokens.to_vec(), *settings, tx)) {
            Ok(t) => Ok(t),
            Err(e) => Ok(Typed::Impossible {
                reason: e.to_string(),
            }),
        }
    }

    pub(crate) fn send_paste_chord(&mut self, chord: PasteChord) -> Result<Chord, PlatformError> {
        match self.ask(|tx| Command::Stroke(chord_stroke(chord), tx)) {
            Ok(None) => Ok(Chord::Sent),
            Ok(Some(reason)) => Ok(Chord::Impossible { reason }),
            Err(e) => Ok(Chord::Impossible {
                reason: e.to_string(),
            }),
        }
    }
}

fn spawn(
    connector: Connector,
    token: Arc<Mutex<Option<String>>>,
    sink: Option<RestoreTokenSink>,
) -> Result<Worker, PlatformError> {
    let (commands, rx) = mpsc::channel();
    let (wake, wake_rx) = UnixStream::pair().map_err(|e| PlatformError::Ei(e.to_string()))?;
    let (ready_tx, ready_rx) = mpsc::channel();
    let thread = thread::Builder::new()
        .name("bd-libei".into())
        .spawn(move || run(connector, token, sink, rx, wake_rx, ready_tx))
        .map_err(|e| PlatformError::Ei(e.to_string()))?;
    match ready_rx.recv() {
        Ok(Ok(())) => Ok(Worker {
            commands,
            wake,
            thread: Some(thread),
        }),
        Ok(Err(e)) => {
            let _ = thread.join();
            Err(e)
        }
        Err(_) => Err(PlatformError::Ei(
            "the libei worker exited during setup".into(),
        )),
    }
}

fn run(
    connector: Connector,
    token: Arc<Mutex<Option<String>>>,
    sink: Option<RestoreTokenSink>,
    commands: Receiver<Command>,
    mut wake: UnixStream,
    ready: Sender<Result<(), PlatformError>>,
) {
    let previous = token.lock().map(|t| t.clone()).unwrap_or(None);
    let granted = match connector(previous.as_deref()) {
        Ok(g) => g,
        Err(e) => {
            let _ = ready.send(Err(e));
            return;
        }
    };
    if let Some(new) = &granted.restore_token {
        if let Ok(mut t) = token.lock() {
            *t = Some(new.clone());
        }
        if let Some(sink) = &sink {
            sink(new);
        }
    }
    let _keepalive = granted.keepalive;
    let mut ei = match EiSession::connect(granted.eis) {
        Ok(s) => s,
        Err(e) => {
            let _ = ready.send(Err(e));
            return;
        }
    };
    let _ = ready.send(Ok(()));
    tracing::info!("libei keyboard ready");

    loop {
        let readable = match wait_readable(&[ei.fd(), &wake], None) {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!(error = %e, "libei poll failed");
                break;
            }
        };
        if readable[0] && ei.dispatch().is_err() {
            break;
        }
        if readable[1] {
            let mut buf = [0u8; 64];
            match wake.read(&mut buf) {
                Ok(0) | Err(_) => break, // injector dropped
                Ok(_) => {}
            }
            while let Ok(cmd) = commands.try_recv() {
                match cmd {
                    Command::WaitModifiers(t, tx) => drop(tx.send(ei.wait_modifiers_released(t))),
                    Command::Type(tokens, s, tx) => drop(tx.send(ei.type_tokens(&tokens, &s))),
                    Command::Stroke(stroke, tx) => drop(tx.send(ei.send_stroke(stroke))),
                }
            }
        }
        if let Err(e) = ei.alive() {
            tracing::warn!(error = %e, "libei session ended");
            break;
        }
    }
}
