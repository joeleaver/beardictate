//! American or British spelling, made consistent: the recognizer writes
//! either, even in one sentence ("colouring … coloring", a tester,
//! 2026-10-20). Whole words, from VarCon's table of variant spellings (about
//! 12,000 words with their endings); nothing is guessed from a pattern, so
//! "promise" and "advertise" are left alone.
//!
//! A word only becomes the other spelling when that spelling never uses it
//! for any meaning: "check" (verify) is British too, so it never becomes
//! "cheque", while "cheque" always becomes "check".

use std::collections::HashMap;
use std::sync::OnceLock;

use bd_core::Spelling;

/// From VarCon (`data/make_spelling.py`): a word, then how American and
/// British spell it (empty where it stays).
const TABLE: &str = include_str!("../data/spelling.tsv");

struct Tables {
    to_american: HashMap<&'static str, &'static str>,
    to_british: HashMap<&'static str, &'static str>,
}

fn tables() -> &'static Tables {
    static TABLES: OnceLock<Tables> = OnceLock::new();
    TABLES.get_or_init(|| {
        let mut t = Tables { to_american: HashMap::new(), to_british: HashMap::new() };
        for line in TABLE.lines().filter(|l| !l.starts_with('#')) {
            let mut cols = line.split('\t');
            let (Some(word), Some(us), Some(uk)) = (cols.next(), cols.next(), cols.next()) else { continue };
            if !us.is_empty() {
                t.to_american.insert(word, us);
            }
            if !uk.is_empty() {
                t.to_british.insert(word, uk);
            }
        }
        t
    })
}

/// `text` in `spelling`. The special words in `keep`, where the whole of one
/// appears, stay as they are, and so does a word's casing: Colour, COLOUR,
/// colour.
pub fn respell(text: &str, spelling: Spelling, keep: &[String]) -> String {
    let table = match spelling {
        Spelling::Unchanged => return text.to_string(),
        Spelling::American => &tables().to_american,
        Spelling::British => &tables().to_british,
    };
    let phrases: Vec<Vec<String>> = keep
        .iter()
        .map(|k| crate::tokens(k).map(|t| k[t].to_lowercase()).collect::<Vec<_>>())
        .filter(|p| !p.is_empty())
        .collect();
    let toks: Vec<std::ops::Range<usize>> = crate::tokens(text).collect();
    // The Colour Sergeant's: a possessive is still the special word.
    let lower: Vec<String> = toks
        .iter()
        .map(|t| {
            let w = text[t.clone()].to_lowercase();
            ["'s", "\u{2019}s"].iter().find_map(|s| w.strip_suffix(s)).map_or(w.clone(), str::to_string)
        })
        .collect();
    let mut out = String::with_capacity(text.len());
    let mut copied = 0;
    let mut i = 0;
    while i < toks.len() {
        if let Some(n) = phrases.iter().map(Vec::len).filter(|&n| phrases.iter().any(|p| p.len() == n && lower.get(i..i + n) == Some(p.as_slice()))).max() {
            i += n;
            continue;
        }
        let tok = toks[i].clone();
        out.push_str(&text[copied..tok.start]);
        // Each part of "colour-coded" on its own.
        let parts: Vec<String> = text[tok.clone()].split('-').map(|p| respell_word(p, table)).collect();
        out.push_str(&parts.join("-"));
        copied = tok.end;
        i += 1;
    }
    out.push_str(&text[copied..]);
    out
}

fn respell_word(word: &str, table: &HashMap<&'static str, &'static str>) -> String {
    let Some(&new) = table.get(word.to_lowercase().as_str()) else {
        return word.to_string();
    };
    let mut chars = word.chars();
    let first_upper = chars.next().is_some_and(char::is_uppercase);
    if first_upper && word.chars().count() > 1 && word.chars().all(|c| !c.is_lowercase()) {
        return new.to_uppercase();
    }
    if first_upper {
        let mut c = new.chars();
        return c.next().map(|f| f.to_uppercase().chain(c).collect()).unwrap_or_default();
    }
    new.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn us(text: &str) -> String {
        respell(text, Spelling::American, &[])
    }

    fn uk(text: &str) -> String {
        respell(text, Spelling::British, &[])
    }

    #[test]
    fn a_testers_sentences_come_out_one_way() {
        assert_eq!(us("Colouring and coloring, the CENTRE was centred."), "Coloring and coloring, the CENTER was centered.");
        assert_eq!(uk("Colouring and coloring, the center was centered."), "Colouring and colouring, the centre was centred.");
        assert_eq!(us("I realise they've organised a catalogue of travellers."), "I realize they've organized a catalog of travelers.");
        assert_eq!(uk("I realize the gray, colorful neighborhood."), "I realise the grey, colourful neighbourhood.");
    }

    #[test]
    fn words_that_only_look_british_are_left_alone() {
        for word in ["promise", "advertise", "exercise", "surprise", "humorous", "vigorous", "honorary", "censor", "liter"] {
            assert_eq!(us(word), word, "{word}");
        }
        assert_eq!(uk("censor"), "censor");
    }

    #[test]
    fn american_twins_of_british_words_stay_american() {
        assert_eq!(us("Pay by cheque, check the tyres."), "Pay by check, check the tires.");
        assert_eq!(uk("Pay by check, check the tires and the meter."), "Pay by check, check the tires and the meter.");
        assert_eq!(us("She practised the programme."), "She practiced the program.");
    }

    #[test]
    fn hyphenated_words_special_words_and_as_heard() {
        assert_eq!(us("colour-coded"), "color-coded");
        let keep = vec!["Colour Sergeant".to_string()];
        assert_eq!(respell("the Colour Sergeant's colours", Spelling::American, &keep), "the Colour Sergeant's colors");
        assert_eq!(respell("colour", Spelling::Unchanged, &[]), "colour");
    }

    #[test]
    fn the_table_never_maps_a_word_to_itself_or_back_and_forth() {
        let t = tables();
        assert!(t.to_american.len() > 5000 && t.to_british.len() > 5000);
        for (from, to) in t.to_american.iter().chain(&t.to_british) {
            assert_ne!(from, to);
        }
        // Converting and converting back is stable: no word flip-flops.
        for (uk, us) in &t.to_american {
            assert!(!t.to_american.contains_key(us), "{uk} -> {us} -> again");
        }
    }
}
