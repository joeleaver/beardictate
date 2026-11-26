//! The Writing page: quotation marks, punctuation, numbers, paragraphs
//! and pauses.

use rinch::prelude::*;

use super::spelling::spelling;
use crate::settings::{Punctuation, QuoteMarks, Settings};
use crate::ui::{Shared, change};
use bd_commands::NumberStyle;

pub(super) fn writing(__scope: &mut RenderScope, s: &Shared) -> NodeHandle {
    let m1 = marks(__scope, s, "“Double”, with ‘single’ inside: US, and many UK publishers", QuoteMarks::Double);
    let m2 = marks(__scope, s, "‘Single’, with “double” inside: traditional British books", QuoteMarks::Single);
    let c1 = curly(__scope, s, "Curly: “like this” and ‘like this’", true);
    let c2 = curly(__scope, s, "Straight: \"like this\" and 'like this'", false);
    let p1 = punct(__scope, s, "Always inside: “To the river,” he said.", Punctuation::Inside);
    let p2 = punct(__scope, s, "Inside only when they belong to the speech: she called it ‘the end’.", Punctuation::Logical);
    let t1 = toggle(__scope, s, "When I say “new paragraph”", "And a line break for “new line”. Off, those words are typed.", |st| st.paragraph_on_command, |st| st.paragraph_on_command = !st.paragraph_on_command);
    let t2 = toggle(__scope, s, "When a different character speaks", "Needs Dialog on (Post-processing page). The post-processing model follows who's speaking.", |st| st.paragraph_on_speaker, |st| st.paragraph_on_speaker = !st.paragraph_on_speaker);
    let t3 = toggle(__scope, s, "After a long pause", "", |st| st.paragraph_on_pause, |st| st.paragraph_on_pause = !st.paragraph_on_pause);
    let t4 = toggle(__scope, s, "When the prose calls for one", "A new topic, place or time, as a novelist would break it. The post-processing model decides.", |st| st.paragraph_on_topic, |st| st.paragraph_on_topic = !st.paragraph_on_topic);
    let t5 = toggle(__scope, s, "Say punctuation out loud", "“comma”, “period” or “full stop”, “question mark”, “exclamation point”, “colon”, “semicolon”, “em dash”, “dot dot dot”. Off, those words are typed.", |st| st.spoken_punctuation, |st| st.spoken_punctuation = !st.spoken_punctuation);
    let j1 = toggle(__scope, s, "Carry on a sentence after a pause to think", "When a sentence ends at a pause but what you say next plainly carries it on (“I went down to the” … “shop”), the full stop is taken back. Off by default while we measure how often it helps.", |st| st.rejoin_sentences, |st| st.rejoin_sentences = !st.rejoin_sentences);
    let j2 = toggle(__scope, s, "In a list, keep going on the same item after a short pause", "When you go on before the item is closed, unless it had come to a full stop. Off, every pause starts a new item.", |st| st.list_join_pauses, |st| st.list_join_pauses = !st.list_join_pauses);
    let n1 = numbers(__scope, s, "As you say them", "\u{201C}twenty-five people\u{201D}. Say \u{201C}numeral\u{201D} first for digits: \u{201C}numeral twenty five\u{201D} is 25, \u{201C}numeral first\u{201D} is 1st, \u{201C}numeral four thirty\u{201D} is 4:30.", NumberStyle::AsSaid);
    let n2 = numbers(__scope, s, "As a style guide writes them", "Words up to one hundred, digits above (234), and digits for money ($300), percentages (12 percent), times (4:30 p.m.) and dates (May 21). \u{201C}Numeral\u{201D} still gives digits.", NumberStyle::StyleGuide);
    let r1 = run_on(__scope, s, "Leave it open, and open the next paragraph: \u{201C}…the river.  \u{201C}And then…", false);
    let r2 = run_on(__scope, s, "Close it, and open the next paragraph: \u{201C}…the river.\u{201D}  \u{201C}And then…", true);
    let b1 = breaks(__scope, s, "A blank line (two returns)", true);
    let b2 = breaks(__scope, s, "One return, for editors that space paragraphs themselves", false);
    let (s1, s2, s3) = (s.clone(), s.clone(), s.clone());
    let sp = spelling(__scope, s);
    rsx! {
        div { style: "display: flex; flex-direction: column; gap: 18px;",
            Title { order: 3, "Writing" }
            div { style: "display: flex; align-items: center; gap: 10px;",
                Text { size: "sm", color: "dimmed", "Punctuation, numbers, lists and commands you can say:" }
                Button { variant: "light", size: "xs", onclick: || crate::ui::open_url(&format!("{}/help.html#commands", crate::update::site())), "What you can say" }
            }
            div { style: "display: flex; flex-direction: column; gap: 8px;",
                Text { style: "font-weight: 600;", "Quotation marks" }
                {m1} {m2}
            }
            div { style: "display: flex; flex-direction: column; gap: 8px;",
                Text { style: "font-weight: 600;", "Quote characters" }
                {c1} {c2}
            }
            div { style: "display: flex; flex-direction: column; gap: 8px;",
                Text { style: "font-weight: 600;", "Commas and full stops at the end of a quote" }
                {p1} {p2}
            }
            div { style: "display: flex; flex-direction: column; gap: 8px;",
                Text { style: "font-weight: 600;", "Punctuation" }
                {t5}
            }
            {sp}
            div { style: "display: flex; flex-direction: column; gap: 8px;",
                Text { style: "font-weight: 600;", "Numbers" }
                {n1} {n2}
            }
            div { style: "display: flex; flex-direction: column; gap: 8px;",
                Text { style: "font-weight: 600;", "Start a new paragraph" }
                {t1} {t2} {t3}
                div { style: {move || if s1.settings.with(|st| st.paragraph_on_pause) { "padding-left: 28px; max-width: 260px;" } else { "display: none;" }},
                    Select {
                        data: vec![
                            SelectOption::new("2", "2 seconds"),
                            SelectOption::new("3", "3 seconds"),
                            SelectOption::new("4", "4 seconds"),
                            SelectOption::new("6", "6 seconds"),
                        ],
                        value_fn: move || format!("{}", s2.settings.with(|st| st.pause_paragraph_secs.round() as u32)),
                        onchange: move |v: String| {
                            if let Ok(secs) = v.parse::<f32>() {
                                change(&s3, |st| st.pause_paragraph_secs = secs);
                            }
                        },
                    }
                }
                {t4}
            }
            div { style: "display: flex; flex-direction: column; gap: 8px;",
                Text { style: "font-weight: 600;", "Pauses" }
                {j1} {j2}
            }
            div { style: "display: flex; flex-direction: column; gap: 8px;",
                Text { style: "font-weight: 600;", "When a character keeps speaking into a new paragraph" }
                {r1} {r2}
            }
            div { style: "display: flex; flex-direction: column; gap: 8px;",
                Text { style: "font-weight: 600;", "Between paragraphs" }
                {b1} {b2}
            }
        }
    }
}

