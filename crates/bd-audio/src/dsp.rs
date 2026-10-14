//! Small sample-level helpers: channel downmix, level metering, and the
//! pre-roll ring buffer.

use std::collections::VecDeque;

/// Averages interleaved frames of `channels` samples into mono, appending to
/// `out`. A trailing partial frame is ignored.
pub fn downmix_into(interleaved: &[f32], channels: usize, out: &mut Vec<f32>) {
    match channels {
        0 => {}
        1 => out.extend_from_slice(interleaved),
        n => {
            let scale = 1.0 / n as f32;
            out.extend(
                interleaved
                    .chunks_exact(n)
                    .map(|frame| frame.iter().sum::<f32>() * scale),
            );
        }
    }
}

/// Root-mean-square level of `samples` (0.0 for an empty slice). A full-scale
/// sine reads about 0.707.
pub fn rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let sum: f64 = samples.iter().map(|&s| f64::from(s) * f64::from(s)).sum();
    (sum / samples.len() as f64).sqrt() as f32
}

/// Fixed-capacity ring of the most recent mono samples.
///
/// While dictation is idle, capture keeps writing here so that when the user
/// presses start, the audio from just before the press (the first syllable
/// that raced the hotkey) is still available.
#[derive(Debug, Clone)]
pub struct PreRoll {
    buf: VecDeque<f32>,
    capacity: usize,
}

impl PreRoll {
    /// A ring holding at most `capacity` samples. Zero disables pre-roll.
    pub fn new(capacity: usize) -> Self {
        Self {
            buf: VecDeque::with_capacity(capacity),
            capacity,
        }
    }

    /// Maximum number of samples kept.
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// Number of samples currently held.
    pub fn len(&self) -> usize {
        self.buf.len()
    }

    pub fn is_empty(&self) -> bool {
        self.buf.is_empty()
    }

    /// Appends samples, discarding the oldest ones beyond capacity. Never
    /// reallocates once constructed.
    pub fn push(&mut self, samples: &[f32]) {
        if self.capacity == 0 {
            return;
        }
        let keep = samples.len().min(self.capacity);
        let samples = &samples[samples.len() - keep..];
        let overflow = (self.buf.len() + keep).saturating_sub(self.capacity);
        self.buf.drain(..overflow);
        self.buf.extend(samples.iter().copied());
    }

    /// Moves the held samples, oldest first, to the end of `out` and empties
    /// the ring.
    pub fn drain_into(&mut self, out: &mut Vec<f32>) {
        out.extend(self.buf.drain(..));
    }

    /// Discards all held samples.
    pub fn clear(&mut self) {
        self.buf.clear();
    }
}
