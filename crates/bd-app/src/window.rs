//! The settings window: a sidebar of sections, each a page of settings.
//! Every change is saved at once and sent to the engine; nothing waits for
//! an OK button. Opened from the tray ("Settings…").

use std::cell::RefCell;
use std::rc::Rc;

use rinch::core::{KeyEventData, clear_keyboard_interceptor, set_keyboard_interceptor};
use rinch::hotkey::{CaptureStep, HotkeyCapture};
use rinch::prelude::*;

use crate::settings::{Punctuation, QuoteMarks, Settings, TrayTheme};
use crate::ui::{self, Section, Shared, change, check_hotkey};

mod words;
use words::words;

/// The window's content.
pub fn view(__scope: &mut RenderScope, s: &Shared) -> NodeHandle {
    let [n1, n2, n3, n4, n5, n6] = [
        (Section::General, "General"),
        (Section::Microphone, "Microphone"),
        (Section::Cleanup, "Cleanup"),
        (Section::Writing, "Writing"),
        (Section::Words, "Special words"),
        (Section::About, "About"),
    ]
    .map(|(section, label)| nav(__scope, s, section, label));
    let section = s.section;
    let (listening, listening2) = (s.clone(), s.clone());
    let (general, mic, cleanup, writing, words, about) =
        (general(__scope, s), microphone(__scope, s), cleanup_page(__scope, s), writing(__scope, s), words(__scope, s), about(__scope, s));
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
                    onclick: || crate::engine_send(crate::engine::Command::Toggle),
                    {move || if ui::is_listening(&listening2) { "Stop dictating" } else { "Start dictating" }}
                }
            }
            div { style: "flex: 1; min-width: 0; padding: 24px 28px; overflow-y: auto;",
                div { style: {move || shown(section.get() == Section::General)}, {general} }
                div { style: {move || shown(section.get() == Section::Microphone)}, {mic} }
                div { style: {move || shown(section.get() == Section::Cleanup)}, {cleanup} }
                div { style: {move || shown(section.get() == Section::Writing)}, {writing} }
                div { style: {move || shown(section.get() == Section::Words)}, {words} }
                div { style: {move || shown(section.get() == Section::About)}, {about} }
            }
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
            onclick: move || b.section.set(section),
        }
    }
}

fn shown(on: bool) -> &'static str {
    if on { "display: flex; flex-direction: column; gap: 18px;" } else { "display: none;" }
}

