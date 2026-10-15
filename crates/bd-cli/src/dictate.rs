//! `bd file` and `bd mic`: run the pipeline and deliver its text.

use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::thread;

use anyhow::{Context, Result, bail};
use bd_audio::{AudioError, Capture, CaptureConfig, CaptureEvent, downmix_into, resample_to_16k};
use bd_core::SAMPLE_RATE;
use bd_pipeline::{Event, Parts, Pipeline, PipelineConfig};
use bd_platform::{InjectorConfig, PlatformInjector};
use clap::Args;
use crossbeam_channel::Receiver;

use crate::setup::{self, CleanupArgs, StyleArg};

/// Options shared by `bd file` and `bd mic`.
#[derive(Debug, Args)]
pub struct DictateArgs {
    #[command(flatten)]
    pub cleanup: CleanupArgs,
    #[arg(long, value_enum, default_value_t)]
    pub style: StyleArg,
    /// Treat spoken "comma", "period", "question mark" etc. as marks.
    #[arg(long)]
    pub literal_punctuation: bool,
    /// Report each segment's transcript and timings on stderr.
    #[arg(short, long)]
    pub verbose: bool,
}

#[derive(Debug, Args)]
pub struct FileArgs {
    /// WAV files (any rate, any channel count). Each is its own passage.
    #[arg(required = true)]
    pub wavs: Vec<PathBuf>,
    #[command(flatten)]
    pub dictate: DictateArgs,
}

#[derive(Debug, Args)]
pub struct MicArgs {
    /// Input device name (see `bd devices`). Default: the system default.
    #[arg(long)]
    pub device: Option<String>,
    /// Type the text into the focused window as well as printing it.
    #[arg(long)]
    pub inject: bool,
    #[command(flatten)]
    pub dictate: DictateArgs,
}

fn spawn_pipeline(args: &DictateArgs) -> Result<Pipeline> {
    let store = bd_models::Store::from_env()?;
    let segmenter = setup::load_segmenter(&store)?;
    let asr = setup::load_asr(&store)?;
    let cleanup = setup::load_cleanup(&args.cleanup, &store)?;
    match &cleanup {
        Some(c) => eprintln!("cleanup: {}", c.description),
        None => eprintln!("cleanup: off (ASR text with spoken commands applied)"),
    }
    let parts = Parts {
        segmenter: Box::new(segmenter),
        asr: Box::new(asr),
        cleanup: cleanup.map(|c| Box::new(c.engine) as _),
    };
    let mut config = PipelineConfig { style: args.style.style(), ..PipelineConfig::default() };
    config.commands.literal_punctuation = args.literal_punctuation;
    Pipeline::spawn(parts, config).map_err(|e| anyhow::anyhow!(e))
}

/// Prints delivered text on stdout and everything else on stderr.
struct Reporter {
    verbose: bool,
}

impl Reporter {
    /// Handles one event; returns the text to deliver, if any.
    fn report(&self, event: &Event) -> Option<String> {
        match event {
            Event::Transcribed { id, text, audio, elapsed } if self.verbose => {
                eprintln!("[{id}] asr {:.2}s audio in {:.0} ms: {text:?}", audio.as_secs_f32(), elapsed.as_secs_f64() * 1e3);
            }
            Event::Text { id, text, commanded, cleanup } => {
                if self.verbose {
                    let ms = cleanup.map_or_else(|| "off".into(), |d| format!("{:.0} ms", d.as_secs_f64() * 1e3));
                    eprintln!("[{id}] cleanup {ms}: {commanded:?} -> {text:?}");
                }
                print!("{text}");
                let _ = std::io::stdout().flush();
                return Some(text.clone());
            }
            Event::Scratched { id } => eprintln!("[{id}] scratched"),
            Event::ScratchTooLate => eprintln!("scratch that: the previous segment was already delivered"),
            Event::SessionEnded => {
                println!();
            }
            Event::Error { stage, message } => eprintln!("{stage:?} error: {message}"),
            Event::Transcribed { .. } => {}
        }
        None
    }
}

pub fn file(args: &FileArgs) -> Result<()> {
    let audio: Vec<Vec<f32>> = args.wavs.iter().map(|p| read_wav(p)).collect::<Result<_>>()?;
    let pipeline = spawn_pipeline(&args.dictate)?;
    let reporter = Reporter { verbose: args.dictate.verbose };
    let chunk = SAMPLE_RATE as usize / 10;
    for (path, samples) in args.wavs.iter().zip(audio) {
        if args.wavs.len() > 1 {
            eprintln!("== {}", path.display());
        }
        pipeline.new_passage();
        for part in samples.chunks(chunk) {
            pipeline.push_audio(part.to_vec());
        }
        pipeline.end_session();
        for event in pipeline.events() {
            reporter.report(&event);
            if event == Event::SessionEnded {
                break;
            }
        }
    }
    for event in pipeline.finish().iter() {
        reporter.report(&event);
    }
    Ok(())
}

