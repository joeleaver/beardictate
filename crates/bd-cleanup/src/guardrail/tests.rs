use super::*;

/// (input, output) pairs that must be accepted.
const ACCEPT: &[(&str, &str)] = &[
    // Identity and pure formatting.
    ("He left.", "He left."),
    ("he left", "He left."),
    ("HE LEFT", "he left"),
    ("where are you going she asked", "“Where are you going?” she asked."),
    ("Where are you going? She asked.", "“Where are you going?” she asked."),
    ("to the river he said", "“To the river,” he said."),
    ("to the river he said", "\n\n“To the river,” he said."),
    ("He looked up. I told you, he said.", "He looked up.\n\n“I told you,” he said."),
    ("it was well known", "It was well-known."),
    ("it was well-known", "It was well known."),
    ("he paused then ran", "He paused—then ran."),
    ("he paused - then ran", "He paused – then ran."),
    ("wait", "Wait…"),
    // Apostrophes and quotes in every typographic form.
    ("didn't he know", "Didn’t he know?"),
    ("Didn’t he know", "didn't he know"),
    ("the dogs' bowls", "the dogs’ bowls"),
    ("Tis the season", "‘Tis the season."),
    ("get down Tom hissed", "‘Get down,’ Tom hissed."),
    ("get down Tom hissed", "\"Get down,\" Tom hissed."),
    // Fillers.
    ("um I think so", "I think so."),
    ("I uh think um so", "I think so."),
    ("Er, ah, hmm, well, yes.", "Well, yes."),
    ("it's been flooded, you know, since Tuesday", "It's been flooded since Tuesday."),
    ("it was cold you know, really cold", "It was cold, really cold."),
    ("it was, like, really cold", "It was really cold."),
    ("like I said, no", "I said, no."),
    ("it was um like really cold", "It was really cold."),
    // Fillers kept is fine too.
    ("um I think so", "Um, I think so."),
    // Noise tags and fragments.
    ("just as the [cough] the headlights swept", "just as the headlights swept"),
    ("[noise] He left.", "He left."),
    ("th- the door opened", "The door opened."),
    ("I w- went home", "I went home."),
    // Repeats, with and without fillers between.
    ("Mara was was starting", "Mara was starting"),
    ("from the the map", "from the map"),
    ("I told I told you", "I told you"),
    ("to the river to the river", "to the river"),
    ("I, um, I think", "I think"),
    ("the the the end", "the end"),
    ("he folded the map and and stood up", "He folded the map and stood up."),
    // False starts.
    ("I went to the — I walked to the store", "I walked to the store."),
    ("She was... she is here", "She is here."),
    ("I went, I walked home", "I walked home."),
    ("The old man walked, uh, the old man limped", "The old man limped."),
    ("The old man walked, the old man limped", "The old man limped."),
    ("I know. I just, I needed to hear", "I know. I just needed to hear"),
    ("He said, he said it twice", "He said it twice."),
    // Command marks preserved, possibly moved within their gap.
    ("“get down” Tom hissed\n\nShe dropped", "“Get down,” Tom hissed.\n\nShe dropped."),
    ("“um get down” Tom hissed", "“Get down,” Tom hissed."),
    ("He said “run”", "He said, “Run.”"),
    ("Tom hissed.\n\nShe ran.", "Tom hissed.\n\n\n\nShe ran."),
    ("Dear John,\nI miss you.", "Dear John,\nI miss you."),
    ("Dear John,\nI miss you.", "Dear John,\n\nI miss you."),
    // Nothing left after removing fillers.
    ("Um.", ""),
    ("", ""),
];

/// (input, output) pairs that must be rejected.
const REJECT: &[(&str, &str)] = &[
    // Insertions.
    ("he left", "He left quickly."),
    ("he left", "Then he left."),
    ("she asked", "she asked him"),
    ("", "Hello."),
    // Substitutions, including expansions and number words.
    ("he left", "He departed."),
    ("I don't know", "I do not know."),
    ("it's late", "It is late."),
    ("I do not know", "I don't know."),
    ("three days", "3 days"),
    ("3 days", "three days"),
    ("gonna go", "going to go"),
    ("colour", "color"),
    // Reordering.
    ("he quickly left", "He left quickly."),
    ("Tom hissed get down", "“Get down,” Tom hissed."),
    // Dropping real words.
    ("he left the house", "He left."),
    ("she looked like a ghost", "She looked a ghost."),
    ("I like him", "I him."),
    ("you see", "See."),
    ("you know nothing", "Know nothing."),
    ("he said you know nothing", "He said nothing."),
    ("it's been flooded you know since Tuesday", "It's been flooded since Tuesday."),
    ("very very old", "very old very"),
    ("He opened the door and he walked in", "He walked in."),
    ("He opened the door, he walked in", "He walked in."),
    ("He opened the door, she walked in", "He opened the door, walked in."),
    ("He ran and she, she hid", "He ran and hid."),
    // Partial repeat removal.
    ("I want it I want it", "I it I want it"),
    // Dropping command marks.
    ("“get down” Tom hissed", "Get down, Tom hissed."),
    ("“get down” Tom hissed", "“Get down, Tom hissed."),
    ("Tom hissed.\n\nShe ran.", "Tom hissed. She ran."),
    ("Dear John,\nI miss you.", "Dear John, I miss you."),
    ("\n\nThe end", "The end"),
    // Re-emitting the context or chatter.
    ("he left", "Here is the cleaned text: He left."),
    ("he left", "He left.\n\nNote: removed nothing."),
];

