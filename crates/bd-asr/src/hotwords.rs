//! What sherpa-onnx needs for hotword boosting with Parakeet.
//!
//! Hotwords are encoded into model tokens with a SentencePiece vocabulary
//! (`piece<TAB>score` lines). The Parakeet export ships only `tokens.txt`
//! (`piece id`), so one is derived from it: Parakeet's tokenizer is BPE, where
//! a lower id is an earlier merge, so `-id` ranks pieces the way the
//! tokenizer would. Checked against invented names: they encode to the
//! pieces the model emits for them.

use std::fs;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::{Path, PathBuf};

use crate::AsrError;

/// Writes the vocabulary derived from `tokens` to a cache file and returns
/// its path. The file name hashes the content, so a changed model gets a new
/// file and an unchanged one reuses the old.
pub(crate) fn bpe_vocab(tokens: &Path) -> Result<String, AsrError> {
    let io = |context: String| move |source| AsrError::Io { context, source };
    let text = fs::read_to_string(tokens).map_err(io(format!("reading {}", tokens.display())))?;
    let vocab = vocab_from_tokens(&text)?;
    let mut hasher = DefaultHasher::new();
    vocab.hash(&mut hasher);
    let dir = std::env::temp_dir().join("beardictate");
    let path: PathBuf = dir.join(format!("parakeet-bpe-{:016x}.vocab", hasher.finish()));
    if fs::read_to_string(&path).ok().as_deref() != Some(vocab.as_str()) {
        fs::create_dir_all(&dir).map_err(io(format!("creating {}", dir.display())))?;
        let tmp = path.with_extension(format!("tmp{}", std::process::id()));
        fs::write(&tmp, &vocab).map_err(io(format!("writing {}", tmp.display())))?;
        fs::rename(&tmp, &path).map_err(io(format!("renaming to {}", path.display())))?;
    }
    path.to_str().map(str::to_owned).ok_or(AsrError::NonUtf8Path(path.clone()))
}

fn vocab_from_tokens(tokens: &str) -> Result<String, AsrError> {
    let mut out = String::with_capacity(tokens.len() + tokens.len() / 4);
    for (n, line) in tokens.lines().enumerate() {
        if line.is_empty() {
            continue;
        }
        let (piece, id) = line
            .rsplit_once(' ')
            .and_then(|(p, id)| Some((p, id.trim().parse::<i64>().ok()?)))
            .ok_or_else(|| AsrError::InvalidConfig(format!("tokens.txt line {}: expected `piece id`", n + 1)))?;
        out.push_str(piece);
        out.push('\t');
        out.push_str(&(-id).to_string());
        out.push('\n');
    }
    Ok(out)
}

/// The per-stream hotword string: one entry per word or phrase, separated
/// by `/`. Whitespace is collapsed, `/` and control characters become
/// spaces, and empty or repeated entries are dropped.
pub(crate) fn encode<S: AsRef<str>>(words: &[S]) -> String {
    let mut seen: Vec<String> = Vec::new();
    for w in words {
        let cleaned: String = w.as_ref().chars().map(|c| if c == '/' || c.is_control() { ' ' } else { c }).collect();
        let word = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
        if !word.is_empty() && !seen.contains(&word) {
            seen.push(word);
        }
    }
    seen.join("/")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_become_a_ranked_vocabulary() {
        let v = vocab_from_tokens("<unk> 0\n▁the 7\nsolde 4021\n").unwrap();
        assert_eq!(v, "<unk>\t0\n▁the\t-7\nsolde\t-4021\n");
        assert!(vocab_from_tokens("broken\n").is_err());
    }

    #[test]
    fn hotwords_are_cleaned_and_joined() {
        assert_eq!(encode(&["Ysolde", " Port  Varrow ", "", "Ysolde", "a/b", "x\ny"]), "Ysolde/Port Varrow/a b/x y");
        assert_eq!(encode::<&str>(&[]), "");
    }
}
