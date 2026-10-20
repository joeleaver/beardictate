//! The bear on screen, as the pad window ([`crate::pad_window`]) draws it:
//! on its own (a small round bear), in the Bear Pad's title bar, and its
//! menu.
//!
//! The bear glows in the tray dot's colours for what the app is doing,
//! pulsing while it listens, and its snout turns cobalt at work. A small
//! amber "!" at its upper left says the microphone is too quiet (its menu
//! says so too), and a list mark at its lower right that list mode is on. A click
//! starts or stops dictating, a drag moves it, a right-click opens the
//! tray's menu ([`crate::menu`]).

use rinch::prelude::*;

use crate::menu::{Act, Entry};

/// The window's side while it shows only the bear.
pub const SIDE: f64 = 84.0;
pub const MENU_WIDTH: f64 = 260.0;
const ROW: f64 = 30.0;
const RULE: f64 = 9.0;

/// The menu's height for `entries`, padding included.
pub fn menu_height(entries: &[Entry]) -> f64 {
    entries.iter().map(|e| if matches!(e, Entry::Separator) { RULE } else { ROW }).sum::<f64>() + 12.0
}

/// The glow's colour for a state, if it has one (the tray's dot colours).
fn glow(state: &str) -> Option<&'static str> {
    Some(match state.split(' ').next().unwrap_or("") {
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
    match state.split(' ').next().unwrap_or("") {
        "listening" | "cleaning" => "#2F6FDB",
        "mic-off" | "loading" => "#B9AFA3",
        _ => "#E9E4DC",
    }
}

const INK: &str = "#0E1A30";

/// Whether the state (`"listening list quiet"`) carries `mark`.
fn has(state: &str, mark: &str) -> bool {
    state.split(' ').skip(1).any(|m| m == mark)
}

pub const CSS: &str = "
@keyframes bear-pulse {
    0% { opacity: 0.45; }
    100% { opacity: 1; }
}
.bear-glow { position: absolute; border-radius: 50%; }
.bear-glow.pulse { animation: bear-pulse 0.9s ease-in-out infinite alternate; }
.bear-row { display: flex; align-items: center; gap: 8px; height: 30px; padding: 0 10px; border-radius: 6px; font-size: 13px; white-space: nowrap; overflow: hidden; }
.bear-row.live:hover { background: rgba(47, 111, 219, 0.14); }
.bear-row.off { color: #8A93A3; }
";

/// One part of the glyph, an ellipse in the tray glyph's 32-unit grid,
/// drawn `px` wide with its top left at (`x`, `y`).
fn part(x: f64, y: f64, px: f64, (cx, cy, rx, ry): (f64, f64, f64, f64), colour: &str) -> String {
    let k = px / 32.0;
    format!(
        "position: absolute; left: {:.2}px; top: {:.2}px; width: {:.2}px; height: {:.2}px; border-radius: 50%; background: {colour};",
        x + (cx - rx) * k,
        y + (cy - ry) * k,
        2.0 * rx * k,
        2.0 * ry * k
    )
}

/// The bear on a light disc `disc` px wide at (`at`, `at`), glowing.
pub fn glyph(__scope: &mut RenderScope, state: Signal<String>, at: f64, disc: f64, blur: f64) -> NodeHandle {
    let disc_style = format!(
        "position: absolute; left: {at}px; top: {at}px; width: {disc}px; height: {disc}px; border-radius: 50%; background: #F4F7FD; border: 1px solid rgba(14, 26, 48, 0.15); box-sizing: border-box;"
    );

    let mark = (disc * 0.36).max(9.0);
    let list_style = format!(
        "position: absolute; left: {:.1}px; top: {:.1}px; width: {mark:.1}px; height: {mark:.1}px; border-radius: 50%; background: #2F6FDB; color: white; font: bold {:.1}px/{mark:.1}px sans-serif; text-align: center;",
        at + disc - mark * 0.8,
        at + disc - mark * 0.8,
        mark * 0.8
    );
    let quiet_style = format!(
        "position: absolute; left: {:.1}px; top: {:.1}px; width: {mark:.1}px; height: {mark:.1}px; border-radius: 50%; background: #E0A800; color: #0E1A30; font: bold {:.1}px/{mark:.1}px sans-serif; text-align: center;",
        at - mark * 0.2,
        at - mark * 0.2,
        mark * 0.8
    );
    // The bear spans 44/60 of the disc, centred.
    let (px, o) = (disc * 44.0 / 60.0, at + disc * 8.0 / 60.0);
    rsx! {
        div { style: "position: absolute; inset: 0;",
            div {
                class: {move || if state.get().starts_with("listening") { "bear-glow pulse" } else { "bear-glow" }},
                style: {move || match glow(&state.get()) {
                    Some(c) => format!("left: {at}px; top: {at}px; width: {disc}px; height: {disc}px; box-shadow: 0 0 {blur}px {:.0}px {c}; background: {c};", blur / 2.4),
                    None => format!("left: {at}px; top: {at}px; width: {disc}px; height: {disc}px; box-shadow: 0 1px {:.0}px 1px rgba(14, 26, 48, 0.35);", blur / 2.0),
                }},
            }
            div { style: {disc_style.clone()} }
            div { style: {part(o, o, px, (10.0, 9.0, 5.0, 5.0), INK)} }
            div { style: {part(o, o, px, (22.0, 9.0, 5.0, 5.0), INK)} }
            div { style: {part(o, o, px, (16.0, 18.0, 12.0, 12.0), INK)} }
            div { style: {move || part(o, o, px, (16.0, 22.0, 6.2, 4.8), snout(&state.get()))} }
            div { style: {part(o, o, px, (16.0, 19.4, 2.7, 1.8), INK)} }
            // List mode: a small list mark at the disc's lower right.
            div { style: {move || if has(&state.get(), "list") { list_style.clone() } else { "display: none;".to_string() }},
                "\u{2261}"
            }
            // The microphone is quiet: an amber mark at the disc's upper left.
            div { style: {move || if has(&state.get(), "quiet") { quiet_style.clone() } else { "display: none;".to_string() }},
                "!"
            }
        }
    }
}

/// The menu: `pick` gets the chosen act, `enter` and `leave` the pointer
/// coming over the menu or one of its rows, and leaving one (rinch says
/// "leave" moving from the menu onto its own row, too).
pub fn menu(
    __scope: &mut RenderScope,
    entries: Signal<Vec<Entry>>,
    pick: impl Fn(Act) + Copy + 'static,
    enter: impl Fn() + Copy + 'static,
    leave: impl Fn() + Copy + 'static,
) -> NodeHandle {
    rsx! {
        div {
            style: "position: absolute; inset: 0; padding: 6px; box-sizing: border-box; overflow-y: auto; background: #FBFCFE; color: #0E1A30; border: 1px solid rgba(14, 26, 48, 0.18); border-radius: 10px; font-family: sans-serif;",
            onmouseenter: enter,
            onmouseleave: leave,
            for (i, entry) in entries.get().into_iter().enumerate() {
                div { key: i, onmouseenter: enter, onmouseleave: leave,
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
    }
}
