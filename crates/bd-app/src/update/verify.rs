//! Checking a downloaded build: its size and SHA-256 as the feed lists
//! them, and an Ed25519 signature made with the release key, which never
//! leaves the publishing machine (`~/.config/beardictate-release/`). The
//! hash catches a broken download; the signature, a changed site.

use std::fs;
use std::path::Path;

use anyhow::{Context, Result, bail, ensure};
use ed25519_dalek::{Signature, VerifyingKey};
use sha2::{Digest, Sha256};

/// The release key's public half.
const RELEASE_KEY: &str = "a901ecefae5c3354e5085e8dbdf8bc7211b5d83bc8fae1d1844a8bc41c41e081";

/// Whether `file` is the build the feed describes, signed with the release
/// key.
pub fn check(file: &Path, size: u64, sha256: &str, signature: &str) -> Result<()> {
    check_with(file, size, sha256, signature, RELEASE_KEY)
}

fn check_with(file: &Path, size: u64, sha256: &str, signature: &str, key: &str) -> Result<()> {
    let bytes = fs::read(file).with_context(|| format!("reading {}", file.display()))?;
    ensure!(
        bytes.len() as u64 == size,
        "{} is {} bytes, not {size}",
        file.display(),
        bytes.len()
    );
    let digest = hex::encode(Sha256::digest(&bytes));
    ensure!(
        digest.eq_ignore_ascii_case(sha256),
        "{} doesn't match its SHA-256",
        file.display()
    );
    let key: [u8; 32] = hex::decode(key)?
        .try_into()
        .map_err(|_| anyhow::anyhow!("release key length"))?;
    let key = VerifyingKey::from_bytes(&key)?;
    let signature: [u8; 64] = match hex::decode(signature.trim()) {
        Ok(s) if s.len() == 64 => s.try_into().expect("64 bytes"),
        _ => bail!("the signature isn't 64 hex bytes"),
    };
    key.verify_strict(&bytes, &Signature::from_bytes(&signature))
        .with_context(|| format!("{} isn't signed with the release key", file.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    // Made with openssl from a throwaway key:
    //   openssl genpkey -algorithm ed25519 -out k.pem
    //   printf 'beardictate update test\n' > f
    //   openssl pkeyutl -sign -inkey k.pem -rawin -in f | xxd -p -c 64
    const KEY: &str = "3dfbc2e97510f00faefc346a04b16ceea0be590455e64f33ef4724d2544604ef";
    const BODY: &[u8] = b"beardictate update test\n";

    fn file(dir: &str) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!("bd-verify-{dir}-{}", std::process::id()));
        fs::write(&path, BODY).unwrap();
        path
    }

    #[test]
    fn a_signed_build_passes_and_anything_else_fails() {
        let signature = include_str!("verify-test.sig");
        let path = file("ok");
        let sha = hex::encode(Sha256::digest(BODY));
        let size = BODY.len() as u64;
        check_with(&path, size, &sha, signature, KEY).unwrap();
        assert!(check_with(&path, size + 1, &sha, signature, KEY).is_err());
        assert!(check_with(&path, size, &"0".repeat(64), signature, KEY).is_err());
        assert!(check_with(&path, size, &sha, &"00".repeat(64), KEY).is_err());
        assert!(check_with(&path, size, &sha, signature, RELEASE_KEY).is_err());
        fs::write(&path, b"beardictate update tesT\n").unwrap();
        let sha2 = hex::encode(Sha256::digest(b"beardictate update tesT\n"));
        assert!(check_with(&path, size, &sha2, signature, KEY).is_err());
        fs::remove_file(path).unwrap();
    }
}
