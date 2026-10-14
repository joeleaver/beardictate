//! Unit tests that need no microphone, plus one ignored live test.

use std::f32::consts::TAU;
use std::sync::Arc;
use std::time::{Duration, Instant};

use bd_core::SAMPLE_RATE;

use crate::worker::{Control, Shared, Worker, WorkerParams};
use crate::*;

fn sine(freq: f32, rate: u32, secs: f32, amp: f32) -> Vec<f32> {
    let n = (rate as f32 * secs) as usize;
    (0..n)
        .map(|i| amp * (TAU * freq * i as f32 / rate as f32).sin())
        .collect()
}

/// Frequency from interpolated rising zero crossings.
fn measure_freq(x: &[f32], rate: u32) -> f32 {
    let mut crossings = Vec::new();
    for i in 1..x.len() {
        if x[i - 1] < 0.0 && x[i] >= 0.0 {
            let frac = -x[i - 1] / (x[i] - x[i - 1]);
            crossings.push((i - 1) as f32 + frac);
        }
    }
    let periods = (crossings.len() - 1) as f32;
    rate as f32 * periods / (crossings[crossings.len() - 1] - crossings[0])
}

/// The middle of a signal, away from start-up and tail transients.
fn middle(x: &[f32]) -> &[f32] {
    &x[x.len() / 5..x.len() * 4 / 5]
}

fn check_tone_survives(input_rate: u32) {
    let amp = 0.5;
    let input = sine(1000.0, input_rate, 1.0, amp);
    let out = resample_to_16k(&input, input_rate).unwrap();
    let expected_len = (input.len() as u64 * 16_000).div_ceil(u64::from(input_rate)) as usize;
    assert_eq!(out.len(), expected_len);
    let f = measure_freq(middle(&out), SAMPLE_RATE);
    assert!((f - 1000.0).abs() < 1.0, "{input_rate} Hz: measured {f} Hz");
    let level = rms(middle(&out));
    let want = amp / 2f32.sqrt();
    assert!(
        (level - want).abs() / want < 0.01,
        "{input_rate} Hz: rms {level}, want {want}"
    );
}

#[test]
fn resample_48k_keeps_frequency_and_amplitude() {
    check_tone_survives(48_000);
}

#[test]
fn resample_44k1_keeps_frequency_and_amplitude() {
    check_tone_survives(44_100);
}

#[test]
fn resample_filters_content_above_new_nyquist() {
    // Naive 3:1 decimation would fold 12 kHz to 4 kHz at full level.
    let input = sine(12_000.0, 48_000, 1.0, 0.5);
    let out = resample_to_16k(&input, 48_000).unwrap();
    let level = rms(middle(&out));
    assert!(
        level < 0.5 / 2f32.sqrt() * 0.01,
        "alias leaked at rms {level}"
    );
}

#[test]
fn resample_is_aligned_in_time() {
    // An impulse at 0.5 s should come out near sample 8000, not shifted by
    // the resampler delay.
    let mut input = vec![0.0f32; 48_000];
    input[24_000] = 1.0;
    let out = resample_to_16k(&input, 48_000).unwrap();
    let peak = out
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.abs().total_cmp(&b.1.abs()))
        .unwrap()
        .0;
    assert!(peak.abs_diff(8_000) <= 1, "peak at {peak}");
}

#[test]
fn streaming_matches_one_shot() {
    let input = sine(440.0, 48_000, 0.7, 0.3);
    let whole = resample_to_16k(&input, 48_000).unwrap();
    let mut r = StreamResampler::new(48_000).unwrap();
    let mut out = Vec::new();
    for piece in input.chunks(333) {
        r.push(piece, &mut out).unwrap();
    }
    r.flush(&mut out).unwrap();
    assert_eq!(out.len(), whole.len());
    let max_diff = out
        .iter()
        .zip(&whole)
        .map(|(a, b)| (a - b).abs())
        .fold(0.0, f32::max);
    assert!(max_diff < 1e-5, "max diff {max_diff}");
}

#[test]
fn resampler_reusable_after_flush() {
    let input = sine(1000.0, 48_000, 0.5, 0.5);
    let mut r = StreamResampler::new(48_000).unwrap();
    let mut first = Vec::new();
    r.push(&input, &mut first).unwrap();
    r.flush(&mut first).unwrap();
    let mut second = Vec::new();
    r.push(&input, &mut second).unwrap();
    r.flush(&mut second).unwrap();
    assert_eq!(first, second);
}

