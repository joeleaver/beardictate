//! The Bear Pad window: `beardictate --pad`, a process of its own that the
//! app starts when the pad has something to show.
//!
//! A process of its own because the pad must never take keyboard focus (the
//! text goes to the window you were typing in), which rinch can do only for
//! an app's one window, and on Linux only under X11 (XWayland on a Wayland
//! desktop). It floats on top, with a title bar to move it by and edges to
//! resize it.
//!
//! The app writes [`ToPad`] messages to its stdin, one JSON line each; it
//! answers on stdout with one word a line: `send`, `clear`, `undo` (its
//! buttons) or `closed` (its close button, which only hides it). It quits
//! when its stdin closes.

use std::io::{BufRead, Write};

use rinch::prelude::*;
use serde::{Deserialize, Serialize};

/// From the app to the pad window.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "do", rename_all = "kebab-case")]
pub enum ToPad {
    Show,
    Hide,
    /// The line under the text: listening or not, and how to send.
    Status { text: String },
    /// The pad's text, as (text, clean) parts, and whether it's being cleaned.
    View { parts: Vec<(String, bool)>, cleaning: bool },
}

/// Runs the window until the app goes away.
pub fn run() {
    let props = WindowProps {
        title: "Bear Pad".into(),
        width: 560,
        height: 280,
        always_on_top: true,
        non_activating: true,
        skip_taskbar: true,
        visible: false,
        on_close_requested: Some(std::sync::Arc::new(|| {
            hide_current_window();
            say("closed");
            false
        })),
        ..Default::default()
    };
    // Under X11 the window can refuse focus; on Wayland it can't.
    App::new(pad).window_props(props).prefer_x11(cfg!(target_os = "linux")).run();
}

fn say(word: &str) {
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "{word}");
    let _ = out.flush();
}

const RAW: &str = "color: var(--rinch-color-dimmed); font-style: italic;";

#[component]
fn pad() -> NodeHandle {
    let parts = Signal::new(Vec::<(String, bool)>::new());
    let status = Signal::new(String::new());
    let cleaning = Signal::new(false);
    std::thread::spawn(move || {
        for line in std::io::stdin().lock().lines() {
            let Ok(line) = line else { break };
            match serde_json::from_str::<ToPad>(&line) {
                Ok(ToPad::Show) => show_current_window(),
                Ok(ToPad::Hide) => hide_current_window(),
                Ok(ToPad::Status { text }) => status.send(text),
                Ok(ToPad::View { parts: p, cleaning: c }) => {
                    parts.send(p);
                    cleaning.send(c);
                }
                Err(e) => eprintln!("beardictate --pad: {e}: {line}"),
            }
        }
        // The app has gone.
        std::process::exit(0);
    });
    rsx! {
        div { style: "display: flex; flex-direction: column; height: 100%; box-sizing: border-box; padding: 12px 14px 10px; gap: 8px; font-size: 15px;",
            // Bottom-anchored: the newest words stay in view.
            div { style: "flex: 1; min-height: 0; overflow: hidden; display: flex; flex-direction: column; justify-content: flex-end;",
                if parts.with(|p| p.iter().all(|(t, _)| t.trim().is_empty())) {
                    div { style: "color: var(--rinch-color-dimmed);", "Speak, and your words collect here." }
                } else {
                    div { style: "white-space: pre-wrap; line-height: 1.5;",
                        for part in keyed(parts.get()) {
                            span { key: part.0.clone(), style: {if part.2 { "" } else { RAW }}, {part.1.clone()} }
                        }
                    }
                }
            }
            div { style: "display: flex; align-items: center; gap: 8px; font-size: 12px;",
                div { style: "flex: 1; min-width: 0; color: var(--rinch-color-dimmed);",
                    {move || if cleaning.get() { format!("{} · tidying…", status.get()) } else { status.get() }}
                }
                Button { variant: "subtle", size: "xs", onclick: || say("undo"), "Undo" }
                Button { variant: "subtle", size: "xs", onclick: || say("clear"), "Clear" }
                Button { variant: "filled", size: "xs", onclick: || say("send"), "Send" }
            }
        }
    }
}

/// Each part with a key that changes when it does, so a changed part is
/// drawn again and the others are left alone.
fn keyed(parts: Vec<(String, bool)>) -> Vec<(String, String, bool)> {
    parts.into_iter().enumerate().map(|(i, (text, clean))| (format!("{i}:{clean}:{text}"), text, clean)).collect()
}
