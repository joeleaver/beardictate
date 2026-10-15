//! A few seconds after a microphone opens, how loud it was: logged, and a
//! notification when it delivered nothing at all (the wrong input or
//! channel, a muted interface), which otherwise looks like dictation being
//! ignored.

use std::time::{Duration, Instant};

use super::{Ui, Worker};

/// How long to listen before judging.
const LISTEN: Duration = Duration::from_secs(8);
/// Speech quieter than this (about -42 dBFS) is hard for the ASR, even
/// brought up: the microphone's own hiss comes up with it.
const QUIET: f32 = 0.008;
/// This many quiet sentences in a row earn a warning (once a run).
const QUIET_RUN: u8 = 3;

/// Below this the input is digital silence: no microphone noise floor is
/// this quiet (about -100 dBFS).
const SILENT: f32 = 1e-5;

pub(super) struct MicCheck {
    name: String,
    opened: Instant,
    peak: f32,
}

impl MicCheck {
    pub fn new(name: String) -> Self {
        Self { name, opened: Instant::now(), peak: 0.0 }
    }
}

impl<U: Ui> Worker<U> {
    /// Notes how loud a chunk was (its RMS), while a check is running.
    pub(super) fn note_level(&mut self, rms: f32) {
        if let Some(check) = self.mic_check.as_mut() {
            check.peak = check.peak.max(rms);
        }
    }

    /// On the engine's tick: judge the microphone once it has had time.
    pub(super) fn check_mic_level(&mut self) {
        let Some(check) = self.mic_check.as_ref() else { return };
        if check.opened.elapsed() < LISTEN {
            return;
        }
        let db = 20.0 * check.peak.max(1e-9).log10();
        tracing::info!(device = %check.name, "microphone level over its first seconds: {db:.0} dBFS");
        if check.peak < SILENT {
            self.notifier.notify(
                "mic-silent",
                "No sound from the microphone",
                &format!(
                    "{} is open but sends silence. Check its input, channel and gain, or pick another microphone in Settings.",
                    check.name
                ),
            );
        }
        self.mic_check = None;
    }

    /// Notes how loud a heard sentence was; after a few quiet ones in a
    /// row, suggests turning the microphone up.
    pub(super) fn note_speech_level(&mut self, level: f32, words: usize) {
        if words == 0 {
            return;
        }
        if level >= QUIET {
            self.quiet_run = 0;
            return;
        }
        self.quiet_run = self.quiet_run.saturating_add(1);
        if self.quiet_run == QUIET_RUN && !std::mem::replace(&mut self.quiet_warned, true) {
            let db = 20.0 * level.max(1e-9).log10();
            tracing::info!("speech keeps arriving quiet ({db:.0} dBFS)");
            self.notifier.notify(
                "mic-quiet",
                "Your microphone is quiet",
                "Words get missed or misheard. Turn up the microphone's input volume in your system's sound settings, or move closer to it.",
            );
        }
    }
}