fn general(__scope: &mut RenderScope, s: &Shared) -> NodeHandle {
    let capturing = Signal::new(false);
    let note = Signal::new(String::new());
    let start_capture = Rc::new({
        let s = s.clone();
        move || {
            capturing.set(true);
            note.set("Press the new shortcut (at least two keys). Escape cancels.".into());
            let capture = RefCell::new(HotkeyCapture::new());
            let s = s.clone();
            set_keyboard_interceptor(move |k: &KeyEventData| {
                match capture.borrow_mut().feed(k) {
                    CaptureStep::Pending(held) => note.set(format!("Press the new shortcut: {held}")),
                    CaptureStep::Cancelled => {
                        capturing.set(false);
                        note.set(String::new());
                        clear_keyboard_interceptor();
                    }
                    CaptureStep::Done(sc) => {
                        capturing.set(false);
                        clear_keyboard_interceptor();
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
    });
    let s1 = s.clone();
    let s2 = s.clone();
    let s3 = s.clone();
    let s4 = s.clone();
    let s5 = s.clone();
    let paths = s.paths.clone();
    let login = Signal::new(paths.starts_at_login());
    rsx! {
        div { style: "display: flex; flex-direction: column; gap: 18px;",
            Title { order: 3, "General" }
            div { style: "display: flex; flex-direction: column; gap: 6px;",
                Text { style: "font-weight: 600;", "Dictation shortcut" }
                Text { size: "sm", color: "dimmed", "Press to start dictating, press again to stop. You can also click the bear." }
                Group {
                    Kbd { {move || s1.hotkey_text.get()} }
                    Button { variant: "light", size: "xs", disabled: {move || capturing.get()}, onclick: { let f = start_capture.clone(); move || f() }, "Change…" }
                }
                Text { size: "sm", color: "dimmed", {move || note.get()} }
            }
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
                label: "Notify me when something needs attention",
                description: "A microphone problem, text left on the clipboard, a model that won't load.",
                checked_fn: move || s2.settings.with(|st| st.notifications),
                onchange: move || change(&s3, |st| st.notifications = !st.notifications),
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
        }
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

/// The Select value standing for "no particular microphone".
const DEFAULT_MIC: &str = "(default)";

fn microphone(__scope: &mut RenderScope, s: &Shared) -> NodeHandle {
    let mut data = vec![SelectOption::new(DEFAULT_MIC, "System default")];
    match bd_audio::list_input_devices() {
        Ok(devices) => data.extend(devices.into_iter().map(|d| SelectOption::new(d.name.clone(), d.name))),
        Err(e) => tracing::warn!("listing microphones: {e}"),
    }
    let (s1, s2, s3, s4) = (s.clone(), s.clone(), s.clone(), s.clone());
    rsx! {
        div { style: "display: flex; flex-direction: column; gap: 18px;",
            Title { order: 3, "Microphone" }
            Select {
                label: "Microphone",
                description: "Takes effect at your next dictation.",
                data: data,
                value_fn: move || s1.settings.with(|st| if st.microphone.is_empty() { DEFAULT_MIC.to_string() } else { st.microphone.clone() }),
                onchange: move |v: String| change(&s2, |st| st.microphone = if v == DEFAULT_MIC { String::new() } else { v }),
            }
            Switch {
                label: "Keep the microphone open between dictations",
                description: "Your first word is never clipped, but the desktop shows the microphone as in use all the time.",
                checked_fn: move || s3.settings.with(|st| st.keep_mic_open),
                onchange: move || change(&s4, |st| st.keep_mic_open = !st.keep_mic_open),
            }
        }
    }
}

fn cleanup_page(__scope: &mut RenderScope, s: &Shared) -> NodeHandle {
    let mut options: Vec<(String, String, &'static str)> = vec![
        ("off".into(), "Off".into(), "Your words as heard, with spoken commands. Fastest."),
        ("auto".into(), "Automatic".into(), "The largest model installed."),
    ];
    for (id, name) in s.installed.iter() {
        options.push((id.clone(), name.clone(), "Installed on this computer."));
    }
    let custom = Signal::new(s.settings.with(|st| {
        if st.cleanup.ends_with(".gguf") { st.cleanup.clone() } else { String::new() }
    }));
    let (s1, s2, s3) = (s.clone(), s.clone(), s.clone());
    rsx! {
        div { style: "display: flex; flex-direction: column; gap: 18px;",
            Title { order: 3, "Cleanup" }
            Text { size: "sm", color: "dimmed",
                "After each sentence is transcribed, a small model on your computer drops the ums, fixes punctuation and quotes your dialog."
            }
            Stack { gap: "sm",
                for (value, label, description) in options.clone() {
                    Radio {
                        name: "cleanup",
                        label: label,
                        description: description,
                        checked_fn: {
                            let s = s1.clone();
                            let value = value.clone();
                            move || s.settings.with(|st| st.cleanup == value)
                        },
                        onchange: {
                            let s = s2.clone();
                            let value = value.clone();
                            move || change(&s, |st| st.cleanup = value.clone())
                        },
                    }
                }
            }
            TextInput {
                label: "Or a model file of your own (.gguf)",
                placeholder: "/path/to/model.gguf",
                value_fn: move || custom.get(),
                oninput: move |v: String| custom.set(v),
                onsubmit: {
                    let s = s.clone();
                    move || {
                        let path = custom.get();
                        if !path.trim().is_empty() {
                            change(&s, |st| st.cleanup = path.trim().to_string());
                        }
                    }
                },
            }
            Text { size: "sm", color: "dimmed", {move || ui::status_line(&s3)} }
        }
    }
}

fn writing(__scope: &mut RenderScope, s: &Shared) -> NodeHandle {
    let m1 = marks(__scope, s, "“Double”, with ‘single’ inside: US, and many UK publishers", QuoteMarks::Double);
    let m2 = marks(__scope, s, "‘Single’, with “double” inside: traditional British books", QuoteMarks::Single);
    let c1 = curly(__scope, s, "Curly: “like this” and ‘like this’", true);
    let c2 = curly(__scope, s, "Straight: \"like this\" and 'like this'", false);
    let p1 = punct(__scope, s, "Always inside: “To the river,” he said.", Punctuation::Inside);
    let p2 = punct(__scope, s, "Inside only when they belong to the speech: she called it ‘the end’.", Punctuation::Logical);
    let t1 = toggle(__scope, s, "When I say “new paragraph”", "And a line break for “new line”. Off, those words are typed.", |st| st.paragraph_on_command, |st| st.paragraph_on_command = !st.paragraph_on_command);
    let t2 = toggle(__scope, s, "When a different character speaks", "The cleanup model follows the dialog.", |st| st.paragraph_on_speaker, |st| st.paragraph_on_speaker = !st.paragraph_on_speaker);
    let t3 = toggle(__scope, s, "After a long pause", "", |st| st.paragraph_on_pause, |st| st.paragraph_on_pause = !st.paragraph_on_pause);
    let t4 = toggle(__scope, s, "When the prose calls for one", "A new topic, place or time, as a novelist would break it. The cleanup model decides.", |st| st.paragraph_on_topic, |st| st.paragraph_on_topic = !st.paragraph_on_topic);
    let (s1, s2, s3) = (s.clone(), s.clone(), s.clone());
    rsx! {
        div { style: "display: flex; flex-direction: column; gap: 18px;",
            Title { order: 3, "Writing" }
            div { style: "display: flex; flex-direction: column; gap: 8px;",
                Text { style: "font-weight: 600;", "Quotation marks" }
                {m1} {m2}
            }
            div { style: "display: flex; flex-direction: column; gap: 8px;",
                Text { style: "font-weight: 600;", "Quote characters" }
                {c1} {c2}
            }
            div { style: "display: flex; flex-direction: column; gap: 8px;",
                Text { style: "font-weight: 600;", "Commas and full stops at the end of a quote" }
                {p1} {p2}
            }
            div { style: "display: flex; flex-direction: column; gap: 8px;",
                Text { style: "font-weight: 600;", "Start a new paragraph" }
                {t1} {t2} {t3}
                div { style: {move || if s1.settings.with(|st| st.paragraph_on_pause) { "padding-left: 28px; max-width: 260px;" } else { "display: none;" }},
                    Select {
                        data: vec![
                            SelectOption::new("2", "2 seconds"),
                            SelectOption::new("3", "3 seconds"),
                            SelectOption::new("4", "4 seconds"),
                            SelectOption::new("6", "6 seconds"),
                        ],
                        value_fn: move || format!("{}", s2.settings.with(|st| st.pause_paragraph_secs.round() as u32)),
                        onchange: move |v: String| {
                            if let Ok(secs) = v.parse::<f32>() {
                                change(&s3, |st| st.pause_paragraph_secs = secs);
                            }
                        },
                    }
                }
                {t4}
            }
        }
    }
}

fn toggle(
    __scope: &mut RenderScope,
    s: &Shared,
    label: &'static str,
    description: &'static str,
    get: fn(&Settings) -> bool,
    flip: fn(&mut Settings),
) -> NodeHandle {
    let (a, b) = (s.clone(), s.clone());
    rsx! {
        Checkbox {
            label: label,
            description: description,
            checked_fn: move || a.settings.with(get),
            onchange: move || change(&b, flip),
        }
    }
}

fn marks(__scope: &mut RenderScope, s: &Shared, label: &'static str, value: QuoteMarks) -> NodeHandle {
    let (a, b) = (s.clone(), s.clone());
    rsx! {
        Radio {
            name: "quote-marks",
            label: label,
            checked_fn: move || a.settings.with(|st| st.quote_marks == value),
            onchange: move || change(&b, |st| st.quote_marks = value),
        }
    }
}

fn curly(__scope: &mut RenderScope, s: &Shared, label: &'static str, value: bool) -> NodeHandle {
    let (a, b) = (s.clone(), s.clone());
    rsx! {
        Radio {
            name: "curly",
            label: label,
            checked_fn: move || a.settings.with(|st| st.curly_quotes == value),
            onchange: move || change(&b, |st| st.curly_quotes = value),
        }
    }
}

fn punct(__scope: &mut RenderScope, s: &Shared, label: &'static str, value: Punctuation) -> NodeHandle {
    let (a, b) = (s.clone(), s.clone());
    rsx! {
        Radio {
            name: "punctuation",
            label: label,
            checked_fn: move || a.settings.with(|st| st.punctuation == value),
            onchange: move || change(&b, |st| st.punctuation = value),
        }
    }
}

fn about(__scope: &mut RenderScope, s: &Shared) -> NodeHandle {
    let s1 = s.clone();
    let lines = [
        format!("Settings: {}", s.paths.settings.display()),
        format!("Log: {}", s.paths.log.display()),
    ];
    let log = s.paths.log.clone();
    rsx! {
        div { style: "display: flex; flex-direction: column; gap: 14px;",
            Title { order: 3, "About beardictate" }
            Text { {format!("Version {}", env!("CARGO_PKG_VERSION"))} }
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
