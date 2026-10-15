//! The user's settings, kept as TOML in the platform config directory, and
//! the special word list next to them.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result};
use bd_commands::CommandConfig;
use bd_core::{PunctuationPlacement, QuoteStyle, Style};
use serde::{Deserialize, Serialize};

/// The marks around dialog.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum QuoteMarks {
    /// “Curly double”, with ‘single’ inside: US, and many UK publishers.
    #[default]
    Double,
    /// ‘Curly single’, with “double” inside: traditional British books.
    Single,
}

/// Where a comma or full stop goes against a closing quote mark.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Punctuation {
    /// Always inside: “To the river,” he said. (US)
    #[default]
    Inside,
    /// Inside only when it belongs to the speech. (UK)
    Logical,
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
    /// New paragraph when you say "new paragraph" (and a line break for
    /// "new line"). Off, those words are typed.
    pub paragraph_on_command: bool,
    /// New paragraph when a different character starts speaking.
    pub paragraph_on_speaker: bool,
    /// New paragraph after a pause of `pause_paragraph_secs`.
    pub paragraph_on_pause: bool,
    /// New paragraph where the prose calls for one (topic, place, time).
    pub paragraph_on_topic: bool,
    pub pause_paragraph_secs: f32,
    pub quote_marks: QuoteMarks,
    /// Curly quotation marks (“ ” ‘ ’); off, straight typewriter ones.
    pub curly_quotes: bool,
    pub punctuation: Punctuation,
    /// How strongly the speech recognizer listens for the special words.
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
    /// Names and invented words: the recognizer listens for them, and what
    /// it hears instead is replaced.
    pub special_words: Vec<SpecialWord>,
}

/// One special word.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpecialWord {
    /// The spelling to produce.
    pub word: String,
    /// What the recognizer writes instead, any casing.
    #[serde(default)]
    pub heard: Vec<String>,
}

impl SpecialWord {
    pub fn new(word: &str, heard: &[&str]) -> Self {
        Self { word: word.into(), heard: heard.iter().map(|h| h.to_string()).collect() }
    }
}

/// The list a new user starts with: a real entry, which also shows how.
pub fn starter_words() -> Vec<SpecialWord> {
    vec![SpecialWord::new("beardictate", &["bear dictate", "bare dictate"])]
}

/// The pipeline's vocabulary for `words`; empty entries are skipped.
pub fn vocabulary(words: &[SpecialWord]) -> bd_vocab::Vocabulary {
    bd_vocab::Vocabulary::new(
        words
            .iter()
            .filter(|w| !w.word.trim().is_empty())
            .map(|w| bd_vocab::Word {
                text: w.word.trim().to_string(),
                heard: w.heard.iter().map(|h| h.trim().to_string()).filter(|h| !h.is_empty()).collect(),
            })
            .collect(),
    )
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            hotkey: "Ctrl+Backquote".into(),
            cleanup: "auto".into(),
            paragraph_on_command: true,
            paragraph_on_speaker: true,
            paragraph_on_pause: false,
            paragraph_on_topic: false,
            pause_paragraph_secs: 3.0,
            quote_marks: QuoteMarks::Double,
            curly_quotes: true,
            punctuation: Punctuation::Inside,
            hotword_boost: 2.0,
            microphone: String::new(),
            keep_mic_open: false,
            tray_theme: TrayTheme::Auto,
            models_dir: String::new(),
            notifications: true,
            special_words: starter_words(),
        }
    }
}

/// The settings a running pipeline was built from: a change to any of them
/// means loading it again.
#[derive(Debug, Clone, PartialEq)]
pub struct PipelineSettings {
    cleanup: String,
    models_dir: String,
    style: Style,
    commands: CommandConfig,
    pause_paragraph: Option<Duration>,
    hotword_boost: f32,
    special_words: Vec<SpecialWord>,
}

impl Settings {
    pub fn style(&self) -> Style {
        Style {
            quotes: match (self.quote_marks, self.curly_quotes) {
                (QuoteMarks::Double, true) => QuoteStyle::CurlyDouble,
                (QuoteMarks::Single, true) => QuoteStyle::CurlySingle,
                (QuoteMarks::Double, false) => QuoteStyle::Straight,
                (QuoteMarks::Single, false) => QuoteStyle::StraightSingle,
            },
            punctuation: match self.punctuation {
                Punctuation::Inside => PunctuationPlacement::Inside,
                Punctuation::Logical => PunctuationPlacement::Logical,
            },
            dialog_paragraphs: self.paragraph_on_speaker,
            topic_paragraphs: self.paragraph_on_topic,
        }
    }

    pub fn commands(&self) -> CommandConfig {
        CommandConfig { break_commands: self.paragraph_on_command, ..CommandConfig::default() }
    }

    pub fn pause_paragraph(&self) -> Option<Duration> {
        self.paragraph_on_pause.then(|| Duration::from_secs_f32(self.pause_paragraph_secs.max(0.5)))
    }

    pub fn pipeline(&self) -> PipelineSettings {
        PipelineSettings {
            cleanup: self.cleanup.clone(),
            models_dir: self.models_dir.clone(),
            style: self.style(),
            commands: self.commands(),
            pause_paragraph: self.pause_paragraph(),
            hotword_boost: self.hotword_boost,
            special_words: self.special_words.clone(),
        }
    }
}

