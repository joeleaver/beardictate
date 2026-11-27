//! Writing › Pauses: how long a pause waits for you to go on before the
//! sentence ends, and the wait suggested from your own pauses
//! (`engine/pauses.rs`), which is only ever used when you press its button.

use rinch::prelude::*;
use rinch::reactive::Effect;

use crate::ui::{Shared, change};

/// A suggestion this close to the wait set isn't worth showing.
const NOT_WORTH: f32 = 0.2;

pub(super) fn sentence_wait(__scope: &mut RenderScope, s: &Shared) -> NodeHandle {
    let settings = s.settings;
    let suggested = s.sentence_wait_suggested;
    // The slider's own value, kept with the setting (the file can be edited
    // by hand, or the suggestion used).
    let secs = Signal::new(f64::from(settings.with(|st| st.sentence_wait().as_secs_f32())));
    Effect::new(move || secs.set(f64::from(settings.with(|st| st.sentence_wait().as_secs_f32()))));
    // The suggestion, when there is one far enough from the wait set.
    let offer = move || {
        let (wait, pauses) = suggested.get();
        let set = settings.with(|st| st.sentence_wait().as_secs_f32());
        wait.filter(|w| (w - set).abs() >= NOT_WORTH - 0.01).map(|w| (w, pauses))
    };
    let (s1, s2) = (s.clone(), s.clone());
    rsx! {
        div { style: "display: flex; flex-direction: column; gap: 4px; max-width: 420px;",
            div { style: "display: flex; justify-content: space-between;",
                Text { size: "sm", "Wait before ending a sentence" }
                Text { size: "sm", color: "dimmed", {move || format!("{:.1} s", secs.get())} }
            }
            Slider {
                min: Some(1.0),
                max: Some(3.0),
                step: Some(0.1),
                value_signal: Some(secs),
                onchange: move |v: f64| {
                    secs.set(v);
                    change(&s1, |st| st.sentence_wait_secs = round_tenth(v as f32));
                },
            }
            Text { size: "xs", color: "dimmed", "Longer if you pause to think mid-sentence and get a full stop there; shorter to have each sentence finished, and tidied, sooner. After “and”, “the” or a comma it waits longer still." }
            div { style: {move || if offer().is_some() { "display: flex; align-items: center; gap: 10px; padding-top: 4px;" } else { "display: none;" }},
                Text { size: "sm", {move || offer().map(|(w, pauses)| format!("Suggested for you: {w:.1} s, from your last {pauses} pauses mid-sentence")).unwrap_or_default()} }
                Button {
                    variant: "light",
                    size: "xs",
                    onclick: move || {
                        if let (Some(w), _) = suggested.get() {
                            change(&s2, |st| st.sentence_wait_secs = round_tenth(w));
                        }
                    },
                    {move || offer().map(|(w, _)| format!("Use {w:.1} s")).unwrap_or_default()}
                }
            }
        }
    }
}

/// Seconds to a tenth, as the slider steps (it gives 1.9000001).
fn round_tenth(secs: f32) -> f32 {
    (secs * 10.0).round() / 10.0
}
