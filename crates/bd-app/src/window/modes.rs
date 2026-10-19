//! The General page's choice of where dictated words go, and whether
//! they're tidied up.

use rinch::prelude::*;

use crate::settings::Destination;
use crate::ui::{Shared, change};

pub fn modes(__scope: &mut RenderScope, s: &Shared) -> NodeHandle {
    let choices: Vec<NodeHandle> = Destination::ALL.into_iter().map(|d| choice(__scope, s, d)).collect();
    let (b, c) = (s.clone(), s.clone());
    rsx! {
        div { style: "display: flex; flex-direction: column; gap: 8px;",
            Text { style: "font-weight: 600;", "Where your words go" }
            {choices}
            Switch {
                label: "Tidy up",
                description: TIDY,
                checked_fn: move || b.settings.with(|st| st.tidy),
                onchange: move || change(&c, |st| st.tidy = !st.tidy),
            }
        }
    }
}

fn description(destination: Destination) -> &'static str {
    match destination {
        Destination::App => "Typed where your cursor is, in whatever app you're using.",
        Destination::Pad => "Your words appear in a floating pad as you say them. Edit by voice (\u{201C}bear, change Smith to Smythe\u{201D}, \u{201C}bear, undo\u{201D}) and say \u{201C}bear, send it\u{201D} to type it all where your cursor is.",
    }
}

/// What "Tidy up" does, wherever the words go.
const TIDY: &str = "Fillers out, sentences mended, dialog quoted. In your app, each sentence is typed once, tidied, when you pause; off, your words are typed as you say them, and spoken commands still work. In the pad, the paragraph you're on is tidied in place.";

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
