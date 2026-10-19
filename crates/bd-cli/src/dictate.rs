//! `bd file` and `bd mic`: run the pipeline and deliver its text.

use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use bd_audio::{AudioError, Capture, CaptureConfig, CaptureEvent, downmix_into, resample_to_16k};
use bd_core::SAMPLE_RATE;
use bd_pipeline::{Event, PadAction, Parts, Pipeline, PipelineConfig};
use bd_platform::{Erased, InjectorConfig, PlatformInjector};
use bd_vocab::Vocabulary;
use clap::{Args, ValueEnum};
use crossbeam_channel::Receiver;

use crate::setup::{self, CleanupArgs, StyleArg};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, ValueEnum)]
pub enum ParagraphsArg {
    /// Only dictated breaks.
    Off,
    /// The cleanup model starts a paragraph when a different character
    /// speaks.
    #[default]
    Dialog,
    /// Dialog paragraphs, and a paragraph after a long pause.
    Auto,
}

/// Options shared by `bd file` and `bd mic`.
#[derive(Debug, Args)]
pub struct DictateArgs {
    #[command(flatten)]
    pub cleanup: CleanupArgs,
    #[arg(long, value_enum, default_value_t)]
    pub style: StyleArg,
    /// Where new paragraphs come from besides "new paragraph": nowhere
    /// (off), a change of speaker in dialog (dialog), or that plus long
    /// pauses (auto).
    #[arg(long, value_enum, default_value_t)]
    pub paragraphs: ParagraphsArg,
    /// With --paragraphs auto: a pause this long, in seconds, starts a
    /// paragraph.
    #[arg(long, default_value_t = 3.0, value_name = "SECS")]
    pub pause_paragraph: f32,
    /// Treat spoken "comma", "period", "question mark" etc. as marks.
    #[arg(long)]
    pub literal_punctuation: bool,
    /// Your names and invented words, one per line, optionally with what
    /// the ASR hears instead: `wyrmstone: worm stone, wormstone`.
    #[arg(long, value_name = "FILE")]
    pub words: Option<PathBuf>,
    /// How strongly the ASR listens for --words (score per matching token).
    /// 0 keeps only the fix-up list. Higher finds more names but starts
    /// changing ordinary words (2.5 turned "whispered" into "whisper").
    #[arg(long, default_value_t = 2.0)]
    pub hotword_boost: f32,
    /// Report each segment's transcript and timings on stderr.
    #[arg(short, long)]
    pub verbose: bool,
    /// Bear Pad mode: text collects in the pad (each change on stderr with
    /// -v, raw parts in [brackets]) and is printed when it's sent: by
    /// "bear, send it", and for `bd file` at the end of each file.
    #[arg(long)]
    pub pad: bool,
    /// Stream, as the app does: Nemotron transcribes as you speak and the
    /// words are typed as they come (with --pad, shown in the pad); with a
    /// cleanup model each sentence is then tidied in place. The text as
    /// typed is printed on stderr at the end, since stdout can't take text
    /// back.
    #[arg(long)]
    pub stream: bool,
    /// With --stream and a cleanup model: type each sentence once, cleaned,
    /// instead of as spoken and then tidied in place.
    #[arg(long)]
    pub type_once: bool,
}

