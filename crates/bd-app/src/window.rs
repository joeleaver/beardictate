//! The settings window: a sidebar of sections, each a page of settings.
//! Every change is saved at once and sent to the engine; nothing waits for
//! an OK button. Opened from the tray ("Settings…").

use std::cell::RefCell;
use std::rc::Rc;

use rinch::core::{KeyEventData, set_keyboard_interceptor};
use rinch::hotkey::{CaptureStep, HotkeyCapture};
use rinch::prelude::*;

use crate::settings::TrayTheme;
use crate::ui::{self, Section, Shared, change, check_hotkey};

mod cleanup;
mod mic_page;
mod modes;
mod news;
mod old_models;
mod sentence_wait;
mod setup;
mod spelling;
mod updates;
mod words;
mod writing;
use cleanup::cleanup_page;
use mic_page::microphone;
use modes::modes;
use old_models::old_models;
use setup::setup_card;
use updates::updates;
use words::words;
use writing::writing;

/// The window's content.
pub fn view(__scope: &mut RenderScope, s: &Shared) -> NodeHandle {
    let [n1, n2, n3, n4, n5, n6] = [
        (Section::General, "General"),
        (Section::Microphone, "Microphone"),
        (Section::Cleanup, "Post-processing"),
        (Section::Writing, "Writing"),
        (Section::Words, "Special words"),
        (Section::About, "About"),
    ]
    .map(|(section, label)| nav(__scope, s, section, label));
    let section = s.section;
    let (listening, label, button) = (s.clone(), s.clone(), s.clone());
    let (general, mic, cleanup, writing, words, about) =
        (general(__scope, s), microphone(__scope, s), cleanup_page(__scope, s), writing(__scope, s), words(__scope, s), about(__scope, s));
    let setup = setup_card(__scope, s);
    let news = news::news(__scope, s);
    rsx! {
        div { style: "display: flex; height: 100%; font-size: 14px;",
            div { style: "width: 190px; flex: none; padding: 16px 8px; display: flex; flex-direction: column; gap: 2px; border-right: 1px solid var(--rinch-color-default-border);",
                div { style: "padding: 4px 8px 14px; display: flex; align-items: center; gap: 8px;",
                    img { src: MARK_PNG, style: "width: 30px; height: 30px; flex: none;" }
                    div { style: "font-size: 20px; font-weight: 800; letter-spacing: -0.02em;",
                        "bear" span { style: "color: #2459B8;", "dictate" }
                    }
                }
                {n1} {n2} {n3} {n4} {n5} {n6}
                div { style: "flex: 1;" }
                Button {
                    variant: {move || if ui::is_listening(&listening) { "filled" } else { "light" }},
                    onclick: {let b = button.clone(); move || {
                        if ui::main_button(&b).1 {
                            change(&b, |st| st.microphone_on = true);
                        } else {
                            crate::engine_send(crate::engine::Command::Toggle);
                        }
                    }},
                    {move || ui::main_button(&label).0}
                }
            }
            div { style: "flex: 1; min-width: 0; padding: 24px 28px; overflow-y: auto;",
                {setup}
                div { style: {move || shown(section.get() == Section::General)}, {general} }
                div { style: {move || shown(section.get() == Section::Microphone)}, {mic} }
                div { style: {move || shown(section.get() == Section::Cleanup)}, {cleanup} }
                div { style: {move || shown(section.get() == Section::Writing)}, {writing} }
                div { style: {move || shown(section.get() == Section::Words)}, {words} }
                div { style: {move || shown(section.get() == Section::About)}, {about} }
            }
            {news}
        }
    }
}

/// The mark (`design/logo/mark-96.png`, from `mark.svg`) as a data URI. A
/// raster, because rinch's SVG painter lacks the rounded rect and ellipse
/// the mark uses (rinch#1538).
const MARK_PNG: &str = concat!("data:image/png;base64,", include_str!("mark-96.png.b64"));

fn nav(__scope: &mut RenderScope, s: &Shared, section: Section, label: &'static str) -> NodeHandle {
    let (a, b) = (s.clone(), s.clone());
    rsx! {
        NavLink {
            label: label,
            active_fn: move || a.section.get() == section,
            onclick: move || {
                b.section.set(section);
                section.save(&b.paths);
            },
        }
    }
}

fn shown(on: bool) -> &'static str {
    if on { "display: flex; flex-direction: column; gap: 18px;" } else { "display: none;" }
}

