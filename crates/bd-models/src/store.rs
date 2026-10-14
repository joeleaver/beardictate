//! The on-disk model store: `<root>/<model-id>/<file>`.
//!
//! The root is `BD_MODELS_DIR` if set, else `beardictate/models` under the
//! platform's local data directory (`%LOCALAPPDATA%`, `~/.local/share`,
//! `~/Library/Application Support`). Next to the model directories the store
//! keeps `.locks/<model-id>.lock`, which serializes download, import, verify
//! and delete of one model across threads and processes.

use std::fs::{self, File};
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use crate::error::{Error, FileProblem};
use crate::hash::hash_file;
use crate::manifest::{Manifest, ModelEntry, ModelFile, is_valid_model_id};
use crate::marker;
use crate::progress::{OnEvent, Phase, Reporter};

/// Environment variable that overrides the store root.
pub const MODELS_DIR_ENV: &str = "BD_MODELS_DIR";

/// Suffix of an in-progress download, next to its final path.
pub const PART_SUFFIX: &str = ".part";

const LOCKS_DIR: &str = ".locks";

/// What's on disk for one model.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelStatus {
    /// Nothing on disk.
    NotInstalled,
    /// Some files or partial downloads exist; `bytes_present` counts them.
    Partial { bytes_present: u64 },
    /// Every file is present with the right size. `verified` is true when
    /// each one's hash was checked and the file is unchanged since.
    Installed { verified: bool },
}

/// One file that failed [`Store::verify`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileIssue {
    pub file: String,
    pub problem: FileProblem,
}

/// A model store rooted at one directory.
#[derive(Debug, Clone)]
pub struct Store {
    root: PathBuf,
}

/// Held while operating on one model; released on drop.
#[derive(Debug)]
pub(crate) struct ModelLock {
    _file: File,
}

impl Store {
    /// A store at an explicit root (tests, portable installs).
    pub fn at(root: impl Into<PathBuf>) -> Store {
        Store { root: root.into() }
    }

    /// The store at `BD_MODELS_DIR`, or the platform default.
    pub fn from_env() -> Result<Store, Error> {
        Self::default_root().map(Store::at).ok_or(Error::NoDataDir)
    }

