//! Utterance segmentation with Silero VAD (via sherpa-onnx).

use std::path::Path;
use std::time::Duration;

use bd_core::{AudioSegment, SAMPLE_RATE};
use sherpa_onnx::{SileroVadModelConfig, VadModelConfig, VoiceActivityDetector};

use crate::{AsrError, existing};

/// Silero's window at 16 kHz. Audio is fed to the VAD in these steps.
const WINDOW: usize = 512;
/// Frame used to find the quietest point for a forced cut (20 ms).
const QUIET_FRAME: usize = 320;
/// Extra audio kept behind a running utterance, beyond the pre-pad, in
/// case the VAD dates the speech start earlier than we estimate it.
const KEEP_MARGIN: usize = SAMPLE_RATE as usize;

/// Tuning for [`Segmenter`].
#[derive(Debug, Clone)]
pub struct SegmenterConfig {
    /// Speech probability above which a window counts as speech.
    pub threshold: f32,
    /// Silence that closes an utterance. Default 600 ms.
    pub min_silence: Duration,
    /// Shortest burst that counts as speech. Default 250 ms.
    pub min_speech: Duration,
    /// Longest segment. A run-on longer than this is cut at its quietest
    /// moment in the second half. Default 20 s.
    pub max_segment: Duration,
    /// Audio kept before the detected speech start and after its end, so
    /// soft onsets and trailing consonants reach the ASR. Default 200 ms.
    pub padding: Duration,
    pub threads: usize,
    pub provider: String,
}

impl Default for SegmenterConfig {
    fn default() -> Self {
        Self {
            threshold: 0.5,
            min_silence: Duration::from_millis(600),
            min_speech: Duration::from_millis(250),
            max_segment: Duration::from_secs(20),
            padding: Duration::from_millis(200),
            threads: 1,
            provider: "cpu".into(),
        }
    }
}

/// Cuts a stream of 16 kHz mono audio into utterances.
///
/// Feed chunks of any size with [`accept`](Self::accept); finished
/// utterances come back as [`AudioSegment`]s numbered from 0. When the user
/// stops dictating, [`flush`](Self::flush) returns the utterance in progress
/// and starts a new session.
///
/// Segments are cut from our own copy of the audio, not the VAD's: they
/// carry [`padding`](SegmenterConfig::padding) on both sides (never
/// overlapping a previous segment), and a forced cut at
/// [`max_segment`](SegmenterConfig::max_segment) loses no audio.
pub struct Segmenter {
    vad: VoiceActivityDetector,
    config: SegmenterConfig,
    /// Retained audio; `buf[0]` is stream sample `base`.
    buf: Vec<f32>,
    base: u64,
    /// Stream samples fed to the VAD so far.
    pos: u64,
    /// Stream index of the current VAD instance's sample 0.
    vad_origin: u64,
    /// Input not yet making up a full VAD window.
    partial: Vec<f32>,
    /// Nothing before this stream index is emitted again.
    emitted_until: u64,
    /// Estimated stream index where the current utterance started.
    run_start: Option<u64>,
    /// Stream index of the last window the VAD considered speech.
    last_speech: u64,
    /// Set by a forced cut: where its continuation must start.
    carry_from: Option<u64>,
    seq: u64,
}

impl Segmenter {
    /// Loads Silero VAD from `model` (the `silero_vad.onnx` file).
    pub fn new(model: &Path, config: SegmenterConfig) -> Result<Self, AsrError> {
        if config.max_segment <= config.min_silence * 2 {
            return Err(AsrError::InvalidConfig(
                "max_segment must be well above min_silence".into(),
            ));
        }
        let vad_config = VadModelConfig {
            silero_vad: SileroVadModelConfig {
                model: Some(existing(model)?),
                threshold: config.threshold,
                min_silence_duration: config.min_silence.as_secs_f32(),
                min_speech_duration: config.min_speech.as_secs_f32(),
                window_size: WINDOW as i32,
                // We cut long runs ourselves, at the quietest point; keep the
                // VAD's own limit out of the way.
                max_speech_duration: config.max_segment.as_secs_f32() * 2.0,
            },
            sample_rate: SAMPLE_RATE as i32,
            num_threads: i32::try_from(config.threads.max(1))
                .map_err(|_| AsrError::InvalidConfig("too many threads".into()))?,
            provider: Some(config.provider.clone()),
            debug: false,
            ..Default::default()
        };
        let buffer_secs = config.max_segment.as_secs_f32() * 2.0 + 10.0;
        let vad = VoiceActivityDetector::create(&vad_config, buffer_secs)
            .ok_or(AsrError::CreateFailed("Silero VAD"))?;
        Ok(Self {
            vad,
            config,
            buf: Vec::new(),
            base: 0,
            pos: 0,
            vad_origin: 0,
            partial: Vec::with_capacity(WINDOW),
            emitted_until: 0,
            run_start: None,
            last_speech: 0,
            carry_from: None,
            seq: 0,
        })
    }

    /// Feeds 16 kHz mono samples; returns any utterances that ended.
    pub fn accept(&mut self, samples: &[f32]) -> Vec<AudioSegment> {
        let mut out = Vec::new();
        let mut rest = samples;
        while !rest.is_empty() {
            let take = (WINDOW - self.partial.len()).min(rest.len());
            self.partial.extend_from_slice(&rest[..take]);
            rest = &rest[take..];
            if self.partial.len() == WINDOW {
                let window = std::mem::take(&mut self.partial);
                self.feed_window(&window, &mut out);
                self.partial = window;
                self.partial.clear();
            }
        }
        self.trim();
        out
    }

