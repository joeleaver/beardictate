//! The capture worker thread: drains the lock-free ring the audio callback
//! fills, downmixes, resamples to 16 kHz, keeps the pre-roll, and sends
//! chunks out while dictation is armed.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::time::Duration;

use crossbeam_channel::{Receiver, RecvTimeoutError, Sender};

use crate::dsp::{PreRoll, downmix_into, rms};
use crate::resample::StreamResampler;
use crate::{AudioChunk, AudioError, CaptureEvent};

/// How often the worker polls the ring when no control message arrives.
const POLL: Duration = Duration::from_millis(10);

/// State shared between the [`Capture`](crate::Capture) handle, the audio
/// callback, and the worker. Everything is atomic: the callback never locks.
#[derive(Debug, Default)]
pub(crate) struct Shared {
    /// Latest input RMS as `f32` bits.
    pub level: AtomicU32,
    /// Frames the callback had to drop because the ring was full.
    pub dropped_frames: AtomicU64,
    /// Cleared once the device is lost; the capture can't be restarted.
    pub dead: AtomicBool,
}

impl Shared {
    pub fn level(&self) -> f32 {
        f32::from_bits(self.level.load(Ordering::Relaxed))
    }
}

/// Messages from the [`Capture`](crate::Capture) handle to the worker.
pub(crate) enum Control {
    /// A new stream was built; read its ring from now on.
    Attach(rtrb::Consumer<f32>),
    /// Begin a dictation session, prefixed by the pre-roll.
    Start,
    /// End the session: drain, flush, send the tail and `Stopped`.
    Stop,
    Shutdown,
}

/// Fixed parameters for one worker.
pub(crate) struct WorkerParams {
    pub channels: usize,
    pub input_rate: u32,
    pub chunk_samples: usize,
    pub pre_roll_samples: usize,
}

pub(crate) struct Worker {
    params: WorkerParams,
    shared: Arc<Shared>,
    events: Sender<CaptureEvent>,
    stream_errors: Receiver<cpal::Error>,
    ring: Option<rtrb::Consumer<f32>>,
    resampler: StreamResampler,
    pre_roll: PreRoll,
    armed: bool,
    /// 16 kHz samples not yet sent as a full chunk.
    pending: Vec<f32>,
    /// Session sample offset of `pending[0]`.
    session_offset: u64,
    dropped_at_start: u64,
    // Reused scratch buffers, so steady state doesn't allocate here either.
    interleaved: Vec<f32>,
    mono: Vec<f32>,
    out16: Vec<f32>,
}

impl Worker {
    pub fn new(
        params: WorkerParams,
        shared: Arc<Shared>,
        events: Sender<CaptureEvent>,
        stream_errors: Receiver<cpal::Error>,
    ) -> Result<Self, AudioError> {
        let resampler = StreamResampler::new(params.input_rate)?;
        let pre_roll = PreRoll::new(params.pre_roll_samples);
        let pending = Vec::with_capacity(params.chunk_samples * 4 + params.pre_roll_samples);
        Ok(Self {
            params,
            shared,
            events,
            stream_errors,
            ring: None,
            resampler,
            pre_roll,
            armed: false,
            pending,
            session_offset: 0,
            dropped_at_start: 0,
            interleaved: Vec::new(),
            mono: Vec::new(),
            out16: Vec::new(),
        })
    }

    /// Runs until `Shutdown` or until the handle is dropped.
    pub fn run(mut self, control: Receiver<Control>) {
        loop {
            match control.recv_timeout(POLL) {
                Ok(Control::Attach(ring)) => {
                    self.ring = Some(ring);
                    self.resampler.reset();
                }
                Ok(Control::Start) => self.start(),
                Ok(Control::Stop) => self.stop(),
                Ok(Control::Shutdown) | Err(RecvTimeoutError::Disconnected) => break,
                Err(RecvTimeoutError::Timeout) => {}
            }
            self.check_stream_errors();
            self.drain_ring();
        }
    }

    fn start(&mut self) {
        if self.armed {
            return;
        }
        self.drain_ring();
        self.armed = true;
        self.pending.clear();
        self.session_offset = 0;
        self.dropped_at_start = self.shared.dropped_frames.load(Ordering::Relaxed);
        self.pre_roll.drain_into(&mut self.pending);
        self.send_full_chunks();
    }

