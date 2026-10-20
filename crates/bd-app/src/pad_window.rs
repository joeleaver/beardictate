//! The bear's window: `beardictate --pad`, a process of its own that the
//! app starts while the bear on screen or the Bear Pad is on. It shows the
//! bear on its own ([`crate::bear_view`]), or, while the pad has something
//! to show, the Bear Pad with the bear in its title bar: one window, so the
//! two are never in different places.
//!
//! A process of its own because it must never take keyboard focus (the
//! text goes to the window you were typing in), which rinch can do only for
//! an app's one window. On Wayland it is a layer-shell overlay (KWin,
//! wlroots, Hyprland, niri, COSMIC; an ordinary window on GNOME, which
//! lacks it); on X11, Windows and macOS a non-activating window. Either way
//! it floats on top, borderless; the pad draws its own title bar to move it
//! by and edges to resize it.
//!
//! The app writes [`ToPad`] messages to its stdin, one JSON line each; it
//! answers on stdout one line each: `send`, `clear`, `undo` (the pad's
//! buttons), `closed` (its close button, which only hides the pad),
//! `toggle` (a click on the bear) or `act ` and an [`Act`] as JSON (the
//! bear's menu). It quits when its stdin closes. Where the pad was and how
//! big, and where the bear was, are kept in [`Paths::pad_window`] and
//! [`Paths::bear_window`] for next time.

use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

use rinch::prelude::*;
use serde::{Deserialize, Serialize};

use crate::bear_view::{self, MENU_WIDTH, SIDE};
use crate::menu::{Act, Entry};
use crate::settings::Paths;

static PATHS: OnceLock<Option<Paths>> = OnceLock::new();

fn paths() -> Option<&'static Paths> {
    PATHS.get_or_init(|| Paths::locate().ok()).as_ref()
}

fn pad_file() -> Option<&'static PathBuf> {
    paths().map(|p| &p.pad_window)
}

fn bear_file() -> Option<&'static PathBuf> {
    paths().map(|p| &p.bear_window)
}

/// A position (if any) and a size.
type Geometry = (Option<(i32, i32)>, (u32, u32));

/// The pad's saved position (if any) and size: `x y width height`, logical
/// pixels, `- -` for no position (Wayland, where a window can't place itself).
fn load_pad() -> Option<Geometry> {
    let text = std::fs::read_to_string(pad_file()?).ok()?;
    let f: Vec<&str> = text.split_whitespace().collect();
    let [x, y, w, h] = f[..] else { return None };
    let position = x.parse::<f64>().ok().zip(y.parse::<f64>().ok()).map(|(x, y)| (x as i32, y as i32));
    let size = (w.parse::<f64>().ok()? as u32, h.parse::<f64>().ok()? as u32);
    Some((position, (size.0.max(260), size.1.max(120))))
}

fn save_pad() {
    let (Some(file), Some((w, h))) = (pad_file(), current_window_size()) else { return };
    let position = current_window_position().map_or_else(|| "- -".to_string(), |(x, y)| format!("{x:.0} {y:.0}"));
    write(file, &format!("{position} {w:.0} {h:.0}\n"));
}

/// Where the bear was left; near the top left at first (a Wayland
/// overlay's margins, on its output).
fn load_bear() -> (f64, f64) {
    let saved = || {
        let text = std::fs::read_to_string(bear_file()?).ok()?;
        let mut f = text.split_whitespace().map(|v| v.parse::<f64>().ok());
        Some((f.next()??, f.next()??))
    };
    // A position no screen has (a runaway drag saved one) starts afresh.
    saved().filter(|&(x, y)| (-1000.0..20000.0).contains(&x) && (-1000.0..20000.0).contains(&y)).unwrap_or((24.0, 96.0))
}

fn save_bear(at: Option<(f64, f64)>) {
    if let (Some(file), Some((x, y))) = (bear_file(), at) {
        write(file, &format!("{x:.0} {y:.0}\n"));
    }
}

fn write(file: &PathBuf, text: &str) {
    if let Some(dir) = file.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Err(e) = std::fs::write(file, text) {
        eprintln!("beardictate --pad: saving where the window is: {e}");
    }
}

