//! One libei sender connection: handshake, seat/device tracking, and the
//! actual key emulation. Lives on the Wayland worker thread.

use std::os::fd::{AsFd, OwnedFd};
use std::os::unix::net::UnixStream;
use std::thread;
use std::time::{Duration, Instant};

use reis::PendingRequestResult;
use reis::ei;
use reis::event::{DeviceCapability, EiEvent, EiEventConverter};
use reis::handshake::EiHandshaker;
use rustix::event::{PollFd, PollFlags, poll};
use xkbcommon::xkb;

use super::keymap::{Combo, ReverseMap, combo_events, shortcut_modifier_mask};
use crate::config::Settings;
use crate::driver::Typed;
use crate::error::PlatformError;
use crate::linux::keysym::Stroke;
use crate::text::{Token, describe_chars, untypeable};

/// How long to wait for the compositor to set up and resume a keyboard.
const SETUP_TIMEOUT: Duration = Duration::from_secs(5);
/// Longest UTF-8 string one `ei_text.utf8` request may carry.
const EI_TEXT_MAX_BYTES: usize = 254;

fn ei_err<E: std::fmt::Display>(e: E) -> PlatformError {
    PlatformError::Ei(e.to_string())
}

fn now_micros() -> u64 {
    let t = rustix::time::clock_gettime(rustix::time::ClockId::Monotonic);
    t.tv_sec.unsigned_abs() * 1_000_000 + t.tv_nsec.unsigned_abs() / 1_000
}

/// Waits until `fd` is readable or `timeout` passes; returns readability.
pub(crate) fn wait_readable(
    fds: &[&dyn AsFd],
    timeout: Option<Duration>,
) -> Result<Vec<bool>, PlatformError> {
    let mut pfds: Vec<PollFd<'_>> = fds
        .iter()
        .map(|f| PollFd::from_borrowed_fd(f.as_fd(), PollFlags::IN))
        .collect();
    let ts = timeout.map(|d| rustix::time::Timespec {
        tv_sec: d.as_secs() as i64,
        tv_nsec: i64::from(d.subsec_nanos()),
    });
    match poll(&mut pfds, ts.as_ref()) {
        Ok(_) | Err(rustix::io::Errno::INTR) => {}
        Err(e) => return Err(ei_err(e)),
    }
    Ok(pfds.iter().map(|p| !p.revents().is_empty()).collect())
}

struct Dev {
    device: reis::event::Device,
    resumed: bool,
}

pub(crate) struct EiSession {
    context: ei::Context,
    conv: EiEventConverter,
    keyboard: Option<Dev>,
    text: Option<Dev>,
    keymap: Option<xkb::Keymap>,
    depressed: u32,
    locked: u32,
    group: u32,
    sequence: u32,
    disconnected: Option<String>,
}

