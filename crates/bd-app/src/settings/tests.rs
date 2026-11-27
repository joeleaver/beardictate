use super::*;

#[test]
fn settings_round_trip_and_fill_in_missing_keys() {
    let s = Settings { paragraph_on_pause: true, quote_marks: QuoteMarks::Single, ..Settings::default() };
    let text = toml::to_string_pretty(&s).unwrap();
    assert_eq!(toml::from_str::<Settings>(&text).unwrap(), s);
    // Unknown keys (from an older version) are ignored.
    let partial: Settings = toml::from_str("paragraphs = \"off\"\nparagraph_on_speaker = false\n").unwrap();
    assert_eq!(partial.hotkey, "Ctrl+Backquote");
    assert!(!partial.style().dialog_paragraphs);
    assert!(partial.commands().break_commands);
    assert_eq!(partial.pause_paragraph(), None);
}

#[test]
fn tidy_device_defaults_to_auto_and_changing_it_reloads() {
    let s: Settings = toml::from_str("").unwrap();
    assert_eq!(s.tidy_device, TidyDevice::Auto);
    let cpu: Settings = toml::from_str("tidy_device = \"cpu\"\n").unwrap();
    assert_eq!(cpu.tidy_device, TidyDevice::Cpu);
    assert_ne!(s.models(), cpu.models());
}

#[test]
fn post_processing_off_becomes_tidy_up_off() {
    let mut s: Settings = toml::from_str("cleanup = \"off\"\n").unwrap();
    assert!(s.upgrade());
    assert_eq!((s.tidy, s.cleanup.as_str()), (false, "auto"));
    assert!(s.show_bear && !s.dialog, "the bear on, dialog off, by default");
}

#[test]
fn style_follows_the_choices() {
    let s = Settings { quote_marks: QuoteMarks::Single, punctuation: Punctuation::Logical, paragraph_on_topic: true, ..Settings::default() };
    let style = s.style();
    assert_eq!(style.quotes, QuoteStyle::CurlySingle);
    assert_eq!(style.punctuation, PunctuationPlacement::Logical);
    assert!(style.topic_paragraphs && !style.dialog && !style.dialog_paragraphs, "dialog is off by default");
    let fiction = Settings { dialog: true, ..s.clone() }.style();
    assert!(fiction.dialog && fiction.dialog_paragraphs);
    assert_ne!(s.live(), Settings::default().live());
    assert_eq!(s.models(), Settings::default().models());
}

#[test]
fn word_groups_round_trip_and_old_lists_move_into_one() {
    let s = Settings::default();
    let text = toml::to_string_pretty(&s).unwrap();
    assert_eq!(toml::from_str::<Settings>(&text).unwrap().word_groups, s.word_groups);
    // An emptied list stays empty.
    let none: Settings = toml::from_str("word_groups = []\n").unwrap();
    assert!(none.word_groups.is_empty());

    let old = "[[special_words]]\nword = \"Beardictate\"\nheard = [\"bare dictate\"]\n";
    let mut s: Settings = toml::from_str(old).unwrap();
    assert!(s.upgrade());
    assert_eq!(s.word_groups.len(), 1);
    assert_eq!(s.word_groups[0].name, FIRST_GROUP);
    assert_eq!(s.word_groups[0].words, [SpecialWord::new("Beardictate", &["bare dictate"])]);
    assert!(!s.upgrade());
    assert!(!toml::to_string_pretty(&s).unwrap().contains("special_words"));

    // Bear Pad on in an earlier version: the pad, tidied.
    let mut pad: Settings = toml::from_str("bear_pad = true\n").unwrap();
    assert!(pad.upgrade());
    assert_eq!((pad.destination, pad.tidy), (Destination::Pad, true));
    assert!(!toml::to_string_pretty(&pad).unwrap().contains("bear_pad"));
    // The four modes of 0.1.8.
    for (mode, destination, tidy) in [
        ("classic", Destination::App, true),
        ("streaming", Destination::App, false),
        ("pad-streaming", Destination::Pad, false),
        ("pad-author", Destination::Pad, true),
    ] {
        let mut old: Settings = toml::from_str(&format!("mode = \"{mode}\"\n")).unwrap();
        assert!(old.upgrade());
        assert_eq!((old.destination, old.tidy), (destination, tidy), "{mode}");
        assert!(!toml::to_string_pretty(&old).unwrap().lines().any(|l| l.starts_with("mode ")));
        assert!(!old.upgrade());
    }

    let mut gone: Settings = toml::from_str("cleanup = \"cleanup-qwen3-4b-q4_k_m\"\n").unwrap();
    assert!(gone.upgrade());
    assert_eq!(gone.cleanup, "auto");
    let mut kept: Settings = toml::from_str("cleanup = \"/models/mine.gguf\"\n").unwrap();
    assert!(!kept.upgrade());
}

#[test]
fn the_sentence_wait_stays_within_what_can_be_set() {
    let missing: Settings = toml::from_str("").unwrap();
    assert_eq!(missing.sentence_wait(), Duration::from_millis(1500));
    assert_eq!(missing.live().sentence_wait, Duration::from_millis(1500));
    let wait = |secs: f32| Settings { sentence_wait_secs: secs, ..Settings::default() }.sentence_wait();
    assert_eq!(wait(2.2), Duration::from_secs_f32(2.2));
    assert_eq!(wait(0.2), Duration::from_secs(1));
    assert_eq!(wait(5.0), Duration::from_secs(3));
    assert_eq!(wait(-1.0), Duration::from_millis(1500));
    assert_eq!(wait(f32::NAN), Duration::from_millis(1500));
    // A new wait reaches the running pipeline without a reload.
    let longer = Settings { sentence_wait_secs: 2.0, ..Settings::default() };
    assert_ne!(missing.live(), longer.live());
    assert_eq!(missing.models(), longer.models());
}
