//! Where dictated words go (the General page), and the Post-processing
//! page's switches: whether they're tidied up, and with fiction's dialog.

use rinch::prelude::*;

use crate::settings::Destination;
use crate::ui::{Shared, change};

pub fn modes(__scope: &mut RenderScope, s: &Shared) -> NodeHandle {
    let choices: Vec<NodeHandle> = Destination::ALL.into_iter().map(|d| choice(__scope, s, d)).collect();
    rsx! {
        div { style: "display: flex; flex-direction: column; gap: 8px;",
            Text { style: "font-weight: 600;", "Where your words go" }
            {choices}
        }
    }
}

/// Tidy up and Dialog, on the Post-processing page.
pub fn switches(__scope: &mut RenderScope, s: &Shared) -> NodeHandle {
    let (b, c, e) = (s.clone(), s.clone(), s.clone());
    let settings = s.settings;
    rsx! {
        div { style: "display: flex; flex-direction: column; gap: 8px;",
            Switch {
                label: "Tidy up",
                description: TIDY,
                checked_fn: move || b.settings.with(|st| st.tidy),
                onchange: move || change(&c, |st| st.tidy = !st.tidy),
            }
            Switch {
                label: "Dialog",
                description: DIALOG,
                disabled: {move || !settings.with(|st| st.tidy)},
                checked_fn: move || settings.with(|st| st.dialog),
                onchange: {let e = e.clone(); move || change(&e, |st| st.dialog = !st.dialog)},
            }
        }
    }
}

fn description(destination: Destination) -> &'static str {
    match destination {
        Destination::App => "Typed where your cursor is as you say them, in whatever app you're using.",
        Destination::Pad => "Your words appear in a floating pad as you say them. Edit by voice (\u{201C}bear, change Smith to Smythe\u{201D}, \u{201C}bear, undo\u{201D}) and say \u{201C}bear, send it\u{201D} to type it all where your cursor is.",
    }
}

/// What "Dialog" adds to tidying.
const DIALOG: &str = "For fiction: what your characters say goes in quotation marks, and a new speaker starts a new paragraph. Off, the only quotation marks are the ones you dictate (\u{201C}open quote\u{201D}, \u{201C}end quote\u{201D}). Needs Tidy up.";

/// What "Tidy up" does, wherever the words go.
const TIDY: &str = "Fillers out, sentences mended, punctuation and capitals fixed. Your words still appear as you say them; when you pause, the sentence (in the Bear Pad, the paragraph) is tidied in place, unless you've moved on to another app. Spoken commands work either way.";

fn choice(__scope: &mut RenderScope, s: &Shared, destination: Destination) -> NodeHandle {
    let (a, b) = (s.clone(), s.clone());
    rsx! {
        Radio {
            name: "destination",
            label: destination.label(),
            description: description(destination),
            checked_fn: move || a.settings.with(|st| st.destination == destination),
            onchange: move || change(&b, |st| st.destination = destination),
        }
    }
}
