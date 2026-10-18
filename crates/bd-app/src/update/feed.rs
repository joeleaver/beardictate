//! The site's release list (`/downloads/releases.json`, written by
//! `site/publish.sh`), and which build in it is an update for this app.

use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct Feed {
    #[serde(default)]
    pub builds: Vec<Build>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Build {
    pub platform: String,
    pub version: String,
    /// "Installer", "Archive", "Portable zip"…
    pub kind: String,
    pub file: String,
    pub size: u64,
    pub sha256: String,
    /// Ed25519 over the file, hex (`site/publish.sh` signs what it adds).
    #[serde(default)]
    pub signature: Option<String>,
    /// `stable` when absent; alpha builds update only to alpha builds.
    #[serde(default)]
    pub channel: Option<String>,
    #[serde(default)]
    pub placeholder: bool,
}

/// This build's platform id in the feed.
pub fn platform() -> &'static str {
    match std::env::consts::OS {
        "windows" => "windows",
        "macos" => "macos",
        _ => "linux",
    }
}

/// The kind of build an update installs from: the installer on Windows, the
/// tarball (with its install.sh) on Linux.
pub fn installable_kind() -> &'static str {
    if cfg!(windows) {
        "Installer"
    } else {
        "Archive"
    }
}

pub fn channel() -> &'static str {
    if cfg!(feature = "alpha") {
        "alpha"
    } else {
        "stable"
    }
}

/// The newest build for this platform and channel that installs, if it's
/// newer than `current`.
pub fn newest<'a>(
    feed: &'a Feed,
    current: &str,
    platform: &str,
    channel: &str,
) -> Option<&'a Build> {
    feed.builds
        .iter()
        .filter(|b| !b.placeholder && b.platform == platform && b.kind == installable_kind())
        .filter(|b| b.channel.as_deref().unwrap_or("stable") == channel)
        .filter(|b| newer(&b.version, current))
        .max_by(|a, b| version_key(&a.version).cmp(&version_key(&b.version)))
}

/// `0.1.10` sorts after `0.1.9`; a suffix (`-alpha`) is ignored.
pub fn version_key(v: &str) -> Vec<u64> {
    v.split(['-', '+'])
        .next()
        .unwrap_or("")
        .split('.')
        .map(|p| p.parse().unwrap_or(0))
        .collect()
}

pub fn newer(candidate: &str, current: &str) -> bool {
    version_key(candidate) > version_key(current)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build(platform: &str, version: &str, kind: &str, channel: Option<&str>) -> Build {
        Build {
            platform: platform.into(),
            version: version.into(),
            kind: kind.into(),
            file: format!("beardictate-{version}-{platform}"),
            size: 1,
            sha256: String::new(),
            signature: None,
            channel: channel.map(Into::into),
            placeholder: false,
        }
    }

    #[test]
    fn versions_compare_by_number() {
        assert!(newer("0.1.10", "0.1.9"));
        assert!(newer("0.2.0", "0.1.99"));
        assert!(!newer("0.1.7", "0.1.7"));
        assert!(!newer("0.1.7-alpha", "0.1.7"));
        assert!(newer("1.0", "0.9.9"));
    }

    #[test]
    fn the_newest_installable_build_for_this_platform_and_channel() {
        let kind = installable_kind();
        let feed = Feed {
            builds: vec![
                build("linux", "0.1.8", "Archive", None),
                build("linux", "0.1.9", "Archive", Some("alpha")),
                build("windows", "0.1.8", "Installer", None),
                build("windows", "0.1.9", "Portable zip", None),
                build("windows", "0.1.10", "Installer", Some("alpha")),
                build("linux", "0.1.6", "Archive", None),
            ],
        };
        let p = platform();
        let pick = |channel| newest(&feed, "0.1.7", p, channel).map(|b| b.version.clone());
        if kind == "Archive" {
            assert_eq!(pick("stable").as_deref(), Some("0.1.8"));
            assert_eq!(pick("alpha").as_deref(), Some("0.1.9"));
        } else {
            assert_eq!(pick("stable").as_deref(), Some("0.1.8"));
            assert_eq!(pick("alpha").as_deref(), Some("0.1.10"));
        }
        assert_eq!(newest(&feed, "0.1.8", p, "stable"), None);
    }

    #[test]
    fn the_published_feed_parses() {
        let text = include_str!("../../../../site/public/downloads/releases.json");
        let feed: Feed = serde_json::from_str(text).unwrap();
        assert!(!feed.builds.is_empty());
    }
}