impl EiSession {
    /// Handshakes on `fd` and waits for a resumed keyboard device.
    pub(crate) fn connect(fd: OwnedFd) -> Result<Self, PlatformError> {
        let context = ei::Context::new(UnixStream::from(fd)).map_err(ei_err)?;
        let deadline = Instant::now() + SETUP_TIMEOUT;
        let mut handshaker = EiHandshaker::new("beardictate", ei::handshake::ContextType::Sender);
        let resp = 'hs: loop {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() || !wait_readable(&[&context], Some(left))?[0] {
                return Err(PlatformError::Ei(
                    "timed out during the libei handshake".into(),
                ));
            }
            context.read().map_err(ei_err)?;
            while let Some(r) = context.pending_event() {
                let event = match r {
                    PendingRequestResult::Request(e) => e,
                    PendingRequestResult::ParseError(e) => return Err(ei_err(e)),
                    PendingRequestResult::InvalidObject(_) => continue,
                };
                if let Some(resp) = handshaker.handle_event(event).map_err(ei_err)? {
                    break 'hs resp;
                }
            }
        };
        let conv = EiEventConverter::new(&context, resp);
        let mut session = EiSession {
            context,
            conv,
            keyboard: None,
            text: None,
            keymap: None,
            depressed: 0,
            locked: 0,
            group: 0,
            sequence: 0,
            disconnected: None,
        };
        // The read that completed the handshake may have carried the first
        // seat events too.
        session.process_pending()?;
        if !session.wait_for(SETUP_TIMEOUT, |s| {
            s.keyboard.as_ref().is_some_and(|k| k.resumed)
        })? {
            return Err(PlatformError::Ei(
                "the compositor offered no usable keyboard device".into(),
            ));
        }
        if session.keymap.is_none() {
            return Err(PlatformError::Ei("the compositor sent no keymap".into()));
        }
        Ok(session)
    }

    pub(crate) fn fd(&self) -> &ei::Context {
        &self.context
    }

    pub(crate) fn alive(&self) -> Result<(), PlatformError> {
        match &self.disconnected {
            Some(why) => Err(PlatformError::Ei(format!(
                "the compositor disconnected us: {why}"
            ))),
            None => Ok(()),
        }
    }

    /// Reads and handles whatever the compositor sent. Call when the fd is
    /// readable.
    pub(crate) fn dispatch(&mut self) -> Result<(), PlatformError> {
        if let Err(e) = self.context.read() {
            self.disconnected = Some(e.to_string());
            return self.alive();
        }
        self.process_pending()
    }

    /// Handles events already read into the buffer.
    fn process_pending(&mut self) -> Result<(), PlatformError> {
        while let Some(r) = self.context.pending_event() {
            match r {
                PendingRequestResult::Request(e) => self.conv.handle_event(e).map_err(ei_err)?,
                PendingRequestResult::ParseError(e) => return Err(ei_err(e)),
                PendingRequestResult::InvalidObject(id) => {
                    tracing::debug!(id, "libei event for unknown object")
                }
            }
        }
        while let Some(event) = self.conv.next_event() {
            self.handle(event);
        }
        // Answers to pings and seat binds.
        self.context.flush().map_err(ei_err)?;
        Ok(())
    }

    fn handle(&mut self, event: EiEvent) {
        match event {
            EiEvent::SeatAdded(e) => {
                e.seat
                    .bind_capabilities(DeviceCapability::Keyboard | DeviceCapability::Text);
            }
            EiEvent::DeviceAdded(e) => {
                let d = e.device;
                if d.has_capability(DeviceCapability::Keyboard) {
                    self.keymap = d.keymap().and_then(|k| load_keymap(&k.fd, k.size));
                    self.keyboard = Some(Dev {
                        device: d.clone(),
                        resumed: false,
                    });
                }
                if d.has_capability(DeviceCapability::Text) {
                    self.text = Some(Dev {
                        device: d,
                        resumed: false,
                    });
                }
            }
            EiEvent::DeviceRemoved(e) => {
                for slot in [&mut self.keyboard, &mut self.text] {
                    if slot.as_ref().is_some_and(|d| d.device == e.device) {
                        *slot = None;
                    }
                }
            }
            EiEvent::DeviceResumed(e) => self.set_resumed(&e.device, true),
            EiEvent::DevicePaused(e) => self.set_resumed(&e.device, false),
            EiEvent::KeyboardModifiers(e) => {
                self.depressed = e.depressed;
                self.locked = e.locked;
                self.group = e.group;
            }
            EiEvent::Disconnected(e) => {
                self.disconnected =
                    Some(e.explanation.unwrap_or_else(|| format!("{:?}", e.reason)));
            }
            _ => {}
        }
    }

    fn set_resumed(&mut self, device: &reis::event::Device, resumed: bool) {
        for d in [&mut self.keyboard, &mut self.text].into_iter().flatten() {
            if &d.device == device {
                d.resumed = resumed;
            }
        }
    }

    /// Dispatches events until `done` holds or `timeout` passes.
    fn wait_for(
        &mut self,
        timeout: Duration,
        done: impl Fn(&Self) -> bool,
    ) -> Result<bool, PlatformError> {
        let deadline = Instant::now() + timeout;
        loop {
            self.alive()?;
            if done(self) {
                return Ok(true);
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Ok(false);
            }
            if wait_readable(&[&self.context], Some(left.min(Duration::from_millis(20))))?[0] {
                self.dispatch()?;
            }
        }
    }

    /// Waits for physical Shift/Ctrl/Alt/Super to be released.
    pub(crate) fn wait_modifiers_released(
        &mut self,
        timeout: Duration,
    ) -> Result<bool, PlatformError> {
        let mask = self
            .keymap
            .as_ref()
            .map(shortcut_modifier_mask)
            .unwrap_or(0);
        self.wait_for(timeout, |s| s.depressed & mask == 0)
    }

    fn reverse_map(&self) -> Result<ReverseMap, PlatformError> {
        let keymap = self
            .keymap
            .as_ref()
            .ok_or_else(|| PlatformError::Ei("no keymap".into()))?;
        Ok(ReverseMap::build(keymap, self.locked, self.group))
    }

    /// Types `tokens`. Characters go through the keymap; when the layout
    /// lacks some and the compositor offers `ei_text` (libei 1.6+), those
    /// runs are sent as UTF-8 text instead.
    pub(crate) fn type_tokens(
        &mut self,
        tokens: &[Token],
        settings: &Settings,
    ) -> Result<Typed, PlatformError> {
        self.wait_for(SETUP_TIMEOUT, |s| {
            s.keyboard.as_ref().is_some_and(|k| k.resumed)
        })?;
        let map = self.reverse_map()?;
        let combos: Vec<Option<Combo>> = tokens
            .iter()
            .map(|&t| map.token(t, settings.newline))
            .collect();
        let text_ok = self.text.is_some();
        let missing = untypeable(tokens, |t| {
            map.token(*t, settings.newline).is_some() || (text_ok && matches!(t, Token::Char(_)))
        });
        if !missing.is_empty() {
            return Ok(Typed::Impossible {
                reason: format!(
                    "the active keyboard layout can't type {}",
                    describe_chars(&missing)
                ),
            });
        }
        let mut i = 0;
        let mut since_pause = 0;
        while i < tokens.len() {
            if since_pause >= settings.batch_chars {
                thread::sleep(settings.batch_delay);
                since_pause = 0;
                if let Err(e) = self.pump_nonblocking() {
                    return Ok(Typed::Partial {
                        delivered: i,
                        reason: e.to_string(),
                    });
                }
            }
            let sent = match &combos[i] {
                Some(combo) => self.send_keys(&combo_events(combo)).map(|()| 1),
                None => self.send_text_run(tokens, &combos, i, settings.batch_chars),
            };
            match sent {
                Ok(n) => {
                    i += n;
                    since_pause += n;
                }
                Err(e) if i == 0 => {
                    return Ok(Typed::Impossible {
                        reason: e.to_string(),
                    });
                }
                Err(e) => {
                    return Ok(Typed::Partial {
                        delivered: i,
                        reason: e.to_string(),
                    });
                }
            }
        }
        Ok(Typed::Done)
    }

    /// Sends a chord such as Ctrl+V.
    pub(crate) fn send_stroke(&mut self, s: Stroke) -> Result<Option<String>, PlatformError> {
        self.wait_for(SETUP_TIMEOUT, |s| {
            s.keyboard.as_ref().is_some_and(|k| k.resumed)
        })?;
        let Some(combo) = self.reverse_map()?.chord(s.keysym, s.shift, s.ctrl) else {
            return Ok(Some(
                "the active keyboard layout has no key for the paste shortcut".into(),
            ));
        };
        self.send_keys(&combo_events(&combo))?;
        Ok(None)
    }

    fn pump_nonblocking(&mut self) -> Result<(), PlatformError> {
        if wait_readable(&[&self.context], Some(Duration::ZERO))?[0] {
            self.dispatch()?;
        }
        self.alive()
    }

    fn emulating(
        &mut self,
        text_device: bool,
        body: impl FnOnce(&reis::event::Device, u32),
    ) -> Result<(), PlatformError> {
        self.alive()?;
        let dev = if text_device {
            &self.text
        } else {
            &self.keyboard
        };
        let dev = match dev {
            Some(d) if d.resumed => d.device.clone(),
            _ => {
                return Err(PlatformError::Ei(
                    "the compositor paused our input device".into(),
                ));
            }
        };
        let serial = self.conv.connection().serial();
        self.sequence = self.sequence.wrapping_add(1);
        dev.device().start_emulating(serial, self.sequence);
        body(&dev, serial);
        dev.device().stop_emulating(serial);
        self.context.flush().map_err(ei_err)
    }

    /// One frame per key transition, the way libei's own clients do it.
    fn send_keys(&mut self, events: &[(u32, bool)]) -> Result<(), PlatformError> {
        let keyboard = self
            .keyboard
            .as_ref()
            .and_then(|k| k.device.interface::<ei::Keyboard>())
            .ok_or_else(|| PlatformError::Ei("no keyboard interface".into()))?;
        self.emulating(false, |dev, serial| {
            for &(code, press) in events {
                let state = if press {
                    ei::keyboard::KeyState::Press
                } else {
                    ei::keyboard::KeyState::Released
                };
                keyboard.key(code, state);
                dev.device().frame(serial, now_micros());
            }
        })
    }

    /// Sends the run of keymap-less characters starting at `start` (up to
    /// `max` of them) through `ei_text`; returns how many it covered.
    fn send_text_run(
        &mut self,
        tokens: &[Token],
        combos: &[Option<Combo>],
        start: usize,
        max: usize,
    ) -> Result<usize, PlatformError> {
        let text_if = self
            .text
            .as_ref()
            .and_then(|t| t.device.interface::<ei::Text>())
            .ok_or_else(|| PlatformError::Ei("no text interface".into()))?;
        let mut run = String::new();
        let mut n = 0;
        for (t, c) in tokens[start..]
            .iter()
            .zip(&combos[start..])
            .take(max.max(1))
        {
            let (Token::Char(ch), None) = (t, c) else {
                break;
            };
            if run.len() + ch.len_utf8() > EI_TEXT_MAX_BYTES {
                break;
            }
            run.push(*ch);
            n += 1;
        }
        self.emulating(true, |dev, serial| {
            text_if.utf8(&run);
            dev.device().frame(serial, now_micros());
        })?;
        Ok(n)
    }
}

fn load_keymap(fd: &OwnedFd, size: u32) -> Option<xkb::Keymap> {
    let fd = fd.try_clone().ok()?;
    let ctx = xkb::Context::new(xkb::CONTEXT_NO_FLAGS);
    // SAFETY: the compositor's keymap fd is a sealed, read-only memfd of
    // `size` bytes, which is what `new_from_fd` maps.
    match unsafe {
        xkb::Keymap::new_from_fd(
            &ctx,
            fd,
            size as usize,
            xkb::KEYMAP_FORMAT_TEXT_V1,
            xkb::KEYMAP_COMPILE_NO_FLAGS,
        )
    } {
        Ok(Some(k)) => Some(k),
        other => {
            tracing::warn!(
                ok = other.is_ok(),
                "could not compile the compositor's keymap"
            );
            None
        }
    }
}
