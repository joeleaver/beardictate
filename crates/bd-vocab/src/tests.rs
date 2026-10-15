use super::*;

fn vocab(list: &str) -> Vocabulary {
    Vocabulary::parse(list).unwrap()
}

#[test]
fn heard_forms_become_the_spelling() {
    let v = vocab("Ysolde: Ussel, Isolde\nwyrmstone: worm stone, wormstone\nPort Varrow: Port Vera\nVarrow");
    assert_eq!(
        v.apply("Ussel handed the worm stone to Captain Thrane."),
        "Ysolde handed the wyrmstone to Captain Thrane."
    );
    assert_eq!(v.apply("They sailed into Port Vera at dawn."), "They sailed into Port Varrow at dawn.");
    assert_eq!(v.apply("varrow was quiet."), "Varrow was quiet.");
}

#[test]
fn casing_follows_the_word_and_the_sentence() {
    let v = vocab("wyrmstone: wormstone\nMcAllister");
    assert_eq!(v.apply("Wormstone glowed. The Wormstone dimmed."), "Wyrmstone glowed. The wyrmstone dimmed.");
    assert_eq!(v.apply("“Wormstone,” she said."), "“Wyrmstone,” she said.");
    assert_eq!(v.apply("Ask mcallister."), "Ask McAllister.");
    assert_eq!(v.apply("Done.\n\nWormstone again."), "Done.\n\nWyrmstone again.");
}

#[test]
fn matches_are_whole_words_within_a_phrase() {
    let v = vocab("wyrmstone: worm stone\nThrane: Train");
    // Not across punctuation, and not inside longer words.
    assert_eq!(v.apply("A worm. Stone walls."), "A worm. Stone walls.");
    assert_eq!(v.apply("The worm, stone cold."), "The worm, stone cold.");
    assert_eq!(v.apply("Trainers and training."), "Trainers and training.");
    assert_eq!(v.apply("Captain Train's ship."), "Captain Thrane's ship.");
    assert_eq!(v.apply("Captain Train’s ship."), "Captain Thrane’s ship.");
}

#[test]
fn the_list_format() {
    let v = vocab("# names\n\n  Ysolde : Isolde ,  Ussel  \nKaelith\n");
    assert_eq!(v.words()[0], Word { text: "Ysolde".into(), heard: vec!["Isolde".into(), "Ussel".into()] });
    assert_eq!(v.words()[1], Word { text: "Kaelith".into(), heard: vec![] });
    assert_eq!(v.hotwords(), ["Ysolde", "Kaelith"]);
    assert_eq!(Vocabulary::parse("ok\n: nothing").unwrap_err().line, 2);
    assert!(Vocabulary::default().is_empty());
    assert_eq!(Vocabulary::default().apply("Same text."), "Same text.");
}

#[test]
fn hyphens_and_apostrophes_stay_inside_words() {
    let v = vocab("Fenmere: Fen-mere\nO'Dara: O'Darra");
    assert_eq!(v.apply("Fen-mere and O'Darra met."), "Fenmere and O'Dara met.");
}