fn general(__scope: &mut RenderScope, s: &Shared) -> NodeHandle {
    let capturing = Signal::new(false);
    let note = Signal::new(String::new());
    // The capture's keyboard hook lives as long as the page: installed from
    // the "Change…" click, it went with the click's scope as soon as the
    // handler returned, and no key (not even Escape) ever reached it.
    let capture = Rc::new(RefCell::new(HotkeyCapture::new()));
    // Modifiers seen held, by kind ("Control", …), during a capture.
    let held: Rc<RefCell<Vec<&'static str>>> = Rc::default();
    {
        let (s, capture, held) = (s.clone(), capture.clone(), held.clone());
        set_keyboard_interceptor(move |k: &KeyEventData| {
            if !capturing.get() {
                return false;
            }
            let step = feed_capture(&mut capture.borrow_mut(), &held, k);
            match step {
                CaptureStep::Pending(held) => note.set(format!("Press the new shortcut: {held}")),
                CaptureStep::Cancelled => {
                    capturing.set(false);
                    note.set(String::new());
                }
                CaptureStep::Done(sc) => {
                    capturing.set(false);
                    let shortcut = sc.to_string();
                    match check_hotkey(&shortcut) {
                        Ok(()) => {
                            note.set(String::new());
                            change(&s, |st| st.hotkey = shortcut);
                        }
                        Err(why) => note.set(format!("{shortcut}: {why}")),
                    }
                }
            }
            true
        });
    }
    let start_capture = Rc::new(move || {
        *capture.borrow_mut() = HotkeyCapture::new();
        held.borrow_mut().clear();
        capturing.set(true);
        note.set("Press the new shortcut (at least two keys). Escape cancels.".into());
    });
    let s1 = s.clone();
    let s2 = s.clone();
    let s3 = s.clone();
    let s4 = s.clone();
    let s5 = s.clone();
    let (s6, s7, s8, s9, s10) = (s.clone(), s.clone(), s.clone(), s.clone(), s.clone());
    let (s11, s12, s13, s14, s15) = (s.clone(), s.clone(), s.clone(), s.clone(), s.clone());
    let modes = modes(__scope, s);
    let old_models = old_models(__scope, s);
    let paths = s.paths.clone();
    let login = Signal::new(paths.starts_at_login());
    rsx! {
        div { style: "display: flex; flex-direction: column; gap: 18px;",
            Title { order: 3, "General" }
            div { style: "display: flex; flex-direction: column; gap: 6px;",
                Text { style: "font-weight: 600;", "Dictation shortcut" }
                Text { size: "sm", color: "dimmed",
                    {move || if s6.settings.with(|st| st.voice_wake) {
                        "Press to wake up and start dictating, press again to go to sleep. Clicking the bear on screen or the tray icon does the same."
                    } else {
                        "Press to start dictating, press again to stop. Clicking the bear on screen or the tray icon does the same."
                    }}
                }
                Group {
                    Kbd { {move || s1.hotkey_text.get()} }
                    Button { variant: "light", size: "xs", disabled: {move || capturing.get()}, onclick: { let f = start_capture.clone(); move || f() }, "Change…" }
                }
                Text { size: "sm", color: "dimmed", {move || note.get()} }
                Text { size: "sm", color: "dimmed", {move || s13.hotkey_note.get()} }
            }
            Switch {
                label: "Always listening",
                description: "The microphone stays on. Say \u{201C}bear, wake up\u{201D} (or press the shortcut) to start dictating, and \u{201C}bear, go to sleep\u{201D} to stop. Turn the microphone off and on from the bear\u{2019}s menu. Off, the shortcut turns the microphone on and off.",
                checked_fn: move || s7.settings.with(|st| st.voice_wake),
                onchange: move || change(&s8, |st| st.voice_wake = !st.voice_wake),
            }
            div { style: "display: flex; flex-direction: column; gap: 6px;",
                Text { style: "font-weight: 600;", "Stop listening when nothing\u{2019}s said to it" }
                Text { size: "sm", color: "dimmed", "After this long without your voice, it goes back to sleep (or a dictation stops). Other voices in the room don\u{2019}t count." }
                Select {
                    data: [5, 10, 20, 30, 60, 0].map(|m| SelectOption::new(m.to_string(), sleep_label(m))).to_vec(),
                    value_fn: move || s14.settings.with(|st| st.sleep_after_mins).to_string(),
                    onchange: move |v: String| change(&s15, |st| st.sleep_after_mins = v.parse().unwrap_or(10)),
                }
            }
            {modes}
            {old_models}
            div { style: "display: flex; flex-direction: column; gap: 8px;",
                Text { style: "font-weight: 600;", "The bear" }
                Switch {
                    label: "Bear on screen",
                    description: "A small bear that floats over your other windows and glows to show what beardictate is doing: red while it listens, green while it finishes, amber when it needs you. Click it to start or stop dictating, drag it anywhere, right-click it for its menu (the same as the tray icon\u{2019}s).",
                    checked_fn: move || s11.settings.with(|st| st.show_bear),
                    onchange: move || change(&s12, |st| st.show_bear = !st.show_bear),
                }
            }
            div { style: "display: flex; flex-direction: column; gap: 6px;",
                Text { style: "font-weight: 600;", "Tray icon colours" }
                Select {
                    data: vec![
                        SelectOption::new("auto", "Match my desktop"),
                        SelectOption::new("light", "For a light taskbar"),
                        SelectOption::new("dark", "For a dark taskbar"),
                    ],
                    value_fn: move || theme_key(s4.settings.with(|st| st.tray_theme)).to_string(),
                    onchange: move |v: String| change(&s5, |st| st.tray_theme = theme_from(&v)),
                }
            }
            Text { style: "font-weight: 600;", "Starting and updating" }
            Switch {
                label: "Start beardictate when I sign in",
                checked_fn: move || login.get(),
                onchange: move || {
                    let on = !login.get();
                    match paths.set_start_at_login(on) {
                        Ok(()) => login.set(on),
                        Err(e) => tracing::warn!("start at login: {e:#}"),
                    }
                },
            }
            Switch {
                label: "Keep beardictate up to date",
                description: "Every few hours, look for a new version on beardictate.jkbase.app and download it. It installs the next time beardictate starts, or when you choose Restart to update from the bear\u{2019}s menu.",
                checked_fn: move || s9.settings.with(|st| st.auto_update),
                onchange: move || change(&s10, |st| st.auto_update = !st.auto_update),
            }
            Switch {
                label: "Notify me when something needs attention",
                description: "A microphone problem, text left on the clipboard, a model that won't load.",
                checked_fn: move || s2.settings.with(|st| st.notifications),
                onchange: move || change(&s3, |st| st.notifications = !st.notifications),
            }
        }
    }
}

