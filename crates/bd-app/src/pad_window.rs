//! The Bear Pad window: `beardictate --pad`, a process of its own that the
//! app starts when the pad has something to show.
//!
//! A process of its own because the pad must never take keyboard focus (the
//! text goes to the window you were typing in), which rinch can do only for
//! an app's one window. On Wayland it is a layer-shell overlay (KWin,
//! wlroots, Hyprland, niri, COSMIC; an ordinary window on GNOME, which
//! lacks it); on X11, Windows and macOS a non-activating window. Either way
//! it floats on top, borderless, with its own title bar to move it by and
//! edges to resize it.
//!
//! The app writes [`ToPad`] messages to its stdin, one JSON line each; it
//! answers on stdout with one word a line: `send`, `clear`, `undo` (its
//! buttons) or `closed` (its close button, which only hides it). It quits
//! when its stdin closes. Where it was and how big are kept in
//! [`Paths::pad_window`] for next time.

use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::sync::OnceLock;

use rinch::prelude::*;
use serde::{Deserialize, Serialize};

use crate::settings::Paths;

/// [`Paths::pad_window`], when there is a home directory.
static GEOMETRY: OnceLock<Option<PathBuf>> = OnceLock::new();

fn geometry_file() -> Option<&'static PathBuf> {
    GEOMETRY.get_or_init(|| Paths::locate().ok().map(|p| p.pad_window)).as_ref()
}

/// A position (if any) and a size.
type Geometry = (Option<(i32, i32)>, (u32, u32));

/// The saved position (if any) and size: `x y width height`, logical
/// pixels, `- -` for no position (Wayland, where a window can't place itself).
fn load_geometry() -> Option<Geometry> {
    let text = std::fs::read_to_string(geometry_file()?).ok()?;
    let f: Vec<&str> = text.split_whitespace().collect();
    let [x, y, w, h] = f[..] else { return None };
    let position = x.parse::<f64>().ok().zip(y.parse::<f64>().ok()).map(|(x, y)| (x as i32, y as i32));
    let size = (w.parse::<f64>().ok()? as u32, h.parse::<f64>().ok()? as u32);
    Some((position, (size.0.max(260), size.1.max(120))))
}

fn save_geometry() {
    let (Some(file), Some((w, h))) = (geometry_file(), current_window_size()) else { return };
    let position = current_window_position().map_or_else(|| "- -".to_string(), |(x, y)| format!("{x:.0} {y:.0}"));
    if let Some(dir) = file.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Err(e) = std::fs::write(file, format!("{position} {w:.0} {h:.0}\n")) {
        eprintln!("beardictate --pad: saving where the window is: {e}");
    }
}

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
    let (position, (width, height)) = load_geometry().unwrap_or((None, (560, 280)));
    let props = WindowProps {
        title: "Bear Pad".into(),
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
        // A layer-shell surface has no window frame, so none anywhere: the
        // pad draws its own title bar, and these edges resize it.
        borderless: true,
        resize_inset: Some(6.0),
        on_close_requested: Some(std::sync::Arc::new(|| {
            close();
            false
        })),
        ..Default::default()
    };
    App::new(pad).window_props(props).run();
}

/// Hides the pad until the app shows it again, and says so.
fn close() {
    save_geometry();
    hide_current_window();
    say("closed");
}

/// Notes on stderr which kind of window the pad got, once it exists.
fn note_surface_kind() {
    static NOTED: OnceLock<()> = OnceLock::new();
    std::thread::spawn(|| {
        std::thread::sleep(std::time::Duration::from_secs(1));
        if let Some(kind) = current_window_surface_kind() {
            NOTED.get_or_init(|| eprintln!("beardictate --pad: shown as {kind:?}"));
        }
    });
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
                Ok(ToPad::Show) => {
                    show_current_window();
                    note_surface_kind();
                }
                Ok(ToPad::Hide) => {
                    save_geometry();
                    hide_current_window();
                }
                Ok(ToPad::Status { text }) => status.send(text),
                Ok(ToPad::View { parts: p, cleaning: c }) => {
                    parts.send(p);
                    cleaning.send(c);
                }
                Err(e) => eprintln!("beardictate --pad: {e}: {line}"),
            }
        }
        // The app has gone.
        save_geometry();
        std::process::exit(0);
    });
    rsx! {
        div { style: "display: flex; flex-direction: column; height: 100%; box-sizing: border-box; padding: 0 14px 10px; gap: 8px; font-size: 15px; border: 1px solid var(--rinch-color-default-border);",
            div { style: "display: flex; align-items: center; margin: 0 -14px; padding: 2px 4px 2px 14px; border-bottom: 1px solid var(--rinch-color-default-border); font-size: 12px; font-weight: 600;",
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

/// Each part with a key that changes when it does, so a changed part is
/// drawn again and the others are left alone.
fn keyed(parts: Vec<(String, bool)>) -> Vec<(String, String, bool)> {
    parts.into_iter().enumerate().map(|(i, (text, clean))| (format!("{i}:{clean}:{text}"), text, clean)).collect()
}
