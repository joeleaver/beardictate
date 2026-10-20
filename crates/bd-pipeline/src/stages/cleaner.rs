//! The cleanup model on a thread of its own, so the finisher can go on
//! showing or typing words while a sentence is cleaned: for the Bear Pad,
//! and for text typed as it's spoken and tidied in place.

use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use bd_core::{Cleanup, CleanupRequest, Style};
use crossbeam_channel::{Receiver, Sender, unbounded};

pub(super) type Cleaned = (Result<String, String>, Duration, Option<bd_core::CleanupReport>);

/// Context, raw text, style and special words for one clean.
pub(super) type Job = (String, String, Style, Vec<String>);

/// The cleanup model on its own thread.
pub(super) struct Cleaner {
    pub jobs: Sender<Job>,
    pub results: Receiver<Cleaned>,
    thread: JoinHandle<Box<dyn Cleanup>>,
}

impl Cleaner {
    pub fn spawn(mut engine: Box<dyn Cleanup>) -> Option<Cleaner> {
        let (jobs, job_rx) = unbounded::<Job>();
        let (result_tx, results) = unbounded();
        let thread = thread::Builder::new()
            .name("bd-clean".into())
            .spawn(move || {
                for (context, raw, style, names) in job_rx {
                    let started = Instant::now();
                    let request = CleanupRequest { context: &context, raw: &raw, style: &style, names: &names };
                    let result = engine
                        .clean(&request)
                        .map(|text| bd_vocab::respell(&text, style.spelling, &names))
                        .map_err(|e| e.to_string());
                    let _ = result_tx.send((result, started.elapsed(), engine.last_report()));
                }
                engine
            })
            .map_err(|e| tracing::error!("starting the cleanup thread: {e}"))
            .ok()?;
        Some(Cleaner { jobs, results, thread })
    }

    /// Stops the thread (after the job it's on) and hands the model back.
    pub fn stop(self) -> Option<Box<dyn Cleanup>> {
        drop(self.jobs);
        self.thread.join().map_err(|_| tracing::error!("the cleanup thread panicked")).ok()
    }
}
