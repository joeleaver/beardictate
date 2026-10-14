//! What the injector should do, and the per-app override hook.

use std::fmt;
use std::sync::Arc;
use std::time::Duration;

/// How text reaches the focused app.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Strategy {
    /// Type it; if this platform cannot type the text at all (no permission,
    /// characters missing from the Wayland keymap, no spare X11 keycode),
    /// paste it instead.
    #[default]
    Auto,
    /// Only ever synthesize keystrokes. If typing is impossible the text is
    /// left on the clipboard rather than pasted.
    TypeOnly,
    /// Always go through the clipboard and the paste chord. For apps that
    /// drop synthetic Unicode keystrokes (some games, RDP, a few Electron
    /// apps).
    PasteOnly,
}

/// What key a `'\n'` in the text becomes when typing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum NewlineKey {
    /// Plain Enter.
    #[default]
    Enter,
    /// Shift+Enter: a line break *inside* the message box of chat apps
    /// (Slack, Discord, Teams) where plain Enter sends.
    ShiftEnter,
}

/// The key chord that pastes. Only used by the paste strategy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PasteChord {
    #[default]
    CtrlV,
    /// Terminals on Linux (Konsole, GNOME Terminal) and Windows Terminal.
    CtrlShiftV,
    /// Works in most Windows controls and many X11 apps. Some X11 terminals
    /// (xterm) paste the PRIMARY selection with it, not the clipboard.
    ShiftInsert,
}

/// The application that has keyboard focus, as far as the platform can tell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForegroundApp {
    /// File name of the process executable, as the OS reports it:
    /// `"WINWORD.EXE"` on Windows, `"kate"` on Linux X11.
    pub exe_name: String,
    pub pid: u32,
}

impl ForegroundApp {
    /// Whether this is the executable `name`, ignoring ASCII case and an
    /// optional `.exe` suffix on either side, so a settings list can say
    /// `"winword"` and match `"WINWORD.EXE"`.
    pub fn matches_exe(&self, name: &str) -> bool {
        fn stem(s: &str) -> &str {
            let n = s.len();
            if n > 4 && s[n - 4..].eq_ignore_ascii_case(".exe") {
                &s[..n - 4]
            } else {
                s
            }
        }
        stem(&self.exe_name).eq_ignore_ascii_case(stem(name))
    }
}

/// Per-app adjustments returned by an [`AppOverride`]. `None` fields keep the
/// global setting.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AppPolicy {
    pub strategy: Option<Strategy>,
    pub newline: Option<NewlineKey>,
    pub paste_chord: Option<PasteChord>,
}

/// Called before every injection with the focused app, so the app can
/// implement "always paste in X" (and Shift+Enter in chat apps). Not called
/// when the platform cannot identify the focused app (Wayland).
pub type AppOverride = Arc<dyn Fn(&ForegroundApp) -> AppPolicy + Send + Sync>;

/// Receives a new RemoteDesktop-portal restore token to persist.
pub type RestoreTokenSink = Arc<dyn Fn(&str) + Send + Sync>;

/// Injector settings. Every field has a working default.
#[derive(Clone)]
pub struct InjectorConfig {
    pub strategy: Strategy,
    pub newline: NewlineKey,
    pub paste_chord: PasteChord,
    /// Characters per burst of synthetic keystrokes. Small bursts with a
    /// pause between them keep slow consumers (Electron, RDP, VMs) from
    /// dropping input.
    pub batch_chars: usize,
    /// Pause between bursts.
    pub batch_delay: Duration,
    /// How long to wait after the paste chord before putting the user's
    /// previous clipboard back. The target app reads the clipboard
    /// asynchronously; restoring too early pastes the *old* content. Raise
    /// this for RDP/VM targets.
    pub paste_settle: Duration,
    /// Put the previous clipboard content back after pasting.
    pub restore_clipboard: bool,
    /// How long to wait for Shift/Ctrl/Alt/Super to be released before
    /// typing (a held Ctrl turns Enter into Ctrl+Enter, which sends messages
    /// in many apps). If they are still held after this, the text is left on
    /// the clipboard instead.
    pub modifier_release_timeout: Duration,
    pub app_override: Option<AppOverride>,
    /// Linux Wayland: restore token from a previous portal approval, so the
    /// user is asked only once. Ignored elsewhere.
    pub portal_restore_token: Option<String>,
    /// Linux Wayland: called with each new restore token; persist it and pass
    /// it back as [`InjectorConfig::portal_restore_token`] next launch.
    pub on_portal_restore_token: Option<RestoreTokenSink>,
}