    /// Ends the session: returns the utterance in progress (if any speech
    /// was detected) and resets, so the next segment is `seq` 0 again.
    pub fn flush(&mut self) -> Vec<AudioSegment> {
        let mut out = Vec::new();
        if !self.partial.is_empty() {
            let tail = std::mem::take(&mut self.partial);
            self.buf.extend_from_slice(&tail);
            self.vad.accept_waveform(&tail);
            self.pos += tail.len() as u64;
        }
        self.vad.flush();
        self.collect(&mut out);
        self.reset();
        out
    }

    /// Discards all state without emitting anything.
    pub fn reset(&mut self) {
        self.vad.reset();
        self.buf.clear();
        self.partial.clear();
        self.base = 0;
        self.pos = 0;
        self.vad_origin = 0;
        self.emitted_until = 0;
        self.run_start = None;
        self.last_speech = 0;
        self.carry_from = None;
        self.seq = 0;
    }

    fn feed_window(&mut self, window: &[f32], out: &mut Vec<AudioSegment>) {
        self.buf.extend_from_slice(window);
        self.vad.accept_waveform(window);
        self.pos += window.len() as u64;
        let emitted = self.collect(out);
        if self.vad.detected() {
            self.last_speech = self.pos;
            let lead = (dur_samples(self.config.min_speech) + 2 * WINDOW) as u64;
            let start = *self.run_start.get_or_insert(self.pos.saturating_sub(lead));
            let seg_start = start
                .saturating_sub(dur_samples(self.config.padding) as u64)
                .max(self.emitted_until)
                .max(self.base);
            if self.pos - seg_start >= dur_samples(self.config.max_segment) as u64 {
                self.forced_cut(seg_start, out);
            }
        } else if self.run_start.is_some() {
            // Keep the utterance's audio until the VAD has delivered it. A
            // blip it never turns into a segment is released once it's
            // clearly over.
            let linger = (dur_samples(self.config.min_silence) + KEEP_MARGIN) as u64;
            if emitted || self.pos - self.last_speech > linger {
                self.run_start = None;
                self.carry_from = None;
            }
        }
    }

    /// Emits every segment the VAD has finished; true if there were any.
    fn collect(&mut self, out: &mut Vec<AudioSegment>) -> bool {
        let before = out.len();
        let pad = dur_samples(self.config.padding) as u64;
        while let Some(seg) = self.vad.front() {
            let start = self.vad_origin + seg.start().max(0) as u64;
            let end = start + seg.n().max(0) as u64;
            self.vad.pop();
            let mut from = start
                .saturating_sub(pad)
                .max(self.emitted_until)
                .max(self.base);
            // The continuation of a forced cut picks up exactly at the cut,
            // unless the VAD found it so far on that it's really a new
            // utterance.
            if let Some(cut) = self.carry_from.take() {
                let near = (dur_samples(self.config.min_silence) as u64) + pad;
                if start <= cut + near {
                    from = cut.max(self.base);
                }
            }
            let to = (end + pad).min(self.pos);
            if to > from {
                self.emit(from, to, out);
            }
        }
        out.len() > before
    }

    /// Cuts the running utterance `[seg_start, pos)` at the quietest frame
    /// of its second half, emits the first part, and restarts the VAD on the
    /// remainder so detection continues seamlessly.
    fn forced_cut(&mut self, seg_start: u64, out: &mut Vec<AudioSegment>) {
        let search_from = seg_start + (self.pos - seg_start) / 2;
        let cut = self.quietest_point(search_from, self.pos);
        tracing::debug!(seg_start, cut, "forcing a cut in a long utterance");
        self.emit(seg_start, cut, out);
        self.vad.reset();
        self.vad_origin = cut;
        let rest = &self.buf[(cut - self.base) as usize..];
        self.vad.accept_waveform(rest);
        self.run_start = Some(cut);
        self.carry_from = Some(cut);
        // Anything the replay finished (speech that ended right after the
        // cut) goes out now.
        self.collect(out);
        self.last_speech = self.pos;
    }

    /// Centre of the lowest-energy frame in `[from, to)`.
    fn quietest_point(&self, from: u64, to: u64) -> u64 {
        let a = (from - self.base) as usize;
        let b = (to - self.base) as usize;
        let region = &self.buf[a..b];
        let step = QUIET_FRAME / 2;
        let mut best = (f32::INFINITY, region.len() / 2);
        let mut i = 0;
        while i + QUIET_FRAME <= region.len() {
            let e: f32 = region[i..i + QUIET_FRAME].iter().map(|s| s * s).sum();
            if e < best.0 {
                best = (e, i + QUIET_FRAME / 2);
            }
            i += step;
        }
        from + best.1 as u64
    }

    fn emit(&mut self, from: u64, to: u64, out: &mut Vec<AudioSegment>) {
        let a = (from - self.base) as usize;
        let b = (to - self.base) as usize;
        out.push(AudioSegment {
            samples: self.buf[a..b].to_vec(),
            seq: self.seq,
        });
        self.seq += 1;
        self.emitted_until = to;
    }

    /// Drops retained audio that can no longer be part of a segment.
    fn trim(&mut self) {
        let pad = dur_samples(self.config.padding) as u64;
        let anchor = self.run_start.unwrap_or(self.pos);
        let keep_from = anchor
            .saturating_sub(pad + KEEP_MARGIN as u64 + dur_samples(self.config.min_speech) as u64)
            .max(self.emitted_until.min(self.pos));
        let excess = keep_from.saturating_sub(self.base) as usize;
        // Batch the memmove: only trim once a second's worth is dead.
        if excess >= SAMPLE_RATE as usize {
            self.buf.drain(..excess);
            self.base += excess as u64;
        }
    }
}

fn dur_samples(d: Duration) -> usize {
    (d.as_secs_f64() * f64::from(SAMPLE_RATE)).round() as usize
}