/// The app's files.
#[derive(Debug, Clone)]
pub struct Paths {
    pub settings: PathBuf,
    /// Where the special words used to be kept, as text: imported once.
    pub old_words: PathBuf,
    /// The Wayland remote-input approval, so the desktop asks only once.
    pub portal_token: PathBuf,
    /// This run's log (replaced each start).
    pub log: PathBuf,
    /// Present while beardictate starts at login.
    pub autostart: PathBuf,
}

impl Paths {
    pub fn locate() -> Result<Paths> {
        let dirs = directories::ProjectDirs::from("", "", "beardictate").context("no home directory")?;
        let base = directories::BaseDirs::new().context("no home directory")?;
        let config = dirs.config_dir();
        fs::create_dir_all(config).with_context(|| format!("creating {}", config.display()))?;
        Ok(Paths {
            settings: config.join("settings.toml"),
            old_words: config.join("words.txt"),
            portal_token: dirs.data_local_dir().join("portal-token"),
            log: dirs.state_dir().unwrap_or(dirs.data_local_dir()).join("beardictate.log"),
            autostart: base.config_dir().join("autostart").join("beardictate.desktop"),
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

    /// Moves a word list from the old text file into `settings` (once: the
    /// file is renamed afterwards). Returns whether anything changed.
    pub fn import_old_words(&self, settings: &mut Settings) -> Result<bool> {
        let Ok(text) = fs::read_to_string(&self.old_words) else { return Ok(false) };
        let imported = bd_vocab::Vocabulary::parse(&text).with_context(|| format!("in {}", self.old_words.display()))?;
        let mut changed = false;
        for w in imported.words() {
            if !settings.special_words.iter().any(|s| s.word == w.text) {
                settings.special_words.push(SpecialWord { word: w.text.clone(), heard: w.heard.clone() });
                changed = true;
            }
        }
        fs::rename(&self.old_words, self.old_words.with_extension("txt.imported"))
            .with_context(|| format!("renaming {}", self.old_words.display()))?;
        Ok(changed)
    }

    pub fn load_portal_token(&self) -> Option<String> {
        fs::read_to_string(&self.portal_token).ok().map(|t| t.trim().to_string()).filter(|t| !t.is_empty())
    }

    pub fn starts_at_login(&self) -> bool {
        self.autostart.exists()
    }

    /// Adds or removes the autostart entry, copied from the installed
    /// desktop entry (or written fresh, pointing at this executable).
    pub fn set_start_at_login(&self, on: bool) -> Result<()> {
        if !on {
            return match fs::remove_file(&self.autostart) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.into()),
                _ => Ok(()),
            };
        }
        let exe = std::env::current_exe().context("finding this program")?;
        let entry = format!(
            "[Desktop Entry]\nType=Application\nName=beardictate\nComment=Dictation for fiction writers\n\
             Exec={}\nIcon=beardictate\nTerminal=false\nX-GNOME-Autostart-enabled=true\n",
            exe.display()
        );
        write_atomic(&self.autostart, &entry)
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
# beardictate settings. The settings window rewrites this file; the app also
# reloads it when you save it here.
#
# hotkey: at least two keys, e.g. \"Ctrl+Backquote\" (Ctrl+`), \"Ctrl+Alt+Space\".
# cleanup: \"auto\" (largest installed model), \"off\", a model id from
#   `bd models list`, or the path to a .gguf file.
# quote_marks: \"double\" or \"single\"; curly_quotes: true or false.
# punctuation: \"inside\" or \"logical\".  tray_theme: \"auto\", \"light\", \"dark\".
# models_dir: where the models are; empty for the default location.
# special_words: [[special_words]] entries, word = \"...\", heard = [\"...\"].
";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_round_trip_and_fill_in_missing_keys() {
        let s = Settings { paragraph_on_pause: true, quote_marks: QuoteMarks::Single, ..Settings::default() };
        let text = toml::to_string_pretty(&s).unwrap();
        assert_eq!(toml::from_str::<Settings>(&text).unwrap(), s);
        // Unknown keys (from an older version) are ignored.
        let partial: Settings = toml::from_str("paragraphs = \"off\"\nparagraph_on_speaker = false\n").unwrap();
        assert_eq!(partial.hotkey, "Ctrl+Backquote");
        assert!(!partial.style().dialog_paragraphs);
        assert!(partial.commands().break_commands);
        assert_eq!(partial.pause_paragraph(), None);
    }

    #[test]
    fn style_follows_the_choices() {
        let s = Settings { quote_marks: QuoteMarks::Single, punctuation: Punctuation::Logical, paragraph_on_topic: true, ..Settings::default() };
        let style = s.style();
        assert_eq!(style.quotes, QuoteStyle::CurlySingle);
        assert_eq!(style.punctuation, PunctuationPlacement::Logical);
        assert!(style.topic_paragraphs && style.dialog_paragraphs);
        assert_ne!(s.pipeline(), Settings::default().pipeline());
    }

    #[test]
    fn special_words_start_with_a_working_example() {
        let s = Settings::default();
        let v = vocabulary(&s.special_words);
        assert_eq!(v.apply("I use bare dictate daily."), "I use beardictate daily.");
        assert_eq!(v.hotwords(), ["beardictate"]);
        let text = toml::to_string_pretty(&s).unwrap();
        assert_eq!(toml::from_str::<Settings>(&text).unwrap().special_words, s.special_words);
        // An emptied list stays empty.
        let none: Settings = toml::from_str("special_words = []\n").unwrap();
        assert!(none.special_words.is_empty());
    }
}
