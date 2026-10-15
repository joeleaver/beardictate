//! Windows key-event building, kept free of Win32 types so it is unit-tested
//! on every OS. `windows::sendinput` turns these into `INPUT` structs.

use crate::config::{NewlineKey, PasteChord};
use crate::text::Token;

pub(crate) const VK_BACK: u16 = 0x08;
pub(crate) const VK_TAB: u16 = 0x09;
pub(crate) const VK_RETURN: u16 = 0x0D;
pub(crate) const VK_SHIFT: u16 = 0x10;
pub(crate) const VK_CONTROL: u16 = 0x11;
pub(crate) const VK_INSERT: u16 = 0x2D;
pub(crate) const VK_V: u16 = 0x56;

/// One synthetic key transition.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum KeyEvent {
    /// `KEYEVENTF_UNICODE` with this UTF-16 code unit as the scan code.
    Unicode { unit: u16, up: bool },
    /// A virtual key. `extended` sets `KEYEVENTF_EXTENDEDKEY` (needed for
    /// the navigation-cluster Insert, which is otherwise numpad 0).
    Vk { vk: u16, extended: bool, up: bool },
}

impl KeyEvent {
    fn vk(vk: u16, up: bool) -> Self {
        KeyEvent::Vk {
            vk,
            extended: vk == VK_INSERT,
            up,
        }
    }
}

/// Press `mods` in order, tap `key`, release `mods` in reverse.
fn chord(mods: &[u16], key: u16) -> Vec<KeyEvent> {
    let mut out = Vec::with_capacity(mods.len() * 2 + 2);
    out.extend(mods.iter().map(|&m| KeyEvent::vk(m, false)));
    out.push(KeyEvent::vk(key, false));
    out.push(KeyEvent::vk(key, true));
    out.extend(mods.iter().rev().map(|&m| KeyEvent::vk(m, true)));
    out
}

/// The self-contained event group for one token.
///
/// Characters outside the BMP become a surrogate pair, each unit tapped in
/// turn (high down/up, low down/up): `TranslateMessage` emits one `WM_CHAR`
/// per unit and the receiving window combines them, which is what the
/// on-screen keyboard and IMEs produce too.
pub(crate) fn token_events(token: Token, newline: NewlineKey) -> Vec<KeyEvent> {
    match token {
        Token::Char(c) => {
            let mut buf = [0u16; 2];
            c.encode_utf16(&mut buf)
                .iter()
                .flat_map(|&unit| {
                    [
                        KeyEvent::Unicode { unit, up: false },
                        KeyEvent::Unicode { unit, up: true },
                    ]
                })
                .collect()
        }
        Token::Newline => match newline {
            NewlineKey::Enter => chord(&[], VK_RETURN),
            NewlineKey::ShiftEnter => chord(&[VK_SHIFT], VK_RETURN),
        },
        Token::Tab => chord(&[], VK_TAB),
        Token::Backspace => chord(&[], VK_BACK),
    }
}

/// The events for a paste chord.
pub(crate) fn paste_events(paste: PasteChord) -> Vec<KeyEvent> {
    match paste {
        PasteChord::CtrlV => chord(&[VK_CONTROL], VK_V),
        PasteChord::CtrlShiftV => chord(&[VK_CONTROL, VK_SHIFT], VK_V),
        PasteChord::ShiftInsert => chord(&[VK_SHIFT], VK_INSERT),
    }
}

/// Splits per-token event groups into bursts of at most `max_tokens` tokens.
/// Each burst is `(first token index, events)`.
pub(crate) fn bursts(groups: &[Vec<KeyEvent>], max_tokens: usize) -> Vec<(usize, Vec<KeyEvent>)> {
    groups
        .chunks(max_tokens.max(1))
        .enumerate()
        .map(|(i, chunk)| (i * max_tokens.max(1), chunk.concat()))
        .collect()
}