/// Reads a WAV as 16 kHz mono f32.
fn read_wav(path: &Path) -> Result<Vec<f32>> {
    let mut reader = hound::WavReader::open(path).with_context(|| format!("opening {}", path.display()))?;
    let spec = reader.spec();
    let interleaved: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Float => reader.samples::<f32>().collect::<Result<_, _>>()?,
        hound::SampleFormat::Int => {
            let scale = 1.0 / (1u64 << (spec.bits_per_sample - 1)) as f32;
            reader.samples::<i32>().map(|s| s.map(|v| v as f32 * scale)).collect::<Result<_, _>>()?
        }
    };
    let mut mono = Vec::with_capacity(interleaved.len() / usize::from(spec.channels.max(1)));
    downmix_into(&interleaved, usize::from(spec.channels), &mut mono);
    Ok(resample_to_16k(&mono, spec.sample_rate)?)
}

pub fn mic(args: &MicArgs) -> Result<()> {
    let config = CaptureConfig { device: args.device.clone(), ..CaptureConfig::default() };
    let mut capture = Capture::open(config)?;
    if capture.fell_back_to_default() {
        eprintln!("device {:?} not found; using the default", args.device.as_deref().unwrap_or_default());
    }
    eprintln!("microphone: {}", capture.device_name());
    let pipeline = spawn_pipeline(&args.dictate)?;
    let output = spawn_output(pipeline.events().clone(), args.dictate.verbose, args.inject)?;
    let forward = {
        let feeder = pipeline.feeder();
        let audio = capture.events().clone();
        thread::spawn(move || {
            for event in audio {
                match event {
                    CaptureEvent::Chunk(chunk) => feeder.push_audio(chunk.samples),
                    CaptureEvent::Stopped { dropped_frames } => {
                        if dropped_frames > 0 {
                            eprintln!("({dropped_frames} audio frames dropped)");
                        }
                        feeder.end_session();
                    }
                    CaptureEvent::Error(e @ AudioError::DeviceLost(_)) => {
                        eprintln!("microphone lost: {e}");
                        feeder.end_session();
                        return;
                    }
                    CaptureEvent::Error(e) => eprintln!("capture: {e}"),
                }
            }
        })
    };
    eprintln!("Enter: start/stop dictating   p+Enter: new passage   q+Enter: quit");
    let stdin = std::io::stdin();
    for line in stdin.lock().lines() {
        match line?.trim() {
            "" if capture.is_started() => {
                capture.stop()?;
                eprintln!("-- stopped");
            }
            "" => {
                if !capture.is_alive() {
                    bail!("the microphone is gone");
                }
                capture.start()?;
                eprintln!("-- listening (Enter to stop)");
            }
            "p" => {
                pipeline.new_passage();
                eprintln!("-- new passage");
            }
            "q" => break,
            other => eprintln!("unknown command {other:?}"),
        }
    }
    if capture.is_started() {
        capture.stop()?;
    }
    drop(capture);
    let _ = forward.join();
    pipeline.finish();
    let _ = output.join();
    Ok(())
}

/// Prints (and with `inject`, types) delivered text until the pipeline's
/// event stream closes. The injector lives on this thread, and asks for any
/// permission it needs before dictation starts.
fn spawn_output(events: Receiver<Event>, verbose: bool, inject: bool) -> Result<thread::JoinHandle<()>> {
    let (ready_tx, ready) = crossbeam_channel::bounded(1);
    let handle = thread::spawn(move || {
        let mut injector = if inject {
            let setup = PlatformInjector::new(InjectorConfig::default()).and_then(|mut i| i.prepare().map(|()| i));
            match setup {
                Ok(i) => {
                    let _ = ready_tx.send(Ok(i.backend_name()));
                    Some(i)
                }
                Err(e) => {
                    let _ = ready_tx.send(Err(e.to_string()));
                    return;
                }
            }
        } else {
            let _ = ready_tx.send(Ok("none"));
            None
        };
        let reporter = Reporter { verbose };
        for event in events {
            let Some(text) = reporter.report(&event) else { continue };
            if let Some(injector) = injector.as_mut() {
                match injector.inject_text(&text) {
                    Ok(outcome) if verbose => eprintln!("(injected: {outcome:?})"),
                    Ok(_) => {}
                    Err(e) => eprintln!("could not type the text: {e}"),
                }
            }
        }
    });
    match ready.recv() {
        Ok(Ok(backend)) if inject => eprintln!("typing into the focused window via {backend}"),
        Ok(Ok(_)) => {}
        Ok(Err(e)) => bail!("text injection unavailable: {e}"),
        Err(_) => bail!("the output thread stopped"),
    }
    Ok(handle)
}
