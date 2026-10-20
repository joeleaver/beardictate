//! The special words page: names and invented words, and how hard the
//! ASR listens for them.

use std::rc::Rc;

use rinch::prelude::*;

use crate::settings::{SpecialWord, WordGroup};
use crate::ui::{Shared, change};

/// The recommended stretch of the boost slider, and its top. Below the
/// range names are missed; above it ordinary words start turning into them
/// ("whispered" into "whisper" at 2.5–3 on our 22-name test).
const BOOST_MAX: f64 = 5.0;
const BOOST_GOOD: (f64, f64) = (1.5, 2.5);

fn boost_slider(__scope: &mut RenderScope, s: &Shared) -> NodeHandle {
    let boost = Signal::new(f64::from(untracked(|| s.settings.get()).hotword_boost));
    let s = s.clone();
    let pct = |v: f64| v / BOOST_MAX * 100.0;
    let band = format!(
        "position: absolute; top: 0; height: 6px; border-radius: 3px; left: {:.1}%; width: {:.1}%; background: var(--rinch-color-teal-4, #63e6be);",
        pct(BOOST_GOOD.0),
        pct(BOOST_GOOD.1 - BOOST_GOOD.0)
    );
    let band_label = format!(
        "position: absolute; top: 9px; left: {:.1}%; width: {:.1}%; text-align: center;",
        pct(BOOST_GOOD.0),
        pct(BOOST_GOOD.1 - BOOST_GOOD.0)
    );
    rsx! {
        div { style: "display: flex; flex-direction: column; gap: 6px; padding-top: 14px; border-top: 1px solid var(--rinch-color-default-border);",
            div { style: "display: flex; justify-content: space-between; align-items: baseline;",
                Text { style: "font-weight: 600;", "How hard to listen for these words" }
                Text { size: "sm", color: "dimmed",
                    {move || match boost.get() {
                        v if v <= 0.0 => "off".to_string(),
                        v => format!("{v:.2}"),
                    }}
                }
            }
            Slider {
                min: Some(0.0),
                max: Some(BOOST_MAX),
                step: Some(0.25),
                value_signal: Some(boost),
                onchange: move |v: f64| {
                    boost.set(v);
                    change(&s, |st| st.hotword_boost = v as f32);
                },
            }
            div { style: "position: relative; height: 26px; font-size: 11px; color: var(--rinch-color-dimmed);",
                div { style: band }
                div { style: band_label, "recommended" }
                div { style: "position: absolute; top: 9px; left: 0;", "off" }
                div { style: "position: absolute; top: 9px; right: 0;", "strong" }
            }
            Text { size: "xs", color: "dimmed",
                "Higher catches more of your words, but past the recommended range it starts turning ordinary words into them. The replacements above work at any setting."
            }
        }
    }
}

