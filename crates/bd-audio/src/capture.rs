//! The [`Capture`] handle: owns the cpal stream and the worker thread.

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::thread::JoinHandle;
use std::time::Duration;

use bd_core::SAMPLE_RATE;
use cpal::traits::{DeviceTrait, StreamTrait};
use cpal::{FromSample, SampleFormat, SizedSample};
use crossbeam_channel::{Receiver, Sender};

use crate::device::{Selected, select_input};
use crate::worker::{Control, Shared, Worker, WorkerParams};
use crate::{AudioError, CaptureEvent};

/// How long the backend may take to build a stream before we give up.
const BUILD_TIMEOUT: Duration = Duration::from_secs(5);
/// Queue depth for stream errors; more than this between worker polls are
/// dropped (they'd be repeats of the same failure).
const ERROR_QUEUE: usize = 16;

/// Whether the microphone stays open between dictation sessions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StreamPolicy {
    /// Open the stream at [`Capture::open`] and keep it running, so the
    /// pre-roll always holds the moments before [`Capture::start`]. The OS
    /// shows the mic as in use the whole time.
    #[default]
    KeepOpen,
    /// Open the stream at [`Capture::start`] and close it at
    /// [`Capture::stop`]. The mic is only "in use" while dictating, but
    /// there is no pre-roll and the device's start-up time can clip the
    /// first syllable.
    OpenOnStart,
}

/// How to capture.
#[derive(Debug, Clone)]
pub struct CaptureConfig {
    /// Input device name as reported by
    /// [`list_input_devices`](crate::list_input_devices); `None` means the
    /// system default.
    pub device: Option<String>,
    /// Use the default device when `device` names one that isn't present.
    pub fallback_to_default: bool,
    pub policy: StreamPolicy,
    /// Audio kept from before [`Capture::start`]. Default 300 ms.
    pub pre_roll: Duration,
    /// Size of each [`AudioChunk`](crate::AudioChunk). Default 32 ms, which
    /// is one Silero VAD window at 16 kHz.
    pub chunk: Duration,
    /// Capacity of the callback-to-worker ring. Default 2 s; it only fills if
    /// the worker stalls that long.
    pub ring: Duration,
}

impl Default for CaptureConfig {
    fn default() -> Self {
        Self {
            device: None,
            fallback_to_default: true,
            policy: StreamPolicy::KeepOpen,
            pre_roll: Duration::from_millis(300),
            chunk: Duration::from_millis(32),
            ring: Duration::from_secs(2),
        }
    }
}

/// The device's native stream format, which capture converts from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NativeFormat {
    pub sample_rate: u32,
    pub channels: u16,
    pub sample_format: SampleFormat,
}

/// A live microphone capture shaped for a toggle hotkey.
///
/// Audio arrives on [`events`](Self::events) as 16 kHz mono
/// [`AudioChunk`](crate::AudioChunk)s between [`start`](Self::start) and
/// [`stop`](Self::stop); each session ends with
/// [`CaptureEvent::Stopped`]. The audio callback only converts samples and
/// writes them into a preallocated lock-free ring; downmixing, resampling
/// and chunking happen on a worker thread.
///
/// If the device disappears, [`CaptureEvent::Error`] with
/// [`AudioError::DeviceLost`] is sent (plus `Stopped` if a session was
/// running) and further `start` calls fail; open a new `Capture` to recover.
pub struct Capture {
    selected: Selected,
    native: NativeFormat,
    config: CaptureConfig,
    shared: Arc<Shared>,
    control: Sender<Control>,
    events: Receiver<CaptureEvent>,
    stream_errors: Sender<cpal::Error>,
    stream: Option<cpal::Stream>,
    worker: Option<JoinHandle<()>>,
    armed: bool,
}