    /// Where [`Store::from_env`] would put the store.
    pub fn default_root() -> Option<PathBuf> {
        if let Some(dir) = std::env::var_os(MODELS_DIR_ENV).filter(|d| !d.is_empty()) {
            return Some(PathBuf::from(dir));
        }
        let base = directories::BaseDirs::new()?;
        Some(base.data_local_dir().join("beardictate").join("models"))
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Directory holding `model_id`'s files.
    pub fn model_dir(&self, model_id: &str) -> PathBuf {
        self.root.join(model_id)
    }

    /// Final path of `file` within the model's directory.
    pub(crate) fn path_of(&self, model_id: &str, file: &ModelFile) -> PathBuf {
        let mut path = self.model_dir(model_id);
        path.extend(file.path.split('/'));
        path
    }

    /// Path of the manifest file `file_path` of `model`, if the manifest
    /// lists it. The file may not exist yet; see [`Store::is_installed`].
    pub fn file_path(&self, model: &ModelEntry, file_path: &str) -> Option<PathBuf> {
        model.file(file_path).map(|f| self.path_of(&model.id, f))
    }

    /// True if every file is present with its manifest size. Cheap: no
    /// hashing. Pair with [`Store::is_verified`] for integrity.
    pub fn is_installed(&self, model: &ModelEntry) -> bool {
        model.available
            && !model.files.is_empty()
            && model.files.iter().all(|f| file_len(&self.path_of(&model.id, f)) == Some(f.size))
    }

    /// True if every file was hash-verified (by download, import or
    /// [`Store::verify`]) and none has changed since. Cheap: reads one small
    /// marker file and stats each model file.
    pub fn is_verified(&self, model: &ModelEntry) -> bool {
        model.available && marker::all_verified(&self.model_dir(&model.id), &model.files)
    }

    /// Install state for display.
    pub fn status(&self, model: &ModelEntry) -> ModelStatus {
        if self.is_installed(model) {
            return ModelStatus::Installed { verified: self.is_verified(model) };
        }
        let bytes_present: u64 = model
            .files
            .iter()
            .map(|f| {
                let path = self.path_of(&model.id, f);
                file_len(&path).or_else(|| file_len(&part_path(&path))).unwrap_or(0)
            })
            .sum();
        let any_present = model.files.iter().any(|f| {
            let path = self.path_of(&model.id, f);
            path.exists() || part_path(&path).exists()
        });
        if any_present {
            ModelStatus::Partial { bytes_present }
        } else {
            ModelStatus::NotInstalled
        }
    }

    /// Hashes every file of `model` and records the result, so later
    /// [`Store::is_verified`] calls are cheap. Returns the files that failed;
    /// an empty list means the model is verified.
    pub fn verify(&self, model: &ModelEntry, cancel: &AtomicBool, on_event: OnEvent<'_>) -> Result<Vec<FileIssue>, Error> {
        if !model.available {
            return Err(Error::NotAvailable(model.id.clone()));
        }
        let _lock = self.lock(&model.id)?;
        let dir = self.model_dir(&model.id);
        let total = model.total_size();
        let mut issues = Vec::new();
        let mut base = 0u64;
        for (index, file) in model.files.iter().enumerate() {
            let path = self.path_of(&model.id, file);
            let problem = match file_len(&path) {
                None => Some(FileProblem::Missing),
                Some(actual) if actual != file.size => Some(FileProblem::WrongSize { expected: file.size, actual }),
                Some(_) => {
                    let reporter = Reporter {
                        model_id: &model.id,
                        file: &file.path,
                        file_index: index,
                        file_count: model.files.len(),
                        file_total: file.size,
                        model_base: base,
                        model_total: total,
                    };
                    let (actual, _) = hash_file(&path, cancel, &mut |done| reporter.emit(on_event, Phase::Checking, done))?;
                    (actual != file.sha256).then(|| FileProblem::WrongHash { expected: file.sha256.clone(), actual })
                }
            };
            match problem {
                None => marker::record(&dir, file)?,
                Some(problem) => {
                    if dir.exists() {
                        marker::forget(&dir, &file.path)?;
                    }
                    issues.push(FileIssue { file: file.path.clone(), problem });
                }
            }
            base += file.size;
        }
        Ok(issues)
    }

    /// Removes the model's directory, including partial downloads. Takes an
    /// id rather than an entry so leftovers from older manifests (see
    /// [`Store::unknown_dirs`]) can be removed too.
    pub fn delete(&self, model_id: &str) -> Result<(), Error> {
        if !is_valid_model_id(model_id) {
            return Err(Error::InvalidModelId(model_id.into()));
        }
        let _lock = self.lock(model_id)?;
        let dir = self.model_dir(model_id);
        match fs::remove_dir_all(&dir) {
            Err(e) if e.kind() != ErrorKind::NotFound => Err(Error::io("remove", dir)(e)),
            _ => Ok(()),
        }
    }

    /// Bytes on disk for one model, partial downloads included.
    pub fn disk_usage(&self, model_id: &str) -> Result<u64, Error> {
        if !is_valid_model_id(model_id) {
            return Err(Error::InvalidModelId(model_id.into()));
        }
        dir_size(&self.model_dir(model_id))
    }

    /// Bytes on disk for the whole store.
    pub fn total_disk_usage(&self) -> Result<u64, Error> {
        dir_size(&self.root)
    }

    /// Model directories in the store that `manifest` doesn't list, e.g.
    /// models dropped or renamed by an app update. Sorted.
    pub fn unknown_dirs(&self, manifest: &Manifest) -> Result<Vec<String>, Error> {
        let entries = match fs::read_dir(&self.root) {
            Ok(entries) => entries,
            Err(e) if e.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(Error::io("list", &self.root)(e)),
        };
        let mut unknown = Vec::new();
        for entry in entries {
            let entry = entry.map_err(Error::io("list", &self.root))?;
            let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
            let Ok(name) = entry.file_name().into_string() else { continue };
            if is_dir && is_valid_model_id(&name) && manifest.get(&name).is_none() {
                unknown.push(name);
            }
        }
        unknown.sort();
        Ok(unknown)
    }

    /// Takes the cross-process lock for `model_id`, failing fast with
    /// [`Error::Busy`] rather than waiting.
    pub(crate) fn lock(&self, model_id: &str) -> Result<ModelLock, Error> {
        let dir = self.root.join(LOCKS_DIR);
        fs::create_dir_all(&dir).map_err(Error::io("create", &dir))?;
        let path = dir.join(format!("{model_id}.lock"));
        let file = File::options()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&path)
            .map_err(Error::io("open", &path))?;
        match file.try_lock() {
            Ok(()) => Ok(ModelLock { _file: file }),
            Err(fs::TryLockError::WouldBlock) => Err(Error::Busy(model_id.into())),
            Err(fs::TryLockError::Error(e)) => Err(Error::io("lock", path)(e)),
        }
    }
}

/// `<path>.part`.
pub(crate) fn part_path(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(PART_SUFFIX);
    PathBuf::from(name)
}

/// Length of a regular file, or None if absent or not a file.
pub(crate) fn file_len(path: &Path) -> Option<u64> {
    fs::metadata(path).ok().filter(|m| m.is_file()).map(|m| m.len())
}

/// Sum of regular-file sizes under `dir`, not following symlinks. A missing
/// directory counts as zero.
fn dir_size(dir: &Path) -> Result<u64, Error> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(0),
        Err(e) => return Err(Error::io("list", dir)(e)),
    };
    let mut total = 0;
    for entry in entries {
        let entry = entry.map_err(Error::io("list", dir))?;
        let meta = fs::symlink_metadata(entry.path()).map_err(Error::io("stat", entry.path()))?;
        if meta.is_dir() {
            total += dir_size(&entry.path())?;
        } else if meta.is_file() {
            total += meta.len();
        }
    }
    Ok(total)
}

#[cfg(test)]
#[path = "store_tests.rs"]
mod tests;
