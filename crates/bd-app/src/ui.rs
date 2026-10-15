//! The tray menu, the hotkey, and keeping both in step with the engine.

use std::cell::RefCell;
use std::path::Path;
use std::rc::Rc;

use bd_models::Store;
use rinch::hotkey::{self, Binding, HotkeySpec, HotkeyState};
use rinch::menu::{Menu, MenuItem};
use rinch::prelude::*;
use rinch::reactive::{Effect, untracked};
use rinch::tray::TrayIconImage;

use crate::engine::{self, State, Status};
use crate::icon::{self, Taskbar};
use crate::settings::{Paths, Settings, TrayTheme};
use crate::{engine_send, tray};

/// State the UI shares between the menu, the effects and the window. The
/// signals are `Copy`; the rest is cheap to clone.
#[derive(Clone)]
pub struct Shared {
    pub paths: Paths,
    pub settings: Signal<Settings>,
    pub status: Signal<Status>,
    /// How the hotkey reads in the menu and tooltip.
    pub hotkey_text: Signal<String>,
    /// The settings window's current section.
    pub section: Signal<Section>,
    /// Installed catalog cleanup models: (id, name), smallest first.
    pub installed: Rc<Vec<(String, String)>>,
}

/// The settings window's sections.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Section {
    General,
    Microphone,
    Cleanup,
    Writing,
    Words,
    About,
}

/// Changes the settings: saved at once, and sent to the engine.
pub fn change(s: &Shared, edit: impl FnOnce(&mut Settings)) {
    let current = untracked(|| s.settings.get());
    let mut new = current.clone();
    edit(&mut new);
    if new == current {
        return;
    }
    if let Err(e) = s.paths.save(&new) {
        tracing::warn!("saving settings: {e:#}");
    }
    s.settings.set(new.clone());
    engine_send(engine::Command::Apply(new));
}

impl Shared {
    pub fn new(paths: Paths, settings: Settings) -> Shared {
        let store = match settings.models_dir.trim() {
            "" => Store::from_env().ok(),
            dir => Some(Store::at(dir)),
        };
        let installed = store
            .map(|s| {
                bd_pipeline::setup::installed_cleanup_models(&s)
                    .into_iter()
                    .map(|m| (m.id.clone(), m.name.clone()))
                    .collect()
            })
            .unwrap_or_default();
        Shared {
            paths,
            section: Signal::new(Section::General),
            hotkey_text: Signal::new(readable(&settings.hotkey)),
            status: Signal::new(Status { state: State::Loading, detail: "Starting…".into(), since: None }),
            settings: Signal::new(settings),
            installed: Rc::new(installed),
        }
    }

    /// The engine's way back to the UI thread.
    pub fn bridge(&self) -> Bridge {
        Bridge { status: self.status, settings: self.settings }
    }
}

pub struct Bridge {
    status: Signal<Status>,
    settings: Signal<Settings>,
}

impl engine::Ui for Bridge {
    fn status(&self, status: Status) {
        let signal = self.status;
        rinch::run_on_main_thread(move || signal.set(status));
    }

    fn settings_changed(&self, settings: Settings) {
        let signal = self.settings;
        rinch::run_on_main_thread(move || signal.set(settings));
    }
}

pub fn tray_image(status: &Status, theme: TrayTheme) -> TrayIconImage {
    let rgba = icon::render(status.state, Taskbar::resolve(theme));
    TrayIconImage::from_rgba(rgba, icon::SIZE, icon::SIZE).expect("the icon is SIZE x SIZE RGBA")
}

/// The tray's tooltip. (The item's title, which Plasma sorts the tray by,
/// stays "beardictate", so the icon doesn't move as this changes.)
fn tooltip(status: &Status, hotkey: &str) -> String {
    match status.state {
        State::Loading => "Loading models…".into(),
        State::Idle => format!("{hotkey} to dictate\n{}", status.detail),
        State::Listening => {
            let secs = status.since.map_or(0, |t| t.elapsed().as_secs());
            format!("Listening {}:{:02} · {hotkey} to stop", secs / 60, secs % 60)
        }
        State::Cleaning => "Finishing the last sentences…".into(),
        State::NeedsYou => format!("Needs you: {}", status.detail),
    }
}

/// The right-click menu for the current state.
pub fn tray_menu(shared: &Shared) -> Menu {
    let status = shared.status.get();
    let hotkey = shared.hotkey_text.get();
    let first = match status.state {
        State::Loading => MenuItem::new("Loading models…").enabled(false),
        State::Listening => {
            MenuItem::new(format!("Stop dictating   ({hotkey})")).on_click(|| engine_send(engine::Command::Toggle))
        }
        State::NeedsYou if status.detail.starts_with("Can't type") => MenuItem::new("Start dictating").enabled(false),
        State::NeedsYou => MenuItem::new("Try again").on_click(|| engine_send(engine::Command::Toggle)),
        _ => MenuItem::new(format!("Start dictating   ({hotkey})")).on_click(|| engine_send(engine::Command::Toggle)),
    };
    let mut menu = Menu::new().item(first);
    if status.state == State::NeedsYou {
        menu = menu.item(MenuItem::new(shorten(&status.detail, 70)).enabled(false));
    }

    let open_settings = {
        let s = shared.clone();
        move || {
            s.section.set(Section::General);
            show_current_window();
        }
    };
    menu.separator()
        .item(MenuItem::new("Settings…").on_click(open_settings))
        .separator()
        .item(MenuItem::new("Quit beardictate").on_click(close_current_window))
}

fn shorten(text: &str, max: usize) -> String {
    let line = text.lines().next().unwrap_or("");
    if line.chars().count() <= max {
        return line.to_string();
    }
    let cut: String = line.chars().take(max - 1).collect();
    format!("{cut}…")
}

