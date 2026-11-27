//! The user's recent pauses mid-sentence ([`bd_pipeline::Event::Paused`]),
//! kept in a small file of lengths (no words), and the sentence wait they
//! suggest ([`sentence_wait::suggest`]), shown under the setting.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::time::Duration;

use bd_pipeline::sentence_wait::{self, Pause};
use serde_json::json;

use super::{Ui, Worker};
use crate::telemetry::record;

/// Saved after this many new pauses, as well as when a dictation ends and
/// when the app quits.
const SAVE_EVERY: usize = 20;

pub(super) struct Pauses {
    path: PathBuf,
    /// The most recent [`sentence_wait::KEEP`], oldest first.
    recent: VecDeque<Pause>,
    /// Added since last saved.
    unsaved: usize,
    suggested: Option<Duration>,
}

impl Pauses {
    /// The pauses saved at `path`; none when there's no file or it doesn't
    /// read (it's only ever written by the app).
    pub(super) fn load(path: &Path) -> Pauses {
        let mut recent = std::fs::read_to_string(path).map(|text| parse(&text)).unwrap_or_default();
        let suggested = sentence_wait::suggest(recent.make_contiguous());
        Pauses { path: path.to_path_buf(), recent, unsaved: 0, suggested }
    }

    /// Keeps `pause`. Returns whether enough are unsaved to save now
    /// (always listening can stay awake for hours).
    pub(super) fn add(&mut self, pause: Pause) -> bool {
        if self.recent.len() == sentence_wait::KEEP {
            self.recent.pop_front();
        }
        self.recent.push_back(pause);
        self.unsaved += 1;
        self.unsaved >= SAVE_EVERY
    }

    /// Saves what was added, and works the suggestion out again. Returns
    /// whether the suggestion changed.
    fn settle(&mut self) -> bool {
        if std::mem::take(&mut self.unsaved) == 0 {
            return false;
        }
        if let Err(e) = crate::settings::write_atomic(&self.path, &render(&self.recent)) {
            tracing::warn!("saving the pauses: {e:#}");
        }
        let suggested = sentence_wait::suggest(self.recent.make_contiguous());
        std::mem::replace(&mut self.suggested, suggested) != suggested
    }

    /// Tells the UI the suggestion.
    pub(super) fn tell(&self, ui: &impl Ui) {
        ui.sentence_wait_suggested(self.suggested, self.recent.len());
    }
}

impl<U: Ui> Worker<U> {
    pub(super) fn add_pause(&mut self, pause: Pause) {
        if self.pauses.add(pause) {
            self.settle_pauses();
        }
    }

    /// A dictation ended (or the app quits, or enough came): keep its pauses, and say if the suggested wait
    /// moved (to the UI, and to telemetry, to see what testers are offered).
    pub(super) fn settle_pauses(&mut self) {
        if !self.pauses.settle() {
            return;
        }
        let (suggested, pauses) = (self.pauses.suggested, self.pauses.recent.len());
        tracing::info!(pauses, "suggested sentence wait: {suggested:?}");
        record("state", None, || json!({"sentence_wait_suggested_secs": suggested.map(|d| d.as_secs_f32()), "pauses": pauses}));
        self.pauses.tell(&self.ui);
    }
}

/// The file: `[[pause_ms, wait_ms], …]`, oldest first.
fn parse(text: &str) -> VecDeque<Pause> {
    let pairs: Vec<(u64, u64)> = serde_json::from_str(text).unwrap_or_default();
    let skip = pairs.len().saturating_sub(sentence_wait::KEEP);
    pairs
        .into_iter()
        .skip(skip)
        .map(|(pause, wait)| Pause { pause: Duration::from_millis(pause), wait: Duration::from_millis(wait) })
        .collect()
}

fn render(pauses: &VecDeque<Pause>) -> String {
    let pairs: Vec<(u128, u128)> = pauses.iter().map(|p| (p.pause.as_millis(), p.wait.as_millis())).collect();
    serde_json::to_string(&pairs).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pause(ms: u64) -> Pause {
        Pause { pause: Duration::from_millis(ms), wait: Duration::from_millis(1500) }
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("bd-pauses-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir.join("sentence-pauses.json")
    }

    #[test]
    fn keeps_the_most_recent_and_reads_them_back() {
        let path = scratch("keep");
        let mut pauses = Pauses::load(&path);
        assert!(pauses.recent.is_empty() && pauses.suggested.is_none());
        for i in 0..sentence_wait::KEEP as u64 + 10 {
            pauses.add(pause(i));
        }
        assert_eq!(pauses.recent.len(), sentence_wait::KEEP);
        assert_eq!(pauses.recent.front(), Some(&pause(10)));
        pauses.settle();
        let again = Pauses::load(&path);
        assert_eq!(again.recent, pauses.recent);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn suggests_once_there_are_enough() {
        let path = scratch("suggest");
        let mut pauses = Pauses::load(&path);
        // Spread from 0.1 to 1.4 s, all under the 1.5 s wait.
        for i in 0..sentence_wait::ENOUGH as u64 - 1 {
            pauses.add(pause(100 + i * 1300 / sentence_wait::ENOUGH as u64));
        }
        assert!(!pauses.settle(), "too few to suggest");
        pauses.add(pause(800));
        assert!(pauses.settle());
        assert!(pauses.suggested.is_some());
        // Nothing new, nothing to save or tell.
        assert!(!pauses.settle());
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn a_file_that_does_not_read_is_no_pauses() {
        assert!(parse("not json").is_empty());
        assert_eq!(parse("[[300,1500],[900,2000]]"), VecDeque::from([pause(300), Pause { pause: Duration::from_millis(900), wait: Duration::from_secs(2) }]));
    }
}
