//! When the finisher moves on: how long a half sentence waits for the rest,
//! when cleanup is too far behind to wait for, and when a new passage starts.

use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, select};

use super::{Finisher, Heard, Item};

/// A sentence that waited this long for cleanup, with more queued behind
/// it, is delivered as dictated: cleanup is falling behind the speaker, and
/// text that never appears looks like nothing is working.
pub(super) const BACKLOG: Duration = Duration::from_secs(4);

/// How long to wait when nothing is due.
const IDLE: Duration = Duration::from_secs(3600);

/// What [`Finisher::wait`] got.
pub(super) enum Waited {
    Heard(Heard),
    Timeout,
    Closed,
}

impl Finisher {
    /// Waits for the next transcript, up to `timeout`, swapping in tidied
    /// sentences as they come.
    pub(super) fn wait(&mut self, input: &Receiver<Heard>, timeout: Option<Duration>) -> Waited {
        let deadline = timeout.map(|t| Instant::now() + t);
        loop {
            let tidied = self.tidy_results();
            let left = deadline.map_or(IDLE, |d| d.saturating_duration_since(Instant::now()));
            select! {
                recv(input) -> heard => return heard.map_or(Waited::Closed, Waited::Heard),
                recv(tidied) -> cleaned => if let Ok(cleaned) = cleaned { self.tidied(cleaned) },
                default(left) => if deadline.is_some() { return Waited::Timeout },
            }
        }
    }

    /// Whether the words go on the page as they're spoken.
    pub(super) fn types_live(&self) -> bool {
        self.live && self.live_typing && self.pad.is_none()
    }

    /// When everything queued is one unfinished sentence, how much longer
    /// to wait for the rest of it: a short while after its last part came
    /// in, or, while the next utterance is being spoken, until that one
    /// ends (within a limit).
    pub(super) fn hold(&self) -> Option<Duration> {
        // Live text is on the page already: each utterance is finished alone.
        if self.types_live() {
            return None;
        }
        let mut segments = self.queue.iter().enumerate().map(|(i, item)| match item {
            Item::Segment(p) if i == 0 || p.continues() => Some(p),
            _ => None,
        });
        let (Some(Some(first)), Some(Item::Segment(last))) = (segments.next(), self.queue.back()) else { return None };
        if !segments.all(|p| p.is_some()) || last.ends_sentence() {
            return None;
        }
        let deadline = match self.speaking {
            Some(id) if id > last.id => first.ready + self.sentence_hold_speaking,
            _ => last.ready + self.sentence_hold,
        };
        let left = deadline.saturating_duration_since(Instant::now());
        (!left.is_zero()).then_some(left)
    }

    /// Starts a new passage before this segment if the speaker paused a long
    /// time since the last one, or is dictating into another app: what's in
    /// the context then belongs somewhere else.
    pub(super) fn new_passage_if_moved_on(&mut self) {
        let now = Instant::now();
        let paused = self.last_segment_at.replace(now).is_some_and(|t| now - t >= self.passage_gap);
        let app = self.focus.as_mut().and_then(|probe| probe());
        let moved = matches!((&self.last_focus, &app), (Some(a), Some(b)) if a != b);
        if app.is_some() {
            self.last_focus = app;
        }
        if (paused || moved) && self.passage_has_text {
            tracing::info!(paused, moved, "new passage");
            self.take_in(Heard::NewPassage);
        }
    }
}
