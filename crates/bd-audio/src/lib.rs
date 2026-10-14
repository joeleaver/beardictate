//! Microphone capture for beardictate: any device, any native format,
//! delivered as 16 kHz mono f32 chunks ready for the VAD.
//!
//! [`Capture`] is shaped for a toggle hotkey: [`Capture::start`] begins a
//! session (prefixed by a short pre-roll so the first syllable isn't
//! clipped), [`Capture::stop`] flushes it. Chunks arrive as
//! [`CaptureEvent`]s on a crossbeam channel.
//!
//! The pieces are usable on their own too: [`StreamResampler`] /
//! [`resample_to_16k`] for files, [`PreRoll`], [`downmix_into`] and [`rms`].

mod capture;
mod device;
mod dsp;
mod resample;
mod worker;

#[cfg(test)]
mod tests;

pub use capture::{Capture, CaptureConfig, NativeFormat, StreamPolicy};
pub use device::{InputDeviceInfo, list_input_devices};
pub use dsp::{PreRoll, downmix_into, rms};
pub use resample::{StreamResampler, resample_to_16k};

/// A run of captured audio at [`bd_core::SAMPLE_RATE`], mono.
#[derive(Debug, Clone)]
pub struct AudioChunk {
    pub samples: Vec<f32>,
    /// RMS level of `samples`, for an input meter.
    pub rms: f32,
    /// Index of `samples[0]` within the session, counting from the first
    /// pre-roll sample.
    pub offset: u64,
}

/// What a [`Capture`] sends.
#[derive(Debug)]
pub enum CaptureEvent {
    /// Audio from the running session.
    Chunk(AudioChunk),
    /// The session that [`Capture::stop`] ended is complete: every sample up
    /// to the stop has been sent.
    Stopped {
        /// Frames lost in this session because the worker fell behind.
        dropped_frames: u64,
    },
    /// Something went wrong with the stream. [`AudioError::DeviceLost`] is
    /// final; others are transient and capture continues.
    Error(AudioError),
}

/// Capture errors.
#[derive(Debug, thiserror::Error)]
pub enum AudioError {
    #[error("no input device is available")]
    NoInputDevice,
    #[error("input device {0:?} not found")]
    DeviceNotFound(String),
    #[error("{context}: {source}")]
    Backend {
        context: &'static str,
        #[source]
        source: cpal::Error,
    },
    #[error("input device was lost: {0}")]
    DeviceLost(#[source] cpal::Error),
    #[error("the input device was lost earlier; reopen capture")]
    DeviceGone,
    #[error("input stream error: {0}")]
    Stream(#[source] cpal::Error),
    #[error("unsupported sample format {0}")]
    UnsupportedFormat(String),
    #[error("resampler: {0}")]
    Resampler(String),
    #[error("could not spawn the capture thread: {0}")]
    Thread(#[source] std::io::Error),
    #[error("the capture worker thread has exited")]
    WorkerGone,
}