/// From the app to the window.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "do", rename_all = "kebab-case")]
pub enum ToPad {
    /// The pad has something to show.
    Show,
    /// It hasn't (the bear stays, if it's on).
    Hide,
    /// The bear on screen, on or off.
    Bear { on: bool },
    /// What the app is doing: `idle`, `asleep`, `mic-off`, `listening`,
    /// `cleaning`, `loading` or `needs-you`, then ` list` in list mode and
    /// ` quiet` while the microphone is too quiet.
    Look { state: String },
    /// The bear's menu.
    Menu { entries: Vec<Entry> },
    /// The line under the pad's text: listening or not, and how to send.
    Status { text: String },
    /// The pad's text, as (text, clean) parts, and whether it's being cleaned.
    View { parts: Vec<(String, bool)>, cleaning: bool },
}

/// What the window is showing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Shape {
    Hidden,
    Bear,
    Pad,
}

/// The window's state, shared by the stdin thread and the UI.
struct Window {
    shape: Shape,
    pad: bool,
    bear: bool,
    /// Where the bear was when the menu moved the window to make room.
    home: Option<(f64, f64)>,
    /// Where the bear rests: a release elsewhere was a drag.
    rest: Option<(f64, f64)>,
}

/// When the menu last opened.
static OPENED: Mutex<Option<std::time::Instant>> = Mutex::new(None);

static WINDOW: Mutex<Window> = Mutex::new(Window { shape: Shape::Hidden, pad: false, bear: false, home: None, rest: None });

fn window() -> std::sync::MutexGuard<'static, Window> {
    WINDOW.lock().unwrap_or_else(|p| p.into_inner())
}

/// Shows the pad, the bear or nothing, as asked, saving where the shape it
/// leaves was and putting the new one where it was last.
fn arrange(shape_signal: Signal<Shape>) {
    let mut w = window();
    let shape = match (w.pad, w.bear) {
        (true, _) => Shape::Pad,
        (false, true) => Shape::Bear,
        (false, false) => Shape::Hidden,
    };
    if shape == w.shape {
        return;
    }
    match w.shape {
        Shape::Pad => save_pad(),
        Shape::Bear => save_bear(w.home.take().or_else(current_window_position)),
        Shape::Hidden => {}
    }
    match shape {
        Shape::Hidden => hide_current_window(),
        Shape::Pad => {
            if let Some((position, (width, height))) = load_pad() {
                set_current_window_size(f64::from(width), f64::from(height));
                if let Some((x, y)) = position {
                    set_current_window_position(f64::from(x), f64::from(y));
                }
            } else {
                set_current_window_size(560.0, 280.0);
            }
            show_current_window();
        }
        Shape::Bear => {
            let at = load_bear();
            set_current_window_size(SIDE, SIDE);
            set_current_window_position(at.0, at.1);
            w.rest = Some(at);
            show_current_window();
        }
    }
    w.shape = shape;
    shape_signal.send(shape);
}

/// Runs the window until the app goes away.
pub fn run() {
    let (position, (width, height)) = load_pad().unwrap_or((None, (560, 280)));
    let props = WindowProps {
        title: "beardictate".into(),
        width,
        height,
        x: position.map(|p| p.0),
        y: position.map(|p| p.1),
        level: WindowLevel::Floating,
        // Every virtual desktop (and, on macOS, over full-screen apps).
        visible_on_all_workspaces: true,
        non_activating: true,
        skip_taskbar: true,
        visible: false,
        // Round, the bear shows what's behind it; the pad paints its own
        // background.
        transparent: true,
        // A layer-shell surface has no window frame, so none anywhere: the
        // pad draws its own title bar, and these edges resize it. Around
        // the bear they fall in its transparent margin.
        borderless: true,
        resizable: true,
        resize_inset: Some(6.0),
        on_close_requested: Some(std::sync::Arc::new(|| {
            close();
            false
        })),
        ..Default::default()
    };
    App::new(pad).window_props(props).run();
}

