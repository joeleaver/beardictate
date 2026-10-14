//! The per-model "verified" marker, which makes startup checks cheap.
//!
//! Hashing a multi-GB model takes seconds, so a full verification records,
//! for each file, the sha256 it matched plus the size and mtime the file had
//! at that moment. A later check trusts the file only if the manifest still
//! asks for that same sha256 and size and mtime are unchanged. A manifest
//! update that changes a hash, or anything touching the file, invalidates it.

use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use serde::{Deserialize, Serialize};

use crate::error::Error;
use crate::manifest::ModelFile;

/// File name of the marker inside a model directory. The leading dot keeps
/// it out of the manifest's file namespace.
pub(crate) const MARKER_NAME: &str = ".verified.toml";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Entry {
    path: String,
    sha256: String,
    size: u64,
    mtime_ns: u64,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Marker {
    #[serde(default, rename = "file")]
    files: Vec<Entry>,
}

/// Size and mtime of `path` now, if it is a regular file with a usable mtime.
fn stamp(path: &Path) -> Option<(u64, u64)> {
    let meta = fs::metadata(path).ok()?;
    if !meta.is_file() {
        return None;
    }
    let mtime = meta.modified().ok()?.duration_since(UNIX_EPOCH).ok()?;
    Some((meta.len(), u64::try_from(mtime.as_nanos()).ok()?))
}

fn marker_path(model_dir: &Path) -> PathBuf {
    model_dir.join(MARKER_NAME)
}

/// A missing or unreadable marker is just "nothing verified yet".
fn load(model_dir: &Path) -> Marker {
    fs::read_to_string(marker_path(model_dir))
        .ok()
        .and_then(|text| toml::from_str(&text).ok())
        .unwrap_or_default()
}

fn save(model_dir: &Path, marker: &Marker) -> Result<(), Error> {
    let path = marker_path(model_dir);
    let tmp = model_dir.join(format!("{MARKER_NAME}.tmp"));
    let text = toml::to_string(marker).expect("marker serializes");
    fs::write(&tmp, text).map_err(Error::io("write", &tmp))?;
    fs::rename(&tmp, &path).map_err(Error::io("rename", &path))
}

/// True if `file` was verified before and hasn't changed since.
pub(crate) fn is_verified(model_dir: &Path, file: &ModelFile) -> bool {
    let marker = load(model_dir);
    is_verified_in(&marker, model_dir, file)
}

fn is_verified_in(marker: &Marker, model_dir: &Path, file: &ModelFile) -> bool {
    let Some((size, mtime_ns)) = stamp(&model_dir.join(&file.path)) else {
        return false;
    };
    marker.files.iter().any(|e| {
        e.path == file.path && e.sha256 == file.sha256 && e.size == file.size && size == file.size && e.mtime_ns == mtime_ns
    })
}

/// True if every file in `files` is verified (and `files` is non-empty).
pub(crate) fn all_verified(model_dir: &Path, files: &[ModelFile]) -> bool {
    let marker = load(model_dir);
    !files.is_empty() && files.iter().all(|f| is_verified_in(&marker, model_dir, f))
}

/// Records that `file` (already in place at its final path) hashed to its
/// manifest sha256. Must be called under the model lock.
pub(crate) fn record(model_dir: &Path, file: &ModelFile) -> Result<(), Error> {
    let path = model_dir.join(&file.path);
    let (size, mtime_ns) = stamp(&path).ok_or_else(|| Error::Io {
        op: "stat",
        path: path.clone(),
        source: ErrorKind::NotFound.into(),
    })?;
    let mut marker = load(model_dir);
    marker.files.retain(|e| e.path != file.path);
    marker.files.push(Entry {
        path: file.path.clone(),
        sha256: file.sha256.clone(),
        size,
        mtime_ns,
    });
    save(model_dir, &marker)
}

/// Drops any record for `path` (it failed verification or is being replaced).
pub(crate) fn forget(model_dir: &Path, path: &str) -> Result<(), Error> {
    let mut marker = load(model_dir);
    let before = marker.files.len();
    marker.files.retain(|e| e.path != path);
    if marker.files.len() == before {
        return Ok(());
    }
    save(model_dir, &marker)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(path: &str, sha: &str, size: u64) -> ModelFile {
        ModelFile { path: path.into(), size, sha256: sha.into() }
    }

    #[test]
    fn record_then_check_then_invalidate() {
        let dir = tempfile::tempdir().unwrap();
        let f = file("a.bin", &"a".repeat(64), 3);
        fs::write(dir.path().join("a.bin"), b"abc").unwrap();
        assert!(!is_verified(dir.path(), &f));
        record(dir.path(), &f).unwrap();
        assert!(is_verified(dir.path(), &f));
        assert!(all_verified(dir.path(), std::slice::from_ref(&f)));

        // A manifest that now wants different bytes doesn't trust the record.
        assert!(!is_verified(dir.path(), &file("a.bin", &"b".repeat(64), 3)));

        // Touching the file (new mtime) invalidates it.
        let later = std::time::SystemTime::now() + std::time::Duration::from_secs(5);
        fs::File::options()
            .write(true)
            .open(dir.path().join("a.bin"))
            .unwrap()
            .set_modified(later)
            .unwrap();
        assert!(!is_verified(dir.path(), &f));

        record(dir.path(), &f).unwrap();
        forget(dir.path(), "a.bin").unwrap();
        assert!(!is_verified(dir.path(), &f));
    }

    #[test]
    fn corrupt_marker_means_unverified() {
        let dir = tempfile::tempdir().unwrap();
        let f = file("a.bin", &"a".repeat(64), 1);
        fs::write(dir.path().join("a.bin"), b"a").unwrap();
        fs::write(dir.path().join(MARKER_NAME), "not [ toml").unwrap();
        assert!(!is_verified(dir.path(), &f));
        record(dir.path(), &f).unwrap();
        assert!(is_verified(dir.path(), &f));
    }
}
