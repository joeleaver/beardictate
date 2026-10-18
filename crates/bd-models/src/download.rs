//! The resumable, verified, mirror-falling-back model downloader.
//!
//! Per file: skip it if it's already verified; otherwise, for each source in
//! manifest order, resume (or start) `<file>.part`, re-hashing any existing
//! prefix first, stream the rest through SHA-256, fsync, check the hash, and
//! atomically rename into place. Network and HTTP failures, size problems
//! and hash mismatches move on to the next source; local I/O errors and
//! cancellation stop the download, leaving the `.part` for next time.
//!
//! Blocking by design: the app runs it on a worker thread.

use std::fs;
use std::path::Path;
use std::sync::atomic::AtomicBool;

use sha2::{Digest, Sha256};
use ureq::Agent;

use crate::error::{Error, SourceError, SourceFailure, SourceFailures};
use crate::hash::{hash_file, hash_file_into, hex_digest};
use crate::http::{HttpOptions, agent};
use crate::manifest::{ModelArchive, ModelEntry, ModelFile};
use crate::marker;
use crate::progress::{Event, OnEvent, Phase, Reporter};
use crate::store::{Store, file_len, part_path};
use crate::transfer::{Failure, Target, probe, receive};

/// Downloads manifest models into a [`Store`]. Clones share one connection
/// pool, so make one per app.
#[derive(Debug, Clone)]
pub struct Downloader {
    agent: Agent,
}

impl Default for Downloader {
    fn default() -> Self {
        Downloader::new(&HttpOptions::default())
    }
}

impl Downloader {
    pub fn new(options: &HttpOptions) -> Downloader {
        Downloader { agent: agent(options) }
    }

    /// Makes every file of `model` present and verified in `store`.
    ///
    /// Returns [`Error::Cancelled`] if `cancel` is set (partial files stay
    /// for a resume), [`Error::Busy`] if another download, import, verify or
    /// delete of this model is running, [`Error::AllSourcesFailed`] when no
    /// source could deliver a file.
    pub fn download(
        &self,
        store: &Store,
        model: &ModelEntry,
        cancel: &AtomicBool,
        on_event: OnEvent<'_>,
    ) -> Result<(), Error> {
        if !model.available {
            return Err(Error::NotAvailable(model.id.clone()));
        }
        let _lock = store.lock(&model.id)?;
        if let Some(archive) = &model.archive {
            return self.download_archive(store, model, archive, cancel, on_event);
        }
        let dir = store.model_dir(&model.id);
        let total = model.total_size();
        let mut base = 0u64;
        for (index, file) in model.files.iter().enumerate() {
            let dest = store.path_of(&model.id, file);
            let parent = dest.parent().expect("store paths have a parent");
            fs::create_dir_all(parent).map_err(Error::io("create", parent))?;
            let reporter = Reporter {
                model_id: &model.id,
                file: &file.path,
                file_index: index,
                file_count: model.files.len(),
                file_total: file.size,
                model_base: base,
                model_total: total,
            };
            if !already_present(&dir, &dest, file, cancel, &reporter, on_event)? {
                self.fetch_file(&model.sources, &dest, file, cancel, &reporter, on_event)?;
                marker::record(&dir, file)?;
            }
            on_event(&Event::FileDone { file: &file.path });
            base += file.size;
        }
        Ok(())
    }

    /// [`Self::download`] for a model published as an archive: fetch the
    /// archive (resumable, hash-checked, like any file), take the missing
    /// files out of it, delete it.
    fn download_archive(
        &self,
        store: &Store,
        model: &ModelEntry,
        archive: &ModelArchive,
        cancel: &AtomicBool,
        on_event: OnEvent<'_>,
    ) -> Result<(), Error> {
        let dir = store.model_dir(&model.id);
        fs::create_dir_all(&dir).map_err(Error::io("create", &dir))?;
        fn reporter<'a>(id: &'a str, file: &'a str, file_index: usize, file_total: u64, model_total: u64) -> Reporter<'a> {
            Reporter { model_id: id, file, file_index, file_count: 1, file_total, model_base: 0, model_total }
        }
        let mut complete = true;
        for (index, file) in model.files.iter().enumerate() {
            let dest = store.path_of(&model.id, file);
            complete &= already_present(&dir, &dest, file, cancel, &reporter(&model.id, &file.path, index, file.size, archive.size), on_event)?;
        }
        if !complete {
            let packed = ModelFile { path: archive.path.clone(), size: archive.size, sha256: archive.sha256.clone() };
            let dest = dir.join(&archive.path);
            let report = reporter(&model.id, &archive.path, 0, archive.size, archive.size);
            if !already_present(&dir, &dest, &packed, cancel, &report, on_event)? {
                self.fetch_file(&model.sources, &dest, &packed, cancel, &report, on_event)?;
            }
            crate::archive::unpack(store, model, archive, &dest, cancel)?;
            marker::forget(&dir, &archive.path)?;
            remove(&dest)?;
        }
        for file in &model.files {
            on_event(&Event::FileDone { file: &file.path });
        }
        Ok(())
    }

