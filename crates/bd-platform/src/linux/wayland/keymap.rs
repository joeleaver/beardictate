//! Character → key lookup through the keymap the compositor hands libei.
//!
//! libei sends evdev keycodes that the compositor interprets with *its*
//! keymap, which we can't change. So each character must be found in that
//! keymap: we simulate an xkb state carrying the compositor's current locked
//! modifiers (Caps Lock) and layout group, press each combination of Shift
//! and AltGr, and record what every key then produces. Characters the
//! layout can't produce (curly quotes and em-dashes on a US layout) are
//! reported so the caller can paste instead.

use std::collections::HashMap;

use xkbcommon::xkb::{self, Keycode, Keymap, State};

use crate::config::NewlineKey;
use crate::linux::keysym::{XK_INSERT, XK_RETURN, XK_TAB, XK_V};
use crate::text::Token;

const XK_SHIFT_L: u32 = 0xffe1;
const XK_CONTROL_L: u32 = 0xffe3;
const XK_ISO_LEVEL3_SHIFT: u32 = 0xfe03;

/// A key plus the modifier keys to hold around it (xkb keycodes).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Combo {
    pub key: u32,
    pub mods: Vec<u32>,
}

/// Lookup tables for one keymap + lock/group state.
#[derive(Debug, Default)]
pub(crate) struct ReverseMap {
    by_char: HashMap<char, Combo>,
    by_keysym: HashMap<u32, Combo>,
    shift: Option<u32>,
    ctrl: Option<u32>,
    /// A key with lowercase `v` in the first layout, for Ctrl+V while a
    /// non-Latin layout is active (toolkits fall back to it for shortcuts).
    latin_v: Option<u32>,
}

/// Keycode whose first level in `layout` is `keysym`.
fn find_level0(keymap: &Keymap, layout: u32, keysym: u32) -> Option<u32> {
    (keymap.min_keycode().raw()..=keymap.max_keycode().raw()).find(|&kc| {
        keymap
            .key_get_syms_by_level(Keycode::new(kc), layout, 0)
            .iter()
            .any(|s| s.raw() == keysym)
    })
}

impl ReverseMap {
    /// `locked` is the compositor's locked-modifier mask, `group` its active
    /// layout index (both from `ei_keyboard.modifiers`).
    pub(crate) fn build(keymap: &Keymap, locked: u32, group: u32) -> Self {
        let shift = find_level0(keymap, group, XK_SHIFT_L);
        let altgr = find_level0(keymap, group, XK_ISO_LEVEL3_SHIFT);
        let ctrl = find_level0(keymap, group, XK_CONTROL_L);
        let mut combos: Vec<Vec<u32>> = vec![vec![]];
        combos.extend(shift.map(|s| vec![s]));
        combos.extend(altgr.map(|a| vec![a]));
        if let (Some(s), Some(a)) = (shift, altgr) {
            combos.push(vec![s, a]);
        }
        let modifier_keys: Vec<u32> = [shift, altgr, ctrl].into_iter().flatten().collect();

        let mut map = ReverseMap {
            shift,
            ctrl,
            latin_v: find_level0(keymap, 0, XK_V),
            ..ReverseMap::default()
        };
        // Keycodes above 255 (KEY_DOLLAR, KEY_EURO, ...) exist in the evdev
        // keymap but can't reach X11 clients through Xwayland, so a regular
        // key needing Shift/AltGr is preferred over them.
        let (min, max) = (keymap.min_keycode().raw(), keymap.max_keycode().raw());
        let ranges = [min..=max.min(255), 256.max(min)..=max];
        for range in ranges {
            for mods in &combos {
                let mut state = State::new(keymap);
                state.update_mask(0, 0, locked, 0, 0, group);
                for &m in mods {
                    state.update_key(Keycode::new(m), xkb::KeyDirection::Down);
                }
                for kc in range.clone() {
                    if modifier_keys.contains(&kc) {
                        continue;
                    }
                    let combo = Combo {
                        key: kc,
                        mods: mods.clone(),
                    };
                    let sym = state.key_get_one_sym(Keycode::new(kc)).raw();
                    if sym != 0 {
                        map.by_keysym.entry(sym).or_insert_with(|| combo.clone());
                    }
                    if let Some(c) = char::from_u32(state.key_get_utf32(Keycode::new(kc)))
                        && !c.is_control()
                    {
                        map.by_char.entry(c).or_insert(combo);
                    }
                }
            }
        }
        map
    }

    /// The combo that types `token`, if the layout has it.
    pub(crate) fn token(&self, token: Token, newline: NewlineKey) -> Option<Combo> {
        match token {
            Token::Char(c) => self.by_char.get(&c).cloned(),
            Token::Tab => self.by_keysym.get(&XK_TAB).cloned(),
            Token::Newline => {
                let mut combo = self.by_keysym.get(&XK_RETURN)?.clone();
                if newline == NewlineKey::ShiftEnter {
                    combo.mods = vec![self.shift?];
                }
                Some(combo)
            }
        }
    }

