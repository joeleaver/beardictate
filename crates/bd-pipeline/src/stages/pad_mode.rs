//! The finisher in Bear Pad mode ([`crate::pad`]): segments go into the
//! pad, and the cleanup model runs on the pad, on a thread of its own, so a
//! segment shows in the pad while the one before is being cleaned.

use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use bd_core::{Cleanup, CleanupRequest, Style};
use crossbeam_channel::{Receiver, Sender, never, select, unbounded};

use super::{Finisher, Heard, pad_voice};
use crate::compose::single_return_paragraphs;
use crate::pad::{Batch, Pad};
use crate::{Event, PadAction, Stage};

/// Clean the finished sentences once the speaker has paused this long...
const PAUSE: Duration = Duration::from_millis(1500);
/// ...and an unfinished one after this long.
const LONG_PAUSE: Duration = Duration::from_secs(5);
/// How long to wait when nothing is due.
const IDLE: Duration = Duration::from_secs(3600);

pub(super) struct PadMode {
    pad: Pad,
    /// `None` without a cleanup model: the pad is sent as dictated.
    cleaner: Option<Cleaner>,
    in_flight: Option<Batch>,
    last_heard: Instant,
}

impl PadMode {
    /// A batch to clean now, if one is due.
    fn due(&self) -> Option<Batch> {
        if self.cleaner.is_none() || self.in_flight.is_some() {
            return None;
        }
        match self.last_heard.elapsed() {
            idle if idle >= LONG_PAUSE => self.pad.next_batch(true),
            idle if idle >= PAUSE => self.pad.next_batch(false),
            _ => None,
        }
    }

    /// How long until a batch may come due.
    fn wait(&self) -> Duration {
        if self.cleaner.is_none() || self.in_flight.is_some() || !self.pad.has_raw() {
            return IDLE;
        }
        let idle = self.last_heard.elapsed();
        if idle < PAUSE { PAUSE - idle } else { LONG_PAUSE.saturating_sub(idle).max(Duration::from_millis(10)) }
    }
}

type Cleaned = (Result<String, String>, Duration);

/// Context, raw text, style and special words for one clean.
type Job = (String, String, Style, Vec<String>);

/// The cleanup model on its own thread.
struct Cleaner {
    jobs: Sender<Job>,
    results: Receiver<Cleaned>,
    thread: JoinHandle<Box<dyn Cleanup>>,
}

impl Cleaner {
    fn spawn(mut engine: Box<dyn Cleanup>) -> Option<Cleaner> {
        let (jobs, job_rx) = unbounded::<Job>();
        let (result_tx, results) = unbounded();
        let thread = thread::Builder::new()
            .name("bd-pad-clean".into())
            .spawn(move || {
                for (context, raw, style, names) in job_rx {
                    let started = Instant::now();
                    let request = CleanupRequest { context: &context, raw: &raw, style: &style, names: &names };
                    let result = engine.clean(&request).map_err(|e| e.to_string());
                    let _ = result_tx.send((result, started.elapsed()));
                }
                engine
            })
            .map_err(|e| tracing::error!("starting the pad's cleanup thread: {e}"))
            .ok()?;
        Some(Cleaner { jobs, results, thread })
    }

    /// Stops the thread (after the job it's on) and hands the model back.
    fn stop(self) -> Option<Box<dyn Cleanup>> {
        drop(self.jobs);
        self.thread.join().map_err(|_| tracing::error!("the pad's cleanup thread panicked")).ok()
    }
}

impl Finisher {
    /// Turns Bear Pad mode on or off. Turning it off drops what's in the pad.
    pub(super) fn set_pad(&mut self, on: bool) {
        match (on, self.pad.take()) {
            (true, None) => {
                let cleaner = self.cleanup.take().and_then(Cleaner::spawn);
                self.pad = Some(PadMode { pad: Pad::default(), cleaner, in_flight: None, last_heard: Instant::now() });
                self.pad_emit();
            }
            (true, Some(mode)) => self.pad = Some(mode),
            (false, Some(mode)) => {
                if !mode.pad.is_empty() {
                    tracing::warn!("Bear Pad turned off with text in it; dropped");
                }
                self.cleanup = mode.cleaner.and_then(Cleaner::stop);
                self.emit(Event::Pad(Default::default()));
            }
            (false, None) => {}
        }
    }

    /// One turn of the loop in pad mode. False when the input has closed.
    pub(super) fn pad_step(&mut self, input: &Receiver<Heard>) -> bool {
        // Stops, wakes, and segments queued before pad mode came on.
        while let Some(item) = self.queue.pop_front() {
            self.handle(item, input);
        }
        let Some(mode) = self.pad.as_mut() else { return true };
        if let Some(batch) = mode.due() {
            self.pad_start(batch);
        }
        let Some(mode) = self.pad.as_ref() else { return true };
        let results = mode.cleaner.as_ref().map_or_else(never, |c| c.results.clone());
        let wait = mode.wait();
        select! {
            recv(input) -> heard => match heard {
                Ok(heard) => self.take_in(heard),
                Err(_) => return false,
            },
            recv(results) -> cleaned => if let Ok(cleaned) = cleaned { self.pad_cleaned(cleaned) },
            default(wait) => {}
        }
        true
    }

