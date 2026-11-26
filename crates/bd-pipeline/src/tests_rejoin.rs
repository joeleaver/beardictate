//! A sentence the listener closed at a thinking pause, opened again when
//! the next words carry it on (`stages/rejoin.rs`). The script recognizer
//! can't hold its stream, so every utterance is closed as it ends.

use super::*;

fn rejoining() -> PipelineConfig {
    PipelineConfig { rejoin: true, ..PipelineConfig::default() }
}

const TO_THE: [&str; 4] = ["I went", "I went down", "I went down to", "I went down to the"];
const SHOP: [&str; 4] = ["Shop", "Shop on", "Shop on Friday", "Shop on Friday"];

fn rejoined(events: &[Event]) -> Vec<(&'static str, String)> {
    events
        .iter()
        .filter_map(|e| match e {
            Event::Rejoined { how, erased, .. } => Some((*how, erased.clone())),
            _ => None,
        })
        .collect()
}

#[test]
fn a_sentence_closed_on_a_word_it_cant_end_on_goes_on() {
    let p = live_rig(vec![TO_THE.to_vec(), SHOP.to_vec()], rejoining());
    say(&p, 0);
    say(&p, 1);
    let events: Vec<Event> = p.finish().try_iter().collect();
    assert_eq!(page(&events), "I went down to the shop on Friday.");
    assert_eq!(rejoined(&events), vec![("dangling", ".".to_string())]);
}

#[test]
fn or_when_the_next_words_only_carry_one_on() {
    let p = live_rig(vec![vec!["It's the", "It's the sort of", "It's the sort of tool"], vec!["That", "That lasts", "That lasts for years", "That lasts for years."]], rejoining());
    say(&p, 0);
    say(&p, 1);
    let events: Vec<Event> = p.finish().try_iter().collect();
    assert_eq!(page(&events), "It's the sort of tool that lasts for years.");
    assert_eq!(rejoined(&events), vec![("continues", ".".to_string())]);
}

#[test]
fn off_by_default() {
    let p = live_rig(vec![TO_THE.to_vec(), SHOP.to_vec()], PipelineConfig::default());
    say(&p, 0);
    say(&p, 1);
    let events: Vec<Event> = p.finish().try_iter().collect();
    assert_eq!(page(&events), "I went down to the. Shop on Friday.");
    assert!(rejoined(&events).is_empty());
}

#[test]
fn a_finished_sentence_stays_finished() {
    let p = live_rig(
        vec![vec!["The train", "The train was", "The train was late"], vec!["But", "But nobody", "But nobody minded"]],
        rejoining(),
    );
    say(&p, 0);
    say(&p, 1);
    let events: Vec<Event> = p.finish().try_iter().collect();
    assert_eq!(page(&events), "The train was late. But nobody minded.");
    assert!(rejoined(&events).is_empty());
}

#[test]
fn a_break_said_between_leaves_it_closed() {
    let p = live_rig(vec![TO_THE.to_vec(), vec!["New", "New paragraph", "New paragraph."], SHOP.to_vec()], rejoining());
    say(&p, 0);
    say(&p, 1);
    say(&p, 2);
    let events: Vec<Event> = p.finish().try_iter().collect();
    assert_eq!(page(&events), "I went down to the.\n\nShop on Friday.");
    assert!(rejoined(&events).is_empty());
}

#[test]
fn a_scratched_sentence_is_not_reopened() {
    let p = live_rig(
        vec![TO_THE.to_vec(), vec!["Scratch", "Scratch that", "Scratch that."], SHOP.to_vec()],
        rejoining(),
    );
    say(&p, 0);
    say(&p, 1);
    say(&p, 2);
    let events: Vec<Event> = p.finish().try_iter().collect();
    assert_eq!(page(&events), "Shop on Friday.");
    assert!(rejoined(&events).is_empty());
}

#[test]
fn the_mark_goes_back_when_what_reopened_it_comes_to_nothing() {
    let p = live_rig(vec![TO_THE.to_vec(), vec!["Shop", "Shop on", "Shop on", ""]], rejoining());
    say(&p, 0);
    say(&p, 1);
    let events: Vec<Event> = p.finish().try_iter().collect();
    assert_eq!(page(&events), "I went down to the.");
}

#[test]
fn and_when_what_reopened_it_is_scratched() {
    let p = live_rig(vec![TO_THE.to_vec(), SHOP.to_vec(), vec!["Scratch", "Scratch that."]], rejoining());
    say(&p, 0);
    say(&p, 1);
    say(&p, 2);
    let events: Vec<Event> = p.finish().try_iter().collect();
    assert_eq!(page(&events), "I went down to the.");
}

#[test]
fn a_sentence_tidied_to_an_end_is_left_closed() {
    let contexts = Arc::new(Mutex::new(Vec::new()));
    let config = PipelineConfig { rejoin: true, ..typed_once() };
    let p = live_rig_with(
        vec![vec!["We sailed", "We sailed over", "We sailed over the"], vec!["Lighthouse", "Lighthouse", "Lighthouse"]],
        config,
        Some(contexts),
        None,
    );
    say(&p, 0);
    say(&p, 1);
    let events: Vec<Event> = p.finish().try_iter().collect();
    // The model took "over the" out: "sailed" ends a sentence.
    assert_eq!(page(&events), "We sailed. Lighthouse.");
    assert!(rejoined(&events).is_empty());
}

#[test]
fn the_tidy_of_the_closed_sentence_alone_is_void_and_the_two_are_tidied_whole() {
    let contexts = Arc::new(Mutex::new(Vec::new()));
    let (started_tx, started) = unbounded();
    let (go, go_rx) = bounded(0);
    let p = live_rig_gated(
        vec![vec!["Um, we", "Um, we went", "Um, we went to the"], SHOP.to_vec()],
        rejoining(),
        Some(contexts.clone()),
        None,
        Some((started_tx, go_rx)),
    );
    say(&p, 0);
    // The closed sentence is with the model when the next words come.
    started.recv_timeout(Duration::from_secs(5)).unwrap();
    say(&p, 1);
    std::thread::sleep(Duration::from_millis(100));
    go.send(()).unwrap();
    // Then the whole sentence's turn.
    started.recv_timeout(Duration::from_secs(5)).unwrap();
    go.send(()).unwrap();
    let events: Vec<Event> = p.finish().try_iter().collect();
    assert_eq!(page(&events), "We went to the shop on Friday.");
    assert_eq!(contexts.lock().unwrap().len(), 2);
    assert_eq!(rejoined(&events).len(), 1);
}

#[test]
fn typed_once_the_mark_is_taken_back_before_the_next_words() {
    let contexts = Arc::new(Mutex::new(Vec::new()));
    let config = PipelineConfig { rejoin: true, ..typed_once() };
    let p = live_rig_with(vec![TO_THE.to_vec(), SHOP.to_vec()], config, Some(contexts), None);
    say(&p, 0);
    say(&p, 1);
    let events: Vec<Event> = p.finish().try_iter().collect();
    assert_eq!(page(&events), "I went down to the shop on Friday.");
    assert_eq!(rejoined(&events), vec![("dangling", ".".to_string())]);
}