/// After `sent` events of a burst went through, how many of its token groups
/// were delivered completely, and which key-ups are owed for a group that was
/// cut in half (so no modifier is left stuck down).
pub(crate) fn account_partial(groups: &[Vec<KeyEvent>], sent: usize) -> (usize, Vec<KeyEvent>) {
    let mut remaining = sent;
    for (i, group) in groups.iter().enumerate() {
        if remaining >= group.len() {
            remaining -= group.len();
            continue;
        }
        let mut owed: Vec<KeyEvent> = Vec::new();
        for ev in &group[..remaining] {
            match *ev {
                KeyEvent::Vk {
                    vk,
                    extended,
                    up: false,
                } => owed.push(KeyEvent::Vk {
                    vk,
                    extended,
                    up: true,
                }),
                KeyEvent::Vk { vk, up: true, .. } => {
                    owed.retain(|e| !matches!(e, KeyEvent::Vk { vk: v, .. } if *v == vk))
                }
                KeyEvent::Unicode { unit, up: false } => {
                    owed.push(KeyEvent::Unicode { unit, up: true })
                }
                KeyEvent::Unicode { unit, up: true } => {
                    owed.retain(|e| !matches!(e, KeyEvent::Unicode { unit: u, .. } if *u == unit))
                }
            }
        }
        owed.reverse();
        return (i, owed);
    }
    (groups.len(), Vec::new())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::text::tokenize;
    use KeyEvent::*;

    fn u(unit: u16, up: bool) -> KeyEvent {
        Unicode { unit, up }
    }
    fn v(vk: u16, up: bool) -> KeyEvent {
        KeyEvent::vk(vk, up)
    }

    #[test]
    fn bmp_char_is_one_tap() {
        assert_eq!(
            token_events(Token::Char('—'), NewlineKey::Enter),
            vec![u(0x2014, false), u(0x2014, true)]
        );
    }

    #[test]
    fn astral_char_is_surrogate_pair() {
        // U+1F600 = D83D DE00
        assert_eq!(
            token_events(Token::Char('😀'), NewlineKey::Enter),
            vec![
                u(0xD83D, false),
                u(0xD83D, true),
                u(0xDE00, false),
                u(0xDE00, true)
            ]
        );
    }

    #[test]
    fn newline_variants() {
        assert_eq!(
            token_events(Token::Newline, NewlineKey::Enter),
            vec![v(VK_RETURN, false), v(VK_RETURN, true)]
        );
        assert_eq!(
            token_events(Token::Newline, NewlineKey::ShiftEnter),
            vec![
                v(VK_SHIFT, false),
                v(VK_RETURN, false),
                v(VK_RETURN, true),
                v(VK_SHIFT, true)
            ]
        );
        assert_eq!(
            token_events(Token::Tab, NewlineKey::Enter),
            vec![v(VK_TAB, false), v(VK_TAB, true)]
        );
    }

    #[test]
    fn paste_chords_release_in_reverse_and_insert_is_extended() {
        assert_eq!(
            paste_events(PasteChord::CtrlShiftV),
            vec![
                v(VK_CONTROL, false),
                v(VK_SHIFT, false),
                v(VK_V, false),
                v(VK_V, true),
                v(VK_SHIFT, true),
                v(VK_CONTROL, true)
            ]
        );
        assert!(
            paste_events(PasteChord::ShiftInsert)
                .iter()
                .any(|e| matches!(
                    e,
                    Vk {
                        vk: VK_INSERT,
                        extended: true,
                        ..
                    }
                ))
        );
        assert!(paste_events(PasteChord::CtrlV).iter().all(|e| matches!(
            e,
            Vk {
                extended: false,
                ..
            }
        )));
    }

    #[test]
    fn bursts_never_split_a_group() {
        let groups: Vec<_> = tokenize("a😀b\nc")
            .into_iter()
            .map(|t| token_events(t, NewlineKey::ShiftEnter))
            .collect();
        let b = bursts(&groups, 2);
        assert_eq!(b.len(), 3);
        assert_eq!(b[0].0, 0);
        assert_eq!(b[0].1.len(), 2 + 4);
        assert_eq!(b[1].0, 2);
        assert_eq!(b[1].1.len(), 2 + 4);
        assert_eq!(b[2].0, 4);
        assert_eq!(
            b.iter().map(|x| x.1.len()).sum::<usize>(),
            groups.iter().map(Vec::len).sum::<usize>()
        );
    }

    #[test]
    fn partial_accounting_releases_stuck_shift() {
        let groups = vec![
            token_events(Token::Char('a'), NewlineKey::Enter),
            token_events(Token::Newline, NewlineKey::ShiftEnter),
        ];
        // a (2 events) + Shift down + Return down went through.
        let (done, owed) = account_partial(&groups, 4);
        assert_eq!(done, 1);
        assert_eq!(owed, vec![v(VK_RETURN, true), v(VK_SHIFT, true)]);
        // Everything went through.
        assert_eq!(account_partial(&groups, 6), (2, vec![]));
        // Cut exactly at a group boundary: nothing owed.
        assert_eq!(account_partial(&groups, 2), (1, vec![]));
        // Half a surrogate pair.
        let groups = vec![token_events(Token::Char('😀'), NewlineKey::Enter)];
        assert_eq!(account_partial(&groups, 1), (0, vec![u(0xD83D, true)]));
    }
}
