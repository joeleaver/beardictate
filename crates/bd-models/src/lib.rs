//! Model manifest, resumable verified downloader, and on-disk store.
//!
//! The installer ships no weights. The [`Manifest`] compiled into the app
//! lists every model it knows, pinned to exact upstream revisions with a
//! size and sha256 per file. The [`Downloader`] fetches a model into the
//! [`Store`] (resuming `.part` files, falling back across mirrors, verifying
//! every byte); [`import_folder`] installs one from a pre-downloaded copy
//! instead. Nothing here knows about audio, inference or UI: callers ask
//! "is it installed?", "where is file X?", and run downloads on a worker
//! thread with a progress callback and a cancel flag.
//!
//! ```no_run
//! use std::sync::atomic::AtomicBool;
//! use bd_models::{Downloader, Manifest, Store};
//!
//! let store = Store::from_env()?;
//! let vad = Manifest::builtin().get("vad-silero").unwrap();
//! if !store.is_verified(vad) {
//!     Downloader::default().download(&store, vad, &AtomicBool::new(false), &mut |event| {
//!         println!("{event:?}");
//!     })?;
//! }
//! let onnx = store.file_path(vad, "silero_vad.onnx").unwrap();
//! # let _ = onnx;
//! # Ok::<(), bd_models::Error>(())
//! ```

mod download;
mod error;
mod hash;
mod http;
mod import;
mod manifest;
mod marker;
mod progress;
mod store;
mod transfer;

pub use download::Downloader;
pub use error::{Error, FileProblem, SourceError, SourceFailure, SourceFailures};
pub use http::HttpOptions;
pub use import::{ImportReport, detect_importable, import_folder};
pub use manifest::{CleanupParams, Manifest, ManifestError, ModelEntry, ModelFile, ModelKind, Sampling};
pub use progress::{Event, OnEvent, Phase, Progress};
pub use store::{FileIssue, MODELS_DIR_ENV, ModelStatus, PART_SUFFIX, Store};
