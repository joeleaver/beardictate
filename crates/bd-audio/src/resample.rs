//! Streaming mono resampling from a device's native rate to [`SAMPLE_RATE`].
//!
//! Wraps rubato's synchronous FFT resampler, which band-limits before
//! decimating, so content above the 8 kHz output Nyquist is filtered out
//! instead of aliasing back into the speech band the way naive sample
//! dropping would.

use bd_core::SAMPLE_RATE;
use rubato::audioadapter_buffers::direct::InterleavedSlice;
use rubato::{Fft, FixedSync, Indexing, Resampler};

use crate::AudioError;

/// Input frames per resampler call. About 21 ms at 48 kHz: small enough to
/// keep latency low, big enough for a decent FFT filter.
const CHUNK_FRAMES: usize = 1024;

/// Resamples a continuous mono stream to [`SAMPLE_RATE`].
///
/// Push input in any slice sizes; output appears as whole resampler chunks
/// complete, already trimmed of the resampler's startup delay so output
/// sample `n` lines up with input time `n / SAMPLE_RATE`. Call
/// [`flush`](Self::flush) at the end of a stream to drain the tail; after a
/// flush the resampler is ready for a fresh, unrelated stream.
pub struct StreamResampler {
    /// `None` when the input is already at [`SAMPLE_RATE`].
    inner: Option<FftState>,
    input_rate: u32,
}

struct FftState {
    fft: Fft<f32>,
    /// Input waiting for a full chunk.
    pending: Vec<f32>,
    /// Scratch output, sized for one chunk.
    scratch: Vec<f32>,
    /// Startup-delay frames still to discard from the output.
    delay_left: usize,
    /// Input frames seen since the last flush.
    in_total: u64,
    /// Output frames emitted since the last flush.
    out_total: u64,
}

impl StreamResampler {
    /// Creates a resampler from `input_rate` Hz to [`SAMPLE_RATE`].
    pub fn new(input_rate: u32) -> Result<Self, AudioError> {
        if input_rate == 0 {
            return Err(AudioError::Resampler("input sample rate is 0".into()));
        }
        if input_rate == SAMPLE_RATE {
            return Ok(Self {
                inner: None,
                input_rate,
            });
        }
        let fft = Fft::<f32>::new(
            input_rate as usize,
            SAMPLE_RATE as usize,
            CHUNK_FRAMES,
            1,
            FixedSync::Input,
        )
        .map_err(|e| AudioError::Resampler(e.to_string()))?;
        let scratch = vec![0.0; fft.output_frames_max()];
        let delay_left = fft.output_delay();
        let pending = Vec::with_capacity(fft.input_frames_max() * 2);
        Ok(Self {
            inner: Some(FftState {
                fft,
                pending,
                scratch,
                delay_left,
                in_total: 0,
                out_total: 0,
            }),
            input_rate,
        })
    }

    /// The input rate this resampler was built for.
    pub fn input_rate(&self) -> u32 {
        self.input_rate
    }

    /// Feeds `input` and appends any finished 16 kHz samples to `out`.
    pub fn push(&mut self, input: &[f32], out: &mut Vec<f32>) -> Result<(), AudioError> {
        let Some(st) = self.inner.as_mut() else {
            out.extend_from_slice(input);
            return Ok(());
        };
        st.in_total += input.len() as u64;
        st.pending.extend_from_slice(input);
        let mut consumed = 0;
        loop {
            let need = st.fft.input_frames_next();
            if st.pending.len() - consumed < need {
                break;
            }
            let (used, produced) = st.run(consumed, None)?;
            consumed += used;
            st.emit(produced, out);
        }
        st.pending.drain(..consumed);
        Ok(())
    }

    /// Drains everything still buffered, so the total output since the last
    /// flush is exactly `ceil(input_frames * 16000 / input_rate)` samples,
    /// then resets for a new stream.
    pub fn flush(&mut self, out: &mut Vec<f32>) -> Result<(), AudioError> {
        let Some(st) = self.inner.as_mut() else {
            return Ok(());
        };
        let expected = (st.in_total * u64::from(SAMPLE_RATE)).div_ceil(u64::from(self.input_rate));
        let start_len = out.len();
        // The leftover partial chunk, then zeros until the delay line empties.
        let mut partial = Some(st.pending.len());
        while st.out_total < expected {
            let (_, produced) = st.run(0, partial)?;
            st.pending.clear();
            partial = Some(0);
            st.emit(produced, out);
        }
        let excess = (st.out_total - expected) as usize;
        out.truncate(out.len() - excess.min(out.len() - start_len));
        st.pending.clear();
        st.fft.reset();
        st.delay_left = st.fft.output_delay();
        st.in_total = 0;
        st.out_total = 0;
        Ok(())
    }

    /// Discards all buffered state without producing output.
    pub fn reset(&mut self) {
        if let Some(st) = self.inner.as_mut() {
            st.pending.clear();
            st.fft.reset();
            st.delay_left = st.fft.output_delay();
            st.in_total = 0;
            st.out_total = 0;
        }
    }
}

impl FftState {
    /// Runs one resampler chunk reading `pending[offset..]`, optionally as a
    /// partial (zero-padded) chunk. Returns `(input used, output produced)`.
    fn run(&mut self, offset: usize, partial: Option<usize>) -> Result<(usize, usize), AudioError> {
        let avail = self.pending.len() - offset;
        let need = self.fft.input_frames_next();
        // The adapter must cover a full chunk even for a partial call; when
        // short, the resampler reads only `partial` frames from it.
        let in_slice: &[f32] = &self.pending[offset..];
        let zeros;
        let input = if avail >= need {
            in_slice
        } else {
            zeros = pad_to(in_slice, need);
            &zeros[..]
        };
        let adapter_in = InterleavedSlice::new(input, 1, need)
            .map_err(|e| AudioError::Resampler(e.to_string()))?;
        let out_frames = self.scratch.len();
        let mut adapter_out = InterleavedSlice::new_mut(&mut self.scratch[..], 1, out_frames)
            .map_err(|e| AudioError::Resampler(e.to_string()))?;
        let indexing = partial.map(|n| Indexing::new().partial_len(n));
        self.fft
            .process_into_buffer(&adapter_in, &mut adapter_out, indexing.as_ref())
            .map_err(|e| AudioError::Resampler(e.to_string()))
    }

    /// Appends `produced` scratch samples to `out`, minus any startup delay.
    fn emit(&mut self, produced: usize, out: &mut Vec<f32>) {
        let skip = self.delay_left.min(produced);
        self.delay_left -= skip;
        out.extend_from_slice(&self.scratch[skip..produced]);
        self.out_total += (produced - skip) as u64;
    }
}

/// Copies `src` into a new buffer of length `len`, zero-padding the tail.
/// Only used on the flush path, never per chunk.
fn pad_to(src: &[f32], len: usize) -> Vec<f32> {
    let mut v = vec![0.0; len];
    let n = src.len().min(len);
    v[..n].copy_from_slice(&src[..n]);
    v
}

/// Resamples a whole mono clip at `input_rate` to [`SAMPLE_RATE`].
///
/// Convenience for files and tests; live capture uses [`StreamResampler`].
pub fn resample_to_16k(samples: &[f32], input_rate: u32) -> Result<Vec<f32>, AudioError> {
    let mut r = StreamResampler::new(input_rate)?;
    let mut out = Vec::with_capacity(
        (samples.len() as u64 * u64::from(SAMPLE_RATE) / u64::from(input_rate)) as usize + 1,
    );
    r.push(samples, &mut out)?;
    r.flush(&mut out)?;
    Ok(out)
}
