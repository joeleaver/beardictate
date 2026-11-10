//! The "What's new" dialog over the settings window (see `whats_new.rs`):
//! at the first start of a new version, and from About.

use rinch::prelude::*;

use crate::ui::Shared;
use crate::whats_new::Release;

pub fn news(__scope: &mut RenderScope, s: &Shared) -> NodeHandle {
    let news = s.news;
    let opened_window = s.news_opened_window;
    let close = move || {
        news.set(Vec::new());
        // Opened only for this: back to the tray.
        if opened_window.get() {
            opened_window.set(false);
            hide_current_window();
        }
    };
    // Each release's heading, then its items: (is a heading, text).
    let rows = move || {
        news.with(|n| {
            n.iter()
                .flat_map(|r| std::iter::once((true, format!("beardictate {}", r.version))).chain(r.items.iter().map(|i| (false, i.clone()))))
                .collect::<Vec<_>>()
        })
    };
    rsx! {
        Modal {
            opened_fn: move || news.with(|n| !n.is_empty()),
            onclose: close,
            title: "What\u{2019}s new",
            size: "lg",
            centered: true,
            div { style: "display: flex; flex-direction: column; gap: 8px; max-height: 320px; overflow-y: auto; padding-right: 12px;",
                for (heading, text) in rows() {
                    if heading {
                        Text { weight: "700", {text.clone()} }
                    } else {
                        div { style: "display: flex; gap: 8px;",
                            Text { size: "sm", "\u{2022}" }
                            Text { size: "sm", {text.clone()} }
                        }
                    }
                }
            }
            Group { justify: "flex-end", style: "margin-top: 16px;",
                Button { onclick: close, "Got it" }
            }
        }
    }
}

/// Shows this version's notes again (About's button).
pub fn show_current(news: Signal<Vec<Release>>) {
    if let Some(r) = crate::whats_new::current() {
        news.set(vec![r]);
    }
}
