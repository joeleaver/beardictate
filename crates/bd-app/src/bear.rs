//! The bear on screen, on the app's side: runs `beardictate --bear`
//! ([`crate::bear_window`]) while the setting is on, keeps its look and
//! menu in step with the app, and does what it asks. On the UI thread,
//! which has the menu's state and does its acts.

use std::cell::RefCell;
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::thread;

use rinch::reactive::Effect;

use crate::bear_window::ToBear;
use crate::engine::{self, State};
use crate::menu::{self, Act, Entry};
use crate::ui::Shared;

struct Host {
    child: Child,
    stdin: ChildStdin,
    /// What the window was last told.
    look: String,
    menu: Vec<Entry>,
}

thread_local! {
    static BEAR: RefCell<Option<Host>> = const { RefCell::new(None) };
}

fn state_name(state: State) -> &'static str {
    match state {
        State::Loading => "loading",
        State::Idle => "idle",
        State::Asleep => "asleep",
        State::MicOff => "mic-off",
        State::Listening => "listening",
        State::Cleaning => "cleaning",
        State::NeedsYou => "needs-you",
    }
}

/// Keeps the bear on screen while the setting is on.
pub fn wire(shared: &Shared) {
    let s = shared.clone();
    Effect::new(move || {
        if !s.settings.with(|st| st.show_bear) {
            return stop();
        }
        let look = state_name(s.status.get().state).to_string();
        let entries = menu::entries(&s);
        BEAR.with(|b| {
            let mut b = b.borrow_mut();
            if b.is_none() {
                *b = start();
            }
            let Some(host) = b.as_mut() else { return };
            let mut ok = true;
            if host.look != look {
                ok &= tell(&mut host.stdin, &ToBear::Look { state: look.clone() });
                host.look = look;
            }
            if host.menu != entries {
                ok &= tell(&mut host.stdin, &ToBear::Menu { entries: entries.clone() });
                host.menu = entries;
            }
            if !ok {
                tracing::warn!("the bear's window went away; starting it again at the next change");
                *b = None;
            }
        });
    });
}

fn tell(stdin: &mut ChildStdin, msg: &ToBear) -> bool {
    let Ok(line) = serde_json::to_string(msg) else { return true };
    writeln!(stdin, "{line}").and_then(|()| stdin.flush()).is_ok()
}

fn start() -> Option<Host> {
    let exe = std::env::current_exe().map_err(|e| tracing::warn!("the bear: finding this program: {e}")).ok()?;
    let mut child = Command::new(exe)
        .arg("--bear")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .map_err(|e| tracing::warn!("the bear: starting its window: {e}"))
        .ok()?;
    let (stdin, stdout) = (child.stdin.take()?, child.stdout.take()?);
    thread::Builder::new()
        .name("bd-bear-window".into())
        .spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                let line = line.trim();
                if line == "toggle" {
                    rinch::run_on_main_thread(|| crate::engine_send(engine::Command::Toggle));
                } else if let Some(json) = line.strip_prefix("act ") {
                    match serde_json::from_str::<Act>(json) {
                        Ok(act) => rinch::run_on_main_thread(move || {
                            if let Some(s) = crate::shared() {
                                menu::perform(&s, act);
                            }
                        }),
                        Err(e) => tracing::warn!("the bear: {e}: {json}"),
                    }
                }
            }
        })
        .ok()?;
    tracing::info!("the bear's window started");
    Some(Host { child, stdin, look: String::new(), menu: Vec::new() })
}

/// Closes the bear's stdin, which tells it to save where it is and quit;
/// kills it if it hasn't within a second.
pub fn stop() {
    let Some(Host { mut child, stdin, .. }) = BEAR.with(|b| b.borrow_mut().take()) else { return };
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
