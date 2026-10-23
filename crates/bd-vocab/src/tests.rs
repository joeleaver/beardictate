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

#[test]
fn a_spelling_can_ask_for_breaks() {
    use bd_core::{LINE_BREAK as L, PARAGRAPH_BREAK as P};
    let v = Vocabulary::new(vec![
        Word { text: "Best wishes,<new line>Ysolde".into(), heard: vec!["sign off".into()] },
        Word { text: "Yours,\\n\\nJoe".into(), heard: vec!["letter end".into()] },
    ]);
    assert_eq!(v.apply("Thanks again. Sign off."), format!("Thanks again. Best wishes, {L} Ysolde."));
    assert_eq!(v.apply("letter end"), format!("Yours, {P} Joe"));
    // What the recognizer listens for and the cleanup keeps has no breaks.
    assert_eq!(v.spellings(), ["Best wishes, Ysolde", "Yours, Joe"]);
    assert_eq!(v.hotwords(), v.spellings());
    // Saying the words themselves gives the breaks too.
    assert_eq!(v.apply("best wishes Ysolde"), format!("Best wishes, {L} Ysolde"));
    assert_eq!(breaks_as_text(&v.apply("Thanks. Sign off.")), "Thanks. Best wishes,\nYsolde.");
}

#[test]
fn break_forms_ignore_case_and_paragraphs_win() {
    let v = vocab("Regards,<PARAGRAPH>Ann: regards ann\nA<Line>B: ab\nC\\nD: cd");
    assert_eq!(breaks_as_text(&v.apply("regards ann")), "Regards,\n\nAnn");
    assert_eq!(breaks_as_text(&v.apply("ab")), "A\nB");
    assert_eq!(breaks_as_text(&v.apply("cd")), "C\nD");
}

#[test]
fn windows_returns_and_other_spellings_of_a_break() {
    // A tester's word, typed with a literal "\r" (2026-10-22).
    let v = Vocabulary::new(vec![Word { text: "A:\\r B:".into(), heard: vec!["a colon b colon".into()] }]);
    assert_eq!(breaks_as_text(&v.apply("A colon B colon.")), "A:\nB:");
    let v = vocab("E<Enter>F: ef\nG<br>H: gh\nI\\r\\nJ: ij\nK\\r\\n\\r\\nL: kl");
    assert_eq!(breaks_as_text(&v.apply("ef")), "E\nF");
    assert_eq!(breaks_as_text(&v.apply("gh")), "G\nH");
    assert_eq!(breaks_as_text(&v.apply("ij")), "I\nJ");
    assert_eq!(breaks_as_text(&v.apply("kl")), "K\n\nL");
}

#[test]
fn a_spelling_ending_in_a_mark_drops_the_recognizers() {
    let v = Vocabulary::new(vec![Word { text: "A: B:".into(), heard: vec!["a colon b colon".into()] }]);
    assert_eq!(v.apply("A colon B colon."), "A: B:");
    assert_eq!(v.apply("Use the A colon B colon for this."), "Use the A: B: for this.");
    assert_eq!(v.apply("A colon B colon?"), "A: B:?");
    // Spellings ending in a letter keep what follows.
    assert_eq!(vocab("wyrmstone: worm stone").apply("The worm stone."), "The wyrmstone.");
}
