//! The General page's choice of mode: how dictation reaches the page, and
//! the streaming speech model the streaming modes need.

use rinch::prelude::*;

use crate::models::{self, ModelState, size};
use crate::settings::Mode;
use crate::ui::{Shared, change};

pub fn modes(__scope: &mut RenderScope, s: &Shared) -> NodeHandle {
    let choices: Vec<NodeHandle> = Mode::ALL.into_iter().map(|m| choice(__scope, s, m)).collect();
    let download = streaming_model(__scope, s);
    rsx! {
        div { style: "display: flex; flex-direction: column; gap: 8px;",
            Text { style: "font-weight: 600;", "Mode" }
            {choices}
            {download}
        }
    }
}

fn description(mode: Mode) -> &'static str {
    match mode {
        Mode::Classic => "Each sentence is tidied up and typed into your app when you pause.",
        Mode::Streaming => "Your words are typed into your app as you say them, with the least delay. No tidying up; spoken commands still work.",
        Mode::PadStreaming => "Your words appear in a floating pad as you say them. Edit by voice (\u{201C}bear, change Smith to Smythe\u{201D}, \u{201C}bear, undo\u{201D}) and say \u{201C}bear, send it\u{201D} to type it all where your cursor is.",
        Mode::PadAuthor => "The pad, tidied up as you go: when you pause, the paragraph you're on is cleaned in place, dialog quoted and names fixed.",
    }
}

fn choice(__scope: &mut RenderScope, s: &Shared, mode: Mode) -> NodeHandle {
    let (a, b) = (s.clone(), s.clone());
    rsx! {
        Radio {
            name: "mode",
            label: mode.label(),
            description: description(mode),
            checked_fn: move || a.settings.with(|st| st.mode == mode),
            onchange: move || change(&b, |st| st.mode = mode),
        }
    }
}

/// The streaming speech model, when a streaming mode is chosen and it isn't
/// downloaded yet.
fn streaming_model(__scope: &mut RenderScope, s: &Shared) -> NodeHandle {
    let model = models::streaming();
    let id: &'static str = &model.id;
    let states = s.models.states;
    let settings = s.settings;
    let state = move || states.with(|m| m.get(id).cloned().unwrap_or(ModelState::Missing));
    let needed = move || settings.with(|st| st.mode.streams()) && state() != ModelState::Installed;
    let (d, c) = (s.clone(), s.clone());
    let note = format!("Streaming needs {} ({}, downloaded once).", model.name, size(model.download_size()));
    rsx! {
        div { style: "padding-left: 28px;",
            if needed() {
                div { style: "display: flex; gap: 10px; align-items: center; padding: 10px 12px; border: 1px solid var(--rinch-color-default-border); border-radius: 8px;",
                    div { style: "flex: 1; min-width: 0;",
                        Text { size: "sm", {note.clone()} }
                        Text { size: "xs", color: "dimmed", {move || match state() {
                            ModelState::Downloading { done, total } => format!("{} of {}", size(done), size(total)),
                            ModelState::Failed(why) => why,
                            _ => String::new(),
                        }} }
                    }
                    if matches!(state(), ModelState::Downloading { .. }) {
                        Button { variant: "subtle", size: "xs", onclick: {let c = c.clone(); move || c.models.cancel(id)}, "Cancel" }
                    } else {
                        Button { variant: "light", size: "xs",
                            onclick: {let d = d.clone(); move || if let Some(st) = models::store(&d.settings.with(|x| x.models_dir.clone())) { d.models.download(st, id) }},
                            "Download"
                        }
                    }
                }
            }
        }
    }
}