impl Capture {
    /// Selects the device, spawns the worker, and (for
    /// [`StreamPolicy::KeepOpen`]) starts the stream.
    pub fn open(config: CaptureConfig) -> Result<Self, AudioError> {
        let selected = select_input(config.device.as_deref(), config.fallback_to_default)?;
        let supported =
            selected
                .device
                .default_input_config()
                .map_err(|source| AudioError::Backend {
                    context: "querying input config",
                    source,
                })?;
        let native = NativeFormat {
            sample_rate: supported.sample_rate(),
            channels: supported.channels(),
            sample_format: supported.sample_format(),
        };
        tracing::info!(device = %selected.name, ?native, "opening capture");

        let shared = Arc::new(Shared::default());
        let (control, control_rx) = crossbeam_channel::unbounded();
        let (events_tx, events) = crossbeam_channel::unbounded();
        let (stream_errors, errors_rx) = crossbeam_channel::bounded(ERROR_QUEUE);
        let params = WorkerParams {
            channels: usize::from(native.channels),
            input_rate: native.sample_rate,
            chunk_samples: samples_16k(config.chunk).max(1),
            pre_roll_samples: samples_16k(config.pre_roll),
        };
        let worker = Worker::new(params, Arc::clone(&shared), events_tx, errors_rx)?;
        let handle = std::thread::Builder::new()
            .name("bd-audio-capture".into())
            .spawn(move || worker.run(control_rx))
            .map_err(AudioError::Thread)?;

        let mut capture = Self {
            selected,
            native,
            config,
            shared,
            control,
            events,
            stream_errors,
            stream: None,
            worker: Some(handle),
            armed: false,
        };
        if capture.config.policy == StreamPolicy::KeepOpen {
            capture.open_stream()?;
        }
        Ok(capture)
    }

    /// Begins a dictation session. Chunks start with up to the configured
    /// pre-roll of audio from just before this call. No-op if already
    /// started.
    pub fn start(&mut self) -> Result<(), AudioError> {
        if self.shared.dead.load(Ordering::Relaxed) {
            return Err(AudioError::DeviceGone);
        }
        if self.armed {
            return Ok(());
        }
        if self.stream.is_none() {
            self.open_stream()?;
        }
        self.send(Control::Start)?;
        self.armed = true;
        Ok(())
    }

    /// Ends the session. Returns immediately; the worker drains what the
    /// device delivered up to now, sends the final (possibly short) chunk,
    /// then [`CaptureEvent::Stopped`]. No-op if not started.
    pub fn stop(&mut self) -> Result<(), AudioError> {
        if !self.armed {
            return Ok(());
        }
        self.armed = false;
        if self.config.policy == StreamPolicy::OpenOnStart {
            // Dropping the stream stops the callback; the worker still owns
            // the ring and drains what's left when it handles `Stop`.
            self.stream = None;
        }
        self.send(Control::Stop)
    }

    /// Whether a session is running.
    pub fn is_started(&self) -> bool {
        self.armed
    }

    /// The receiving end for chunks, session ends and errors. Clones share
    /// one queue, so use a single consumer.
    pub fn events(&self) -> &Receiver<CaptureEvent> {
        &self.events
    }

    /// Latest input level (RMS, 0.0 to about 1.0), updated while idle too,
    /// for a "say something" meter.
    pub fn level(&self) -> f32 {
        self.shared.level()
    }

    /// Name of the device actually in use.
    pub fn device_name(&self) -> &str {
        &self.selected.name
    }

    /// True if the configured device was missing and the default was used.
    pub fn fell_back_to_default(&self) -> bool {
        self.selected.fell_back
    }

    /// The device's native format, before conversion.
    pub fn native_format(&self) -> NativeFormat {
        self.native
    }

    /// Frames dropped since open because the worker fell behind.
    pub fn dropped_frames(&self) -> u64 {
        self.shared.dropped_frames.load(Ordering::Relaxed)
    }

    /// False once the device has been lost.
    pub fn is_alive(&self) -> bool {
        !self.shared.dead.load(Ordering::Relaxed)
    }

    fn send(&self, msg: Control) -> Result<(), AudioError> {
        self.control.send(msg).map_err(|_| AudioError::WorkerGone)
    }

