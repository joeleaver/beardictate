//! The Post-processing page: which model tidies your dictation, and the model
//! manager (download, resume, delete).

use rinch::prelude::*;

use crate::models::{self, ModelState, catalog, in_use, size};
use crate::ui::{self, Shared, change};

pub fn cleanup_page(__scope: &mut RenderScope, s: &Shared) -> NodeHandle {
    let custom = Signal::new(s.settings.with(|st| {
        if st.cleanup.ends_with(".gguf") { st.cleanup.clone() } else { String::new() }
    }));
    let off = choice(__scope, s, false, "Off", "Your words as heard, with spoken commands. Fastest.");
    let on = choice(__scope, s, true, "On", "With the model checked below: the recommended one, unless you pick another.");
    let rows: Vec<NodeHandle> = catalog().into_iter().map(|m| row(__scope, s, m)).collect();
    let (s1, s2) = (s.clone(), s.clone());
    rsx! {
        div { style: "display: flex; flex-direction: column; gap: 18px;",
            Title { order: 3, "Post-processing" }
            Text { size: "sm", color: "dimmed",
                "With Tidy up on, a small model on your computer drops the ums and fixes punctuation after each sentence, and with Dialog on, quotes what your characters say."
            }
            Stack { gap: "sm", {off} {on} }
            div { style: "display: flex; flex-direction: column; gap: 8px;",
                Text { style: "font-weight: 600;", "Models" }
                Text { size: "sm", color: "dimmed", "Bigger models tidy and quote dialog more reliably but need a faster computer. Downloads continue in the background and pick up where they left off." }
                {rows}
            }
            TextInput {
                label: "Or a model file of your own (.gguf)",
                placeholder: "/path/to/model.gguf",
                value_fn: move || custom.get(),
                oninput: move |v: String| custom.set(v),
                onsubmit: move || {
                    let path = custom.get();
                    if !path.trim().is_empty() {
                        change(&s1, |st| st.cleanup = path.trim().to_string());
                    }
                },
            }
            Text { size: "sm", color: "dimmed", {move || ui::status_line(&s2)} }
        }
    }
}

/// Cleanup off or on. Turning it on from off starts with "auto" (the
/// recommended model); a model already picked stays picked.
fn choice(__scope: &mut RenderScope, s: &Shared, on: bool, label: &'static str, description: &'static str) -> NodeHandle {
    let (a, b) = (s.clone(), s.clone());
    rsx! {
        Radio {
            name: "cleanup-on",
            label: label,
            description: description,
            checked_fn: move || a.settings.with(|st| (st.cleanup != "off") == on),
            onchange: move || change(&b, |st| {
                if !on {
                    st.cleanup = "off".into();
                } else if st.cleanup == "off" {
                    st.cleanup = "auto".into();
                }
            }),
        }
    }
}

/// One catalog model: choose it, and download, cancel or delete it.
fn row(__scope: &mut RenderScope, s: &Shared, model: &'static bd_models::ModelEntry) -> NodeHandle {
    let id: &'static str = &model.id;
    let states = s.models.states;
    let state = move || states.with(|m| m.get(id).cloned().unwrap_or(ModelState::Missing));
    let details = format!(
        "{}{} · {}",
        if model.recommended { "Recommended · " } else { "" },
        size(model.total_size()),
        model.license
    );
    let (a, b, c, d, e) = (s.clone(), s.clone(), s.clone(), s.clone(), s.clone());
    let store = move |s: &Shared| models::store(&s.settings.with(|st| st.models_dir.clone()));
    rsx! {
        div { key: id, style: "display: flex; gap: 12px; align-items: center; padding: 10px 12px; border: 1px solid var(--rinch-color-default-border); border-radius: 8px;",
            div { style: "flex: 1; min-width: 0;",
                Radio {
                    name: "cleanup-model",
                    label: model.name.clone(),
                    description: details.clone(),
                    disabled: {move || state() != ModelState::Installed},
                    // The model that runs, which "auto" resolves to the recommended one.
                    checked_fn: {let a = a.clone(); move || in_use(&a.settings.with(|st| st.cleanup.clone()), &states.get()).as_deref() == Some(id)},
                    onchange: {let b = b.clone(); move || change(&b, |st| st.cleanup = id.to_string())},
                }
            }
            match state() {
                ModelState::Installed => {
                    // Follows the setting, which can change while this stays Installed.
                    let settings = c.settings;
                    // Signals and a &'static str: the closure is Copy.
                    let used = move || in_use(&settings.with(|st| st.cleanup.clone()), &states.get()).as_deref() == Some(id);
                    let c = c.clone();
                    div { style: "display: flex; gap: 8px; align-items: center;",
                        Text { size: "xs", color: "dimmed", {move || if used() { "In use" } else { "Downloaded" }} }
                        Button { variant: "subtle", color: "red", size: "xs", disabled: {move || in_use(&settings.with(|st| st.cleanup.clone()), &states.get()).as_deref() == Some(id)},
                            onclick: {let c = c.clone(); move || if let Some(st) = store(&c) { c.models.delete(st, id) }},
                            "Delete"
                        }
                    }
                }
                ModelState::Downloading { .. } => {
                    let d = d.clone();
                    // The arm stays while the numbers change: read them live.
                    let progress = move || match state() {
                        ModelState::Downloading { done, total } => (done, total),
                        _ => (0, 1),
                    };
                    div { style: "display: flex; gap: 8px; align-items: center; width: 240px;",
                        div { style: "flex: 1;",
                            Progress { value_fn: move || { let (d, t) = progress(); d as f32 / t.max(1) as f32 * 100.0 }, size: "sm" }
                            Text { size: "xs", color: "dimmed", {move || { let (d, t) = progress(); format!("{} of {}", size(d), size(t)) }} }
                        }
                        Button { variant: "subtle", size: "xs", onclick: move || d.models.cancel(id), "Cancel" }
                    }
                }
                ModelState::Deleting => Text { size: "xs", color: "dimmed", "Deleting…" },
                other => {
                    let e = e.clone();
                    let (label, note) = match &other {
                        ModelState::Partial(bytes) => ("Resume", format!("{} so far", size(*bytes))),
                        ModelState::Failed(why) => ("Try again", why.clone()),
                        _ => ("Download", String::new()),
                    };
                    div { style: "display: flex; gap: 8px; align-items: center; max-width: 280px;",
                        Text { size: "xs", color: {if matches!(other, ModelState::Failed(_)) { "red" } else { "dimmed" }}, {note} }
                        Button { variant: "light", size: "xs",
                            onclick: move || if let Some(st) = store(&e) { e.models.download(st, id) },
                            {label}
                        }
                    }
                }
            }
        }
    }
}
