//! Spare-keycode bookkeeping for X11 typing.
//!
//! XTest can only press keycodes, and the user's layout may lack the
//! character (curly quotes, em-dashes) or put it in another group. So every
//! keysym we type is bound to an otherwise *unused* keycode for the length of
//! one injection, with the keysym in every column so neither Shift, AltGr nor
//! the active layout group changes what it produces. Afterwards the keycodes
//! are unbound again.
//!
//! Rebinding a keycode while clients still have unprocessed key events for
//! it makes them look up the new keysym, so bindings are planned per burst:
//! a burst never needs more distinct keysyms than there are spare keycodes,
//! and only keysyms not already bound cause a remap.

use std::ops::Range;

use super::keysym::{Stroke, keysym_for_char};

/// Keycodes with no keysyms in any column, from a core keyboard mapping
/// (`keysyms_per_keycode` columns per keycode starting at `min_keycode`).
pub(crate) fn spare_keycodes(min_keycode: u8, per: u8, keysyms: &[u32]) -> Vec<u8> {
    if per == 0 {
        return Vec::new();
    }
    keysyms
        .chunks(usize::from(per))
        .enumerate()
        .filter(|(_, row)| row.iter().all(|&k| k == 0))
        .filter_map(|(i, _)| u8::try_from(usize::from(min_keycode) + i).ok())
        .collect()
}

/// The keysym columns to bind for `keysym`: `(lower, upper)` pairs for a
/// lowercase letter, so Shift+key gives the uppercase keysym the way real
/// keys do (shortcut matching such as Xt's `Shift <Key>V` relies on it);
/// the keysym alone in every column otherwise.
pub(crate) fn binding(keysym: u32, per: u8) -> Vec<u32> {
    let upper = char_of(keysym).and_then(|c| {
        let mut up = c.to_uppercase();
        match (up.next(), up.next()) {
            (Some(u), None) if u != c && c.is_lowercase() => Some(keysym_for_char(u)),
            _ => None,
        }
    });
    (0..usize::from(per))
        .map(|col| match upper {
            Some(u) if col % 2 == 1 => u,
            _ => keysym,
        })
        .collect()
}

/// Inverse of [`keysym_for_char`] for character keysyms.
fn char_of(keysym: u32) -> Option<char> {
    match keysym {
        0x20..=0x7e | 0xa0..=0xff => char::from_u32(keysym),
        0x0100_0000..=0x0110_ffff => char::from_u32(keysym - 0x0100_0000),
        _ => None,
    }
}

/// Splits strokes into bursts of at most `max_strokes`, each needing at most
/// `capacity` distinct keysyms.
pub(crate) fn plan_bursts(
    strokes: &[Stroke],
    max_strokes: usize,
    capacity: usize,
) -> Vec<Range<usize>> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut distinct: Vec<u32> = Vec::new();
    for (i, s) in strokes.iter().enumerate() {
        let new = !distinct.contains(&s.keysym);
        if i > start && (i - start >= max_strokes || (new && distinct.len() >= capacity)) {
            out.push(start..i);
            start = i;
            distinct.clear();
        }
        if !distinct.contains(&s.keysym) {
            distinct.push(s.keysym);
        }
    }
    if start < strokes.len() {
        out.push(start..strokes.len());
    }
    out
}

/// Which spare keycode holds which keysym right now.
#[derive(Debug)]
pub(crate) struct Slots {
    /// `(keycode, bound keysym, last burst that used it)`.
    slots: Vec<(u8, Option<u32>, u64)>,
    burst: u64,
}

impl Slots {
    pub(crate) fn new(spare: Vec<u8>) -> Self {
        Slots {
            slots: spare.into_iter().map(|k| (k, None, 0)).collect(),
            burst: 0,
        }
    }

    pub(crate) fn capacity(&self) -> usize {
        self.slots.len()
    }