/// Whether a key event says a modifier is held.
type Held = fn(&KeyEventData) -> bool;

/// Modifier kinds, the code of the left one, and whether an event says it's held.
const MODIFIERS: [(&str, &str, Held); 4] = [
    ("Control", "ControlLeft", |k| k.ctrl),
    ("Shift", "ShiftLeft", |k| k.shift),
    ("Alt", "AltLeft", |k| k.alt),
    ("Super", "SuperLeft", |k| k.meta),
];

/// Feeds a key to the shortcut capture. A key whose event says a modifier
/// is held that the capture never saw go down (a platform or input that
/// reports modifiers only as flags) gets that modifier's press first.
fn feed_capture(capture: &mut HotkeyCapture, held: &RefCell<Vec<&'static str>>, k: &KeyEventData) -> CaptureStep {
    let down = k.kind == rinch::core::KeyEventKind::Down;
    let kind = MODIFIERS.iter().find(|(name, _, _)| k.code.starts_with(name) || (*name == "Super" && k.code.starts_with("Meta")));
    if let Some((name, _, _)) = kind {
        let mut h = held.borrow_mut();
        h.retain(|n| n != name);
        if down {
            h.push(name);
        }
    } else if down {
        for (name, code, on) in MODIFIERS {
            if on(k) && !held.borrow().contains(&name) {
                held.borrow_mut().push(name);
                let _ = capture.feed(&KeyEventData::new(name, code));
            }
        }
    }
    let step = capture.feed(k);
    if !matches!(step, CaptureStep::Pending(_)) {
        held.borrow_mut().clear();
    }
    step
}

fn sleep_label(minutes: u32) -> String {
    match minutes {
        0 => "Never".into(),
        60 => "After an hour".into(),
        m => format!("After {m} minutes"),
    }
}

fn theme_key(t: TrayTheme) -> &'static str {
    match t {
        TrayTheme::Auto => "auto",
        TrayTheme::Light => "light",
        TrayTheme::Dark => "dark",
    }
}

fn theme_from(v: &str) -> TrayTheme {
    match v {
        "light" => TrayTheme::Light,
        "dark" => TrayTheme::Dark,
        _ => TrayTheme::Auto,
    }
}

fn about(__scope: &mut RenderScope, s: &Shared) -> NodeHandle {
    let s1 = s.clone();
    let lines = [
        format!("Settings: {}", s.paths.settings.display()),
        format!("Log: {}", s.paths.log.display()),
    ];
    let log = s.paths.log.clone();
    let updates = updates(__scope, s);
    let notes = s.news;
    rsx! {
        div { style: "display: flex; flex-direction: column; gap: 14px;",
            Title { order: 3, "About beardictate" }
            Group {
                Text { {format!("Version {}", env!("CARGO_PKG_VERSION"))} }
                if crate::whats_new::current().is_some() {
                    Button { variant: "subtle", size: "xs", onclick: move || news::show_current(notes), "What\u{2019}s new" }
                }
            }
            {updates}
            if cfg!(feature = "alpha") {
                Text { size: "sm",
                    "This is an alpha: what you dictate (the recordings and the text), with timings and your computer's details, is sent to the developer to fix problems and train better post-processing. Nothing said while beardictate is asleep is sent."
                }
                Group {
                    Button { variant: "light", size: "xs", onclick: move || ui::open_url("https://beardictate.jkbase.app/download.html#alpha"), "What it sends" }
                }
            }
            Text { {move || ui::status_line(&s1)} }
            for line in lines.clone() {
                Text { size: "sm", color: "dimmed", {line} }
            }
            Group {
                Button { variant: "light", onclick: move || ui::open_path(&log), "Open the log" }
            }
        }
    }
}
