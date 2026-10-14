//! Streaming SHA-256 over files on disk.

use std::fs::File;
use std::io::{ErrorKind, Read};
use std::path::Path;
use std::sync::atomic::AtomicBool;

use sha2::{Digest, Sha256};

use crate::error::Error;
use crate::progress::check_cancel;

/// Read/write granularity for hashing, copying and downloading. Cancellation
/// and progress are checked once per chunk.
pub(crate) const CHUNK: usize = 256 * 1024;

/// Lowercase hex of a finished hash.
pub(crate) fn hex_digest(hasher: Sha256) -> String {
    hex::encode(hasher.finalize())
}

/// Feeds the whole file at `path` into `hasher`, calling `on_bytes` with the
/// running byte count after each chunk. Returns the number of bytes hashed.
pub(crate) fn hash_file_into(
    path: &Path,
    hasher: &mut Sha256,
    cancel: &AtomicBool,
    on_bytes: &mut dyn FnMut(u64),
) -> Result<u64, Error> {
    let mut file = File::open(path).map_err(Error::io("open", path))?;
    let mut buf = vec![0u8; CHUNK];
    let mut done = 0u64;
    loop {
        check_cancel(cancel)?;
        let n = match file.read(&mut buf) {
            Ok(0) => return Ok(done),
            Ok(n) => n,
            Err(e) if e.kind() == ErrorKind::Interrupted => continue,
            Err(e) => return Err(Error::io("read", path)(e)),
        };
        hasher.update(&buf[..n]);
        done += n as u64;
        on_bytes(done);
    }
}

/// Hex SHA-256 of the file at `path`.
pub(crate) fn hash_file(
    path: &Path,
    cancel: &AtomicBool,
    on_bytes: &mut dyn FnMut(u64),
) -> Result<(String, u64), Error> {
    let mut hasher = Sha256::new();
    let len = hash_file_into(path, &mut hasher, cancel, on_bytes)?;
    Ok((hex_digest(hasher), len))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hashes_known_vector() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("abc");
        std::fs::write(&path, b"abc").unwrap();
        let (hex, len) = hash_file(&path, &AtomicBool::new(false), &mut |_| {}).unwrap();
        assert_eq!(len, 3);
        assert_eq!(hex, "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
    }

    #[test]
    fn honours_cancel() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("x");
        std::fs::write(&path, b"x").unwrap();
        let err = hash_file(&path, &AtomicBool::new(true), &mut |_| {}).unwrap_err();
        assert!(matches!(err, Error::Cancelled));
    }
}
