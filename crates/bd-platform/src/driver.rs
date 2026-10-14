//! The strategy logic shared by every platform: when to type, when to paste,
//! when to give up and leave the text on the clipboard. Backends only supply
//! the OS primitives through [`Backend`], which keeps this testable with a
//! fake backend.

use std::thread;
use std::time::Duration;

use bd_core::InjectOutcome;

use crate::config::{ForegroundApp, InjectorConfig, PasteChord, Settings, Strategy};
use crate::error::PlatformError;
use crate::text::{Token, detokenize, tokenize};

/// Whether text can be delivered to whatever has focus right now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Target {
    /// Go ahead. The app is `None` when the platform can't identify it.
    Ready(Option<ForegroundApp>),
    /// Synthetic input would be silently dropped (e.g. UIPI on Windows).
    Blocked { reason: String },
}

/// Result of trying to type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Typed {
    Done,
    /// Nothing was sent, because this text can't be typed here.
    Impossible {
        reason: String,
    },
    /// The first `delivered` tokens went out, then input was refused.
    Partial {
        delivered: usize,
        reason: String,
    },
}

/// Result of sending the paste chord.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Chord {
    Sent,
    Impossible { reason: String },
}

/// OS primitives one platform backend provides.
pub(crate) trait Backend {
    /// Saved clipboard content.
    type Snapshot;

    fn target(&mut self) -> Result<Target, PlatformError>;
    /// Waits up to `timeout` for Shift/Ctrl/Alt/Super to be released.
    /// Returns whether they were.
    fn wait_modifiers_released(&mut self, timeout: Duration) -> Result<bool, PlatformError>;
    fn type_tokens(
        &mut self,
        tokens: &[Token],
        settings: &Settings,
    ) -> Result<Typed, PlatformError>;
    fn send_paste_chord(
        &mut self,
        chord: PasteChord,
        settings: &Settings,
    ) -> Result<Chord, PlatformError>;
    fn clipboard_snapshot(&mut self) -> Result<Self::Snapshot, PlatformError>;
    /// Replaces the clipboard with `text`. `transient` asks clipboard
    /// history/managers to ignore it (it is about to be replaced again).
    fn clipboard_set(&mut self, text: &str, transient: bool) -> Result<(), PlatformError>;
    /// Whether the clipboard still holds what the last `clipboard_set` put
    /// there, i.e. neither the user nor another app replaced it since.
    fn clipboard_still_ours(&mut self, text: &str) -> Result<bool, PlatformError>;
    fn clipboard_restore(&mut self, snapshot: Self::Snapshot) -> Result<(), PlatformError>;
}

/// Delivers `text` with the configured strategy.
pub(crate) fn inject<B: Backend>(
    backend: &mut B,
    config: &InjectorConfig,
    text: &str,
) -> Result<InjectOutcome, PlatformError> {
    let tokens = tokenize(text);
    if tokens.is_empty() {
        return Ok(InjectOutcome::Typed);
    }
    let app = match backend.target()? {
        Target::Ready(app) => app,
        Target::Blocked { reason } => return leave_on_clipboard(backend, text, reason),
    };
    let settings = config.resolve(app.as_ref());
    if !backend.wait_modifiers_released(settings.modifier_release_timeout)? {
        return leave_on_clipboard(
            backend,
            text,
            "modifier keys (Shift/Ctrl/Alt/Super) were still held, which would have turned \
             the text into shortcuts"
                .to_owned(),
        );
    }
    if settings.strategy == Strategy::PasteOnly {
        return paste(backend, text, &settings);
    }
    match backend.type_tokens(&tokens, &settings)? {
        Typed::Done => Ok(InjectOutcome::Typed),
        Typed::Partial { delivered, reason } => {
            let rest = detokenize(&tokens[delivered.min(tokens.len())..]);
            leave_on_clipboard(
                backend,
                &rest,
                format!(
                    "typing stopped after {delivered} of {} characters ({reason}); \
                     the rest is on the clipboard",
                    tokens.len()
                ),
            )
        }
        Typed::Impossible { reason } if settings.strategy == Strategy::Auto => {
            tracing::info!(%reason, "cannot type this text here, pasting instead");
            paste(backend, text, &settings)
        }
        Typed::Impossible { reason } => leave_on_clipboard(backend, text, reason),
    }
}

