//! American or British spelling, made consistent: the recognizer learned
//! both and writes either, even in one sentence ("colouring … coloring",
//! a tester, 2026-10-20). Whole words from a fixed list, with their usual
//! endings; nothing is guessed from a pattern, so "promise" and "advertise"
//! are left alone.
//!
//! Some British words are another word in American English: a cheque is a
//! check, but a check is not a cheque. Those only ever become American.

use std::collections::HashMap;
use std::sync::OnceLock;

use bd_core::Spelling;

/// -or / -our: color, colour.
const OUR: &[&str] = &[
    "col", "discol", "flav", "hon", "dishon", "hum", "lab", "neighb", "behavi", "misbehavi", "fav", "disfav", "harb",
    "rum", "vap", "vig", "val", "savi", "sav", "splend", "arm", "od", "parl", "endeav", "tum", "clam", "ranc", "cand",
    "ard", "ferv", "rig", "demean", "arb",
];
const OUR_ENDINGS: &[&str] =
    &["", "s", "ed", "ing", "ful", "fully", "less", "able", "ably", "ite", "ites", "er", "ers", "hood", "hoods", "ly", "y"];

/// -er / -re: center, centre.
const RE: &[&str] = &["cent", "theat", "fib", "calib", "sab", "somb", "spect", "lust", "meag", "sepulch", "scept", "lit", "mit"];
/// (American, British) endings after the stem.
const RE_ENDINGS: &[(&str, &str)] = &[("er", "re"), ("ers", "res"), ("ered", "red"), ("ering", "ring")];

/// -ize / -ise: realize, realise.
const IZE: &[&str] = &[
    "real", "organ", "recogn", "apolog", "critic", "emphas", "priorit", "summar", "memor", "final", "special", "minim",
    "maxim", "util", "character", "author", "civil", "sympath", "legal", "normal", "stabil", "visual", "symbol",
    "standard", "modern", "categor", "custom", "harmon", "idol", "jeopard", "mobil", "neutral", "optim", "patron",
    "penal", "public", "scrutin", "terror", "theor", "vandal", "verbal", "agon", "antagon", "capital", "central",
    "colon", "fantas", "fertil", "hospital", "hypnot", "immun", "item", "local", "monopol", "moral", "popular", "privat",
    "rational", "revolution", "steril", "subsid", "synchron", "tantal", "traumat", "material", "energ", "equal",
    "familiar", "general", "global", "human", "initial", "internal", "italic", "legitim", "marginal", "metabol", "sanit",
    "urban", "vocal", "western", "actual", "alphabet", "bapt", "brutal", "canon", "caramel", "carbon", "cauter",
    "commercial", "computer", "democrat", "demon", "deodor", "dramat", "econom", "empath", "eulog", "evangel",
    "external", "factor", "fossil", "fratern", "galvan", "homogen", "hybrid", "ideal", "immortal", "individual",
    "industrial", "ion", "journal", "liberal", "lion", "magnet", "martyr", "mechan", "mesmer", "militar", "miniatur",
    "mineral", "moistur", "motor", "natural", "national", "ostrac", "oxid", "particular", "pasteur", "personal",
    "philosoph", "plagiar", "polar", "politic", "pressur", "professional", "pulver", "radical", "random", "regular",
    "romantic", "satir", "scandal", "secular", "sensational", "sensit", "serial", "sermon", "sexual", "social",
    "solemn", "spiritual", "stigmat", "systemat", "trivial", "tyrann", "union", "victim", "vulcan", "colonial",
];
const IZE_ENDINGS: &[&str] = &["e", "es", "ed", "ing", "ation", "ations", "er", "ers", "able"];

/// -yze / -yse: analyze, analyse.
const YZE: &[&str] = &["anal", "paral", "catal", "breathal", "electrol", "dial"];
const YZE_ENDINGS: &[&str] = &["e", "es", "ed", "ing", "er", "ers"];

