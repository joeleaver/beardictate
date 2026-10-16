use super::*;

fn pad(segs: &[&str]) -> Pad {
    let mut p = Pad::default();
    for (i, s) in segs.iter().enumerate() {
        p.push(i as u64, s.to_string(), false);
    }
    p
}

fn text(p: &Pad) -> String {
    p.view(true).text()
}

#[test]
fn segments_join_and_show_raw_until_cleaned() {
    let mut p = pad(&["It was late.", "\n\nUm, she left."]);
    assert_eq!(text(&p), "It was late.\n\nUm, she left. ");
    let v = p.view(false);
    assert!(v.parts.iter().all(|part| !part.clean));
    let b = p.next_batch(false).unwrap();
    assert_eq!((b.raw.as_str(), b.context.as_str()), ("It was late.", ""));
    assert!(p.apply(b, "It was late."));
    assert_eq!(
        p.view(false).parts,
        [PadPart { text: "It was late.".into(), clean: true }, PadPart { text: "\n\nUm, she left. ".into(), clean: false }]
    );
    let b = p.next_batch(false).unwrap();
    assert_eq!((b.raw.as_str(), b.context.as_str()), ("\n\nUm, she left.", "It was late."));
    assert!(p.apply(b, "\n\nShe left."));
    assert!(!p.has_raw());
    assert_eq!(p.view(false).parts, [PadPart { text: "It was late.\n\nShe left. ".into(), clean: true }]);
}

#[test]
fn a_batch_is_the_current_paragraph_up_to_its_last_finished_sentence() {
    let mut p = pad(&["First paragraph.", "\n\nHe said"]);
    let b = p.next_batch(false).unwrap();
    assert_eq!(b.raw, "First paragraph.");
    assert!(p.apply(b, "First paragraph."));
    // "He said" isn't finished: nothing to clean yet, unless asked for all.
    assert_eq!(p.next_batch(false), None);
    assert_eq!(p.next_batch(true).unwrap().raw, "\n\nHe said");
    p.push(5, "where are you going.".into(), false);
    let b = p.next_batch(false).unwrap();
    assert_eq!(b.raw, "\n\nHe said where are you going.");
    assert_eq!(b.context, "First paragraph.");
    assert!(p.apply(b, "\n\nHe said, \u{201C}Where are you going?\u{201D}"));
    // The same paragraph is cleaned again, whole, when it grows.
    p.push(6, "She didn't answer.".into(), false);
    let b = p.next_batch(false).unwrap();
    assert_eq!(b.raw, "\n\nHe said, \u{201C}Where are you going?\u{201D} She didn't answer.");
    assert_eq!(b.context, "First paragraph.");
}

#[test]
fn a_break_said_at_the_end_of_a_segment_ends_the_batch() {
    let p = pad(&["It was late.\n\n", "Morning came."]);
    assert_eq!(p.next_batch(true).unwrap().raw, "It was late.\n\n");
}

#[test]
fn context_is_the_two_paragraphs_before() {
    let mut p = pad(&["One.", "\n\nTwo.", "\n\nThree.", "\n\nFour."]);
    for _ in 0..3 {
        let b = p.next_batch(false).unwrap();
        let raw = b.raw.clone();
        assert!(p.apply(b, &raw));
    }
    let b = p.next_batch(false).unwrap();
    assert_eq!((b.raw.as_str(), b.context.as_str()), ("\n\nFour.", "Two.\n\nThree."));
}

#[test]
fn a_result_for_a_pad_that_changed_meanwhile_is_dropped() {
    let mut p = pad(&["One.", "Two."]);
    let b = p.next_batch(false).unwrap();
    p.undo(false);
    assert!(!p.apply(b, "One. Two."));
    assert_eq!(text(&p), "One. ");
    // New segments after the batch don't stop it.
    let b = p.next_batch(false).unwrap();
    p.push(7, "Three.".into(), false);
    assert!(p.apply(b, "One!"));
    assert_eq!(text(&p), "One! Three. ");
}

#[test]
fn scratch_takes_one_segment_out_of_cleaned_text() {
    let mut p = Pad::default();
    p.push(1, "One.".into(), false);
    p.push(2, "Open \u{201C}two.".into(), false);
    p.push(3, "Three.".into(), true);
    let b = p.next_batch(false).unwrap();
    assert!(p.apply(b, "One. \u{201C}Two. Three."));
    assert_eq!(p.undo(false), Some(true), "the quotation was open before segment 3");
    assert_eq!(text(&p), "One. Open \u{201C}two. ");
    assert!(p.has_raw());
    assert_eq!(p.undo(false), Some(false));
    assert_eq!(p.undo(false), Some(false));
    assert!(p.is_empty());
    assert_eq!(p.undo(false), None);
}

#[test]
fn edits_change_the_last_match_and_undo() {
    let mut p = pad(&["Smith met smith.", "The smithy was cold."]);
    assert!(p.change("smith", "Smythe"));
    assert_eq!(text(&p), "Smith met Smythe. The smithy was cold. ");
    assert!(p.change("smith", "smythe"));
    assert_eq!(text(&p), "Smythe met Smythe. The smithy was cold. ", "takes the capital of what it replaces");
    assert!(!p.change("Jones", "Bob"));
    assert!(p.change("cold", ""));
    assert_eq!(text(&p), "Smythe met Smythe. The smithy was. ");
    assert!(p.change("The", ""));
    assert_eq!(text(&p), "Smythe met Smythe. smithy was. ");
    p.undo(false);
    p.undo(false);
    assert_eq!(text(&p), "Smythe met Smythe. The smithy was cold. ");
}

#[test]
fn clear_undoes_and_take_empties_for_good() {
    let mut p = pad(&["Keep this."]);
    assert!(p.clear());
    assert!(p.is_empty());
    assert!(!p.clear());
    p.undo(false);
    assert_eq!(text(&p), "Keep this. ");
    assert_eq!(p.take(), "Keep this.");
    assert!(p.is_empty());
    assert_eq!(p.undo(false), None);
}