#[test]
fn resample_16k_is_passthrough() {
    let input = sine(1000.0, 16_000, 0.1, 0.5);
    assert_eq!(resample_to_16k(&input, 16_000).unwrap(), input);
}

#[test]
fn downmix_averages_channels() {
    let mut out = Vec::new();
    downmix_into(&[1.0, -1.0, 0.5, 0.25], 2, &mut out);
    assert_eq!(out, vec![0.0, 0.375]);
    out.clear();
    downmix_into(&[0.3, 0.3, 0.3, 0.9, 0.0, 0.0, 1.0], 3, &mut out);
    assert_eq!(out.len(), 2, "trailing partial frame is ignored");
    assert!((out[0] - 0.3).abs() < 1e-6 && (out[1] - 0.3).abs() < 1e-6);
    out.clear();
    downmix_into(&[0.1, 0.2], 1, &mut out);
    assert_eq!(out, vec![0.1, 0.2]);
}

#[test]
fn rms_of_sine_and_silence() {
    assert_eq!(rms(&[]), 0.0);
    assert_eq!(rms(&[0.0; 64]), 0.0);
    let s = sine(1000.0, 16_000, 1.0, 1.0);
    assert!((rms(&s) - std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-3);
}

#[test]
fn pre_roll_keeps_only_most_recent() {
    let mut p = PreRoll::new(4);
    p.push(&[1.0, 2.0, 3.0]);
    p.push(&[4.0, 5.0]);
    assert_eq!(p.len(), 4);
    let mut out = vec![0.0];
    p.drain_into(&mut out);
    assert_eq!(out, vec![0.0, 2.0, 3.0, 4.0, 5.0]);
    assert!(p.is_empty());

    p.push(&[1.0, 2.0, 3.0, 4.0, 5.0, 6.0]);
    let mut out = Vec::new();
    p.drain_into(&mut out);
    assert_eq!(out, vec![3.0, 4.0, 5.0, 6.0]);

    let mut off = PreRoll::new(0);
    off.push(&[1.0]);
    assert!(off.is_empty());
}

/// A worker fed through a real ring, as the audio callback would.
struct Harness {
    producer: rtrb::Producer<f32>,
    control: crossbeam_channel::Sender<Control>,
    events: crossbeam_channel::Receiver<CaptureEvent>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl Harness {
    fn new(rate: u32, channels: usize, pre_roll_samples: usize, chunk_samples: usize) -> Self {
        let (producer, consumer) = rtrb::RingBuffer::new(rate as usize * channels * 4);
        let (control, control_rx) = crossbeam_channel::unbounded();
        let (events_tx, events) = crossbeam_channel::unbounded();
        let (_err_tx, err_rx) = crossbeam_channel::bounded(1);
        let params = WorkerParams {
            channels,
            input_rate: rate,
            chunk_samples,
            pre_roll_samples,
        };
        let worker = Worker::new(params, Arc::new(Shared::default()), events_tx, err_rx).unwrap();
        control.send(Control::Attach(consumer)).unwrap();
        let handle = std::thread::spawn(move || worker.run(control_rx));
        Self {
            producer,
            control,
            events,
            handle: Some(handle),
        }
    }

    /// Writes samples and waits until the worker has consumed them.
    fn feed(&mut self, samples: &[f32]) {
        let chunk = self.producer.write_chunk_uninit(samples.len()).unwrap();
        chunk.fill_from_iter(samples.iter().copied());
        let deadline = Instant::now() + Duration::from_secs(5);
        while self.producer.slots() < self.producer.buffer().capacity() {
            assert!(Instant::now() < deadline, "worker did not drain the ring");
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    /// Collects chunks until `Stopped`.
    fn collect_session(&self) -> Vec<AudioChunk> {
        let mut chunks = Vec::new();
        loop {
            match self.events.recv_timeout(Duration::from_secs(5)).unwrap() {
                CaptureEvent::Chunk(c) => chunks.push(c),
                CaptureEvent::Stopped { dropped_frames } => {
                    assert_eq!(dropped_frames, 0);
                    return chunks;
                }
                CaptureEvent::Error(e) => panic!("unexpected error: {e}"),
            }
        }
    }
}

impl Drop for Harness {
    fn drop(&mut self) {
        let _ = self.control.send(Control::Shutdown);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

#[test]
fn session_starts_with_pre_roll_and_ends_at_stop() {
    // 16 kHz mono so samples pass through untouched and can be identified.
    let mut h = Harness::new(16_000, 1, 4_800, 512);
    let ramp: Vec<f32> = (0..20_000).map(|i| i as f32).collect();
    h.feed(&ramp[..16_000]); // idle: only the last 4800 should survive
    h.control.send(Control::Start).unwrap();
    // Wait for the first pre-roll chunk so the worker has armed before more
    // audio arrives (audio it drains earlier would rightly count as idle).
    let mut chunks = match h.events.recv_timeout(Duration::from_secs(5)).unwrap() {
        CaptureEvent::Chunk(c) => vec![c],
        other => panic!("expected a pre-roll chunk, got {other:?}"),
    };
    h.feed(&ramp[16_000..]);
    h.control.send(Control::Stop).unwrap();
    chunks.extend(h.collect_session());

    let all: Vec<f32> = chunks
        .iter()
        .flat_map(|c| c.samples.iter().copied())
        .collect();
    assert_eq!(all, ramp[16_000 - 4_800..].to_vec());
    // Fixed-size chunks except the last, with running offsets.
    let mut offset = 0;
    for (i, c) in chunks.iter().enumerate() {
        assert_eq!(c.offset, offset);
        if i + 1 < chunks.len() {
            assert_eq!(c.samples.len(), 512);
        }
        offset += c.samples.len() as u64;
    }

    // Audio between sessions isn't duplicated into the next pre-roll.
    h.control.send(Control::Start).unwrap();
    h.control.send(Control::Stop).unwrap();
    assert!(h.collect_session().is_empty());
    assert!(h.events.try_recv().is_err());
}

#[test]
fn stereo_48k_session_is_16k_mono() {
    // Pre-roll longer than the clip, so audio the worker drains before it
    // has processed `Start` is still part of the session.
    let mut h = Harness::new(48_000, 2, 32_000, 512);
    h.control.send(Control::Start).unwrap();
    let tone = sine(1000.0, 48_000, 1.0, 0.5);
    let stereo: Vec<f32> = tone.iter().flat_map(|&s| [s, s]).collect();
    for piece in stereo.chunks(960) {
        h.feed(piece);
    }
    h.control.send(Control::Stop).unwrap();
    let chunks = h.collect_session();
    let all: Vec<f32> = chunks
        .iter()
        .flat_map(|c| c.samples.iter().copied())
        .collect();
    assert_eq!(all.len(), 16_000);
    assert!((measure_freq(middle(&all), SAMPLE_RATE) - 1000.0).abs() < 1.0);
    let level = rms(middle(&all));
    assert!((level - 0.5 / 2f32.sqrt()).abs() < 0.01, "rms {level}");
}

/// Records 2 s from the default input. Run with `--ignored` on a machine
/// with a microphone.
#[test]
#[ignore = "needs a real microphone"]
fn live_record_two_seconds() {
    for d in list_input_devices().unwrap() {
        eprintln!(
            "input: {}{}",
            d.name,
            if d.is_default { " (default)" } else { "" }
        );
    }
    let mut cap = Capture::open(CaptureConfig::default()).unwrap();
    eprintln!("using {} {:?}", cap.device_name(), cap.native_format());
    cap.start().unwrap();
    std::thread::sleep(Duration::from_secs(2));
    cap.stop().unwrap();
    let mut samples = 0;
    let mut peak_rms = 0f32;
    loop {
        match cap.events().recv_timeout(Duration::from_secs(5)).unwrap() {
            CaptureEvent::Chunk(c) => {
                samples += c.samples.len();
                peak_rms = peak_rms.max(c.rms);
            }
            CaptureEvent::Stopped { dropped_frames } => {
                eprintln!("dropped {dropped_frames} frames");
                break;
            }
            CaptureEvent::Error(e) => panic!("{e}"),
        }
    }
    let secs = samples as f32 / SAMPLE_RATE as f32;
    eprintln!("captured {secs:.2} s, peak chunk rms {peak_rms:.4}");
    assert!((1.8..3.0).contains(&secs), "captured {secs} s");
}
