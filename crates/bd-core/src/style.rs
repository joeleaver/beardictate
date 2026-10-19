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
    /// "Straight" double.
    Straight,
    /// 'Straight' single.
    StraightSingle,
}

impl QuoteStyle {
    /// Typewriter marks, which face neither way.
    pub fn is_straight(self) -> bool {
        matches!(self, QuoteStyle::Straight | QuoteStyle::StraightSingle)
    }

    /// The (open, close) marks for dialog.
    pub fn marks(self) -> (char, char) {
        match self {
            QuoteStyle::CurlyDouble => ('\u{201C}', '\u{201D}'),
            QuoteStyle::CurlySingle => ('\u{2018}', '\u{2019}'),
            QuoteStyle::Straight => ('"', '"'),
            QuoteStyle::StraightSingle => ('\'', '\''),
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Style {
    pub quotes: QuoteStyle,
    pub punctuation: PunctuationPlacement,
    /// Let the cleanup model put speech it hears in the prose in quote
    /// marks (fiction). Off, only dictated quote marks stay.
    pub dialog: bool,
    /// Let the cleanup model start a new paragraph when a different
    /// character speaks. Off, the only breaks are the dictated ones ("new
    /// paragraph", "new line") and any the pipeline adds at long pauses.
    pub dialog_paragraphs: bool,
    /// Let the cleanup model also start a new paragraph where the prose
    /// calls for one: a shift of topic, place, time or focus.
    pub topic_paragraphs: bool,
    /// When one character's speech runs on into a new paragraph: close the
    /// quotation at the end of each paragraph too. Off (the usual book
    /// style), the paragraph ends with no closing mark and only the next one
    /// opens: “…the river.¶“And then…
    pub close_continued_speech: bool,
}

impl Style {
    /// Whether the cleanup model may add paragraph breaks at all.
    pub fn model_paragraphs(&self) -> bool {
        self.dialog_paragraphs || self.topic_paragraphs
    }
}

impl Default for Style {
    fn default() -> Self {
        Self {
            quotes: QuoteStyle::default(),
            punctuation: PunctuationPlacement::default(),
            dialog: true,
            dialog_paragraphs: true,
            topic_paragraphs: false,
            close_continued_speech: false,
        }
    }
}
