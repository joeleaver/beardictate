//! "Import model folder…": install a model from a pre-downloaded copy, for
//! air-gapped machines and IT deploys.
//!
//! The folder may hold the model's files directly or contain a `<model-id>/`
//! subfolder (a copy of another machine's store). Every file is checked
//! against the manifest's size and sha256 before it lands in the store. Files
//! are hard-linked when the folder is on the same filesystem as the store
//! (instant, no extra space) and copied otherwise.

use std::fs::{self, File};
use std::io::{ErrorKind, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use sha2::{Digest, Sha256};

use crate::download::sync_parent;
use crate::error::{Error, FileProblem};
use crate::hash::{CHUNK, hash_file, hex_digest};
use crate::manifest::{Manifest, ModelEntry, ModelFile};
use crate::marker;
use crate::progress::{Event, OnEvent, Phase, Reporter, check_cancel};
use crate::store::{Store, file_len, part_path};

/// How each file of an import reached the store.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ImportReport {
    /// Hard-linked from the folder.
    pub linked: usize,
    /// Copied from the folder.
    pub copied: usize,
    /// Already verified in the store; left alone.
    pub already_present: usize,
}

/// Models from `manifest` whose files are all in `dir` (or `dir/<id>/`) with
/// the right sizes: what an "Import model folder…" dialog can offer. Hashes
/// are checked by [`import_folder`], not here.
pub fn detect_importable<'m>(manifest: &'m Manifest, dir: &Path) -> Vec<&'m ModelEntry> {
    manifest
        .models
        .iter()
        .filter(|m| m.available && find_source_dir(m, dir).is_some())
        .collect()
}

/// Verifies `model`'s files in `dir` (or `dir/<id>/`) against the manifest
/// and installs them into `store`.
///
/// Nothing is touched unless every file is present with the right size
/// ([`Error::Import`] with [`FileProblem::Missing`] or `WrongSize`). A file
/// whose hash is wrong fails with `WrongHash`, after earlier files were
/// already installed (they're verified, so that's safe).
pub fn import_folder(
    store: &Store,
    model: &ModelEntry,
    dir: &Path,
    cancel: &AtomicBool,
    on_event: OnEvent<'_>,
) -> Result<ImportReport, Error> {
    if !model.available {
        return Err(Error::NotAvailable(model.id.clone()));
    }
    let src_dir = match find_source_dir(model, dir) {
        Some(d) => d,
        None => {
            // Report the first problem against the folder as given.
            let (file, problem) = first_problem(model, dir).expect("some file is missing or mis-sized");
            return Err(Error::Import { file, problem });
        }
    };
    let _lock = store.lock(&model.id)?;
    let model_dir = store.model_dir(&model.id);
    let total = model.total_size();
    let mut report = ImportReport::default();
    let mut base = 0u64;
    for (index, file) in model.files.iter().enumerate() {
        let src = join(&src_dir, file);
        let dest = store.path_of(&model.id, file);
        let reporter = Reporter {
            model_id: &model.id,
            file: &file.path,
            file_index: index,
            file_count: model.files.len(),
            file_total: file.size,
            model_base: base,
            model_total: total,
        };
        if marker::is_verified(&model_dir, file) {
            report.already_present += 1;
        } else if same_file(&src, &dest) {
            // Importing the store into itself: just verify in place.
            check_hash(&src, file, cancel, &reporter, on_event)?;
            marker::record(&model_dir, file)?;
            report.already_present += 1;
        } else {
            let parent = dest.parent().expect("store paths have a parent");
            fs::create_dir_all(parent).map_err(Error::io("create", parent))?;
            marker::forget(&model_dir, &file.path)?;
            if install_file(&src, &dest, file, cancel, &reporter, on_event)? {
                report.linked += 1;
            } else {
                report.copied += 1;
            }
            marker::record(&model_dir, file)?;
        }
        on_event(&Event::FileDone { file: &file.path });
        base += file.size;
    }
    Ok(report)
}

/// `dir` itself if it holds every file at the right size, else `dir/<id>`.
fn find_source_dir(model: &ModelEntry, dir: &Path) -> Option<PathBuf> {
    [dir.to_path_buf(), dir.join(&model.id)]
        .into_iter()
        .find(|d| first_problem(model, d).is_none())
}

