//! The bear on screen: `beardictate --bear`, a process of its own like the
//! Bear Pad's (see [`crate::pad_window`] for why), started by the app
//! ([`crate::bear`]) when the setting is on.
//!
//! A small round bear that floats on top and never takes keyboard focus. Its
//! glow says what the app is doing, in the tray icon's dot colours, pulsing
//! while it listens. A click starts or stops dictating; pressing and moving
//! drags it; a right-click opens the tray's menu, drawn in the window, which
//! grows towards the middle of the screen to hold it.
//!
//! The app writes [`ToBear`] messages to its stdin, one JSON line each; it
//! answers on stdout: `toggle`, or `act ` and an [`Act`] as JSON. It quits
//! when its stdin closes. Where it was left is kept in
//! [`Paths::bear_window`] for next time.

use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

use rinch::prelude::*;
use serde::{Deserialize, Serialize};

use crate::menu::{Act, Entry};
use crate::settings::Paths;

/// The window's side while it shows only the bear.
const SIDE: f64 = 84.0;
const MENU_WIDTH: f64 = 260.0;
const ROW: f64 = 30.0;
const RULE: f64 = 9.0;

/// From the app to the bear.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "do", rename_all = "kebab-case")]
pub enum ToBear {
    /// What the app is doing: `idle`, `asleep`, `mic-off`, `listening`,
    /// `cleaning`, `loading` or `needs-you`.
    Look { state: String },
    Menu { entries: Vec<Entry> },
}

/// Where the bear was when the menu moved the window to make room.
static HOME: Mutex<Option<(f64, f64)>> = Mutex::new(None);

fn home() -> Option<(f64, f64)> {
    *HOME.lock().unwrap_or_else(|p| p.into_inner())
}

fn set_home(at: Option<(f64, f64)>) {
    *HOME.lock().unwrap_or_else(|p| p.into_inner()) = at;
}

static POSITION_FILE: OnceLock<Option<PathBuf>> = OnceLock::new();

fn position_file() -> Option<&'static PathBuf> {
    POSITION_FILE.get_or_init(|| Paths::locate().ok().map(|p| p.bear_window)).as_ref()
}

fn load_position() -> Option<(i32, i32)> {
    let text = std::fs::read_to_string(position_file()?).ok()?;
    let mut f = text.split_whitespace().map(|v| v.parse::<f64>().ok());
    Some((f.next()?? as i32, f.next()?? as i32))
}

/// Where the bear is, unless the menu has moved the window.
fn save_position(at: Option<(f64, f64)>) {
    let (Some(file), Some((x, y))) = (position_file(), at) else { return };
    if let Some(dir) = file.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(file, format!("{x:.0} {y:.0}\n"));
}

fn say(line: &str) {
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "{line}");
    let _ = out.flush();
}

/// Runs the window until the app goes away.
pub fn run() {
    // Out of the way until moved: near the top left (a Wayland overlay's
    // margins, on its output).
    let (x, y) = load_position().unwrap_or((24, 96));
    let props = WindowProps {
        title: "beardictate".into(),
        width: SIDE as u32,
        height: SIDE as u32,
        x: Some(x),
        y: Some(y),
        borderless: true,
        transparent: true,
        resizable: false,
        level: WindowLevel::Floating,
        visible_on_all_workspaces: true,
        non_activating: true,
        skip_taskbar: true,
        ..Default::default()
    };
    App::new(bear).window_props(props).run();
}

/// The glow's colour for a state, if it has one (the tray's dot colours).
fn glow(state: &str) -> Option<&'static str> {
    Some(match state {
        "listening" => "#D1452F",
        "cleaning" => "#1FA38A",
        "loading" => "#8B6CD9",
        "needs-you" => "#E0A800",
        "asleep" => "#7D8CA6",
        _ => return None,
    })
}

/// The snout, which turns cobalt while the bear is at work (as in the tray).
fn snout(state: &str) -> &'static str {
    match state {
        "listening" | "cleaning" => "#2F6FDB",
        "mic-off" | "loading" => "#B9AFA3",
        _ => "#E9E4DC",
    }
}

