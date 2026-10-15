//! The user's settings, kept as TOML in the platform config directory, and
//! the word list next to them.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result};
use bd_core::{PunctuationPlacement, QuoteStyle, Style};
use serde::{Deserialize, Serialize};

/// Where new paragraphs come from besides a dictated "new paragraph".
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Paragraphs {
    /// Only dictated breaks.
    Off,
    /// The cleanup model starts one when a different character speaks.
    #[default]
    Dialog,
    /// Dialog paragraphs, and one after a long pause.
    Auto,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum QuoteConvention {
    /// “Curly double” quotes, commas and periods inside (US).
    #[default]
    Us,
    /// ‘Curly single’ quotes, logical punctuation (UK).
    Uk,
}

/// The tray glyph's colours: dark ink for a light taskbar, light ink for a
/// dark one.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TrayTheme {
    /// Guess from the desktop's settings.
    #[default]
    Auto,
    Light,
    Dark,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// The dictation hotkey: press to start, press again to stop. At least
    /// two keys (DESIGN.md "Hotkey"). On Wayland the desktop asks to approve
    /// it and may change it.
    pub hotkey: String,
    /// `auto` (the largest installed), `off`, a catalog id, or a .gguf path.
    pub cleanup: String,
    pub paragraphs: Paragraphs,
    /// With `paragraphs = "auto"`: a pause this long, in seconds, starts a
    /// paragraph.
    pub pause_paragraph_secs: f32,
    pub quotes: QuoteConvention,
    /// How strongly the speech recognizer listens for the word list.
    pub hotword_boost: f32,
    /// Microphone name; empty for the system default.
    pub microphone: String,
    /// Keep the microphone open between dictations. The first syllable is
    /// never clipped, but the desktop shows the mic as in use all the time.
    pub keep_mic_open: bool,
    pub tray_theme: TrayTheme,
    /// Where the models are; empty for the default (or `BD_MODELS_DIR`).
    pub models_dir: String,
    /// OS notifications when something needs attention.
    pub notifications: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            hotkey: "Ctrl+Alt+Space".into(),
            cleanup: "auto".into(),
            paragraphs: Paragraphs::Dialog,
            pause_paragraph_secs: 3.0,
            quotes: QuoteConvention::Us,
            hotword_boost: 2.0,
            microphone: String::new(),
            keep_mic_open: false,
            tray_theme: TrayTheme::Auto,
            models_dir: String::new(),
            notifications: true,
        }
    }
}

impl Settings {
    pub fn style(&self) -> Style {
        let mut style = match self.quotes {
            QuoteConvention::Us => Style::default(),
            QuoteConvention::Uk => Style {
                quotes: QuoteStyle::CurlySingle,
                punctuation: PunctuationPlacement::Logical,
                ..Style::default()
            },
        };
        style.dialog_paragraphs = self.paragraphs != Paragraphs::Off;
        style
    }

    pub fn pause_paragraph(&self) -> Option<Duration> {
        (self.paragraphs == Paragraphs::Auto).then(|| Duration::from_secs_f32(self.pause_paragraph_secs.max(0.5)))
    }
}

/// The app's files.
#[derive(Debug, Clone)]
pub struct Paths {
    pub settings: PathBuf,
    pub words: PathBuf,
    /// The Wayland remote-input approval, so the desktop asks only once.
    pub portal_token: PathBuf,
}

impl Paths {
    pub fn locate() -> Result<Paths> {
        let dirs = directories::ProjectDirs::from("", "", "beardictate").context("no home directory")?;
        let config = dirs.config_dir();
        fs::create_dir_all(config).with_context(|| format!("creating {}", config.display()))?;
        Ok(Paths {
            settings: config.join("settings.toml"),
            words: config.join("words.txt"),
            portal_token: dirs.data_local_dir().join("portal-token"),
        })
    }

    /// Reads the settings, writing the defaults on first run. A file that
    /// doesn't parse is an error (so a typo isn't silently replaced).
    pub fn load(&self) -> Result<Settings> {
        if !self.settings.exists() {
            let settings = Settings::default();
            self.save(&settings)?;
            return Ok(settings);
        }
        let text = fs::read_to_string(&self.settings).with_context(|| format!("reading {}", self.settings.display()))?;
        toml::from_str(&text).with_context(|| format!("in {}", self.settings.display()))
    }

    pub fn save(&self, settings: &Settings) -> Result<()> {
        let body = toml::to_string_pretty(settings).context("serializing settings")?;
        let text = format!("{SETTINGS_HEADER}\n{body}");
        write_atomic(&self.settings, &text)
    }

    /// Reads the word list, creating an explained empty one on first run.
    pub fn load_words(&self) -> Result<bd_vocab::Vocabulary> {
        if !self.words.exists() {
            write_atomic(&self.words, WORDS_TEMPLATE)?;
        }
        let text = fs::read_to_string(&self.words).with_context(|| format!("reading {}", self.words.display()))?;
        bd_vocab::Vocabulary::parse(&text).with_context(|| format!("in {}", self.words.display()))
    }

    pub fn load_portal_token(&self) -> Option<String> {
        fs::read_to_string(&self.portal_token).ok().map(|t| t.trim().to_string()).filter(|t| !t.is_empty())
    }
}

fn write_atomic(path: &Path, text: &str) -> Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    }
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, text).with_context(|| format!("writing {}", tmp.display()))?;
    fs::rename(&tmp, path).with_context(|| format!("replacing {}", path.display()))
}

const SETTINGS_HEADER: &str = "\
# beardictate settings. The app rewrites this file when you change a setting
# from the tray menu, and reloads it when you save it here.
#
# hotkey: at least two keys, e.g. \"Ctrl+Alt+Space\", \"Super+Shift+D\".
# cleanup: \"auto\" (largest installed model), \"off\", a model id from
#   `bd models list`, or the path to a .gguf file.
# paragraphs: \"off\", \"dialog\" (new paragraph per speaker) or \"auto\"
#   (dialog, plus a paragraph after a pause of pause_paragraph_secs).
# quotes: \"us\" or \"uk\".  tray_theme: \"auto\", \"light\" or \"dark\".
# models_dir: where the models are; empty for the default location.
";

const WORDS_TEMPLATE: &str = "\
# Your names and invented words, one per line. beardictate listens for these
# spellings and replaces what it tends to hear instead (after the colon,
# comma-separated). Saved changes apply to the next dictation.
#
# Ysolde: Isolde, Ussel
# wyrmstone: worm stone, wormstone
# Port Varrow
";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_round_trip_and_fill_in_missing_keys() {
        let s = Settings { paragraphs: Paragraphs::Auto, quotes: QuoteConvention::Uk, ..Settings::default() };
        let text = toml::to_string_pretty(&s).unwrap();
        assert_eq!(toml::from_str::<Settings>(&text).unwrap(), s);
        let partial: Settings = toml::from_str("paragraphs = \"off\"\n").unwrap();
        assert_eq!(partial.paragraphs, Paragraphs::Off);
        assert_eq!(partial.hotkey, "Ctrl+Alt+Space");
        assert!(!partial.style().dialog_paragraphs);
        assert_eq!(partial.pause_paragraph(), None);
    }

    #[test]
    fn the_word_template_parses_to_nothing() {
        assert!(bd_vocab::Vocabulary::parse(WORDS_TEMPLATE).unwrap().is_empty());
    }
}
