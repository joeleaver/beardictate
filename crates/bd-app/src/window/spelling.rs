//! The Writing page's spelling choice: the recognizer writes colour or
//! color as it pleases, so the app makes it one or the other.

use bd_core::Spelling;
use rinch::prelude::*;

use crate::ui::{Shared, change};

pub fn spelling(__scope: &mut RenderScope, s: &Shared) -> NodeHandle {
    let (a, b) = (s.clone(), s.clone());
    rsx! {
        div { style: "display: flex; flex-direction: column; gap: 6px;",
            Text { style: "font-weight: 600;", "Spelling" }
            Text { size: "sm", color: "dimmed", "Speech recognition knows both and writes either, even in one sentence. Your special words keep their own spelling." }
            Select {
                data: vec![
                    SelectOption::new("american", "American: color, center, realize, traveled"),
                    SelectOption::new("british", "British: colour, centre, realise, travelled"),
                    SelectOption::new("as-heard", "As heard (either)"),
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
        Spelling::British => "british",
        Spelling::AsHeard => "as-heard",
    }
}

fn from_key(v: &str) -> Spelling {
    match v {
        "british" => Spelling::British,
        "as-heard" => Spelling::AsHeard,
        _ => Spelling::American,
    }
}
