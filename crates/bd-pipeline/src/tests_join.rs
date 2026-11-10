//! What's typed reads on from what's typed before it: no capital after a
//! comma, a special word's own spaces.

use super::*;
use crate::CaseMode;

fn vocab(list: &str) -> PipelineConfig {
    PipelineConfig { vocabulary: bd_vocab::Vocabulary::parse(list).unwrap(), ..PipelineConfig::default() }
}

#[test]
fn a_new_dictation_after_a_comma_reads_on_in_lower_case() {
    // Cleaned, after a stop and a start; the cleanup's capital ("Um, w"
    // becomes "W") goes too.
    let r = rig(vec!["Hello,", "Um, what now?"], true, false);
    let rx = r.pipeline.events().clone();
    say(&r.pipeline, 0);
    r.pipeline.end_session();
    let mut page = until(&rx, |e| *e == Event::SessionEnded);
    say(&r.pipeline, 1);
    page.extend(r.pipeline.finish().try_iter());
    assert_eq!(texts(&page), "Hello, what now?");
    // Within one dictation, a half sentence joined to the rest before
    // cleanup (the recognizer capitalizes each utterance).
    let r = rig(vec!["Dear,", "How are you?"], false, false);
    say(&r.pipeline, 0);
    say(&r.pipeline, 1);
    let events: Vec<Event> = r.pipeline.finish().try_iter().collect();
    assert_eq!(texts(&events), "Dear, how are you?");
}

#[test]
fn names_keep_their_capitals_and_a_new_passage_its_own() {
    let r = rig(vec!["Hello,", "How are you?", "Thanks,", "Sarah said hi.", "Well,", "I said so.", "Wait,", "Then go."], false, false);
    let rx = r.pipeline.events().clone();
    let mut page = Vec::new();
    for line in 0..8 {
        if line == 7 {
            // A new passage: the page could be anywhere.
            r.pipeline.new_passage();
        }
        say(&r.pipeline, line);
        r.pipeline.end_session();
        page.extend(until(&rx, |e| *e == Event::SessionEnded));
    }
    assert_eq!(texts(&page), "Hello, how are you? Thanks, Sarah said hi. Well, I said so. Wait, Then go.");
}

#[test]
fn a_case_mode_keeps_its_own_capitals() {
    let config = PipelineConfig { case_mode: CaseMode::Upper, ..PipelineConfig::default() };
    let r = rig_with(vec!["Hello,", "A dog."], false, false, config);
    say(&r.pipeline, 0);
    say(&r.pipeline, 1);
    let events: Vec<Event> = r.pipeline.finish().try_iter().collect();
    assert_eq!(texts(&events), "HELLO, A DOG.");
}

#[test]
fn a_special_words_own_spaces_are_typed() {
    let r = rig_with(vec!["Ussel came.", "Then Ussel left."], true, false, vocab("<space><space>Ysolde: Ussel"));
    say(&r.pipeline, 0);
    say(&r.pipeline, 1);
    let events: Vec<Event> = r.pipeline.finish().try_iter().collect();
    // At the passage's start, and two where one would be.
    assert_eq!(texts(&events), "  Ysolde came. Then  Ysolde left.");
    let r = rig_with(vec!["Thanks. Sign off."], true, false, vocab("Regards,<new line><space><space>Ysolde: sign off"));
    say(&r.pipeline, 0);
    let events: Vec<Event> = r.pipeline.finish().try_iter().collect();
    assert_eq!(texts(&events), "Thanks. Regards,\n  Ysolde.");
}