#[test]
fn accepts_allowed_edits() {
    for (input, output) in ACCEPT {
        let v = review(input, output);
        assert!(v.is_accepted(), "should accept {input:?} -> {output:?}: {v:?}");
    }
}

#[test]
fn rejects_everything_else() {
    for (input, output) in REJECT {
        let v = review(input, output);
        assert!(!v.is_accepted(), "should reject {input:?} -> {output:?}");
    }
}

#[test]
fn reports_what_was_removed_and_why() {
    let v = review("Mara was was um starting [cough] th- to", "Mara was starting to");
    let GuardrailVerdict::Accepted { removed } = v else { panic!("rejected") };
    let reasons: Vec<_> = removed.iter().map(|r| (r.words.as_str(), r.reason)).collect();
    assert_eq!(
        reasons,
        [
            ("was", RemovalReason::Repeat),
            ("um", RemovalReason::Filler),
            ("[cough]", RemovalReason::NoiseTag),
            ("th", RemovalReason::Fragment),
        ]
    );
}

#[test]
fn explains_substitution_insertion_drop_and_reorder() {
    let v = review("he left the house", "He departed the house.");
    assert_eq!(
        v,
        GuardrailVerdict::Rejected(Rejection::WordsChanged {
            inserted: vec![],
            dropped: vec![],
            substituted: vec![("left".into(), "departed".into())],
            reordered: vec![],
        })
    );
    let GuardrailVerdict::Rejected(r) = review("he left", "Then he left quickly.") else { panic!() };
    assert_eq!(
        r,
        Rejection::WordsChanged {
            inserted: vec!["Then".into(), "quickly".into()],
            dropped: vec![],
            substituted: vec![],
            reordered: vec![],
        }
    );
    let GuardrailVerdict::Rejected(r) = review("he quickly left", "He left quickly.") else { panic!() };
    assert!(matches!(&r, Rejection::WordsChanged { reordered, .. } if reordered == &["quickly"]), "{r:?}");
    let GuardrailVerdict::Rejected(r) = review("she looked like a ghost", "She looked a ghost.") else { panic!() };
    assert!(matches!(&r, Rejection::WordsChanged { dropped, .. } if dropped == &["like"]), "{r:?}");
    assert!(r.to_string().contains("dropped like"));
}

#[test]
fn explains_lost_marks() {
    assert_eq!(
        review("“get down” Tom hissed", "Get down, Tom hissed."),
        GuardrailVerdict::Rejected(Rejection::QuoteLost { before: Some("Get".into()) })
    );
    assert_eq!(
        review("Tom hissed.\n\nShe ran.", "Tom hissed. She ran."),
        GuardrailVerdict::Rejected(Rejection::BreakLost { before: Some("She".into()) })
    );
    assert_eq!(
        review("He ran.\n\n", "He ran."),
        GuardrailVerdict::Rejected(Rejection::BreakLost { before: None })
    );
}

/// Long input: the alignment stays fast and correct well past a segment.
#[test]
fn long_passage() {
    let sentence = "the the rain um had not stopped for three days and Mara was was starting to think it never would ";
    let input = sentence.repeat(40);
    let output = "The rain had not stopped for three days, and Mara was starting to think it never would. ".repeat(40);
    assert!(review(&input, &output).is_accepted());
    let mut changed = output.clone();
    changed.push_str("Amen.");
    assert!(!review(&input, &changed).is_accepted());
}

#[test]
fn context_echo_only_at_the_start() {
    let ctx = "“There's something under the floorboards, something that";
    assert!(review_in_context(ctx, "Something that moves at night.", "moves at night.”").is_accepted());
    assert!(!review("Something that moves at night.", "moves at night.”").is_accepted());
    // Only words that end the context, and only at the segment's start.
    assert!(!review_in_context(ctx, "Under the bed it moves.", "the bed it moves.").is_accepted());
    assert!(!review_in_context(ctx, "It moves, something that", "It moves,").is_accepted());
}