fn first_problem(model: &ModelEntry, dir: &Path) -> Option<(String, FileProblem)> {
    model.files.iter().find_map(|f| {
        let problem = match file_len(&join(dir, f)) {
            None => FileProblem::Missing,
            Some(actual) if actual != f.size => FileProblem::WrongSize { expected: f.size, actual },
            Some(_) => return None,
        };
        Some((f.path.clone(), problem))
    })
}

fn join(dir: &Path, file: &ModelFile) -> PathBuf {
    let mut path = dir.to_path_buf();
    path.extend(file.path.split('/'));
    path
}

fn same_file(a: &Path, b: &Path) -> bool {
    match (fs::canonicalize(a), fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

fn wrong_hash(file: &ModelFile, actual: String) -> Error {
    Error::Import {
        file: file.path.clone(),
        problem: FileProblem::WrongHash { expected: file.sha256.clone(), actual },
    }
}

fn check_hash(
    path: &Path,
    file: &ModelFile,
    cancel: &AtomicBool,
    reporter: &Reporter<'_>,
    on_event: OnEvent<'_>,
) -> Result<(), Error> {
    let (actual, _) = hash_file(path, cancel, &mut |done| reporter.emit(on_event, Phase::Checking, done))?;
    if actual == file.sha256 { Ok(()) } else { Err(wrong_hash(file, actual)) }
}

/// Puts a verified copy of `src` at `dest` via `<dest>.part`. Returns true if
/// it was hard-linked, false if copied.
fn install_file(
    src: &Path,
    dest: &Path,
    file: &ModelFile,
    cancel: &AtomicBool,
    reporter: &Reporter<'_>,
    on_event: OnEvent<'_>,
) -> Result<bool, Error> {
    let part = part_path(dest);
    remove_if_exists(&part)?;
    let linked = match fs::hard_link(src, &part) {
        Ok(()) => {
            // Same inode as the source: hash it once, through the link.
            if let Err(e) = check_hash(&part, file, cancel, reporter, on_event) {
                remove_if_exists(&part)?;
                return Err(e);
            }
            true
        }
        Err(e) => {
            tracing::debug!(error = %e, "hard link failed; copying");
            copy_verified(src, &part, file, cancel, reporter, on_event)?;
            false
        }
    };
    fs::rename(&part, dest).map_err(Error::io("rename", dest))?;
    sync_parent(dest)?;
    Ok(linked)
}

/// Copies `src` to `part`, hashing the bytes as they pass. The part is
/// removed on any failure, including cancellation (a local copy is cheap to
/// redo, unlike a download).
fn copy_verified(
    src: &Path,
    part: &Path,
    file: &ModelFile,
    cancel: &AtomicBool,
    reporter: &Reporter<'_>,
    on_event: OnEvent<'_>,
) -> Result<(), Error> {
    let result = (|| {
        let mut input = File::open(src).map_err(Error::io("open", src))?;
        let mut output = File::create(part).map_err(Error::io("create", part))?;
        let mut hasher = Sha256::new();
        let mut buf = vec![0u8; CHUNK];
        let mut done = 0u64;
        loop {
            check_cancel(cancel)?;
            let n = match input.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => n,
                Err(e) if e.kind() == ErrorKind::Interrupted => continue,
                Err(e) => return Err(Error::io("read", src)(e)),
            };
            output.write_all(&buf[..n]).map_err(Error::io("write", part))?;
            hasher.update(&buf[..n]);
            done += n as u64;
            reporter.emit(on_event, Phase::Copying, done);
        }
        output.sync_all().map_err(Error::io("sync", part))?;
        let actual = hex_digest(hasher);
        if done != file.size {
            return Err(Error::Import {
                file: file.path.clone(),
                problem: FileProblem::WrongSize { expected: file.size, actual: done },
            });
        }
        if actual != file.sha256 {
            return Err(wrong_hash(file, actual));
        }
        Ok(())
    })();
    if result.is_err() {
        remove_if_exists(part)?;
    }
    result
}

fn remove_if_exists(path: &Path) -> Result<(), Error> {
    match fs::remove_file(path) {
        Err(e) if e.kind() != ErrorKind::NotFound => Err(Error::io("remove", path)(e)),
        _ => Ok(()),
    }
}

#[cfg(test)]
#[path = "import_tests.rs"]
mod tests;