fn leave_on_clipboard<B: Backend>(
    backend: &mut B,
    text: &str,
    reason: String,
) -> Result<InjectOutcome, PlatformError> {
    tracing::warn!(%reason, "leaving dictated text on the clipboard");
    backend.clipboard_set(text, false)?;
    Ok(InjectOutcome::LeftOnClipboard { reason })
}

fn paste<B: Backend>(
    backend: &mut B,
    text: &str,
    settings: &Settings,
) -> Result<InjectOutcome, PlatformError> {
    let snapshot = if settings.restore_clipboard {
        match backend.clipboard_snapshot() {
            Ok(s) => Some(s),
            Err(e) => {
                tracing::warn!(error = %e, "could not save the clipboard; it won't be restored");
                None
            }
        }
    } else {
        None
    };
    backend.clipboard_set(text, true)?;
    if let Chord::Impossible { reason } =
        backend.send_paste_chord(settings.paste_chord, settings)?
    {
        // Re-set without the transient marking so clipboard history keeps it
        // for the user to paste by hand.
        return leave_on_clipboard(backend, text, reason);
    }
    // The target reads the clipboard asynchronously after it processes the
    // chord. Restoring before that would paste the user's old content.
    thread::sleep(settings.paste_settle);
    if let Some(snapshot) = snapshot {
        match backend.clipboard_still_ours(text) {
            Ok(true) => {
                if let Err(e) = backend.clipboard_restore(snapshot) {
                    // The text was delivered; a failed restore is not a failed
                    // injection.
                    tracing::warn!(error = %e, "could not restore the previous clipboard");
                }
            }
            Ok(false) => tracing::info!("clipboard changed while pasting; not restoring it"),
            Err(e) => tracing::warn!(error = %e, "could not check the clipboard; not restoring"),
        }
    }
    Ok(InjectOutcome::Pasted)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{AppPolicy, NewlineKey};
    use std::sync::Arc;

    /// Records what the driver asked for.
    #[derive(Default)]
    struct Fake {
        target: Option<Target>,
        modifiers_released: bool,
        typed: Option<Typed>,
        chord: Option<Chord>,
        clipboard: String,
        foreign_change: bool,
        log: Vec<String>,
        seen_newline: Option<NewlineKey>,
    }

    impl Backend for Fake {
        type Snapshot = String;
        fn target(&mut self) -> Result<Target, PlatformError> {
            Ok(self.target.clone().unwrap_or(Target::Ready(None)))
        }
        fn wait_modifiers_released(&mut self, _: Duration) -> Result<bool, PlatformError> {
            Ok(self.modifiers_released)
        }
        fn type_tokens(&mut self, t: &[Token], s: &Settings) -> Result<Typed, PlatformError> {
            self.seen_newline = Some(s.newline);
            self.log.push(format!("type:{}", detokenize(t)));
            Ok(self.typed.clone().unwrap_or(Typed::Done))
        }
        fn send_paste_chord(
            &mut self,
            c: PasteChord,
            _: &Settings,
        ) -> Result<Chord, PlatformError> {
            self.log.push(format!("chord:{c:?}"));
            if self.foreign_change {
                self.clipboard = "user copied this".into();
            }
            Ok(self.chord.clone().unwrap_or(Chord::Sent))
        }
        fn clipboard_snapshot(&mut self) -> Result<String, PlatformError> {
            Ok(self.clipboard.clone())
        }
        fn clipboard_set(&mut self, text: &str, transient: bool) -> Result<(), PlatformError> {
            self.log.push(format!("set:{text}:{transient}"));
            self.clipboard = text.into();
            Ok(())
        }
        fn clipboard_still_ours(&mut self, text: &str) -> Result<bool, PlatformError> {
            Ok(self.clipboard == text)
        }
        fn clipboard_restore(&mut self, s: String) -> Result<(), PlatformError> {
            self.log.push(format!("restore:{s}"));
            self.clipboard = s;
            Ok(())
        }
    }

    fn fake() -> Fake {
        Fake {
            modifiers_released: true,
            clipboard: "old".into(),
            ..Fake::default()
        }
    }

    fn config(strategy: Strategy) -> InjectorConfig {
        InjectorConfig {
            strategy,
            paste_settle: Duration::ZERO,
            ..InjectorConfig::default()
        }
    }

    #[test]
    fn auto_types() {
        let mut b = fake();
        let out = inject(&mut b, &config(Strategy::Auto), "hi\r\n").unwrap();
        assert_eq!(out, InjectOutcome::Typed);
        assert_eq!(b.log, vec!["type:hi\n"]);
        assert_eq!(b.clipboard, "old");
    }

    #[test]
    fn auto_falls_back_to_paste_and_restores() {
        let mut b = fake();
        b.typed = Some(Typed::Impossible {
            reason: "no keymap".into(),
        });
        let out = inject(&mut b, &config(Strategy::Auto), "“hi”").unwrap();
        assert_eq!(out, InjectOutcome::Pasted);
        assert_eq!(
            b.log,
            vec!["type:“hi”", "set:“hi”:true", "chord:CtrlV", "restore:old"]
        );
        assert_eq!(b.clipboard, "old");
    }

    #[test]
    fn type_only_does_not_paste() {
        let mut b = fake();
        b.typed = Some(Typed::Impossible {
            reason: "no keymap".into(),
        });
        let out = inject(&mut b, &config(Strategy::TypeOnly), "x").unwrap();
        assert_eq!(
            out,
            InjectOutcome::LeftOnClipboard {
                reason: "no keymap".into()
            }
        );
        assert_eq!(b.clipboard, "x");
        assert!(!b.log.iter().any(|l| l.starts_with("chord")));
    }

    #[test]
    fn partial_typing_leaves_only_the_rest() {
        let mut b = fake();
        b.typed = Some(Typed::Partial {
            delivered: 3,
            reason: "blocked".into(),
        });
        let out = inject(&mut b, &config(Strategy::Auto), "abc😀def").unwrap();
        assert!(matches!(out, InjectOutcome::LeftOnClipboard { .. }));
        assert_eq!(b.clipboard, "😀def");
    }

    #[test]
    fn blocked_target_leaves_everything_without_typing() {
        let mut b = fake();
        b.target = Some(Target::Blocked {
            reason: "elevated".into(),
        });
        let out = inject(&mut b, &config(Strategy::Auto), "x").unwrap();
        assert_eq!(
            out,
            InjectOutcome::LeftOnClipboard {
                reason: "elevated".into()
            }
        );
        assert_eq!(b.log, vec!["set:x:false"]);
    }

    #[test]
    fn held_modifiers_leave_on_clipboard() {
        let mut b = fake();
        b.modifiers_released = false;
        let out = inject(&mut b, &config(Strategy::Auto), "x").unwrap();
        assert!(matches!(out, InjectOutcome::LeftOnClipboard { .. }));
        assert_eq!(b.log, vec!["set:x:false"]);
    }

    #[test]
    fn no_restore_when_clipboard_changed_meanwhile() {
        let mut b = fake();
        b.foreign_change = true;
        let out = inject(&mut b, &config(Strategy::PasteOnly), "x").unwrap();
        assert_eq!(out, InjectOutcome::Pasted);
        assert_eq!(b.clipboard, "user copied this");
    }

    #[test]
    fn chord_impossible_keeps_text_as_normal_entry() {
        let mut b = fake();
        b.chord = Some(Chord::Impossible {
            reason: "no portal".into(),
        });
        let out = inject(&mut b, &config(Strategy::PasteOnly), "x").unwrap();
        assert_eq!(
            out,
            InjectOutcome::LeftOnClipboard {
                reason: "no portal".into()
            }
        );
        assert_eq!(b.log.last().unwrap(), "set:x:false");
        assert_eq!(b.clipboard, "x");
    }

    #[test]
    fn empty_text_is_a_no_op() {
        let mut b = fake();
        assert_eq!(
            inject(&mut b, &config(Strategy::Auto), "\u{7}").unwrap(),
            InjectOutcome::Typed
        );
        assert!(b.log.is_empty());
    }

    #[test]
    fn app_override_reaches_backend() {
        let mut b = fake();
        b.target = Some(Target::Ready(Some(ForegroundApp {
            exe_name: "slack.exe".into(),
            pid: 7,
        })));
        let mut c = config(Strategy::Auto);
        c.app_override = Some(Arc::new(|_: &ForegroundApp| AppPolicy {
            newline: Some(NewlineKey::ShiftEnter),
            ..AppPolicy::default()
        }));
        inject(&mut b, &c, "a\nb").unwrap();
        assert_eq!(b.seen_newline, Some(NewlineKey::ShiftEnter));
    }
}
