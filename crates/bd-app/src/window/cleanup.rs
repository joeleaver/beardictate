//! The Cleanup page: which model cleans up your dictation, and the model
//! manager (download, resume, delete).

use rinch::prelude::*;

use crate::models::{self, ModelState, catalog, in_use, size};
use crate::ui::{self, Shared, change};

pub fn cleanup_page(__scope: &mut RenderScope, s: &Shared) -> NodeHandle {
    let custom = Signal::new(s.settings.with(|st| {
        if st.cleanup.ends_with(".gguf") { st.cleanup.clone() } else { String::new() }
    }));
    let choices = [
        ("off", "Off", "Your words as heard, with spoken commands. Fastest."),
        ("auto", "Automatic", "The largest model you've downloaded."),
    ]
    .map(|(value, label, description)| choice(__scope, s, value, label, description));
    let [off, auto] = choices;
    let rows: Vec<NodeHandle> = catalog().into_iter().map(|m| row(__scope, s, m)).collect();
    let (s1, s2) = (s.clone(), s.clone());
    rsx! {
        div { style: "display: flex; flex-direction: column; gap: 18px;",
            Title { order: 3, "Cleanup" }
            Text { size: "sm", color: "dimmed",
                "After each sentence is transcribed, a small model on your computer drops the ums, fixes punctuation and quotes your dialog."
            }
            Stack { gap: "sm", {off} {auto} }
            div { style: "display: flex; flex-direction: column; gap: 8px;",
                Text { style: "font-weight: 600;", "Models" }
                Text { size: "sm", color: "dimmed", "Bigger models quote dialog more reliably but need a faster computer. Downloads continue in the background and pick up where they left off." }
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

fn choice(__scope: &mut RenderScope, s: &Shared, value: &'static str, label: &'static str, description: &'static str) -> NodeHandle {
    let (a, b) = (s.clone(), s.clone());
    rsx! {
        Radio {
            name: "cleanup",
            label: label,
            description: description,
            checked_fn: move || a.settings.with(|st| st.cleanup == value),
            onchange: move || change(&b, |st| st.cleanup = value.to_string()),
        }
    }
}

/// One catalog model: choose it, and download, cancel or delete it.
fn row(__scope: &mut RenderScope, s: &Shared, model: &'static bd_models::ModelEntry) -> NodeHandle {
    let id: &'static str = &model.id;
    let states = s.models.states;
    let state = move || states.with(|m| m.get(id).cloned().unwrap_or(ModelState::Missing));
    let details = format!("{} · {}", size(model.total_size()), model.license);
    let (a, b, c, d, e) = (s.clone(), s.clone(), s.clone(), s.clone(), s.clone());
    let store = move |s: &Shared| models::store(&s.settings.with(|st| st.models_dir.clone()));
    rsx! {
        div { key: id, style: "display: flex; gap: 12px; align-items: center; padding: 10px 12px; border: 1px solid var(--rinch-color-default-border); border-radius: 8px;",
            div { style: "flex: 1; min-width: 0;",
                Radio {
                    name: "cleanup",
                    label: model.name.clone(),
                    description: details.clone(),
                    disabled: {move || state() != ModelState::Installed},
                    checked_fn: {let a = a.clone(); move || a.settings.with(|st| st.cleanup == id)},
                    onchange: {let b = b.clone(); move || change(&b, |st| st.cleanup = id.to_string())},
                }
            }
            match state() {
                ModelState::Installed => {
                    let used = in_use(&c.settings.with(|st| st.cleanup.clone()), &states.get()).as_deref() == Some(id);
                    let c = c.clone();
                    div { style: "display: flex; gap: 8px; align-items: center;",
                        Text { size: "xs", color: "dimmed", {if used { "In use" } else { "Downloaded" }} }
                        Button { variant: "subtle", color: "red", size: "xs", disabled: used,
                            onclick: move || if let Some(st) = store(&c) { c.models.delete(st, id) },
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