#[derive(Debug, Args)]
pub struct FileArgs {
    /// WAV files (any rate, any channel count). Each is its own passage.
    #[arg(required = true)]
    pub wavs: Vec<PathBuf>,
    /// Type the text into the focused window as well as printing it, as the
    /// app would (for testing typing into a given app).
    #[arg(long)]
    pub inject: bool,
    /// Feed the audio at speaking pace, as a microphone would, instead of
    /// as fast as it goes: for timing (with -v, each text's wait after its
    /// transcript).
    #[arg(long)]
    pub realtime: bool,
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
    let vocabulary = match &args.words {
        Some(path) => {
            let list = std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
            let v = Vocabulary::parse(&list).with_context(|| path.display().to_string())?;
            eprintln!("words: {} from {}", v.words().len(), path.display());
            v
        }
        None => Vocabulary::default(),
    };
    let store = bd_models::Store::from_env()?;
    let segmenter = setup::load_segmenter(&store)?;
    let asr = if args.stream {
        let asr = setup::load_streaming_asr(&store, &vocabulary.hotwords(), args.hotword_boost)?;
        bd_pipeline::Recognizer::Live(Box::new(asr))
    } else {
        bd_pipeline::Recognizer::Segments(Box::new(setup::load_asr(&store, &vocabulary.hotwords(), args.hotword_boost)?))
    };
    let cleanup = setup::load_cleanup(&args.cleanup, &store)?;
    let live_typing = !(args.type_once && cleanup.is_some());
    match &cleanup {
        Some(c) => eprintln!("cleanup: {}", c.description),
        None => eprintln!("cleanup: off (ASR text with spoken commands applied)"),
    }
    let parts = Parts {
        segmenter: Box::new(segmenter),
        asr,
        cleanup: cleanup.map(|c| Box::new(c.engine) as _),
        focus: None,
    };
    let mut style = args.style.style();
    style.dialog_paragraphs = args.paragraphs != ParagraphsArg::Off;
    let pause_paragraph = (args.paragraphs == ParagraphsArg::Auto)
        .then(|| Duration::try_from_secs_f32(args.pause_paragraph))
        .transpose()
        .context("--pause-paragraph must be a positive number of seconds")?;
    let mut config = PipelineConfig { style, vocabulary, pause_paragraph, pad: args.pad, live_typing, ..PipelineConfig::default() };
    config.commands.literal_punctuation = args.literal_punctuation;
    Pipeline::spawn(parts, config).map_err(|e| anyhow::anyhow!(e))
}

/// Prints delivered text on stdout and everything else on stderr.
struct Reporter {
    verbose: bool,
    /// Trailing spaces not yet printed: stdout can't take one back before
    /// a break or a closing mark, so they wait for the next text.
    pending: std::cell::Cell<usize>,
    /// The text as typed, take-backs and erasures applied.
    page: std::cell::RefCell<String>,
    /// Print the page at each session's end.
    show_page: bool,
    /// When each segment's transcript came, for the wait to its text.
    heard_at: std::cell::RefCell<std::collections::HashMap<u64, std::time::Instant>>,
}

/// What a pipeline event asks of the focused window.
enum Output {
    /// Take back this many characters, then type the text.
    Type(usize, String),
    Erase(String),
}

impl Reporter {
    /// Handles one event; returns what to do in the focused window, if
    /// anything.
    fn report(&self, event: &Event) -> Option<Output> {
        match event {
            Event::Text { take_back, text, .. } => {
                let mut page = self.page.borrow_mut();
                for _ in 0..*take_back {
                    page.pop();
                }
                page.push_str(text);
            }
            Event::Erase { text, .. } => {
                let mut page = self.page.borrow_mut();
                for _ in text.chars() {
                    page.pop();
                }
            }
            Event::SessionEnded if self.show_page => eprintln!("\n--- as typed ---\n{}\n---", self.page.borrow().trim_end()),
            _ => {}
        }
        match event {
            Event::Transcribed { id, text, audio, pause, elapsed, .. } if self.verbose => {
                self.heard_at.borrow_mut().insert(*id, std::time::Instant::now());
                let pause = pause.map_or(String::new(), |p| format!(" after {:.1}s pause", p.as_secs_f32()));
                eprintln!(
                    "[{id}] asr {:.2}s audio{pause} in {:.0} ms: {text:?}",
                    audio.as_secs_f32(),
                    elapsed.as_secs_f64() * 1e3
                );
            }
            Event::Text { id, take_back, text, commanded, cleanup } => {
                if self.verbose {
                    let ms = cleanup.map_or_else(|| "off".into(), |d| format!("{:.0} ms", d.as_secs_f64() * 1e3));
                    let wait = self.heard_at.borrow().get(id).map_or_else(String::new, |t| format!(" ({:.2}s after its transcript)", t.elapsed().as_secs_f64()));
                    eprintln!("[{id}] cleanup {ms}{wait}: {commanded:?} -> {text:?}");
                }
                let pending = self.pending.get().saturating_sub(*take_back);
                let body = text.trim_end_matches(' ');
                print!("{}{body}", " ".repeat(pending));
                self.pending.set(text.len() - body.len());
                let _ = std::io::stdout().flush();
                return Some(Output::Type(*take_back, text.clone()));
            }
            Event::Erase { id, text } => {
                // Stdout can't take text back; say what went (scratched, or
                // to be typed again: live text that changed, a tidied sentence).
                eprintln!("\n[{id}] taken back: {text:?}");
                return Some(Output::Erase(text.clone()));
            }
            Event::Scratched { id } => eprintln!("[{id}] scratched"),
            Event::Woke => eprintln!("[awake]"),
            Event::Slept => eprintln!("[asleep]"),
            Event::ScratchTooLate => eprintln!("scratch that: nothing left to take back in this dictation"),
            Event::SessionEnded => {
                println!();
            }
            Event::Error { stage, message } => eprintln!("{stage:?} error: {message}"),
            Event::Transcribed { .. } => {}
            Event::Pad(view) if self.verbose => {
                let text: String = view.parts.iter().map(|p| if p.clean { p.text.clone() } else { format!("[{}]", p.text) }).collect();
                eprintln!("[pad{}] {text:?}", if view.cleaning { ", cleaning" } else { "" });
            }
            Event::Pad(_) => {}
            Event::PadSend { text } => {
                println!("{text}");
                return Some(Output::Type(0, text.clone()));
            }
            Event::PadNotFound { words } => eprintln!("pad: {words:?} isn't in the pad"),
            // Diagnostics, for the app's telemetry.
            Event::Partial { .. } | Event::Commanded { .. } | Event::Cleaned { .. } => {}
        }
        None
    }
}