    /// Splits a pad command off the end of a segment.
    pub(super) fn pad_command(&self, text: String) -> (String, Option<PadAction>) {
        match self.pad.as_ref().and_then(|_| pad_voice::heard(&text)) {
            Some(h) => (h.before, Some(h.action)),
            None => (text, None),
        }
    }

    pub(super) fn pad_push(&mut self, id: u64, text: String, quote_before: bool) {
        let Some(mode) = self.pad.as_mut() else { return };
        mode.pad.push(id, text, quote_before);
        mode.last_heard = Instant::now();
        self.pad_emit();
    }

    /// "Scratch that": see [`Pad::undo`].
    pub(super) fn pad_undo(&mut self) -> Option<bool> {
        let quote = self.pad.as_mut()?.pad.undo(self.quote_open);
        self.pad_emit();
        quote
    }

    pub(super) fn pad_action(&mut self, action: PadAction) {
        let Some(mode) = self.pad.as_mut() else { return };
        tracing::info!(?action, "pad");
        match action {
            PadAction::Send => return self.pad_send(),
            PadAction::Clear => {
                mode.pad.clear();
            }
            PadAction::Undo => match self.pad_undo() {
                Some(quote) => self.quote_open = quote,
                None => self.emit(Event::ScratchTooLate),
            },
            PadAction::Change { from, to } => {
                if !mode.pad.change(&from, &to) {
                    self.emit(Event::PadNotFound { words: from });
                }
            }
            PadAction::Delete(words) => {
                if !mode.pad.change(&words, "") {
                    self.emit(Event::PadNotFound { words });
                }
            }
        }
        self.pad_emit();
    }

    /// Cleans what's left, then delivers the pad and empties it.
    fn pad_send(&mut self) {
        loop {
            let Some(mode) = self.pad.as_mut() else { return };
            if mode.in_flight.is_some() {
                let cleaned = mode.cleaner.as_ref().and_then(|c| c.results.recv().ok());
                match cleaned {
                    Some(cleaned) => self.pad_cleaned(cleaned),
                    None => mode.in_flight = None,
                }
            } else if let Some(batch) = mode.cleaner.as_ref().and_then(|_| mode.pad.next_batch(true)) {
                self.pad_start(batch);
            } else {
                break;
            }
        }
        let Some(mode) = self.pad.as_mut() else { return };
        let text = mode.pad.take();
        self.quote_open = false;
        if !text.trim().is_empty() {
            let text = if self.blank_line_paragraphs { text } else { single_return_paragraphs(&text) };
            tracing::info!(chars = text.chars().count(), "sending the pad");
            self.emit(Event::PadSend { text });
        }
        self.pad_emit();
    }

    fn pad_start(&mut self, batch: Batch) {
        let (style, names) = (self.style.clone(), self.vocabulary.spellings());
        let Some(mode) = self.pad.as_mut() else { return };
        if !batch.raw.chars().any(char::is_alphanumeric) {
            // Only breaks or marks: nothing for the model to do.
            let raw = batch.raw.clone();
            mode.pad.apply(batch, &raw);
            return self.pad_emit();
        }
        let Some(cleaner) = mode.cleaner.as_ref() else { return };
        if cleaner.jobs.send((batch.context.clone(), batch.raw.clone(), style, names)).is_ok() {
            mode.in_flight = Some(batch);
        }
        self.pad_emit();
    }

    fn pad_cleaned(&mut self, (result, elapsed): Cleaned) {
        let Some(mode) = self.pad.as_mut() else { return };
        let Some(batch) = mode.in_flight.take() else { return };
        let raw = batch.raw.clone();
        let text = match result {
            Ok(text) => keep_leading_break(&raw, text),
            Err(e) => {
                self.emit(Event::Error { stage: Stage::Cleanup, message: e });
                raw.clone()
            }
        };
        let Some(mode) = self.pad.as_mut() else { return };
        let kept = mode.pad.apply(batch, &text);
        tracing::info!(ms = elapsed.as_millis(), chars = raw.chars().count(), kept, "pad cleaned");
        self.pad_emit();
    }

    fn pad_emit(&self) {
        let Some(mode) = self.pad.as_ref() else { return };
        let mut view = mode.pad.view(mode.cleaner.is_none());
        view.cleaning = mode.in_flight.is_some();
        self.emit(Event::Pad(view));
    }
}

/// The break a batch starts with, which a model may drop.
fn keep_leading_break(raw: &str, cleaned: String) -> String {
    let lead = &raw[..raw.len() - raw.trim_start_matches('\n').len()];
    if lead.is_empty() || cleaned.starts_with('\n') { cleaned } else { format!("{lead}{}", cleaned.trim_start()) }
}