    fn stop(&mut self) {
        if !self.armed {
            return;
        }
        self.drain_ring();
        let mut tail = std::mem::take(&mut self.out16);
        tail.clear();
        if let Err(e) = self.resampler.flush(&mut tail) {
            self.send(CaptureEvent::Error(e));
        }
        self.pending.extend_from_slice(&tail);
        self.out16 = tail;
        self.send_full_chunks();
        if !self.pending.is_empty() {
            let rest = std::mem::take(&mut self.pending);
            self.send_chunk(rest);
        }
        let dropped = self.shared.dropped_frames.load(Ordering::Relaxed) - self.dropped_at_start;
        if dropped > 0 {
            tracing::warn!(dropped, "capture ring overflowed; frames were dropped");
        }
        self.send(CaptureEvent::Stopped {
            dropped_frames: dropped,
        });
        self.armed = false;
        self.pre_roll.clear();
    }

    /// Forwards stream errors; device loss ends any running session.
    fn check_stream_errors(&mut self) {
        while let Ok(err) = self.stream_errors.try_recv() {
            let fatal = matches!(
                err.kind(),
                cpal::ErrorKind::DeviceNotAvailable
                    | cpal::ErrorKind::StreamInvalidated
                    | cpal::ErrorKind::HostUnavailable
            );
            match err.kind() {
                cpal::ErrorKind::Xrun => tracing::debug!("input xrun"),
                cpal::ErrorKind::DeviceChanged => {
                    tracing::info!("input rerouted to the new default device")
                }
                _ if fatal => {
                    self.shared.dead.store(true, Ordering::Relaxed);
                    self.send(CaptureEvent::Error(AudioError::DeviceLost(err)));
                    // Deliver what was captured before the loss.
                    self.stop();
                }
                _ => self.send(CaptureEvent::Error(AudioError::Stream(err))),
            }
        }
    }

    /// Moves everything the callback has written so far through downmix and
    /// resampling into either the pre-roll or the outgoing chunks.
    fn drain_ring(&mut self) {
        let Some(ring) = self.ring.as_mut() else {
            return;
        };
        let ch = self.params.channels.max(1);
        let n = ring.slots() / ch * ch;
        if n == 0 {
            return;
        }
        let Ok(chunk) = ring.read_chunk(n) else {
            return;
        };
        let (a, b) = chunk.as_slices();
        self.interleaved.clear();
        self.interleaved.extend_from_slice(a);
        self.interleaved.extend_from_slice(b);
        chunk.commit_all();

        self.mono.clear();
        downmix_into(&self.interleaved, ch, &mut self.mono);
        self.out16.clear();
        if let Err(e) = self.resampler.push(&self.mono, &mut self.out16) {
            self.send(CaptureEvent::Error(e));
            return;
        }
        if self.out16.is_empty() {
            return;
        }
        if self.armed {
            self.pending.extend_from_slice(&self.out16);
            self.send_full_chunks();
        } else {
            self.shared
                .level
                .store(rms(&self.out16).to_bits(), Ordering::Relaxed);
            self.pre_roll.push(&self.out16);
        }
    }

    fn send_full_chunks(&mut self) {
        let size = self.params.chunk_samples;
        let full = self.pending.len() / size * size;
        if full == 0 {
            return;
        }
        // Collect first: `send_chunk` borrows `self` mutably.
        let chunks: Vec<Vec<f32>> = self.pending[..full]
            .chunks(size)
            .map(<[f32]>::to_vec)
            .collect();
        self.pending.drain(..full);
        for c in chunks {
            self.send_chunk(c);
        }
    }

    fn send_chunk(&mut self, samples: Vec<f32>) {
        let level = rms(&samples);
        self.shared.level.store(level.to_bits(), Ordering::Relaxed);
        let offset = self.session_offset;
        self.session_offset += samples.len() as u64;
        self.send(CaptureEvent::Chunk(AudioChunk {
            samples,
            rms: level,
            offset,
        }));
    }

    fn send(&self, event: CaptureEvent) {
        // The receiver lives in the `Capture` handle (and its clones); if all
        // are gone, nobody is listening and dropping the event is correct.
        let _ = self.events.send(event);
    }
}