pub fn file(args: &FileArgs) -> Result<()> {
    let audio: Vec<Vec<f32>> = args.wavs.iter().map(|p| read_wav(p)).collect::<Result<_>>()?;
    let pipeline = spawn_pipeline(&args.dictate)?;
    let reporter = Reporter {
        verbose: args.dictate.verbose,
        pending: Default::default(),
        page: Default::default(),
        show_page: args.dictate.stream && !args.dictate.pad,
        heard_at: Default::default(),
    };
    let mut injector = if args.inject {
        let mut injector = PlatformInjector::new(InjectorConfig::default())?;
        injector.prepare()?;
        eprintln!("typing into the focused window via {}", injector.backend_name());
        Some(injector)
    } else {
        None
    };
    let mut handle = |event: &Event| {
        if let (Some(output), Some(injector)) = (reporter.report(event), injector.as_mut()) {
            deliver(injector, output, args.dictate.verbose);
        }
    };
    let chunk = SAMPLE_RATE as usize / 10;
    for (path, samples) in args.wavs.iter().zip(audio) {
        if args.wavs.len() > 1 {
            eprintln!("== {}", path.display());
        }
        pipeline.new_passage();
        for part in samples.chunks(chunk) {
            pipeline.push_audio(part.to_vec());
            if args.realtime {
                std::thread::sleep(Duration::from_millis(100));
                for event in pipeline.events().try_iter() {
                    handle(&event);
                }
            }
        }
        pipeline.end_session();
        for event in pipeline.events() {
            handle(&event);
            if event == Event::SessionEnded {
                break;
            }
        }
        if args.dictate.pad {
            pipeline.pad(PadAction::Send);
            for event in pipeline.events() {
                handle(&event);
                if matches!(&event, Event::Pad(v) if v.is_empty()) {
                    break;
                }
            }
        }
    }
    for event in pipeline.finish().iter() {
        handle(&event);
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

/// Types (or erases) one piece of delivered text.
fn deliver(injector: &mut PlatformInjector, output: Output, verbose: bool) {
    match output {
        Output::Type(take_back, text) => {
            if take_back > 0 {
                deliver(injector, Output::Erase(" ".repeat(take_back)), verbose);
            }
            match injector.inject_text(&text) {
            Ok(outcome) if verbose => eprintln!("(injected: {outcome:?})"),
            Ok(_) => {}
                Err(e) => eprintln!("could not type the text: {e}"),
            }
        }
        Output::Erase(text) => match injector.erase(&text) {
            Ok(Erased::Done) => {}
            Ok(Erased::Refused { reason }) => eprintln!("could not erase it: {reason}"),
            Err(e) => eprintln!("could not erase it: {e}"),
        },
    }
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
        let reporter = Reporter { verbose, pending: Default::default(), page: Default::default(), show_page: false, heard_at: Default::default() };
        for event in events {
            let Some(output) = reporter.report(&event) else { continue };
            let Some(injector) = injector.as_mut() else { continue };
            deliver(injector, output, verbose);
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
