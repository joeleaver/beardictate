//! The bear's menu, one list for both places it opens: the tray icon's
//! right-click and the on-screen bear's ([`crate::bear`]). Each entry says
//! what it does as an [`Act`], which the bear's window sends back by value.

use rinch::menu::{Menu, MenuItem};
use rinch::prelude::*;
use serde::{Deserialize, Serialize};

use crate::engine::{self, State};
use crate::settings::Destination;
use crate::engine_send;
use crate::ui::{Section, Shared, change};

/// What a menu entry does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "act", rename_all = "kebab-case")]
pub enum Act {
    /// Start or stop dictating (or wake up and go to sleep).
    Toggle,
    Microphone { on: bool },
    Destination { to: Destination },
    Tidy { on: bool },
    Dialog { on: bool },
    /// The bear on screen.
    Bear { on: bool },
    /// List mode: items on lines of their own, bare.
    List { on: bool },
    RestartToUpdate,
    Settings,
    /// Settings, at the Microphone page.
    MicrophoneSettings,
    Quit,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum Entry {
    /// Greyed out when `act` is `None`.
    Item { label: String, act: Option<Act> },
    Check { label: String, on: bool, act: Act },
    /// One of a run of radio entries.
    Radio { label: String, on: bool, act: Act },
    Separator,
}

fn item(label: impl Into<String>, act: Option<Act>) -> Entry {
    Entry::Item { label: label.into(), act }
}

/// The menu for the current state and settings.
pub fn entries(shared: &Shared) -> Vec<Entry> {
    let status = shared.status.get();
    let hotkey = shared.hotkey_text.get();
    let settings = shared.settings.get();
    let mut out = vec![match status.state {
        State::Loading => item(status.detail.clone(), None),
        State::Listening if status.voice => item(format!("Go to sleep   ({hotkey})"), Some(Act::Toggle)),
        State::Asleep => item(format!("Wake up   ({hotkey})"), Some(Act::Toggle)),
        State::MicOff => item("Microphone off", None),
        State::Listening => item(format!("Stop dictating   ({hotkey})"), Some(Act::Toggle)),
        State::NeedsYou if status.detail.starts_with("Can't type") => item("Start dictating", None),
        State::NeedsYou => item("Try again", Some(Act::Toggle)),
        _ => item(format!("Start dictating   ({hotkey})"), Some(Act::Toggle)),
    }];
    if status.state == State::NeedsYou {
        out.push(item(capitalized(&shorten(&status.detail, 70)), None));
    }
    if status.warning.is_some() {
        out.push(item("\u{26A0} Your microphone is quiet\u{2026}", Some(Act::MicrophoneSettings)));
    }
    if status.voice && status.state != State::Loading {
        let on = status.state != State::MicOff;
        let label = if on { "Turn microphone off" } else { "Turn microphone on" };
        out.push(item(label, Some(Act::Microphone { on: !on })));
    }
    out.push(Entry::Separator);
    for d in Destination::ALL {
        out.push(Entry::Radio { label: d.label().into(), on: settings.destination == d, act: Act::Destination { to: d } });
    }
    out.push(Entry::Check { label: "Tidy up".into(), on: settings.tidy, act: Act::Tidy { on: !settings.tidy } });
    out.push(Entry::Check { label: "Dialog".into(), on: settings.dialog, act: Act::Dialog { on: !settings.dialog } });
    out.push(Entry::Check { label: "List mode".into(), on: settings.list_mode, act: Act::List { on: !settings.list_mode } });
    out.push(Entry::Check { label: "Bear on screen".into(), on: settings.show_bear, act: Act::Bear { on: !settings.show_bear } });
    if let crate::update::Phase::Ready { version } = shared.updates.phase.get() {
        out.push(Entry::Separator);
        out.push(item(format!("Restart to update to {version}"), Some(Act::RestartToUpdate)));
    }
    out.extend([Entry::Separator, item("Settings…", Some(Act::Settings)), Entry::Separator, item("Quit beardictate", Some(Act::Quit))]);
    out
}

/// Does what an entry says. On the UI thread.
pub fn perform(shared: &Shared, act: Act) {
    match act {
        Act::Toggle => engine_send(engine::Command::Toggle),
        Act::Microphone { on } => change(shared, |st| st.microphone_on = on),
        Act::Destination { to } => change(shared, |st| st.destination = to),
        Act::Tidy { on } => change(shared, |st| st.tidy = on),
        Act::Dialog { on } => change(shared, |st| st.dialog = on),
        Act::Bear { on } => change(shared, |st| st.show_bear = on),
        Act::List { on } => change(shared, |st| st.list_mode = on),
        Act::RestartToUpdate => crate::update::restart_to_update(),
        // On the section it was last left on.
        Act::Settings => show_current_window(),
        Act::MicrophoneSettings => {
            shared.section.set(Section::Microphone);
            show_current_window();
        }
        Act::Quit => close_current_window(),
    }
}

/// The entries as the tray's menu.
pub fn tray(shared: &Shared, entries: Vec<Entry>) -> Menu {
    let mut menu = Menu::new();
    // Each run of radio entries shares a signal holding the checked one.
    let mut group: Option<Signal<usize>> = None;
    for (i, entry) in entries.into_iter().enumerate() {
        let s = shared.clone();
        menu = match entry {
            Entry::Item { label, act: None } => {
                group = None;
                menu.item(MenuItem::new(label).enabled(false))
            }
            Entry::Item { label, act: Some(act) } => {
                group = None;
                menu.item(MenuItem::new(label).on_click(move || perform(&s, act)))
            }
            Entry::Check { label, on, act } => {
                group = None;
                menu.item(MenuItem::checkbox(label, Signal::new(on)).on_click(move || perform(&s, act)))
            }
            Entry::Radio { label, on, act } => {
                let g = *group.get_or_insert_with(|| Signal::new(usize::MAX));
                if on {
                    g.set(i);
                }
                menu.item(MenuItem::radio(label, g, i).on_click(move || perform(&s, act)))
            }
            Entry::Separator => {
                group = None;
                menu.separator()
            }
        };
    }
    menu
}

fn capitalized(text: &str) -> String {
    let mut c = text.chars();
    c.next().map_or_else(String::new, |f| f.to_uppercase().chain(c).collect())
}

pub fn shorten(text: &str, max: usize) -> String {
    let line = text.lines().next().unwrap_or("");
    if line.chars().count() <= max {
        return line.to_string();
    }
    let cut: String = line.chars().take(max - 1).collect();
    format!("{cut}\u{2026}")
}
