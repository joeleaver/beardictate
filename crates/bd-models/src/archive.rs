//! Taking a model's files out of a downloaded `.tar.bz2`
//! ([`crate::ModelArchive`]). Only the files the manifest lists are
//! written, each through SHA-256 and checked before it's renamed into
//! place, so the archive's other contents (test clips, READMEs) and any
//! path tricks in it never reach the store.

use std::fs;
use std::io::{self, Read, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use sha2::{Digest, Sha256};

use crate::download::sync_parent;
use crate::error::Error;
use crate::hash::hex_digest;
use crate::manifest::{ModelArchive, ModelEntry};
use crate::marker;
use crate::store::{Store, part_path};

/// Writes every listed file of `model` that isn't verified yet from
/// `archive_file`, and records each in the marker.
pub(crate) fn unpack(store: &Store, model: &ModelEntry, archive: &ModelArchive, archive_file: &Path, cancel: &AtomicBool) -> Result<(), Error> {
    let dir = store.model_dir(&model.id);
    let fail = |problem: String| Error::Archive { archive: archive.path.clone(), problem };
    let file = fs::File::open(archive_file).map_err(Error::io("open", archive_file))?;
    let mut tar = tar::Archive::new(bzip2::read::BzDecoder::new(io::BufReader::new(file)));
    let mut missing: Vec<_> = model.files.iter().filter(|f| !marker::is_verified(&dir, f)).collect();
    let entries = tar.entries().map_err(|e| fail(e.to_string()))?;
    for entry in entries {
        if missing.is_empty() {
            break;
        }
        if cancel.load(Ordering::Relaxed) {
            return Err(Error::Cancelled);
        }
        let mut entry = entry.map_err(|e| fail(e.to_string()))?;
        let name = entry.path().map_err(|e| fail(e.to_string()))?.to_string_lossy().replace('\\', "/");
        let Some(rel) = name.strip_prefix(&archive.strip) else { continue };
        let Some(at) = missing.iter().position(|f| f.path == rel) else { continue };
        let want = missing.remove(at);
        let dest = store.path_of(&model.id, want);
        let part = part_path(&dest);
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent).map_err(Error::io("create", parent))?;
        }
        let mut out = fs::File::create(&part).map_err(Error::io("create", &part))?;
        let mut hasher = Sha256::new();
        let mut size = 0u64;
        let mut buf = vec![0u8; 1 << 20];
        loop {
            let n = entry.read(&mut buf).map_err(|e| fail(e.to_string()))?;
            if n == 0 {
                break;
            }
            hasher.update(&buf[..n]);
            out.write_all(&buf[..n]).map_err(Error::io("write", &part))?;
            size += n as u64;
        }
        out.sync_all().map_err(Error::io("sync", &part))?;
        let actual = hex_digest(hasher);
        if size != want.size || actual != want.sha256 {
            let _ = fs::remove_file(&part);
            return Err(fail(format!("{} is {size} bytes with sha256 {actual}, not as listed", want.path)));
        }
        fs::rename(&part, &dest).map_err(Error::io("rename", &dest))?;
        sync_parent(&dest)?;
        marker::record(&dir, want)?;
    }
    match missing.first() {
        Some(f) => Err(fail(format!("{} isn't in it", f.path))),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::{ModelFile, ModelKind};

    fn pack(dir: &Path, entries: &[(&str, &[u8])]) -> std::path::PathBuf {
        let path = dir.join("m.tar.bz2");
        let enc = bzip2::write::BzEncoder::new(fs::File::create(&path).unwrap(), bzip2::Compression::fast());
        let mut tar = tar::Builder::new(enc);
        for (name, data) in entries {
            let mut header = tar::Header::new_gnu();
            header.set_size(data.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            tar.append_data(&mut header, name, *data).unwrap();
        }
        tar.into_inner().unwrap().finish().unwrap();
        path
    }

    fn listed(path: &str, data: &[u8]) -> ModelFile {
        ModelFile { path: path.into(), size: data.len() as u64, sha256: hex::encode(Sha256::digest(data)) }
    }

    fn model(files: Vec<ModelFile>) -> (ModelEntry, ModelArchive) {
        let archive = ModelArchive { path: "m.tar.bz2".into(), size: 1, sha256: "0".repeat(64), strip: "pkg/".into() };
        let entry = ModelEntry {
            id: "asr-test".into(),
            kind: ModelKind::Asr,
            name: "test".into(),
            license: "MIT".into(),
            attribution: None,
            homepage: None,
            required: false,
            available: true,
            recommended: false,
            sources: vec!["https://example.invalid/".into()],
            files,
            archive: Some(archive.clone()),
            cleanup: None,
        };
        (entry, archive)
    }

    #[test]
    fn takes_out_only_the_listed_files_and_checks_them() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::at(tmp.path().join("store"));
        let packed = pack(tmp.path(), &[("pkg/README.md", b"hi"), ("pkg/enc.onnx", b"weights"), ("pkg/tokens.txt", b"a 0")]);
        let (entry, archive) = model(vec![listed("enc.onnx", b"weights"), listed("tokens.txt", b"a 0")]);
        unpack(&store, &entry, &archive, &packed, &AtomicBool::new(false)).unwrap();
        let dir = store.model_dir("asr-test");
        assert_eq!(fs::read(dir.join("enc.onnx")).unwrap(), b"weights");
        assert!(!dir.join("README.md").exists());
        assert!(entry.files.iter().all(|f| marker::is_verified(&dir, f)));

        // A file with other bytes than listed is refused, and so is a missing one.
        let (bad, archive) = model(vec![listed("enc.onnx", b"other")]);
        let store2 = Store::at(tmp.path().join("store2"));
        assert!(matches!(unpack(&store2, &bad, &archive, &packed, &AtomicBool::new(false)), Err(Error::Archive { .. })));
        assert!(!store2.model_dir("asr-test").join("enc.onnx").exists());
        let (gone, archive) = model(vec![listed("joiner.onnx", b"x")]);
        assert!(matches!(unpack(&store2, &gone, &archive, &packed, &AtomicBool::new(false)), Err(Error::Archive { .. })));
    }
}