    /// Checks that `url` resolves to a file of `expected_size` bytes by
    /// fetching at most one byte. `Ok(true)` if the server honours ranges
    /// (so downloads from it can resume).
    pub fn probe(&self, url: &str, expected_size: u64) -> Result<bool, SourceError> {
        probe(&self.agent, url, expected_size)
    }

    /// Tries each source in turn until one delivers a verified file.
    fn fetch_file(
        &self,
        sources: &[String],
        dest: &Path,
        file: &ModelFile,
        cancel: &AtomicBool,
        reporter: &Reporter<'_>,
        on_event: OnEvent<'_>,
    ) -> Result<(), Error> {
        let mut failures = Vec::new();
        for source in sources {
            let url = ModelEntry::file_url(source, file);
            let mut result = self.attempt(&url, dest, file, cancel, reporter, on_event);
            // A mismatch after resuming may be the old part's fault rather
            // than this source's (the part is gone now): one clean retry.
            if let Err(Attempt::Source { error: SourceError::HashMismatch { .. }, resumed: true }) = &result {
                tracing::warn!(url, "hash mismatch on a resumed download; retrying from zero");
                result = self.attempt(&url, dest, file, cancel, reporter, on_event);
            }
            let error = match result {
                Ok(()) => return Ok(()),
                Err(Attempt::Fatal(e)) => return Err(e),
                Err(Attempt::Source { error, .. }) => error,
            };
            tracing::warn!(url, %error, "source failed");
            on_event(&Event::SourceFailed { file: &file.path, url: &url, error: &error });
            failures.push(SourceFailure { url, error });
        }
        Err(Error::AllSourcesFailed { file: file.path.clone(), failures: SourceFailures(failures) })
    }

    /// One source, one try: resume the part, finish it, verify, rename.
    fn attempt(
        &self,
        url: &str,
        dest: &Path,
        file: &ModelFile,
        cancel: &AtomicBool,
        reporter: &Reporter<'_>,
        on_event: OnEvent<'_>,
    ) -> Result<(), Attempt> {
        let part = part_path(dest);
        let mut existing = file_len(&part).unwrap_or(0);
        if existing > file.size {
            remove(&part)?;
            existing = 0;
        }
        let mut hasher = Sha256::new();
        if existing > 0 {
            existing = hash_file_into(&part, &mut hasher, cancel, &mut |done| {
                reporter.emit(on_event, Phase::Checking, done)
            })?;
        }
        let resumed = existing > 0;
        if existing < file.size {
            let target = Target { part: &part, size: file.size, cancel, reporter };
            receive(&self.agent, url, existing, &mut hasher, &target, on_event).map_err(|f| match f {
                Failure::Fatal(e) => Attempt::Fatal(e),
                Failure::Source(error) => Attempt::Source { error, resumed },
            })?;
        } else {
            // A complete part left by an earlier run: make sure it's durable.
            let f = fs::File::open(&part).map_err(Error::io("open", &part))?;
            f.sync_all().map_err(Error::io("sync", &part))?;
        }
        let actual = hex_digest(hasher);
        if actual != file.sha256 {
            remove(&part)?;
            let error = SourceError::HashMismatch { expected: file.sha256.clone(), actual };
            return Err(Attempt::Source { error, resumed });
        }
        fs::rename(&part, dest).map_err(Error::io("rename", dest))?;
        sync_parent(dest)?;
        Ok(())
    }
}

/// Outcome of a failed [`Downloader::attempt`].
enum Attempt {
    Fatal(Error),
    /// `resumed`: the attempt started from an existing part.
    Source { error: SourceError, resumed: bool },
}

impl From<Error> for Attempt {
    fn from(e: Error) -> Self {
        Attempt::Fatal(e)
    }
}

/// True if `dest` already holds the right bytes, by marker or by hashing.
/// Wrong files are removed so the download starts clean.
fn already_present(
    dir: &Path,
    dest: &Path,
    file: &ModelFile,
    cancel: &AtomicBool,
    reporter: &Reporter<'_>,
    on_event: OnEvent<'_>,
) -> Result<bool, Error> {
    if marker::is_verified(dir, file) {
        return Ok(true);
    }
    match file_len(dest) {
        None => return Ok(false),
        Some(len) if len == file.size => {
            let (actual, _) = hash_file(dest, cancel, &mut |done| reporter.emit(on_event, Phase::Checking, done))?;
            if actual == file.sha256 {
                marker::record(dir, file)?;
                return Ok(true);
            }
            tracing::warn!(file = %file.path, "existing file has the wrong hash; downloading again");
        }
        Some(_) => tracing::warn!(file = %file.path, "existing file has the wrong size; downloading again"),
    }
    marker::forget(dir, &file.path)?;
    remove(dest)?;
    Ok(false)
}

fn remove(path: &Path) -> Result<(), Error> {
    match fs::remove_file(path) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(Error::io("remove", path)(e)),
        _ => Ok(()),
    }
}

/// Makes a rename durable. Directories can't be opened for sync on Windows,
/// where NTFS journals the rename itself.
pub(crate) fn sync_parent(path: &Path) -> Result<(), Error> {
    #[cfg(unix)]
    {
        let parent = path.parent().expect("store paths have a parent");
        fs::File::open(parent)
            .and_then(|d| d.sync_all())
            .map_err(Error::io("sync", parent))?;
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}
