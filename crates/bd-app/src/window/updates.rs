//! The About page's updates line: what the checker found, and the buttons
//! for it.

use rinch::prelude::*;

use crate::models::size;
use crate::ui::Shared;
use crate::update::{self, Phase};

fn describe(phase: &Phase) -> String {
    match phase {
        Phase::Idle => "Updates: not checked yet.".into(),
        Phase::Checking => "Checking for updates…".into(),
        Phase::UpToDate => "beardictate is up to date.".into(),
        Phase::Downloading {
            version,
            done,
            total,
        } => format!("Downloading {version}… {} of {}", size(*done), size(*total)),
        Phase::Ready { version } => {
            format!("Version {version} is ready. It installs the next time beardictate starts.")
        }
        Phase::Manual { version } => format!(
            "Version {version} is out. This copy can't update itself: get it from the download page."
        ),
        Phase::Failed(e) => format!("Couldn't check for updates: {e}"),
    }
}

pub fn updates(__scope: &mut RenderScope, s: &Shared) -> NodeHandle {
    let phase = s.updates.phase;
    let note = s.updates.note.clone();
    let (check, ready, manual) = (s.updates.clone(), phase, phase);
    rsx! {
        div { style: "display: flex; flex-direction: column; gap: 8px;",
            if let Some(n) = note.clone() {
                Text { size: "sm", {n} }
            }
            Text { {move || describe(&phase.get())} }
            Group {
                if matches!(ready.get(), Phase::Ready { .. }) {
                    Button { onclick: update::restart_to_update, "Restart and update now" }
                }
                if matches!(manual.get(), Phase::Manual { .. }) {
                    Button { variant: "light", onclick: move || crate::ui::open_url(&update::page()), "Open the download page" }
                }
                Button {
                    variant: "light",
                    disabled: {move || matches!(phase.get(), Phase::Checking | Phase::Downloading { .. })},
                    onclick: {let c = check.clone(); move || c.check_now()},
                    "Check for updates"
                }
            }
        }
    }
}