pub fn words(__scope: &mut RenderScope, s: &Shared) -> NodeHandle {
    // The group whose delete button was pressed once: it asks to confirm.
    let confirm_delete: Signal<Option<usize>> = Signal::new(None);
    let list = s.clone();
    let add_group = {
        let s = s.clone();
        move || {
            change(&s, |st| {
                let n = st.word_groups.len() + 1;
                st.word_groups.push(WordGroup::new(&format!("Group {n}")));
            })
        }
    };
    rsx! {
        div { style: "display: flex; flex-direction: column; gap: 14px;",
            Title { order: 3, "Special words" }
            Text { size: "sm", color: "dimmed",
                "Names and invented words. beardictate listens for each spelling, and replaces what it tends to hear instead. Group them by book or series and switch a whole group off when you're not writing it. Changes apply right away."
            }
            Text { size: "sm", color: "dimmed",
                "A spelling can have breaks in it: \u{201C}Best wishes,<new line>Ysolde\u{201D} types two lines, and <paragraph> starts a new paragraph. Give it something to hear, such as \u{201C}sign off\u{201D}."
            }
            for (gi, g) in list.settings.get().word_groups.into_iter().enumerate() {
                let (ga, gb, gc, gd) = (list.clone(), list.clone(), list.clone(), list.clone());
                let new_word = Signal::new(String::new());
                let new_heard = Signal::new(String::new());
                let add = Rc::new({
                    let s = list.clone();
                    move || {
                        let word = new_word.get().trim().to_string();
                        if word.is_empty() {
                            return;
                        }
                        let heard = split_heard(&new_heard.get());
                        let mut w = SpecialWord::new(&word, &[]);
                        w.heard = heard;
                        change(&s, |st| st.word_groups[gi].words.push(w));
                        new_word.set(String::new());
                        new_heard.set(String::new());
                    }
                });
                let (add1, add2, add3) = (add.clone(), add.clone(), add.clone());
                let count = g.words.len();
                let dim = if g.enabled { "" } else { "opacity: 0.5;" };
                div { key: format!("g{gi}:{}", g.name), style: "display: flex; flex-direction: column; gap: 8px; padding: 12px; border: 1px solid var(--rinch-color-default-border); border-radius: 8px;",
                    div { style: "display: flex; gap: 8px; align-items: center;",
                        Checkbox {
                            checked: g.enabled,
                            onchange: move || change(&ga, |st| st.word_groups[gi].enabled = !st.word_groups[gi].enabled),
                        }
                        TextInput {
                            value: g.name.clone(),
                            style: "flex: 1; font-weight: 600;",
                            onchange: move |v: String| {
                                let v = v.trim().to_string();
                                if !v.is_empty() {
                                    change(&gb, |st| st.word_groups[gi].name = v);
                                }
                            },
                        }
                        if confirm_delete.get() == Some(gi) {
                            Text { size: "sm", {format!("Delete {count} word{}?", if count == 1 { "" } else { "s" })} }
                            Button { variant: "light", color: "red", size: "xs",
                                onclick: {let gc = gc.clone(); move || {
                                    confirm_delete.set(None);
                                    change(&gc, |st| { st.word_groups.remove(gi); });
                                }},
                                "Delete"
                            }
                            Button { variant: "subtle", size: "xs", onclick: move || confirm_delete.set(None), "Keep" }
                        } else {
                            ActionIcon {
                                variant: "subtle",
                                color: "red",
                                onclick: {let gd = gd.clone(); move || {
                                    if count == 0 {
                                        change(&gd, |st| { st.word_groups.remove(gi); });
                                    } else {
                                        confirm_delete.set(Some(gi));
                                    }
                                }},
                                "×"
                            }
                        }
                    }
                    div { style: format!("display: flex; flex-direction: column; gap: 6px; padding-left: 30px; {dim}"),
                        for (wi, w) in g.words.clone().into_iter().enumerate() {
                            let (a, b, c, d) = (list.clone(), list.clone(), list.clone(), list.clone());
                            div { key: format!("g{gi}w{wi}:{}", w.word), style: "display: flex; gap: 8px; align-items: center;",
                                Checkbox {
                                    checked: w.enabled,
                                    onchange: move || change(&d, |st| {
                                        let w = &mut st.word_groups[gi].words[wi];
                                        w.enabled = !w.enabled;
                                    }),
                                }
                                TextInput {
                                    value: w.word.clone(),
                                    style: if w.enabled { "width: 180px; font-weight: 600;" } else { "width: 180px; font-weight: 600; opacity: 0.5;" },
                                    onchange: move |v: String| {
                                        let v = v.trim().to_string();
                                        if !v.is_empty() {
                                            change(&a, |st| st.word_groups[gi].words[wi].word = v);
                                        }
                                    },
                                }
                                TextInput {
                                    value: w.heard.join(", "),
                                    placeholder: "heard as (separate with commas)",
                                    style: if w.enabled { "flex: 1; min-width: 0;" } else { "flex: 1; min-width: 0; opacity: 0.5;" },
                                    onchange: move |v: String| change(&b, |st| st.word_groups[gi].words[wi].heard = split_heard(&v)),
                                }
                                ActionIcon {
                                    variant: "subtle",
                                    color: "red",
                                    onclick: move || change(&c, |st| { st.word_groups[gi].words.remove(wi); }),
                                    "×"
                                }
                            }
                        }
                        div { style: "display: flex; gap: 8px; align-items: center;",
                            div { style: "width: 20px; flex-shrink: 0;" }
                            TextInput {
                                placeholder: "Add a word",
                                style: "width: 180px; flex-shrink: 0;",
                                value_fn: move || new_word.get(),
                                oninput: move |v: String| new_word.set(v),
                                onsubmit: move || add1(),
                            }
                            TextInput {
                                placeholder: "heard as… (optional)",
                                style: "flex: 1; min-width: 0;",
                                value_fn: move || new_heard.get(),
                                oninput: move |v: String| new_heard.set(v),
                                onsubmit: move || add2(),
                            }
                            Button { variant: "light", size: "xs", onclick: move || add3(), "Add" }
                        }
                    }
                }
            }
            div {
                Button { variant: "default", onclick: add_group, "New group" }
            }
            {boost_slider(__scope, s)}
        }
    }
}

/// "a, b ,c" → ["a", "b", "c"], dropping empty entries.
fn split_heard(text: &str) -> Vec<String> {
    text.split(',').map(str::trim).filter(|h| !h.is_empty()).map(String::from).collect()
}
