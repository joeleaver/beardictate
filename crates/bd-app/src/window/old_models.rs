//! The General page's offer to delete the speech models earlier versions
//! downloaded, which nothing uses now (Parakeet, Nemotron's 560 ms export).

use rinch::prelude::*;

use crate::models::{self, ModelState, size};
use crate::ui::Shared;

pub fn old_models(__scope: &mut RenderScope, s: &Shared) -> NodeHandle {
    let states = s.models.states;
    // What's on disk of them, wholly or partly downloaded.
    let present = move || {
        states.with(|m| {
            models::retired()
                .into_iter()
                .filter(|e| matches!(m.get(&e.id), Some(ModelState::Installed | ModelState::Partial(_) | ModelState::Deleting)))
                .collect::<Vec<_>>()
        })
    };
    let deleting = move || states.with(|m| models::retired().iter().any(|e| m.get(&e.id) == Some(&ModelState::Deleting)));
    let d = s.clone();
    rsx! {
        div {
            if !present().is_empty() {
                div { style: "display: flex; gap: 10px; align-items: center; padding: 10px 12px; border: 1px solid var(--rinch-color-default-border); border-radius: 8px;",
                    div { style: "flex: 1; min-width: 0;",
                        Text { size: "sm", {move || {
                            let old = present();
                            let names: Vec<&str> = old.iter().map(|e| e.name.as_str()).collect();
                            let bytes: u64 = old.iter().map(|e| e.files.iter().map(|f| f.size).sum::<u64>()).sum();
                            format!("Speech models from earlier versions, no longer used: {} ({}).", names.join(", "), size(bytes))
                        }} }
                    }
                    if deleting() {
                        Text { size: "xs", color: "dimmed", "Deleting…" }
                    } else {
                        Button { variant: "light", color: "red", size: "xs",
                            onclick: {let d = d.clone(); move || {
                                let Some(store) = models::store(&d.settings.with(|x| x.models_dir.clone())) else { return };
                                for e in present() {
                                    d.models.delete(store.clone(), &e.id);
                                }
                            }},
                            "Delete them"
                        }
                    }
                }
            }
        }
    }
}
