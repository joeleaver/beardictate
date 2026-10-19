//! First run: a card above every page until the speech models are
//! downloaded. The installer ships no weights (DESIGN.md "Windows install and
//! first run"); the app opens its window with this card when they're missing.
//! Downloads go through the model manager ([`crate::models::Models`]), so
//! they resume and can be cancelled like the cleanup models'.

use std::collections::HashMap;

use rinch::prelude::*;

use crate::models::{self, ModelState, missing_speech, size};
use crate::ui::Shared;

/// Bytes done and in total across the speech models.
fn progress(states: &HashMap<String, ModelState>) -> (u64, u64) {
    models::speech().iter().fold((0, 0), |(done, total), m| {
        let size = m.total_size();
        let got = match states.get(&m.id) {
            Some(ModelState::Installed) => size,
            Some(ModelState::Downloading { done, .. }) => *done,
            Some(ModelState::Partial(bytes)) => *bytes,
            _ => 0,
        };
        (done + got, total + size)
    })
}

fn downloading(states: &HashMap<String, ModelState>) -> bool {
    models::speech().iter().any(|m| matches!(states.get(&m.id), Some(ModelState::Downloading { .. })))
}

fn failure(states: &HashMap<String, ModelState>) -> Option<String> {
    models::speech().iter().find_map(|m| match states.get(&m.id) {
        Some(ModelState::Failed(e)) => Some(format!("{}: {e}", m.name)),
        _ => None,
    })
}

pub fn setup_card(__scope: &mut RenderScope, s: &Shared) -> NodeHandle {
    let states = s.models.states;
    let total: u64 = models::speech().iter().map(|m| m.total_size()).sum();
    let start = {
        let s = s.clone();
        move || {
            let Some(store) = models::store(&s.settings.with(|st| st.models_dir.clone())) else { return };
            for m in states.with(missing_speech) {
                if !matches!(states.with(|st| st.get(&m.id).cloned()), Some(ModelState::Downloading { .. })) {
                    s.models.download(store.clone(), &m.id);
                }
            }
        }
    };
    let stop = {
        let models = s.models.clone();
        move || {
            for m in models::speech() {
                models.cancel(&m.id);
            }
        }
    };
    rsx! {
        div { style: {move || if states.with(|st| missing_speech(st).is_empty()) { "display: none;" } else {
                "display: flex; flex-direction: column; gap: 10px; padding: 14px 16px; margin-bottom: 22px; border-radius: 8px; border: 1px solid var(--rinch-color-blue-4, #74c0fc); background: var(--rinch-color-blue-light, rgba(34,139,230,0.08));"
            }},
            Text { style: "font-weight: 700; font-size: 15px;", "One more step: the speech models" }
            Text { size: "sm",
                {format!("beardictate listens on your computer, so it needs Nemotron (speech to text) and Silero (which hears your pauses) before it can dictate: {} in all, downloaded once from their makers' published copies and checked byte for byte. Post-processing models are optional, on the Post-processing page.", size(total))}
            }
            if states.with(downloading) {
                Progress { value_fn: move || states.with(|st| { let (d, t) = progress(st); if t == 0 { 0.0 } else { (d as f64 * 100.0 / t as f64) as f32 } }), size: "lg" }
                Group {
                    Text { size: "sm", {move || states.with(|st| { let (d, t) = progress(st); format!("Downloading… {} of {}", size(d), size(t)) })} }
                    Button { variant: "light", size: "xs", onclick: stop.clone(), "Cancel" }
                }
            } else {
                Group {
                    Button { onclick: start.clone(),
                        {move || if states.with(|st| progress(st).0) > 0 { "Resume the download" } else { "Download" }}
                    }
                    if let Some(e) = states.with(failure) {
                        Text { size: "sm", color: "red", {e} }
                    }
                }
            }
        }
    }
}
