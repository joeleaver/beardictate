//! Progress reporting and cancellation shared by download, verify and import.
//!
//! Every long operation takes a `&AtomicBool` cancel flag, checked between
//! chunks, and a `&mut dyn FnMut(&Event)` callback. To feed a channel, send
//! from the callback; to throttle UI updates, rate-limit in the callback.

use std::sync::atomic::{AtomicBool, Ordering};

use crate::error::{Error, SourceError};

/// What the bytes in a [`Progress`] are counting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// Hashing data already on disk (a finished file, a resumed `.part`, or
    /// an import source).
    Checking,
    /// Receiving bytes from a source.
    Downloading,
    /// Copying into the store (import across filesystems).
    Copying,
}

/// A snapshot of one operation on one model.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Progress<'a> {
    pub model_id: &'a str,
    pub phase: Phase,
    /// The file being worked on, as named in the manifest.
    pub file: &'a str,
    /// 0-based index of `file` in the model's file list.
    pub file_index: usize,
    pub file_count: usize,
    /// Bytes of `file` processed in this phase so far.
    pub file_done: u64,
    pub file_total: u64,
    /// Bytes of the whole model accounted for: finished files plus
    /// `file_done`. Monotonic within a download, except that it steps back
    /// when a source fails and its partial bytes are discarded.
    pub model_done: u64,
    pub model_total: u64,
}

/// Something worth telling the user while an operation runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event<'a> {
    Progress(Progress<'a>),
    /// A source failed for a file; the downloader is moving on (or, if it
    /// was the last, about to return [`Error::AllSourcesFailed`]).
    SourceFailed { file: &'a str, url: &'a str, error: &'a SourceError },
    /// One file is complete and verified in the store.
    FileDone { file: &'a str },
}

/// Callback type used by every long-running operation.
pub type OnEvent<'a> = &'a mut dyn FnMut(&Event<'_>);

/// Returns `Err(Cancelled)` once the flag is set.
pub(crate) fn check_cancel(cancel: &AtomicBool) -> Result<(), Error> {
    if cancel.load(Ordering::Relaxed) {
        Err(Error::Cancelled)
    } else {
        Ok(())
    }
}

/// Fixed context for emitting progress about one file of one model.
pub(crate) struct Reporter<'a> {
    pub model_id: &'a str,
    pub file: &'a str,
    pub file_index: usize,
    pub file_count: usize,
    pub file_total: u64,
    /// Model bytes finished before this file.
    pub model_base: u64,
    pub model_total: u64,
}

impl Reporter<'_> {
    pub fn emit(&self, on_event: &mut dyn FnMut(&Event<'_>), phase: Phase, file_done: u64) {
        on_event(&Event::Progress(Progress {
            model_id: self.model_id,
            phase,
            file: self.file,
            file_index: self.file_index,
            file_count: self.file_count,
            file_done,
            file_total: self.file_total,
            model_done: self.model_base + file_done,
            model_total: self.model_total,
        }));
    }
}
