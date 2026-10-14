//! The author's typographic conventions, fed to the command pass and the
//! cleanup model.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum QuoteStyle {
    /// “Curly double” — US default.
    #[default]
    CurlyDouble,
    /// ‘Curly single’ — UK default.
    CurlySingle,
    /// "Straight"
    Straight,
}

impl QuoteStyle {
    /// The (open, close) marks for dialog.
    pub fn marks(self) -> (char, char) {
        match self {
            QuoteStyle::CurlyDouble => ('\u{201C}', '\u{201D}'),
            QuoteStyle::CurlySingle => ('\u{2018}', '\u{2019}'),
            QuoteStyle::Straight => ('"', '"'),
        }
    }
}

/// Where a comma or period goes relative to a closing quote.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PunctuationPlacement {
    /// Always inside: “To the river,” he said. (US)
    #[default]
    Inside,
    /// Inside only when it belongs to the quoted speech. (UK)
    Logical,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Style {
    pub quotes: QuoteStyle,
    pub punctuation: PunctuationPlacement,
}