/// One l before an ending in American English, two in British: traveled,
/// travelled.
const LL: &[&str] = &[
    "travel", "cancel", "label", "model", "level", "signal", "total", "fuel", "marvel", "quarrel", "tunnel", "channel",
    "shovel", "grovel", "revel", "dial", "duel", "equal", "funnel", "pedal", "pencil", "panel", "refuel", "unravel",
    "ravel", "yodel", "bevel", "chisel", "enamel", "libel", "spiral", "counsel", "jewel", "barrel", "cudgel", "drivel",
    "gravel", "marshal", "parcel", "rival", "snivel", "swivel", "towel", "shrivel", "stencil", "pummel", "carol",
    "dishevel", "remodel", "relabel",
];
const LL_ENDINGS: &[&str] = &["ed", "ing", "er", "ers"];

/// (American, British), either way.
const PAIRS: &[(&str, &str)] = &[
    ("gray", "grey"), ("grays", "greys"), ("grayed", "greyed"), ("graying", "greying"), ("grayish", "greyish"),
    ("grayer", "greyer"), ("grayest", "greyest"), ("grayness", "greyness"),
    ("plow", "plough"), ("plows", "ploughs"), ("plowed", "ploughed"), ("plowing", "ploughing"),
    ("mold", "mould"), ("molds", "moulds"), ("molded", "moulded"), ("molding", "moulding"), ("moldy", "mouldy"),
    ("molder", "moulder"), ("molt", "moult"), ("molted", "moulted"), ("molting", "moulting"),
    ("smolder", "smoulder"), ("smolders", "smoulders"), ("smoldered", "smouldered"), ("smoldering", "smouldering"),
    ("aluminum", "aluminium"), ("mustache", "moustache"), ("mustaches", "moustaches"), ("pajamas", "pyjamas"),
    ("jewelry", "jewellery"), ("cozy", "cosy"), ("cozier", "cosier"), ("coziest", "cosiest"), ("coziness", "cosiness"),
    ("skeptic", "sceptic"), ("skeptics", "sceptics"), ("skeptical", "sceptical"), ("skeptically", "sceptically"),
    ("skepticism", "scepticism"), ("aging", "ageing"), ("artifact", "artefact"), ("artifacts", "artefacts"),
    ("catalog", "catalogue"), ("catalogs", "catalogues"), ("cataloged", "catalogued"), ("cataloging", "cataloguing"),
    ("defense", "defence"), ("defenses", "defences"), ("defenseless", "defenceless"), ("offense", "offence"),
    ("offenses", "offences"), ("pretense", "pretence"), ("pretenses", "pretences"),
    ("anemia", "anaemia"), ("anemic", "anaemic"), ("anesthesia", "anaesthesia"), ("anesthetic", "anaesthetic"),
    ("anesthetics", "anaesthetics"), ("anesthetist", "anaesthetist"), ("encyclopedia", "encyclopaedia"),
    ("encyclopedias", "encyclopaedias"), ("pediatric", "paediatric"), ("pediatrician", "paediatrician"),
    ("esophagus", "oesophagus"), ("estrogen", "oestrogen"), ("fetus", "foetus"), ("fetuses", "foetuses"),
    ("fetal", "foetal"), ("hemorrhage", "haemorrhage"), ("hemoglobin", "haemoglobin"), ("leukemia", "leukaemia"),
    ("orthopedic", "orthopaedic"), ("diarrhea", "diarrhoea"), ("sulfur", "sulphur"), ("behoove", "behove"),
    ("behooves", "behoves"), ("mollusk", "mollusc"), ("mollusks", "molluscs"), ("omelet", "omelette"),
    ("omelets", "omelettes"), ("carburetor", "carburettor"), ("tidbit", "titbit"), ("tidbits", "titbits"),
    ("yogurt", "yoghurt"), ("maneuver", "manoeuvre"), ("maneuvers", "manoeuvres"), ("maneuvered", "manoeuvred"),
    ("maneuvering", "manoeuvring"), ("maneuverable", "manoeuvrable"), ("kilometer", "kilometre"),
    ("kilometers", "kilometres"), ("centimeter", "centimetre"), ("centimeters", "centimetres"),
    ("millimeter", "millimetre"), ("millimeters", "millimetres"), ("enroll", "enrol"), ("enrolls", "enrols"),
    ("enrollment", "enrolment"), ("enrollments", "enrolments"), ("fulfill", "fulfil"), ("fulfills", "fulfils"),
    ("fulfillment", "fulfilment"), ("instill", "instil"), ("instills", "instils"), ("distill", "distil"),
    ("distills", "distils"), ("installment", "instalment"), ("installments", "instalments"), ("skillful", "skilful"),
    ("skillfully", "skilfully"), ("willful", "wilful"), ("willfully", "wilfully"), ("enthrall", "enthral"),
    ("enthralls", "enthrals"), ("counselor", "counsellor"), ("counselors", "counsellors"), ("panelist", "panellist"),
    ("panelists", "panellists"), ("medalist", "medallist"), ("medalists", "medallists"), ("duelist", "duellist"),
    ("crueler", "crueller"), ("cruelest", "cruellest"), ("woolen", "woollen"), ("woolens", "woollens"),
];