    /// Builds and plays a stream feeding a fresh ring, and hands the ring's
    /// consumer to the worker.
    fn open_stream(&mut self) -> Result<(), AudioError> {
        let ch = usize::from(self.native.channels);
        let cap = (samples_at(self.native.sample_rate, self.config.ring) * ch).max(ch * 1024);
        let (producer, consumer) = rtrb::RingBuffer::<f32>::new(cap);
        self.send(Control::Attach(consumer))?;
        let stream = build_stream(
            &self.selected.device,
            self.native,
            producer,
            Arc::clone(&self.shared),
            self.stream_errors.clone(),
        )?;
        stream.play().map_err(|source| AudioError::Backend {
            context: "starting input stream",
            source,
        })?;
        self.stream = Some(stream);
        Ok(())
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        self.stream = None;
        let _ = self.control.send(Control::Shutdown);
        if let Some(h) = self.worker.take() {
            let _ = h.join();
        }
    }
}

fn samples_at(rate: u32, d: Duration) -> usize {
    (d.as_secs_f64() * f64::from(rate)).round() as usize
}

fn samples_16k(d: Duration) -> usize {
    samples_at(SAMPLE_RATE, d)
}

fn build_stream(
    device: &cpal::Device,
    native: NativeFormat,
    producer: rtrb::Producer<f32>,
    shared: Arc<Shared>,
    errors: Sender<cpal::Error>,
) -> Result<cpal::Stream, AudioError> {
    let config = cpal::StreamConfig {
        channels: native.channels,
        sample_rate: native.sample_rate,
        buffer_size: cpal::BufferSize::Default,
    };
    let ch = usize::from(native.channels);
    macro_rules! build {
        ($t:ty) => {
            build_typed::<$t>(device, config, ch, producer, shared, errors)
        };
    }
    match native.sample_format {
        SampleFormat::F32 => build!(f32),
        SampleFormat::F64 => build!(f64),
        SampleFormat::I8 => build!(i8),
        SampleFormat::I16 => build!(i16),
        SampleFormat::I24 => build!(cpal::I24),
        SampleFormat::I32 => build!(i32),
        SampleFormat::I64 => build!(i64),
        SampleFormat::U8 => build!(u8),
        SampleFormat::U16 => build!(u16),
        SampleFormat::U24 => build!(cpal::U24),
        SampleFormat::U32 => build!(u32),
        SampleFormat::U64 => build!(u64),
        other => Err(AudioError::UnsupportedFormat(format!("{other:?}"))),
    }
}

fn build_typed<T>(
    device: &cpal::Device,
    config: cpal::StreamConfig,
    channels: usize,
    mut producer: rtrb::Producer<f32>,
    shared: Arc<Shared>,
    errors: Sender<cpal::Error>,
) -> Result<cpal::Stream, AudioError>
where
    T: SizedSample + Send + 'static,
    f32: FromSample<T>,
{
    // Realtime path: no locks, no allocation. Whole frames only, so the
    // worker's reads stay channel-aligned; overflow drops the newest frames.
    let data = move |input: &[T], _: &cpal::InputCallbackInfo| {
        let frames = input.len() / channels;
        let fit = (producer.slots() / channels).min(frames);
        if fit < frames {
            shared
                .dropped_frames
                .fetch_add((frames - fit) as u64, Ordering::Relaxed);
        }
        if fit == 0 {
            return;
        }
        if let Ok(chunk) = producer.write_chunk_uninit(fit * channels) {
            chunk.fill_from_iter(input.iter().map(|s| s.to_sample::<f32>()));
        }
    };
    let on_error = move |e: cpal::Error| {
        let _ = errors.try_send(e);
    };
    device
        .build_input_stream::<T, _, _>(config, data, on_error, Some(BUILD_TIMEOUT))
        .map_err(|source| AudioError::Backend {
            context: "building input stream",
            source,
        })
}
