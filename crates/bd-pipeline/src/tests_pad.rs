//! Bear Pad mode, end to end with the scripted stages.

use super::*;

fn pad_rig(script: Vec<&'static str>, cleanup: bool) -> Rig {
    rig_with(script, cleanup, false, PipelineConfig { pad: true, ..PipelineConfig::default() })
}

fn sent(events: &[Event]) -> Option<&str> {
    events.iter().find_map(|e| match e {
        Event::PadSend { text } => Some(text.as_str()),
        _ => None,
    })
}

#[test]
fn the_pad_cleans_when_the_speaker_pauses_and_sends_on_command() {
    let r = pad_rig(vec!["Um, we stayed in.", "Bear, change stayed to slept.", "Bear, send it."], true);
    let rx = r.pipeline.events().clone();
    say(&r.pipeline, 0);
    say(&r.pipeline, 1);
    let events = until(&rx, |e| matches!(e, Event::Pad(v) if v.text() == "Um, we slept in. "));
    assert!(events.iter().all(|e| !matches!(e, Event::Text { .. })), "nothing is typed");
    // After the pause, the cleanup model cleans the pad.
    until(&rx, |e| matches!(e, Event::Pad(v) if v.text() == "We slept in. " && v.parts.iter().all(|p| p.clean)));
    say(&r.pipeline, 2);
    let events = until(&rx, |e| matches!(e, Event::PadSend { .. }));
    assert_eq!(sent(&events), Some("We slept in."));
    assert!(matches!(next(&rx), Event::Pad(v) if v.is_empty()));
}

#[test]
fn sending_cleans_what_is_left_first_and_scratch_that_takes_back_a_segment() {
    let r = pad_rig(vec!["Um, we went out.", "It rained.", "Scratch that.", "Bear, send it."], true);
    for i in 0..4 {
        say(&r.pipeline, i);
    }
    let events: Vec<Event> = r.pipeline.finish().try_iter().collect();
    assert_eq!(sent(&events), Some("We went out."));
    assert!(events.iter().all(|e| !matches!(e, Event::Text { .. })));
}

#[test]
fn without_cleanup_the_pad_sends_what_was_dictated() {
    let r = pad_rig(vec!["Um, we went out.", "New paragraph. Bear, send it."], false);
    say(&r.pipeline, 0);
    say(&r.pipeline, 1);
    let events: Vec<Event> = r.pipeline.finish().try_iter().collect();
    assert_eq!(sent(&events), Some("Um, we went out.\n\n"));
}

fn pad_vocab(list: &str) -> PipelineConfig {
    PipelineConfig { pad: true, vocabulary: bd_vocab::Vocabulary::parse(list).unwrap(), ..PipelineConfig::default() }
}

#[test]
fn the_pad_reads_on_after_a_comma_like_your_app() {
    for cleanup in [false, true] {
        let r = pad_rig(vec!["Hello,", "How are you?", "Thanks,", "Sarah said hi.", "Bear, send it."], cleanup);
        for i in 0..5 {
            say(&r.pipeline, i);
        }
        let events: Vec<Event> = r.pipeline.finish().try_iter().collect();
        assert_eq!(sent(&events), Some("Hello, how are you? Thanks, Sarah said hi."), "cleanup {cleanup}");
    }
}

#[test]
fn the_pad_keeps_a_special_words_own_spaces() {
    for cleanup in [false, true] {
        let r = rig_with(vec!["Ussel came.", "Then Ussel left.", "Bear, send it."], cleanup, false, pad_vocab("<space><space>Ysolde: Ussel"));
        for i in 0..3 {
            say(&r.pipeline, i);
        }
        let events: Vec<Event> = r.pipeline.finish().try_iter().collect();
        assert_eq!(sent(&events), Some("  Ysolde came. Then  Ysolde left."), "cleanup {cleanup}");
    }
}