/// Opens a file in the user's default app for it.
pub fn open_path(path: &Path) {
    #[cfg(target_os = "windows")]
    let result = std::process::Command::new("cmd").args(["/C", "start", ""]).arg(path).spawn();
    #[cfg(target_os = "macos")]
    let result = std::process::Command::new("open").arg(path).spawn();
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    let result = std::process::Command::new("xdg-open").arg(path).spawn();
    if let Err(e) = result {
        tracing::warn!("opening {}: {e}", path.display());
    }
}

/// Starts the effects that keep tray, settings and hotkey in step. Called
/// once, from the app's root component.
pub fn wire(shared: &Shared) {
    // State → icon, tooltip and menu.
    let s = shared.clone();
    Effect::new(move || {
        let status = s.status.get();
        let hotkey = s.hotkey_text.get();
        let theme = s.settings.with(|st| st.tray_theme);
        let Some(tray) = tray() else { return };
        let _ = tray.set_icon(&tray_image(&status, theme));
        let _ = tray.set_tooltip(tooltip(&status, &hotkey));
        // The menu reads signals of its own; build it untracked so this
        // effect follows the state alone.
        let s = s.clone();
        untracked(move || {
            tray.set_menu(Some(tray_menu(&s)));
        });
    });

    // While listening, the tooltip's clock ticks.
    fn tick(s: Shared) {
        set_timeout(1000, move || {
            let status = s.status.get();
            if status.state == State::Listening
                && let Some(tray) = tray()
            {
                let _ = tray.set_tooltip(tooltip(&status, &s.hotkey_text.get()));
            }
            tick(s);
        });
    }
    tick(shared.clone());

    wire_hotkey(shared);
}

/// Registers the dictation hotkey and rebinds it when the setting changes.
fn wire_hotkey(shared: &Shared) {
    let text = shared.hotkey_text;
    let describe = move |b: &Binding, wanted: &str| match b {
        Binding::Exact(sc) => readable(&sc.to_string()),
        Binding::SystemChosen { description } => description.clone().unwrap_or_else(|| readable(wanted)),
        Binding::Pending => format!("{} (waiting for your approval)", readable(wanted)),
        Binding::Declined => "no hotkey (declined; click the bear)".into(),
        _ => readable(wanted),
    };
    let handle: Rc<RefCell<Option<hotkey::Hotkey>>> = Rc::new(RefCell::new(None));
    let s = shared.clone();
    Effect::new(move || {
        let wanted = s.settings.with(|st| st.hotkey.clone());
        untracked(|| {
            if let Err(why) = check_hotkey(&wanted) {
                text.set(format!("no hotkey ({why})"));
                return;
            }
            let mut slot = handle.borrow_mut();
            if let Some(hk) = slot.as_ref() {
                match hk.rebind(&wanted) {
                    Ok(()) => text.set(describe(&hk.binding(), &wanted)),
                    Err(e) => text.set(format!("{wanted} unavailable: {e}")),
                }
                return;
            }
            let spec = HotkeySpec::new(wanted.clone()).id("dictate").description("Start or stop dictating");
            match hotkey::register(spec, |ev| {
                // A toggle: act on the press. Bindings are always two keys or
                // more, so a release never needs telling apart from a chord.
                tracing::info!(state = ?ev.state, "hotkey");
                if ev.state == HotkeyState::Pressed {
                    engine_send(engine::Command::Toggle);
                }
            }) {
                Ok(hk) => {
                    tracing::info!(backend = ?hotkey::backend(), binding = ?hk.binding(), "hotkey registered");
                    text.set(describe(&hk.binding(), &wanted));
                    let w = wanted.clone();
                    hk.on_binding_change(move |b| text.set(describe(b, &w)));
                    *slot = Some(hk);
                }
                Err(e) => {
                    tracing::warn!("hotkey {wanted}: {e}");
                    text.set(format!("{wanted} unavailable: {e}"));
                }
            }
        });
    });
}

/// A shortcut as people write it: `Ctrl+Backquote` is `Ctrl+\``.
pub fn readable(shortcut: &str) -> String {
    shortcut.replace("Backquote", "`")
}

/// Every binding is at least two keys (DESIGN.md "Hotkey").
pub fn check_hotkey(shortcut: &str) -> Result<(), &'static str> {
    let keys = shortcut.split('+').filter(|k| !k.trim().is_empty()).count();
    if keys < 2 { Err("use at least two keys, e.g. Ctrl+Alt+Space") } else { Ok(()) }
}

pub fn is_listening(shared: &Shared) -> bool {
    shared.status.get().state == State::Listening
}

/// The status window's first line: the same as the tooltip.
pub fn status_line(shared: &Shared) -> String {
    tooltip(&shared.status.get(), &shared.hotkey_text.get())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hotkeys_need_two_keys() {
        assert!(check_hotkey("Ctrl+Alt+Space").is_ok());
        assert!(check_hotkey("Super+D").is_ok());
        assert!(check_hotkey("RightCtrl").is_err());
        assert!(check_hotkey("F9").is_err());
        assert_eq!(readable("Ctrl+Backquote"), "Ctrl+`");
    }

    #[test]
    fn tooltips_say_what_to_do() {
        let idle = Status { state: State::Idle, detail: "Cleanup: Qwen3 4B".into(), since: None };
        assert_eq!(tooltip(&idle, "Ctrl+Alt+Space"), "Ctrl+Alt+Space to dictate\nCleanup: Qwen3 4B");
        assert_eq!(shorten("abcdef", 4), "abc…");
    }
}
