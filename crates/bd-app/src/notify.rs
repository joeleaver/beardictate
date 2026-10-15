//! OS notifications for things that need the user, rate-limited per kind.
//!
//! Through rinch's notifications, shown from the UI thread (the engine
//! calls from its own). The desktop finds the icon and name from
//! `beardictate.desktop`.

use std::collections::HashMap;
use std::time::{Duration, Instant};

/// The same kind of notice at most this often.
const MIN_INTERVAL: Duration = Duration::from_secs(60);

pub struct Notifier {
    enabled: bool,
    last: HashMap<&'static str, Instant>,
}

impl Notifier {
    pub fn new(enabled: bool) -> Self {
        Self { enabled, last: HashMap::new() }
    }

    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
    }

    /// Shows a notification unless one of the same `kind` went out within
    /// the last minute. Always logged.
    pub fn notify(&mut self, kind: &'static str, title: &str, body: &str) {
        tracing::warn!(kind, "{title}: {body}");
        if !self.enabled {
            return;
        }
        let now = Instant::now();
        if self.last.get(kind).is_some_and(|t| now.duration_since(*t) < MIN_INTERVAL) {
            return;
        }
        self.last.insert(kind, now);
        let (title, body) = (title.to_string(), body.to_string());
        rinch::run_on_main_thread(move || {
            let shown = rinch::notification::Notification::new(title)
                .body(body)
                .on_error(|e| tracing::warn!("notification not shown: {e}"))
                .show();
            if let Err(e) = shown {
                tracing::warn!("could not show a notification: {e}");
            }
        });
    }
}
