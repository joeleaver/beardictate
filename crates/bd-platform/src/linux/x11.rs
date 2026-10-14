//! X11 backend: XTest fake key events on temporarily bound spare keycodes
//! (see [`super::x11_slots`]), plus EWMH to name the focused app.

use std::thread;
use std::time::{Duration, Instant};

use x11rb::connection::Connection;
use x11rb::protocol::xproto::{
    AtomEnum, ConnectionExt as _, KEY_PRESS_EVENT, KEY_RELEASE_EVENT, Window,
};
use x11rb::protocol::xtest::ConnectionExt as _;
use x11rb::rust_connection::RustConnection;

use super::keysym::{Stroke, chord_stroke, token_stroke};
use super::x11_slots::{Slots, binding, plan_bursts, spare_keycodes};
use crate::config::{ForegroundApp, PasteChord, Settings};
use crate::driver::{Chord, Target, Typed};
use crate::error::PlatformError;
use crate::text::Token;

/// How long clients get to look up keysyms for keys we pressed before the
/// keycodes are rebound. Toolkits re-fetch the keymap lazily after a
/// `MappingNotify`, so rebinding too soon makes them see the *new* keysym.
const REMAP_SETTLE: Duration = Duration::from_millis(60);

// Core modifier mask bits / modifier-map rows.
const SHIFT: u16 = 1 << 0;
const LOCK: u16 = 1 << 1;
const CONTROL: u16 = 1 << 2;
const MOD1: u16 = 1 << 3;
const MOD4: u16 = 1 << 6;
const ROW_SHIFT: usize = 0;
const ROW_LOCK: usize = 1;
const ROW_CONTROL: usize = 2;

fn x11<E: std::fmt::Display>(e: E) -> PlatformError {
    PlatformError::X11(e.to_string())
}

pub(crate) struct X11Backend {
    conn: RustConnection,
    root: Window,
    net_active_window: u32,
    net_wm_pid: u32,
}

/// The keyboard layout facts one injection needs.
struct Layout {
    per: u8,
    spare: Vec<u8>,
    shift: Option<u8>,
    control: Option<u8>,
    /// The Caps Lock key, if Lock is currently on. Xlib and xkbcommon
    /// uppercase letters while Lock is set, whatever keysym a key carries,
    /// so it is switched off around typing and back on afterwards.
    caps_on: Option<u8>,
}

impl X11Backend {
    pub(crate) fn connect() -> Result<Self, PlatformError> {
        let (conn, screen) = x11rb::connect(None).map_err(x11)?;
        let root = conn.setup().roots[screen].root;
        conn.xtest_get_version(2, 2)
            .map_err(x11)?
            .reply()
            .map_err(|e| PlatformError::X11(format!("XTEST extension unavailable: {e}")))?;
        let atom = |name: &[u8]| -> Result<u32, PlatformError> {
            Ok(conn
                .intern_atom(false, name)
                .map_err(x11)?
                .reply()
                .map_err(x11)?
                .atom)
        };
        let net_active_window = atom(b"_NET_ACTIVE_WINDOW")?;
        let net_wm_pid = atom(b"_NET_WM_PID")?;
        Ok(Self {
            conn,
            root,
            net_active_window,
            net_wm_pid,
        })
    }

    fn sync(&self) -> Result<(), PlatformError> {
        self.conn
            .get_input_focus()
            .map_err(x11)?
            .reply()
            .map_err(x11)?;
        Ok(())
    }

    fn layout(&self) -> Result<Layout, PlatformError> {
        let setup = self.conn.setup();
        let (min, max) = (setup.min_keycode, setup.max_keycode);
        let map = self
            .conn
            .get_keyboard_mapping(min, max - min + 1)
            .map_err(x11)?
            .reply()
            .map_err(x11)?;
        let mods = self
            .conn
            .get_modifier_mapping()
            .map_err(x11)?
            .reply()
            .map_err(x11)?;
        let per_mod = usize::from(mods.keycodes_per_modifier());
        let first_in_row = |row: usize| {
            mods.keycodes
                .get(row * per_mod..(row + 1) * per_mod)
                .and_then(|r| r.iter().copied().find(|&k| k != 0))
        };
        let lock_active = self.modifier_state()? & LOCK != 0;
        Ok(Layout {
            caps_on: first_in_row(ROW_LOCK).filter(|_| lock_active),
            per: map.keysyms_per_keycode,
            spare: spare_keycodes(min, map.keysyms_per_keycode, &map.keysyms),
            shift: first_in_row(ROW_SHIFT),
            control: first_in_row(ROW_CONTROL),
        })
    }

    fn bind(&self, per: u8, keycode: u8, keysym: u32) -> Result<(), PlatformError> {
        let syms = if keysym == 0 {
            vec![0; usize::from(per)]
        } else {
            binding(keysym, per)
        };
        self.conn
            .change_keyboard_mapping(1, keycode, per, &syms)
            .map_err(x11)?;
        Ok(())
    }

    fn key(&self, keycode: u8, press: bool) -> Result<(), PlatformError> {
        let kind = if press {
            KEY_PRESS_EVENT
        } else {
            KEY_RELEASE_EVENT
        };
        self.conn
            .xtest_fake_input(kind, keycode, x11rb::CURRENT_TIME, x11rb::NONE, 0, 0, 0)
            .map_err(x11)?;
        Ok(())
    }

