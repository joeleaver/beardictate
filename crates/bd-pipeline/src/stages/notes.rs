//! Events that only report what the finisher did, for diagnostics.

use std::time::Duration;

use bd_commands::{ActionKind, CommandOutput};
use bd_core::CleanupReport;

use super::Finisher;
use crate::{Event, PadAction};

impl Finisher {
    pub(super) fn note_commanded(&self, id: u64, heard: &str, vocabulary: &str, out: &CommandOutput, pad: Option<&PadAction>) {
        let actions = out.actions.iter().map(|a| action_name(&a.kind).to_string()).collect();
        self.emit(Event::Commanded {
            id,
            heard: heard.to_string(),
            vocabulary: vocabulary.to_string(),
            commanded: out.text.clone(),
            actions,
            pad_command: pad.map(|a| format!("{a:?}")),
        });
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn note_cleaned(
        &self,
        segments: Vec<u64>,
        context: String,
        raw: &str,
        output: &str,
        elapsed: Duration,
        report: Option<CleanupReport>,
        pad: bool,
    ) {
        let (raw, output) = (raw.to_string(), output.to_string());
        self.emit(Event::Cleaned { segments, context, raw, output, elapsed, report, pad });
    }
}

fn action_name(kind: &ActionKind) -> &'static str {
    match kind {
        ActionKind::OpenQuote => "open_quote",
        ActionKind::CloseQuote => "close_quote",
        ActionKind::ParagraphBreak => "new_paragraph",
        ActionKind::LineBreak => "new_line",
        ActionKind::Punctuation(_) => "punctuation",
        ActionKind::ScratchPreviousSegment => "scratch_previous",
        ActionKind::ScratchWithinSegment { .. } => "scratch_within",
    }
}