/// Closes the pad (the bear stays, if it's on), and says so.
fn close() {
    say("closed");
}

/// Whether the window is a Wayland layer surface, once it has been shown.
fn note_surface_kind(layer: Signal<bool>) {
    std::thread::spawn(move || {
        for _ in 0..20 {
            std::thread::sleep(std::time::Duration::from_millis(250));
            if let Some(kind) = current_window_surface_kind() {
                eprintln!("beardictate --pad: shown as {kind:?}");
                layer.send(kind == WindowSurfaceKind::LayerShell);
                return;
            }
        }
    });
}

fn say(line: &str) {
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "{line}");
    let _ = out.flush();
}

const RAW: &str = "color: var(--rinch-color-dimmed); font-style: italic;";

const CSS: &str = "html, body { background: transparent; margin: 0; overflow: hidden; }";

#[component]
fn pad() -> NodeHandle {
    let parts = Signal::new(Vec::<(String, bool)>::new());
    let status = Signal::new(String::new());
    let cleaning = Signal::new(false);
    let state = Signal::new(String::from("loading"));
    let entries = Signal::new(Vec::<Entry>::new());
    let shape = Signal::new(Shape::Hidden);
    let menu_open = Signal::new(false);
    // The pointer has been over the open menu: leaving it closes it then.
    let menu_entered = Signal::new(false);
    let layer = Signal::new(false);
    // Pressed on the bear (not a layer surface), and moved since: a drag.
    let pressed = Signal::new(false);
    let dragged = Signal::new(false);
    note_surface_kind(layer);
    std::thread::spawn(move || {
        for line in std::io::stdin().lock().lines() {
            let Ok(line) = line else { break };
            match serde_json::from_str::<ToPad>(&line) {
                Ok(ToPad::Show) => {
                    window().pad = true;
                    arrange(shape);
                }
                Ok(ToPad::Hide) => {
                    window().pad = false;
                    arrange(shape);
                }
                Ok(ToPad::Bear { on }) => {
                    window().bear = on;
                    arrange(shape);
                }
                Ok(ToPad::Look { state: s }) => state.send(s),
                Ok(ToPad::Menu { entries: e }) => entries.send(e),
                Ok(ToPad::Status { text }) => status.send(text),
                Ok(ToPad::View { parts: p, cleaning: c }) => {
                    parts.send(p);
                    cleaning.send(c);
                }
                Err(e) => eprintln!("beardictate --pad: {e}: {line}"),
            }
        }
        // The app has gone.
        let w = window();
        match w.shape {
            Shape::Pad => save_pad(),
            Shape::Bear => save_bear(w.home.or_else(current_window_position)),
            Shape::Hidden => {}
        }
        std::process::exit(0);
    });

    // The menu: over the pad, or in a window grown from the bear towards
    // the middle of the screen (left of a bear on the right, above one low
    // down; a window that can't learn where it is, on GNOME, grows right
    // and down).
    let close_menu = move || {
        if !menu_open.get() {
            return;
        }
        menu_open.set(false);
        if shape.get() == Shape::Bear {
            set_current_window_size(SIDE, SIDE);
            if let Some((x, y)) = window().home.take() {
                set_current_window_position(x, y);
            }
        }
    };
    // A leave closes the menu unless the pointer is over it again soon
    // (rinch says "leave" moving onto the menu's own rows too).
    let over = Signal::new(0u32);
    // And six seconds with no move over it, should no leave come.
    let idle_close = move || {
        let seen = over.get();
        set_timeout(6000, move || {
            if over.get() == seen {
                close_menu();
            }
        });
    };
    let open_menu = move || {
        if shape.get() == Shape::Bear {
            let (w, h) = (MENU_WIDTH, entries.with(|e| bear_view::menu_height(e)));
            if let Some((x, y)) = current_window_position() {
                window().home = Some((x, y));
                let nx = if x > 320.0 { x + SIDE - w } else { x };
                let ny = if y > 420.0 { y + SIDE - h } else { y };
                set_current_window_position(nx, ny);
            }
            set_current_window_size(w, h);
        }
        menu_entered.set(false);
        over.update(|n| *n += 1);
        idle_close();
        *OPENED.lock().unwrap_or_else(|p| p.into_inner()) = Some(std::time::Instant::now());
        menu_open.set(true);
    };
    let pick = move |act: Act| {
        // The release of the right-click that opened it, landing on a row.
        if OPENED.lock().unwrap_or_else(|p| p.into_inner()).is_some_and(|t| t.elapsed() < std::time::Duration::from_millis(400)) {
            return;
        }
        say(&format!("act {}", serde_json::to_string(&act).unwrap_or_default()));
        close_menu();
    };
    let enter = move || {
        menu_entered.set(true);
        over.update(|n| *n += 1);
        idle_close();
    };
    let leave = move || {
        if !menu_entered.get() {
            return;
        }
        over.update(|n| *n += 1);
        let seen = over.get();
        set_timeout(250, move || {
            if over.get() == seen {
                close_menu();
            }
        });
    };
    // A release on the bear: where the window didn't move, a click. Not
    // the release of the right-click that opened the menu, which has
    // moved the window to make room.
    let released = move || {
        if menu_open.get() {
            return;
        }
        let now = current_window_position();
        let mut w = window();
        if now.is_none() || now == w.rest {
            drop(w);
            say("toggle");
        } else {
            w.rest = now;
            save_bear(now);
        }
        // A resize at its edge: back to the bear's size.
        if current_window_size().is_some_and(|(a, b)| (a - SIDE).abs() > 1.0 || (b - SIDE).abs() > 1.0) {
            set_current_window_size(SIDE, SIDE);
        }
    };
    rsx! {
        div { style: "position: relative; width: 100%; height: 100%;",
            style { {CSS} }
            style { {bear_view::CSS} }
            if menu_open.get() {
                {bear_view::menu(__scope, entries, pick, enter, leave)}
            } else if shape.get() == Shape::Bear {
                // The bear on its own. On a Wayland overlay the press drags
                // it (as the pad's title does) and a release where it
                // started is a click; elsewhere the system's move starts
                // only once the pointer moves, which keeps clicks.
                if layer.get() {
                    div { style: "position: absolute; inset: 0;", data-drag-window: "true",
                        onmouseup: released,
                        oncontextmenu: open_menu,
                        {bear_view::glyph(__scope, state, 12.0, 60.0, 12.0)}
                    }
                } else {
                    div { style: "position: absolute; inset: 0;",
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
                            pressed.set(false);
                            released();
                        },
                        oncontextmenu: move || {
                            pressed.set(false);
                            open_menu();
                        },
                        {bear_view::glyph(__scope, state, 12.0, 60.0, 12.0)}
                    }
                }
            } else {
                div { style: "display: flex; flex-direction: column; height: 100%; box-sizing: border-box; padding: 0 14px 10px; gap: 8px; font-size: 15px; background: var(--rinch-color-body); color: var(--rinch-color-text); border: 1px solid var(--rinch-color-default-border); border-radius: 8px;",
                    div { style: "display: flex; align-items: center; gap: 6px; margin: 0 -14px; padding: 2px 4px 2px 6px; border-bottom: 1px solid var(--rinch-color-default-border); font-size: 12px; font-weight: 600;",
                        div { style: "position: relative; width: 26px; height: 26px; flex: none; cursor: pointer;",
                            onclick: || say("toggle"),
                            oncontextmenu: open_menu,
                            {bear_view::glyph(__scope, state, 3.0, 20.0, 6.0)}
                        }
                        div { data-drag-window: "true", style: "flex: 1; padding: 4px 0; cursor: move; color: var(--rinch-color-dimmed);", "Bear Pad" }
                        Button { variant: "subtle", size: "xs", onclick: close, "\u{00D7}" }
                    }
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
    }
}

/// Each part with a key that changes when it does, so a changed part is
/// drawn again and the others are left alone.
fn keyed(parts: Vec<(String, bool)>) -> Vec<(String, String, bool)> {
    parts.into_iter().enumerate().map(|(i, (text, clean))| (format!("{i}:{clean}:{text}"), text, clean)).collect()
}