/// (British, American): only ever made American. In American English the
/// British spelling's twin is another word (check, tire, curb, meter,
/// program, license the verb, practice the noun).
const TO_AMERICAN_ONLY: &[(&str, &str)] = &[
    ("cheque", "check"), ("cheques", "checks"), ("chequebook", "checkbook"), ("tyre", "tire"), ("tyres", "tires"),
    ("kerb", "curb"), ("kerbs", "curbs"), ("metre", "meter"), ("metres", "meters"), ("programme", "program"),
    ("programmes", "programs"), ("licence", "license"), ("licences", "licenses"), ("practise", "practice"),
    ("practises", "practices"), ("practised", "practiced"), ("practising", "practicing"), ("learnt", "learned"),
    ("storeys", "stories"), ("pyjama", "pajama"), ("aeroplane", "airplane"), ("aeroplanes", "airplanes"),
];

/// (American, British) for every word the list knows.
fn pairs() -> Vec<(String, String)> {
    let mut out = Vec::new();
    for stem in OUR {
        for e in OUR_ENDINGS {
            out.push((format!("{stem}or{e}"), format!("{stem}our{e}")));
        }
    }
    for stem in RE {
        for (us, uk) in RE_ENDINGS {
            out.push((format!("{stem}{us}"), format!("{stem}{uk}")));
        }
    }
    for stem in IZE {
        for e in IZE_ENDINGS {
            out.push((format!("{stem}iz{e}"), format!("{stem}is{e}")));
        }
    }
    for stem in YZE {
        for e in YZE_ENDINGS {
            out.push((format!("{stem}yz{e}"), format!("{stem}ys{e}")));
        }
    }
    for stem in LL {
        for e in LL_ENDINGS {
            out.push((format!("{stem}{e}"), format!("{stem}l{e}")));
        }
    }
    out.extend(PAIRS.iter().map(|(us, uk)| (us.to_string(), uk.to_string())));
    out
}

struct Tables {
    to_american: HashMap<String, String>,
    to_british: HashMap<String, String>,
}

fn tables() -> &'static Tables {
    static TABLES: OnceLock<Tables> = OnceLock::new();
    TABLES.get_or_init(|| {
        let pairs = pairs();
        let mut to_american: HashMap<String, String> = pairs.iter().map(|(us, uk)| (uk.clone(), us.clone())).collect();
        to_american.extend(TO_AMERICAN_ONLY.iter().map(|(uk, us)| (uk.to_string(), us.to_string())));
        let to_british = pairs.into_iter().collect();
        Tables { to_american, to_british }
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

fn respell_word(word: &str, table: &HashMap<String, String>) -> String {
    let Some(new) = table.get(&word.to_lowercase()) else {
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
    new.clone()
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
    fn no_word_is_both_spellings() {
        // A word spelled the same either way would flip-flop.
        for (us, uk) in pairs() {
            assert_ne!(us, uk);
        }
    }
}
