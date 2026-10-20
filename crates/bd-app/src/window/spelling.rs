//! The Writing page's spelling choice. The recognizer (Nemotron) often
//! writes British spelling, and sometimes American in the same sentence, so
//! the app makes it one or the other. Leaving it alone isn't offered: that
//! would be British with American mixed in (Joe, 2026-10-20).

use bd_core::Spelling;
use rinch::prelude::*;

use crate::ui::{Shared, change};

pub fn spelling(__scope: &mut RenderScope, s: &Shared) -> NodeHandle {
    let (a, b) = (s.clone(), s.clone());
    rsx! {
        div { style: "display: flex; flex-direction: column; gap: 6px;",
            Text { style: "font-weight: 600;", "Spelling" }
            Text { size: "sm", color: "dimmed", "Speech recognition often writes British spelling, and sometimes American in the same sentence. This makes it one or the other. Your special words keep their own spelling." }
            Select {
                data: vec![
                    SelectOption::new("american", "American: color, center, realize, traveled"),
                    SelectOption::new("british", "British: colour, centre, realise, travelled"),
                ],
                value_fn: move || key(a.settings.with(|st| st.spelling)).to_string(),
                onchange: move |v: String| change(&b, |st| st.spelling = from_key(&v)),
            }
        }
    }
}

fn key(s: Spelling) -> &'static str {
    match s {
        Spelling::American => "american",
        // Not offered; shown as what it mostly is.
        Spelling::British | Spelling::Unchanged => "british",
    }
}

fn from_key(v: &str) -> Spelling {
    match v {
        "british" => Spelling::British,
        _ => Spelling::American,
    }
}