const CSS: &str = "
html, body { background: transparent; margin: 0; overflow: hidden; }
@keyframes bear-pulse {
    0% { opacity: 0.45; }
    100% { opacity: 1; }
}
.bear-glow { position: absolute; left: 12px; top: 12px; width: 60px; height: 60px; border-radius: 50%; }
.bear-glow.pulse { animation: bear-pulse 0.9s ease-in-out infinite alternate; }
.bear-row { display: flex; align-items: center; gap: 8px; height: 30px; padding: 0 10px; border-radius: 6px; font-size: 13px; white-space: nowrap; overflow: hidden; }
.bear-row.live:hover { background: rgba(47, 111, 219, 0.14); }
.bear-row.off { color: #8A93A3; }
";

/// One part of the bear: an ellipse, in the tray glyph's 32-unit grid.
fn part(cx: f64, cy: f64, rx: f64, ry: f64, colour: &str) -> String {
    // The 32 units span 44 px, centred in the 60 px disc at (12, 12).
    let (k, o) = (44.0 / 32.0, 20.0);
    format!(
        "position: absolute; left: {:.2}px; top: {:.2}px; width: {:.2}px; height: {:.2}px; border-radius: 50%; background: {colour};",
        o + (cx - rx) * k,
        o + (cy - ry) * k,
        2.0 * rx * k,
        2.0 * ry * k
    )
}

const INK: &str = "#0E1A30";

#[component]
fn bear() -> NodeHandle {
    let state = Signal::new(String::from("loading"));
    let entries = Signal::new(Vec::<Entry>::new());
    let open = Signal::new(false);
    // Pressed on the bear, and whether it has moved since (a drag).
    let pressed = Signal::new(false);
    let dragged = Signal::new(false);
    std::thread::spawn(move || {
        for line in std::io::stdin().lock().lines() {
            let Ok(line) = line else { break };
            match serde_json::from_str::<ToBear>(&line) {
                Ok(ToBear::Look { state: s }) => state.send(s),
                Ok(ToBear::Menu { entries: e }) => entries.send(e),
                Err(e) => eprintln!("beardictate --bear: {e}: {line}"),
            }
        }
        // The app has gone.
        save_position(home().or_else(current_window_position));
        std::process::exit(0);
    });
    let close_menu = move || {
        if !open.get() {
            return;
        }
        open.set(false);
        set_current_window_size(SIDE, SIDE);
        if let Some((x, y)) = home() {
            set_current_window_position(x, y);
        }
        set_home(None);
    };
    let open_menu = move || {
        let rows = entries.with(|e| e.iter().map(|e| if matches!(e, Entry::Separator) { RULE } else { ROW }).sum::<f64>());
        let (w, h) = (MENU_WIDTH, rows + 12.0);
        // Towards the middle of the screen: left of a bear on the right,
        // above one low down. A window that can't learn where it is (GNOME)
        // grows right and down.
        if let Some((x, y)) = current_window_position() {
            set_home(Some((x, y)));
            let nx = if x > 320.0 { x + SIDE - w } else { x };
            let ny = if y > 420.0 { y + SIDE - h } else { y };
            set_current_window_position(nx, ny);
        }
        set_current_window_size(w, h);
        open.set(true);
    };
    let pick = move |act: Act| {
        say(&format!("act {}", serde_json::to_string(&act).unwrap_or_default()));
        close_menu();
    };
    rsx! {
        div { style: "position: relative; width: 100%; height: 100%;",
            style { {CSS} }
            if open.get() {
                div {
                    style: "position: absolute; inset: 0; padding: 6px; box-sizing: border-box; background: #FBFCFE; color: #0E1A30; border: 1px solid rgba(14, 26, 48, 0.18); border-radius: 10px; font-family: sans-serif;",
                    onmouseleave: close_menu,
                    for (i, entry) in entries.get().into_iter().enumerate() {
                        div { key: i,
                            match entry.clone() {
                                Entry::Separator => div { style: "height: 1px; margin: 4px 6px; background: rgba(14, 26, 48, 0.12);" },
                                Entry::Item { label, act: None } => div { class: "bear-row off", span { style: "width: 14px;" } {label} },
                                Entry::Item { label, act: Some(act) } => div { class: "bear-row live", onclick: move || pick(act), span { style: "width: 14px;" } {label} },
                                Entry::Check { label, on, act } => div { class: "bear-row live", onclick: move || pick(act),
                                    span { style: "width: 14px;", {if on { "\u{2713}" } else { "" }} } {label}
                                },
                                Entry::Radio { label, on, act } => div { class: "bear-row live", onclick: move || pick(act),
                                    span { style: "width: 14px;", {if on { "\u{25CF}" } else { "" }} } {label}
                                },
                            }
                        }
                    }
                }
            } else {
                div {
                    style: "position: absolute; inset: 0;",
                    onmousedown: move || {
                        pressed.set(true);
                        dragged.set(false);
                    },
                    onmousemove: move || {
                        if pressed.get() && !dragged.get() {
                            dragged.set(true);
                            start_window_drag();
                        }
                    },
                    onmouseup: move || {
                        let was_click = pressed.get() && !dragged.get();
                        pressed.set(false);
                        if was_click {
                            say("toggle");
                        } else {
                            save_position(current_window_position());
                        }
                    },
                    oncontextmenu: move || {
                        pressed.set(false);
                        open_menu();
                    },
                    div {
                        class: {move || if state.get() == "listening" { "bear-glow pulse" } else { "bear-glow" }},
                        style: {move || match glow(&state.get()) {
                            Some(c) => format!("box-shadow: 0 0 12px 5px {c}; background: {c};"),
                            None => "box-shadow: 0 2px 6px 1px rgba(14, 26, 48, 0.35);".to_string(),
                        }},
                    }
                    div { style: "position: absolute; left: 12px; top: 12px; width: 60px; height: 60px; border-radius: 50%; background: #F4F7FD; border: 1px solid rgba(14, 26, 48, 0.15); box-sizing: border-box;" }
                    div { style: {part(10.0, 9.0, 5.0, 5.0, INK)} }
                    div { style: {part(22.0, 9.0, 5.0, 5.0, INK)} }
                    div { style: {part(16.0, 18.0, 12.0, 12.0, INK)} }
                    div { style: {move || part(16.0, 22.0, 6.2, 4.8, snout(&state.get()))} }
                    div { style: {part(16.0, 19.4, 2.7, 1.8, INK)} }
                }
            }
        }
    }
}