impl Default for InjectorConfig {
    fn default() -> Self {
        Self {
            strategy: Strategy::Auto,
            newline: NewlineKey::Enter,
            paste_chord: PasteChord::CtrlV,
            batch_chars: 16,
            batch_delay: Duration::from_millis(8),
            paste_settle: Duration::from_millis(500),
            restore_clipboard: true,
            modifier_release_timeout: Duration::from_secs(2),
            app_override: None,
            portal_restore_token: None,
            on_portal_restore_token: None,
        }
    }
}

impl fmt::Debug for InjectorConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("InjectorConfig")
            .field("strategy", &self.strategy)
            .field("newline", &self.newline)
            .field("paste_chord", &self.paste_chord)
            .field("batch_chars", &self.batch_chars)
            .field("batch_delay", &self.batch_delay)
            .field("paste_settle", &self.paste_settle)
            .field("restore_clipboard", &self.restore_clipboard)
            .field("modifier_release_timeout", &self.modifier_release_timeout)
            .field("app_override", &self.app_override.as_ref().map(|_| "<fn>"))
            .field(
                "portal_restore_token",
                &self.portal_restore_token.as_ref().map(|_| "<token>"),
            )
            .field(
                "on_portal_restore_token",
                &self.on_portal_restore_token.as_ref().map(|_| "<fn>"),
            )
            .finish()
    }
}

/// The settings for one injection, after the per-app override.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Settings {
    pub strategy: Strategy,
    pub newline: NewlineKey,
    pub paste_chord: PasteChord,
    pub batch_chars: usize,
    pub batch_delay: Duration,
    pub paste_settle: Duration,
    pub restore_clipboard: bool,
    pub modifier_release_timeout: Duration,
}

impl InjectorConfig {
    pub(crate) fn resolve(&self, app: Option<&ForegroundApp>) -> Settings {
        let policy = match (app, &self.app_override) {
            (Some(app), Some(hook)) => hook(app),
            _ => AppPolicy::default(),
        };
        Settings {
            strategy: policy.strategy.unwrap_or(self.strategy),
            newline: policy.newline.unwrap_or(self.newline),
            paste_chord: policy.paste_chord.unwrap_or(self.paste_chord),
            batch_chars: self.batch_chars.max(1),
            batch_delay: self.batch_delay,
            paste_settle: self.paste_settle,
            restore_clipboard: self.restore_clipboard,
            modifier_release_timeout: self.modifier_release_timeout,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app(name: &str) -> ForegroundApp {
        ForegroundApp {
            exe_name: name.into(),
            pid: 1,
        }
    }

    #[test]
    fn exe_matching_ignores_case_and_exe_suffix() {
        assert!(app("WINWORD.EXE").matches_exe("winword"));
        assert!(app("WINWORD.EXE").matches_exe("WinWord.exe"));
        assert!(app("kate").matches_exe("kate"));
        assert!(!app("kate").matches_exe("kwrite"));
        assert!(!app(".exe").matches_exe(""));
    }

    #[test]
    fn override_applies_only_its_fields() {
        let config = InjectorConfig {
            newline: NewlineKey::Enter,
            app_override: Some(Arc::new(|app: &ForegroundApp| {
                if app.matches_exe("slack") {
                    AppPolicy {
                        strategy: Some(Strategy::PasteOnly),
                        newline: Some(NewlineKey::ShiftEnter),
                        paste_chord: None,
                    }
                } else {
                    AppPolicy::default()
                }
            })),
            ..InjectorConfig::default()
        };
        let s = config.resolve(Some(&app("slack.exe")));
        assert_eq!(s.strategy, Strategy::PasteOnly);
        assert_eq!(s.newline, NewlineKey::ShiftEnter);
        assert_eq!(s.paste_chord, PasteChord::CtrlV);

        let s = config.resolve(Some(&app("notepad.exe")));
        assert_eq!(s.strategy, Strategy::Auto);
        assert_eq!(s.newline, NewlineKey::Enter);

        let s = config.resolve(None);
        assert_eq!(s.strategy, Strategy::Auto);
    }

    #[test]
    fn zero_batch_is_clamped() {
        let config = InjectorConfig {
            batch_chars: 0,
            ..InjectorConfig::default()
        };
        assert_eq!(config.resolve(None).batch_chars, 1);
    }
}
