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
/// by `/`, each ending in ` :boost` (sherpa-onnx's per-hotword score, which
/// overrides the recognizer's). Whitespace is collapsed, `/`, `:` and
/// control characters become spaces, and empty or repeated entries are
/// dropped. A boost of 0 or less means no hotwords.
pub(crate) fn encode<S: AsRef<str>>(words: &[S], boost: f32) -> String {
    if !(boost.is_finite() && boost > 0.0) {
        return String::new();
    }
    let mut seen: Vec<String> = Vec::new();
    for w in words {
        let cleaned: String =
            w.as_ref().chars().map(|c| if c == '/' || c == ':' || c.is_control() { ' ' } else { c }).collect();
        let word = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
        if !word.is_empty() && !seen.contains(&word) {
            seen.push(word);
        }
    }
    seen.iter().map(|w| format!("{w} :{boost:.2}")).collect::<Vec<_>>().join("/")
}

/// Drops back-to-back copies of a hotword: "Buttons Buttons Buttons is" ->
/// "Buttons is". Beam search with hotwords can loop on a boosted word with
/// the Parakeet Unified model (an RNNT, which may emit several tokens per
/// frame): at boost 2.0 it did once in 16 TTS sentences, at 2.5 three times.
/// Only copies separated by bare spaces count, so "Buttons, Buttons!" stays.
/// Case is ignored; the last copy, which carries any punctuation, is kept.
pub(crate) fn collapse_repeats<S: AsRef<str>>(text: &str, words: &[S]) -> String {
    let mut tokens: Vec<&str> = text.split_whitespace().collect();
    let mut changed = false;
    for word in words {
        let phrase: Vec<&str> = word.as_ref().split_whitespace().collect();
        let n = phrase.len();
        if n == 0 {
            continue;
        }
        let same = |t: &str, p: &str| t.eq_ignore_ascii_case(p);
        // The last token of the second copy may end in punctuation.
        let same_end = |t: &str, p: &str| same(t.trim_end_matches(|c: char| c.is_ascii_punctuation()), p);
        let mut i = 0;
        while i + 2 * n <= tokens.len() {
            let first = (0..n).all(|k| same(tokens[i + k], phrase[k]));
            let second = (0..n).all(|k| {
                let t = tokens[i + n + k];
                if k == n - 1 { same_end(t, phrase[k]) } else { same(t, phrase[k]) }
            });
            if first && second {
                tokens.drain(i..i + n);
                changed = true;
            } else {
                i += 1;
            }
        }
    }
    if changed { tokens.join(" ") } else { text.to_owned() }
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
        assert_eq!(
            encode(&["Ysolde", " Port  Varrow ", "", "Ysolde", "a/b", "x\ny", "c:d"], 2.0),
            "Ysolde :2.00/Port Varrow :2.00/a b :2.00/x y :2.00/c d :2.00"
        );
        assert_eq!(encode::<&str>(&[], 2.0), "");
        assert_eq!(encode(&["Ysolde"], 0.0), "");
    }

    #[test]
    fn repeated_hotwords_collapse() {
        let words = ["Buttons", "Port Varrow"];
        assert_eq!(collapse_repeats("Buttons Buttons Buttons is a cat.", &words), "Buttons is a cat.");
        assert_eq!(collapse_repeats("Did Buttons buttons eat it?", &words), "Did buttons eat it?");
        assert_eq!(collapse_repeats("Sat by Buttons Buttons.", &words), "Sat by Buttons.");
        assert_eq!(collapse_repeats("To Port Varrow Port Varrow, then home.", &words), "To Port Varrow, then home.");
        // Not back to back, or not a hotword: left alone.
        assert_eq!(collapse_repeats("Buttons, Buttons! Come here.", &words), "Buttons, Buttons! Come here.");
        assert_eq!(collapse_repeats("very  very good", &words), "very  very good");
        assert_eq!(collapse_repeats("Port Port Varrow", &words), "Port Port Varrow");
        assert_eq!(collapse_repeats("Buttons Buttons", &[] as &[&str]), "Buttons Buttons");
    }
}