    fn stroke(&self, s: &Stroke, slots: &Slots, layout: &Layout) -> Result<(), PlatformError> {
        let key = slots.keycode(s.keysym).expect("burst keysyms are bound");
        let mut mods = Vec::new();
        if s.ctrl {
            mods.push(
                layout
                    .control
                    .ok_or_else(|| PlatformError::X11("no Control key in the keymap".into()))?,
            );
        }
        if s.shift {
            mods.push(
                layout
                    .shift
                    .ok_or_else(|| PlatformError::X11("no Shift key in the keymap".into()))?,
            );
        }
        for &m in &mods {
            self.key(m, true)?;
        }
        self.key(key, true)?;
        self.key(key, false)?;
        for &m in mods.iter().rev() {
            self.key(m, false)?;
        }
        Ok(())
    }

    /// Types `strokes`; returns how many went out before an error, if any.
    fn send_strokes(
        &self,
        strokes: &[Stroke],
        settings: &Settings,
    ) -> Result<Typed, PlatformError> {
        let layout = self.layout()?;
        if layout.spare.is_empty() {
            return Ok(Typed::Impossible {
                reason: "the X11 keymap has no unused keycode to bind characters to".into(),
            });
        }
        let mut slots = Slots::new(layout.spare.clone());
        if let Some(caps) = layout.caps_on {
            self.key(caps, true)?;
            self.key(caps, false)?;
        }
        let result = self.send_bursts(strokes, settings, &layout, &mut slots);
        // Always unbind, even after an error, so the user's keymap is clean.
        thread::sleep(REMAP_SETTLE);
        if let Some(caps) = layout.caps_on {
            self.key(caps, true)?;
            self.key(caps, false)?;
        }
        for keycode in slots.used() {
            if let Err(e) = self.bind(layout.per, keycode, 0) {
                tracing::warn!(keycode, error = %e, "could not unbind temporary keycode");
            }
        }
        self.sync()?;
        result
    }

    fn send_bursts(
        &self,
        strokes: &[Stroke],
        settings: &Settings,
        layout: &Layout,
        slots: &mut Slots,
    ) -> Result<Typed, PlatformError> {
        for (i, range) in plan_bursts(strokes, settings.batch_chars, slots.capacity())
            .into_iter()
            .enumerate()
        {
            let burst = &strokes[range.clone()];
            let mut needed: Vec<u32> = Vec::new();
            for s in burst {
                if !needed.contains(&s.keysym) {
                    needed.push(s.keysym);
                }
            }
            let changes = slots.bind(&needed);
            if i > 0 {
                let pause = if changes.is_empty() {
                    settings.batch_delay
                } else {
                    REMAP_SETTLE.max(settings.batch_delay)
                };
                thread::sleep(pause);
            }
            let sent = (|| {
                for &(keycode, keysym) in &changes {
                    self.bind(layout.per, keycode, keysym)?;
                }
                for s in burst {
                    self.stroke(s, slots, layout)?;
                }
                self.sync()
            })();
            if let Err(e) = sent {
                return Ok(if range.start == 0 {
                    Typed::Impossible {
                        reason: e.to_string(),
                    }
                } else {
                    Typed::Partial {
                        delivered: range.start,
                        reason: e.to_string(),
                    }
                });
            }
        }
        Ok(Typed::Done)
    }

    pub(crate) fn foreground_app(&self) -> Option<ForegroundApp> {
        let prop = |win: Window, atom: u32, kind: AtomEnum| -> Option<u32> {
            self.conn
                .get_property(false, win, atom, kind, 0, 1)
                .ok()?
                .reply()
                .ok()?
                .value32()?
                .next()
        };
        let win = prop(self.root, self.net_active_window, AtomEnum::WINDOW).filter(|&w| w != 0)?;
        let pid = prop(win, self.net_wm_pid, AtomEnum::CARDINAL)?;
        let exe_name = std::fs::read_link(format!("/proc/{pid}/exe"))
            .ok()
            .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
            .or_else(|| {
                std::fs::read_to_string(format!("/proc/{pid}/comm"))
                    .ok()
                    .map(|s| s.trim_end().to_owned())
            })?;
        Some(ForegroundApp { exe_name, pid })
    }

    pub(crate) fn target(&self) -> Result<Target, PlatformError> {
        let focus = self
            .conn
            .get_input_focus()
            .map_err(x11)?
            .reply()
            .map_err(x11)?
            .focus;
        if focus == x11rb::NONE {
            return Ok(Target::Blocked {
                reason: "no X11 window has keyboard focus".into(),
            });
        }
        Ok(Target::Ready(self.foreground_app()))
    }

    pub(crate) fn wait_modifiers_released(&self, timeout: Duration) -> Result<bool, PlatformError> {
        let deadline = Instant::now() + timeout;
        loop {
            if self.modifier_state()? & (SHIFT | CONTROL | MOD1 | MOD4) == 0 {
                return Ok(true);
            }
            if Instant::now() >= deadline {
                return Ok(false);
            }
            thread::sleep(Duration::from_millis(10));
        }
    }

    fn modifier_state(&self) -> Result<u16, PlatformError> {
        Ok(u16::from(
            self.conn
                .query_pointer(self.root)
                .map_err(x11)?
                .reply()
                .map_err(x11)?
                .mask,
        ))
    }

    pub(crate) fn type_tokens(
        &self,
        tokens: &[Token],
        settings: &Settings,
    ) -> Result<Typed, PlatformError> {
        let strokes: Vec<Stroke> = tokens
            .iter()
            .map(|&t| token_stroke(t, settings.newline))
            .collect();
        self.send_strokes(&strokes, settings)
    }

    pub(crate) fn send_paste_chord(
        &self,
        chord: PasteChord,
        settings: &Settings,
    ) -> Result<Chord, PlatformError> {
        Ok(match self.send_strokes(&[chord_stroke(chord)], settings)? {
            Typed::Done => Chord::Sent,
            Typed::Impossible { reason } | Typed::Partial { reason, .. } => {
                Chord::Impossible { reason }
            }
        })
    }
}
