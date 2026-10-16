//! Bear Pad on the engine's side: the pad window process, kept in step with
//! the pipeline's pad and the dictation state.

use std::cell::RefCell;
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command as Process, Stdio};
use std::thread;

use bd_pipeline::{PadAction, PadView};
use crossbeam_channel::Sender;

use super::{Command, Ui, Worker};
use crate::pad_window::ToPad;

/// The `beardictate --pad` process, started when there's first something to
/// show.
pub(super) struct PadHost {
    commands: Sender<Command>,
    child: Option<(Child, ChildStdin)>,
    /// What the window was last told, so only changes are sent.
    shown: bool,
    status: String,
    view: Option<ToPad>,
}

impl PadHost {
    pub fn new(commands: Sender<Command>) -> Self {
        Self { commands, child: None, shown: false, status: String::new(), view: None }
    }

    fn start(&mut self) -> Option<&mut ChildStdin> {
        if self.child.is_none() {
            let exe = std::env::current_exe().map_err(|e| tracing::warn!("Bear Pad: finding this program: {e}")).ok()?;
            let mut child = Process::new(exe)
                .arg("--pad")
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .spawn()
                .map_err(|e| tracing::warn!("Bear Pad: starting its window: {e}"))
                .ok()?;
            let (stdin, stdout) = (child.stdin.take()?, child.stdout.take()?);
            let commands = self.commands.clone();
            thread::Builder::new()
                .name("bd-pad-window".into())
                .spawn(move || {
                    for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                        let command = match line.trim() {
                            "send" => Command::Pad(PadAction::Send),
                            "clear" => Command::Pad(PadAction::Clear),
                            "undo" => Command::Pad(PadAction::Undo),
                            "closed" => Command::PadClosed,
                            _ => continue,
                        };
                        let _ = commands.send(command);
                    }
                })
                .ok()?;
            tracing::info!("Bear Pad window started");
            (self.shown, self.status, self.view) = (false, String::new(), None);
            self.child = Some((child, stdin));
        }
        self.child.as_mut().map(|(_, stdin)| stdin)
    }

    fn tell(&mut self, msg: &ToPad) {
        let Ok(line) = serde_json::to_string(msg) else { return };
        let Some(stdin) = self.start() else { return };
        if writeln!(stdin, "{line}").and_then(|()| stdin.flush()).is_err() {
            tracing::warn!("Bear Pad window went away; starting it again next time");
            self.stop();
        }
    }

    /// Brings the window up to date: shown or hidden, its status and text.
    fn sync(&mut self, visible: bool, status: String, view: &PadView) {
        if !visible && self.child.is_none() {
            return;
        }
        let view = ToPad::View { parts: view.parts.iter().map(|p| (p.text.clone(), p.clean)).collect(), cleaning: view.cleaning };
        if self.view.as_ref() != Some(&view) {
            self.tell(&view);
            self.view = Some(view);
        }
        if status != self.status {
            self.tell(&ToPad::Status { text: status.clone() });
            self.status = status;
        }
        if visible != self.shown {
            self.tell(if visible { &ToPad::Show } else { &ToPad::Hide });
            self.shown = visible;
        }
    }

    /// The user closed the window, which hid it.
    pub fn hidden(&mut self) {
        self.shown = false;
    }

    /// Closes the window's stdin, which tells it to save where it is and
    /// quit; kills it if it hasn't within a second.
    pub fn stop(&mut self) {
        if let Some((mut child, stdin)) = self.child.take() {
            drop(stdin);
            for _ in 0..20 {
                if matches!(child.try_wait(), Ok(Some(_))) {
                    return;
                }
                thread::sleep(std::time::Duration::from_millis(50));
            }
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

impl Drop for PadHost {
    fn drop(&mut self) {
        self.stop();
    }
}

/// The pad's state on the engine's side.
pub(super) struct PadState {
    pub host: RefCell<PadHost>,
    pub view: PadView,
    /// Closed by the user: hidden until the pad is emptied or dictation
    /// starts again.
    pub dismissed: bool,
}

impl PadState {
    pub fn new(commands: Sender<Command>) -> Self {
        Self { host: RefCell::new(PadHost::new(commands)), view: PadView::default(), dismissed: false }
    }
}

impl<U: Ui> Worker<U> {
    pub(super) fn on_pad_view(&mut self, view: PadView) {
        if view.is_empty() {
            self.pad.dismissed = false;
        }
        self.pad.view = view;
        self.sync_pad();
    }

    pub(super) fn on_pad_command(&mut self, action: PadAction) {
        match &self.pipeline {
            Some(p) if self.settings.bear_pad => p.pad(action),
            _ => {}
        }
    }

    pub(super) fn on_pad_closed(&mut self) {
        self.pad.dismissed = true;
        self.pad.host.borrow_mut().hidden();
    }

    /// Shows the pad while there's dictation or text in it (unless the user
    /// closed it), and keeps its status line current. Hidden, it keeps the
    /// size and place the user gave it.
    pub(super) fn sync_pad(&self) {
        let mut host = self.pad.host.borrow_mut();
        if !self.settings.bear_pad {
            return host.stop();
        }
        let listening = self.listening_since.is_some();
        let visible = !self.pad.dismissed && (listening || !self.pad.view.is_empty());
        let status = if self.loading.is_some() {
            "Loading…".to_string()
        } else if listening {
            "Listening · \u{201C}bear, send it\u{201D} types this".to_string()
        } else if self.standby {
            "Asleep · \u{201C}bear, wake up\u{201D}".to_string()
        } else {
            format!("Not listening · {} to dictate", self.settings.hotkey)
        };
        host.sync(visible, status, &self.pad.view);
    }
}