fn run_on(__scope: &mut RenderScope, s: &Shared, label: &'static str, value: bool) -> NodeHandle {
    let (a, b) = (s.clone(), s.clone());
    rsx! {
        Radio {
            name: "run-on",
            label: label,
            checked_fn: move || a.settings.with(|st| st.close_continued_speech == value),
            onchange: move || change(&b, |st| st.close_continued_speech = value),
        }
    }
}

fn breaks(__scope: &mut RenderScope, s: &Shared, label: &'static str, value: bool) -> NodeHandle {
    let (a, b) = (s.clone(), s.clone());
    rsx! {
        Radio {
            name: "breaks",
            label: label,
            checked_fn: move || a.settings.with(|st| st.blank_line_paragraphs == value),
            onchange: move || change(&b, |st| st.blank_line_paragraphs = value),
        }
    }
}

fn toggle(
    __scope: &mut RenderScope,
    s: &Shared,
    label: &'static str,
    description: &'static str,
    get: fn(&Settings) -> bool,
    flip: fn(&mut Settings),
) -> NodeHandle {
    let (a, b) = (s.clone(), s.clone());
    rsx! {
        Checkbox {
            label: label,
            description: description,
            checked_fn: move || a.settings.with(get),
            onchange: move || change(&b, flip),
        }
    }
}

fn marks(__scope: &mut RenderScope, s: &Shared, label: &'static str, value: QuoteMarks) -> NodeHandle {
    let (a, b) = (s.clone(), s.clone());
    rsx! {
        Radio {
            name: "quote-marks",
            label: label,
            checked_fn: move || a.settings.with(|st| st.quote_marks == value),
            onchange: move || change(&b, |st| st.quote_marks = value),
        }
    }
}

fn numbers(__scope: &mut RenderScope, s: &Shared, label: &'static str, description: &'static str, value: NumberStyle) -> NodeHandle {
    let (a, b) = (s.clone(), s.clone());
    rsx! {
        Radio {
            name: "numbers",
            label: label,
            description: description,
            checked_fn: move || a.settings.with(|st| st.numbers == value),
            onchange: move || change(&b, |st| st.numbers = value),
        }
    }
}

fn curly(__scope: &mut RenderScope, s: &Shared, label: &'static str, value: bool) -> NodeHandle {
    let (a, b) = (s.clone(), s.clone());
    rsx! {
        Radio {
            name: "curly",
            label: label,
            checked_fn: move || a.settings.with(|st| st.curly_quotes == value),
            onchange: move || change(&b, |st| st.curly_quotes = value),
        }
    }
}

fn punct(__scope: &mut RenderScope, s: &Shared, label: &'static str, value: Punctuation) -> NodeHandle {
    let (a, b) = (s.clone(), s.clone());
    rsx! {
        Radio {
            name: "punctuation",
            label: label,
            checked_fn: move || a.settings.with(|st| st.punctuation == value),
            onchange: move || change(&b, |st| st.punctuation = value),
        }
    }
}
