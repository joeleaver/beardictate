//! X keysyms: the shared vocabulary of X11 and xkbcommon (Wayland).

use crate::config::{NewlineKey, PasteChord};
use crate::text::Token;

pub(crate) const XK_TAB: u32 = 0xff09;
pub(crate) const XK_RETURN: u32 = 0xff0d;
pub(crate) const XK_INSERT: u32 = 0xff63;
pub(crate) const XK_V: u32 = 0x0076;

/// The keysym that produces `c`: Latin-1 printables are their own keysym,
/// everything else uses the Unicode keysym range `0x0100_0000 + codepoint`,
/// which Xlib, GTK, Qt and xkbcommon all translate back to the character.
pub(crate) fn keysym_for_char(c: char) -> u32 {
    let cp = c as u32;
    match cp {
        0x20..=0x7e | 0xa0..=0xff => cp,
        _ => 0x0100_0000 | cp,
    }
}

/// One key tap with modifiers held around it, independent of keycodes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Stroke {
    pub keysym: u32,
    pub shift: bool,
    pub ctrl: bool,
}

impl Stroke {
    fn plain(keysym: u32) -> Self {
        Stroke {
            keysym,
            shift: false,
            ctrl: false,
        }
    }
}

/// The stroke for one token. Characters carry no modifiers: the backends map
/// the character's keysym onto a key directly (X11) or work out Shift/AltGr
/// from the keymap themselves (Wayland).
pub(crate) fn token_stroke(token: Token, newline: NewlineKey) -> Stroke {
    match token {
        Token::Char(c) => Stroke::plain(keysym_for_char(c)),
        Token::Tab => Stroke::plain(XK_TAB),
        Token::Newline => Stroke {
            keysym: XK_RETURN,
            shift: newline == NewlineKey::ShiftEnter,
            ctrl: false,
        },
    }
}

pub(crate) fn chord_stroke(chord: PasteChord) -> Stroke {
    match chord {
        PasteChord::CtrlV => Stroke {
            keysym: XK_V,
            shift: false,
            ctrl: true,
        },
        PasteChord::CtrlShiftV => Stroke {
            keysym: XK_V,
            shift: true,
            ctrl: true,
        },
        PasteChord::ShiftInsert => Stroke {
            keysym: XK_INSERT,
            shift: true,
            ctrl: false,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keysyms() {
        assert_eq!(keysym_for_char('a'), 0x61);
        assert_eq!(keysym_for_char(' '), 0x20);
        assert_eq!(keysym_for_char('é'), 0xe9);
        assert_eq!(keysym_for_char('“'), 0x0100_201c);
        assert_eq!(keysym_for_char('—'), 0x0100_2014);
        assert_eq!(keysym_for_char('😀'), 0x0101_f600);
    }

    #[test]
    fn strokes() {
        assert_eq!(
            token_stroke(Token::Newline, NewlineKey::ShiftEnter),
            Stroke {
                keysym: XK_RETURN,
                shift: true,
                ctrl: false
            }
        );
        assert!(!token_stroke(Token::Newline, NewlineKey::Enter).shift);
        let c = chord_stroke(PasteChord::CtrlShiftV);
        assert!(c.ctrl && c.shift && c.keysym == XK_V);
    }
}
