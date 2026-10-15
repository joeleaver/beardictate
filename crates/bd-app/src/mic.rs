//! The microphone, or a WAV file standing in for it.
//!
//! `BD_TEST_WAV=<file>` makes every dictation "hear" that file, played in
//! real time from the start, then silence until stopped. It exists for
//! end-to-end tests in a nested session, where there is no microphone to
//! use (and where recording the room would be wrong).

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result};
use bd_audio::{AudioChunk, Capture, CaptureConfig, CaptureEvent, downmix_into, resample_to_16k};
use bd_core::SAMPLE_RATE;
use crossbeam_channel::{Receiver, Sender, unbounded};

pub const TEST_WAV_ENV: &str = "BD_TEST_WAV";

pub enum Mic {
    Device(Box<Capture>),
    File(FileMic),
}

impl Mic {
    pub fn open(config: CaptureConfig) -> Result<Mic> {
        if let Some(path) = std::env::var_os(TEST_WAV_ENV) {
            tracing::warn!(path = ?path, "{TEST_WAV_ENV} is set: dictating from a file, not the microphone");
            return Ok(Mic::File(FileMic::open(path.into())?));
        }
        Ok(Mic::Device(Box::new(Capture::open(config)?)))
    }

    pub fn name(&self) -> String {
        match self {
            Mic::Device(c) => c.device_name().to_string(),
            Mic::File(f) => f.path.display().to_string(),
        }
    }

    pub fn events(&self) -> Receiver<CaptureEvent> {
        match self {
            Mic::Device(c) => c.events().clone(),
            Mic::File(f) => f.events.clone(),
        }
    }

    pub fn is_alive(&self) -> bool {
        match self {
            Mic::Device(c) => c.is_alive(),
            Mic::File(_) => true,
        }
    }

    pub fn start(&mut self) -> Result<()> {
        match self {
            Mic::Device(c) => Ok(c.start()?),
            Mic::File(f) => {
                f.start();
                Ok(())
            }
        }
    }

    pub fn stop(&mut self) -> Result<()> {
        match self {
            Mic::Device(c) => Ok(c.stop()?),
            Mic::File(f) => {
                f.stop();
                Ok(())
            }
        }
    }
}

pub struct FileMic {
    path: PathBuf,
    samples: Arc<Vec<f32>>,
    tx: Sender<CaptureEvent>,
    events: Receiver<CaptureEvent>,
    playing: Option<Arc<AtomicBool>>,
}

impl FileMic {
    fn open(path: PathBuf) -> Result<FileMic> {
        let mut reader = hound::WavReader::open(&path).with_context(|| format!("opening {}", path.display()))?;
        let spec = reader.spec();
        let interleaved: Vec<f32> = match spec.sample_format {
            hound::SampleFormat::Float => reader.samples::<f32>().collect::<Result<_, _>>()?,
            hound::SampleFormat::Int => {
                let scale = 1.0 / (1u64 << (spec.bits_per_sample - 1)) as f32;
                reader.samples::<i32>().map(|s| s.map(|v| v as f32 * scale)).collect::<Result<_, _>>()?
            }
        };
        let mut mono = Vec::new();
        downmix_into(&interleaved, usize::from(spec.channels), &mut mono);
        let samples = resample_to_16k(&mono, spec.sample_rate)?;
        let (tx, events) = unbounded();
        Ok(FileMic { path, samples: Arc::new(samples), tx, events, playing: None })
    }

    fn start(&mut self) {
        let playing = Arc::new(AtomicBool::new(true));
        self.playing = Some(playing.clone());
        let (samples, tx) = (self.samples.clone(), self.tx.clone());
        thread::spawn(move || {
            let chunk = SAMPLE_RATE as usize / 10;
            let mut offset = 0usize;
            while playing.load(Ordering::Acquire) {
                let part: Vec<f32> = if offset < samples.len() {
                    samples[offset..(offset + chunk).min(samples.len())].to_vec()
                } else {
                    vec![0.0; chunk]
                };
                let len = part.len();
                let _ = tx.send(CaptureEvent::Chunk(AudioChunk { rms: 0.0, offset: offset as u64, samples: part }));
                offset += len;
                thread::sleep(Duration::from_millis(100));
            }
            let _ = tx.send(CaptureEvent::Stopped { dropped_frames: 0 });
        });
    }

    fn stop(&mut self) {
        if let Some(p) = self.playing.take() {
            p.store(false, Ordering::Release);
        }
    }
}