    /// Makes every keysym in `needed` (≤ capacity distinct) bound, reusing
    /// existing bindings and evicting the least recently used others.
    /// Returns the `(keycode, keysym)` rebinds to send to the server.
    pub(crate) fn bind(&mut self, needed: &[u32]) -> Vec<(u8, u32)> {
        self.burst += 1;
        let burst = self.burst;
        let mut changes = Vec::new();
        for &ks in needed {
            if let Some(slot) = self.slots.iter_mut().find(|s| s.1 == Some(ks)) {
                slot.2 = burst;
                continue;
            }
            let victim = self
                .slots
                .iter_mut()
                .filter(|s| s.2 != burst)
                .min_by_key(|s| (s.1.is_some(), s.2))
                .expect("burst needs at most `capacity` distinct keysyms");
            victim.1 = Some(ks);
            victim.2 = burst;
            changes.push((victim.0, ks));
        }
        changes
    }

    pub(crate) fn keycode(&self, keysym: u32) -> Option<u8> {
        self.slots.iter().find(|s| s.1 == Some(keysym)).map(|s| s.0)
    }

    /// Keycodes that were bound at some point and must be unbound again.
    pub(crate) fn used(&self) -> Vec<u8> {
        self.slots
            .iter()
            .filter(|s| s.1.is_some())
            .map(|s| s.0)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(keysym: u32) -> Stroke {
        Stroke {
            keysym,
            shift: false,
            ctrl: false,
        }
    }

    #[test]
    fn letters_bind_with_their_uppercase() {
        assert_eq!(binding(0x76, 4), vec![0x76, 0x56, 0x76, 0x56]);
        assert_eq!(binding(0xe9, 2), vec![0xe9, 0xc9]);
        assert_eq!(binding(0x0100_03b1, 2), vec![0x0100_03b1, 0x0100_0391]); // α Α
        assert_eq!(binding(0x56, 2), vec![0x56, 0x56]); // already uppercase
        assert_eq!(binding(0x0100_201c, 3), vec![0x0100_201c; 3]); // “
        assert_eq!(binding(0xdf, 2), vec![0xdf, 0xdf]); // ß uppercases to "SS"
        assert_eq!(binding(0xff0d, 2), vec![0xff0d, 0xff0d]); // Return
    }

    #[test]
    fn finds_empty_rows() {
        // min 8, 2 columns: 8 = [a, A], 9 = [0, 0], 10 = [0, b], 11 = [0, 0]
        let map = [0x61, 0x41, 0, 0, 0, 0x62, 0, 0];
        assert_eq!(spare_keycodes(8, 2, &map), vec![9, 11]);
        assert!(spare_keycodes(8, 0, &map).is_empty());
    }

    #[test]
    fn bursts_respect_length_and_capacity() {
        let strokes: Vec<_> = "aaaa".chars().map(|c| s(c as u32)).collect();
        assert_eq!(plan_bursts(&strokes, 2, 1), vec![0..2, 2..4]);
        let strokes: Vec<_> = "abcab".chars().map(|c| s(c as u32)).collect();
        // a,b | c,a | b: the third distinct keysym starts a new burst.
        assert_eq!(plan_bursts(&strokes, 10, 2), vec![0..2, 2..4, 4..5]);
        assert_eq!(plan_bursts(&strokes, 10, 3), vec![0..5]);
        assert!(plan_bursts(&[], 10, 3).is_empty());
    }

    #[test]
    fn rebinds_only_whats_missing_and_evicts_lru() {
        let mut slots = Slots::new(vec![200, 201]);
        assert_eq!(slots.bind(&[1, 2]), vec![(200, 1), (201, 2)]);
        assert_eq!(slots.bind(&[2]), vec![]);
        // 1 was used least recently, so 3 takes its keycode.
        assert_eq!(slots.bind(&[3, 2]), vec![(200, 3)]);
        assert_eq!(slots.keycode(3), Some(200));
        assert_eq!(slots.keycode(1), None);
        assert_eq!(slots.used(), vec![200, 201]);
    }
}