    /// Ctrl/Shift + `v` or `Insert`, as asked.
    pub(crate) fn chord(&self, keysym: u32, shift: bool, ctrl: bool) -> Option<Combo> {
        let key = match keysym {
            XK_V => self
                .by_keysym
                .get(&XK_V)
                .filter(|c| c.mods.is_empty())
                .map(|c| c.key)
                .or(self.latin_v)?,
            XK_INSERT => self.by_keysym.get(&XK_INSERT)?.key,
            _ => return None,
        };
        let mut mods = Vec::new();
        if ctrl {
            mods.push(self.ctrl?);
        }
        if shift {
            mods.push(self.shift?);
        }
        Some(Combo { key, mods })
    }
}

/// Press/release sequence for a combo, as `(evdev keycode, pressed)`.
pub(crate) fn combo_events(combo: &Combo) -> Vec<(u32, bool)> {
    let evdev = |kc: u32| kc.saturating_sub(8);
    let mut out: Vec<(u32, bool)> = combo.mods.iter().map(|&m| (evdev(m), true)).collect();
    out.push((evdev(combo.key), true));
    out.push((evdev(combo.key), false));
    out.extend(combo.mods.iter().rev().map(|&m| (evdev(m), false)));
    out
}

/// Mask of Shift/Ctrl/Alt/Super in this keymap's modifier numbering.
pub(crate) fn shortcut_modifier_mask(keymap: &Keymap) -> u32 {
    [
        xkb::MOD_NAME_SHIFT,
        xkb::MOD_NAME_CTRL,
        xkb::MOD_NAME_ALT,
        xkb::MOD_NAME_LOGO,
    ]
    .iter()
    .map(|name| keymap.mod_get_index(name))
    .filter(|&i| i != xkb::MOD_INVALID && i < 32)
    .fold(0, |mask, i| mask | (1 << i))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keymap(layout: &str) -> Keymap {
        let ctx = xkb::Context::new(xkb::CONTEXT_NO_FLAGS);
        Keymap::new_from_names(
            &ctx,
            "evdev",
            "pc105",
            layout,
            "",
            None,
            xkb::KEYMAP_COMPILE_NO_FLAGS,
        )
        .expect("xkeyboard-config is installed")
    }

    // evdev keycodes + 8: A=38, V=55, Q=24, Return=36, LShift=50, RAlt=108.
    #[test]
    fn us_layout() {
        let km = keymap("us");
        let m = ReverseMap::build(&km, 0, 0);
        assert_eq!(
            m.token(Token::Char('a'), NewlineKey::Enter),
            Some(Combo {
                key: 38,
                mods: vec![]
            })
        );
        assert_eq!(
            m.token(Token::Char('A'), NewlineKey::Enter),
            Some(Combo {
                key: 38,
                mods: vec![50]
            })
        );
        assert_eq!(
            m.token(Token::Newline, NewlineKey::Enter),
            Some(Combo {
                key: 36,
                mods: vec![]
            })
        );
        assert_eq!(
            m.token(Token::Newline, NewlineKey::ShiftEnter),
            Some(Combo {
                key: 36,
                mods: vec![50]
            })
        );
        assert_eq!(m.token(Token::Char('“'), NewlineKey::Enter), None);
        assert_eq!(
            m.token(Token::Char('1'), NewlineKey::Enter).unwrap().key,
            10,
            "main row, not keypad"
        );
        // Shift+4, not the >255 KEY_DOLLAR that Xwayland can't deliver.
        assert_eq!(
            m.token(Token::Char('$'), NewlineKey::Enter),
            Some(Combo {
                key: 13,
                mods: vec![50]
            })
        );
        let paste = m.chord(XK_V, false, true).unwrap();
        assert_eq!(
            paste,
            Combo {
                key: 55,
                mods: vec![37]
            }
        );
        assert_eq!(
            combo_events(&paste),
            vec![(29, true), (47, true), (47, false), (29, false)]
        );
    }

    #[test]
    fn caps_lock_inverts_shift() {
        let km = keymap("us");
        let lock = 1 << km.mod_get_index(xkb::MOD_NAME_CAPS);
        let m = ReverseMap::build(&km, lock, 0);
        assert_eq!(
            m.token(Token::Char('a'), NewlineKey::Enter),
            Some(Combo {
                key: 38,
                mods: vec![50]
            })
        );
        assert_eq!(
            m.token(Token::Char('A'), NewlineKey::Enter),
            Some(Combo {
                key: 38,
                mods: vec![]
            })
        );
    }

    #[test]
    fn altgr_on_german_layout() {
        let km = keymap("de");
        let m = ReverseMap::build(&km, 0, 0);
        // Q + a level-3 shift key (xkeyboard-config lists the LVL3 key, 92,
        // before RAlt, 108; either produces the same state).
        let at = m.token(Token::Char('@'), NewlineKey::Enter).unwrap();
        assert_eq!(at.key, 24);
        assert_eq!(at.mods.len(), 1);
        assert!([92, 108].contains(&at.mods[0]));
        assert!(m.token(Token::Char('ü'), NewlineKey::Enter).is_some());
    }

    #[test]
    fn second_group_and_latin_fallback_for_paste() {
        let km = keymap("us,ru");
        let m = ReverseMap::build(&km, 0, 1);
        assert!(m.token(Token::Char('ж'), NewlineKey::Enter).is_some());
        assert_eq!(m.chord(XK_V, false, true).unwrap().key, 55);
    }

    #[test]
    fn modifier_mask_has_four_bits() {
        assert_eq!(shortcut_modifier_mask(&keymap("us")).count_ones(), 4);
    }
}
