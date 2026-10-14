//! Error types for the store, downloader and importer.
//!
//! The split that matters to the UI: a [`SourceError`] is one mirror failing
//! (the downloader moves on to the next), while an [`Error`] ends the whole
//! operation and says why.

use std::fmt;
use std::io;
use std::path::PathBuf;

/// Why one source failed to deliver one file. The downloader records it and
/// falls back to the next source.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SourceError {
    /// DNS, connect, TLS, or the connection dropped mid-transfer.
    #[error("network error: {0}")]
    Network(String),
    /// No bytes arrived for the configured idle timeout.
    #[error("timed out waiting for the server")]
    Timeout,
    /// The server answered with a non-success status (after redirects).
    #[error("server returned HTTP {0}")]
    HttpStatus(u16),
    /// The server's `Content-Range` didn't match what we asked for.
    #[error("server sent an unexpected Content-Range: {0:?}")]
    BadContentRange(String),
    /// The server reports a different file size than the manifest.
    #[error("server has {actual} bytes, expected {expected}")]
    SizeMismatch { expected: u64, actual: u64 },
    /// The body ended before the whole file arrived. The partial file is
    /// kept, so the next attempt resumes.
    #[error("connection closed after {received} of {expected} bytes")]
    Truncated { expected: u64, received: u64 },
    /// The body ran past the manifest size. The partial file is deleted.
    #[error("server sent more than the expected {expected} bytes")]
    TooLong { expected: u64 },
    /// The complete file hashed wrong. The partial file is deleted.
    #[error("sha256 mismatch: expected {expected}, got {actual}")]
    HashMismatch { expected: String, actual: String },
}

/// One source's failure for one file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceFailure {
    pub url: String,
    pub error: SourceError,
}

/// The list of per-source failures, displayed one per line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceFailures(pub Vec<SourceFailure>);

impl fmt::Display for SourceFailures {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, failure) in self.0.iter().enumerate() {
            if i > 0 {
                f.write_str("; ")?;
            }
            write!(f, "{}: {}", failure.url, failure.error)?;
        }
        Ok(())
    }
}

/// What's wrong with one file on disk, from verification or import.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FileProblem {
    #[error("missing")]
    Missing,
    #[error("size is {actual} bytes, expected {expected}")]
    WrongSize { expected: u64, actual: u64 },
    #[error("sha256 is {actual}, expected {expected}")]
    WrongHash { expected: String, actual: String },
}

/// Why a store, download or import operation stopped.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// No platform data directory and no `BD_MODELS_DIR`.
    #[error("no data directory for models: set BD_MODELS_DIR")]
    NoDataDir,
    /// The id can't name a store directory (only for ids not from a validated
    /// manifest, e.g. deleting a leftover directory).
    #[error("invalid model id {0:?}")]
    InvalidModelId(String),
    /// A catalog placeholder with no weights yet.
    #[error("model {0} is not available for download yet")]
    NotAvailable(String),
    /// Another thread or process is downloading, importing, verifying or
    /// deleting this model.
    #[error("model {0} is busy in another operation")]
    Busy(String),
    /// The caller's cancel flag was set. Partial downloads are kept for resume.
    #[error("cancelled")]
    Cancelled,
    /// Every source failed for this file.
    #[error("could not download {file}: {failures}")]
    AllSourcesFailed { file: String, failures: SourceFailures },
    /// An imported folder's file didn't match the manifest.
    #[error("cannot import {file}: {problem}")]
    Import { file: String, problem: FileProblem },
    /// A local filesystem error (disk full, permissions, ...). Not retried
    /// against other sources, since another mirror won't fix it.
    #[error("{op} {}: {source}", path.display())]
    Io {
        op: &'static str,
        path: PathBuf,
        #[source]
        source: io::Error,
    },
}

impl Error {
    pub(crate) fn io(op: &'static str, path: impl Into<PathBuf>) -> impl FnOnce(io::Error) -> Error {
        let path = path.into();
        move |source| Error::Io { op, path, source }
    }
}
