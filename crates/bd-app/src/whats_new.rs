//! "What's new": the first start of a new version, after an automatic
//! update or an install over an older one, shows what changed since the
//! version that ran last (`WHATS_NEW.md`, built in). A fresh install shows
//! nothing; the first-run setup is enough.
//!
//! The version that ran last is kept in `last-version` in the state folder.
//! Installs from before this file existed have settings but no
//! `last-version`: they're shown this version's notes.

use std::path::Path;

use crate::update::feed::version_key;

const NOTES: &str = include_str!("../WHATS_NEW.md");
const CURRENT: &str = env!("CARGO_PKG_VERSION");

/// One release's notes.
#[derive(Debug, Clone, PartialEq)]
pub struct Release {
    pub version: String,
    pub items: Vec<String>,
}

/// The releases in `text`, as written (newest first).
fn parse(text: &str) -> Vec<Release> {
    let mut out: Vec<Release> = Vec::new();
    for line in text.lines() {
        if let Some(v) = line.strip_prefix("## ") {
            out.push(Release { version: v.trim().to_string(), items: Vec::new() });
        } else if let (Some(item), Some(r)) = (line.strip_prefix("- "), out.last_mut()) {
            r.items.push(item.trim().to_string());
        }
    }
    out.retain(|r| !r.items.is_empty());
    out
}

/// The releases after `last` up to `current`, newest first: only
/// `current`'s when `last` is unknown, none when it's `current` or newer.
fn since(releases: &[Release], last: Option<&str>, current: &str) -> Vec<Release> {
    let now = version_key(current);
    let from = last.map(version_key);
    if from.as_ref().is_some_and(|f| *f >= now) {
        return Vec::new();
    }
    releases
        .iter()
        .filter(|r| {
            let v = version_key(&r.version);
            match &from {
                Some(f) => v > *f && v <= now,
                None => v == now,
            }
        })
        .cloned()
        .collect()
}

/// At start: what to show, if anything, and records this version as the
/// one that ran. `installed_before`: there were settings already (not a
/// fresh install).
pub fn at_start(state: &Path, installed_before: bool) -> Vec<Release> {
    let file = state.join("last-version");
    let last = std::fs::read_to_string(&file).ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
    if last.as_deref() != Some(CURRENT)
        && let Err(e) = std::fs::create_dir_all(state).and_then(|()| std::fs::write(&file, CURRENT))
    {
        tracing::warn!("saving the version that ran: {e}");
    }
    if last.is_none() && !installed_before {
        return Vec::new();
    }
    let news = since(&parse(NOTES), last.as_deref(), CURRENT);
    if !news.is_empty() {
        tracing::info!(from = ?last, to = CURRENT, "showing what's new");
    }
    news
}

/// This version's notes, for About.
pub fn current() -> Option<Release> {
    parse(NOTES).into_iter().find(|r| version_key(&r.version) == version_key(CURRENT))
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEXT: &str = "# What's new\n\nIntro.\n\n## 0.1.24\n\n- C\n\n## 0.1.23\n\n- A\n- B\n\n## 0.1.20\n\n- Old\n";

    fn versions(r: &[Release]) -> Vec<&str> {
        r.iter().map(|r| r.version.as_str()).collect()
    }

    #[test]
    fn notes_are_read_by_version() {
        let r = parse(TEXT);
        assert_eq!(versions(&r), ["0.1.24", "0.1.23", "0.1.20"]);
        assert_eq!(r[1].items, ["A", "B"]);
    }

    #[test]
    fn shows_what_came_since_the_last_version_that_ran() {
        let r = parse(TEXT);
        assert_eq!(versions(&since(&r, Some("0.1.22"), "0.1.24")), ["0.1.24", "0.1.23"]);
        assert_eq!(versions(&since(&r, Some("0.1.23"), "0.1.23")), Vec::<&str>::new());
        // Going back a version shows nothing.
        assert_eq!(versions(&since(&r, Some("0.1.24"), "0.1.23")), Vec::<&str>::new());
        // Unknown (an install from before `last-version`): this version's.
        assert_eq!(versions(&since(&r, None, "0.1.23")), ["0.1.23"]);
        // A suffix doesn't count.
        assert_eq!(versions(&since(&r, Some("0.1.22-alpha"), "0.1.23-alpha")), ["0.1.23"]);
    }

    #[test]
    fn once_per_version_and_never_on_a_fresh_install() {
        let dir = std::env::temp_dir().join(format!("bd-whats-new-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        assert!(at_start(&dir, false).is_empty(), "fresh install");
        assert_eq!(std::fs::read_to_string(dir.join("last-version")).unwrap(), CURRENT);
        assert!(at_start(&dir, true).is_empty(), "same version again");
        std::fs::write(dir.join("last-version"), "0.0.1").unwrap();
        let shown = at_start(&dir, true);
        assert_eq!(shown.is_empty(), parse(NOTES).iter().all(|r| version_key(&r.version) > version_key(CURRENT)));
        assert!(at_start(&dir, true).is_empty(), "shown once");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_built_in_notes_parse() {
        let r = parse(NOTES);
        assert!(!r.is_empty());
        for w in r.windows(2) {
            assert!(version_key(&w[0].version) > version_key(&w[1].version), "newest first: {} then {}", w[0].version, w[1].version);
        }
    }
}
