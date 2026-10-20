//! The user's settings, kept as TOML in the platform config directory.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result};
use bd_commands::{CommandConfig, NumberStyle};
use bd_core::{PunctuationPlacement, QuoteStyle, Spelling, Style};
use serde::{Deserialize, Serialize};

mod words;
pub use words::{FIRST_GROUP, SpecialWord, WordGroup, active_words, starter_groups, vocabulary};

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

/// Where dictated words go. Either way they're heard as they're spoken
/// (Nemotron), and [`Settings::tidy`] says whether they're cleaned up.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Destination {
    /// Typed into the app you're working in.
    #[default]
    App,
    /// Collected in the Bear Pad; "bear, send it" types it.
    Pad,
}

impl Destination {
    pub const ALL: [Destination; 2] = [Destination::App, Destination::Pad];

    pub fn label(self) -> &'static str {
        match self {
            Destination::App => "Into your app",
            Destination::Pad => "Into the Bear Pad",
        }
    }
}

/// The four modes of 0.1.8-0.1.9, read from old settings files and turned
/// into a [`Destination`] and [`Settings::tidy`] on load.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum OldMode {
    Classic,
    Streaming,
    PadStreaming,
    PadAuthor,
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

/// Where the tidying model runs.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TidyDevice {
    /// The graphics card when there is one, else the processor.
    #[default]
    Auto,
    /// The processor only. Faster than a slow graphics card or driver.
    Cpu,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// The dictation hotkey: press to start, press again to stop. At least
    /// two keys (DESIGN.md "Hotkey"). On Wayland the desktop asks to approve
    /// it and may change it.
    pub hotkey: String,
    /// `auto` (the recommended model if downloaded, else the largest), `off`,
    /// a catalog id, or a .gguf path.
    pub cleanup: String,
    /// New paragraph when you say "new paragraph" (and a line break for
    /// "new line"). Off, those words are typed.
    pub paragraph_on_command: bool,
    /// Spoken marks: "comma", "period", "question mark", "exclamation
    /// point"… become the mark. Off, those words are typed.
    pub spoken_punctuation: bool,
    /// How numbers said as words are written: as said, or as a style guide
    /// has them. "numeral …" gives digits either way.
    pub numbers: NumberStyle,
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
    /// A blank line between paragraphs (two returns); off, one return.
    pub blank_line_paragraphs: bool,
    /// Speech running on into a new paragraph: close the quote at each
    /// paragraph's end too. Off, the usual book style: only the next opens.
    pub close_continued_speech: bool,
    pub punctuation: Punctuation,
    /// American or British spelling, or as the recognizer wrote it.
    pub spelling: Spelling,
    /// How strongly the speech recognizer listens for the special words.
    pub hotword_boost: f32,
    /// Microphone name; empty for the system default.
    pub microphone: String,
    /// Always listening: the microphone stays on; "bear, wake up", "bear, go
    /// to sleep" and the hotkey switch typing on and off. Off, the hotkey
    /// opens and closes the microphone.
    pub voice_wake: bool,
    /// With `voice_wake`: the microphone is on. Switched from the tray menu.
    pub microphone_on: bool,
    /// Go back to sleep (or stop a hotkey dictation) after this many minutes
    /// with nothing said to it; 0 never does.
    pub sleep_after_mins: u32,
    /// Learn the microphone gain from your speech, like a call app does.
    pub auto_gain: bool,
    /// The gain when `auto_gain` is off, in dB (0 to 30).
    pub mic_gain_db: f32,
    /// Where the words go.
    pub destination: Destination,
    /// Clean up what's dictated with the cleanup model: fillers out,
    /// sentences mended, dialog quoted. In your app the words are typed as
    /// you say them either way, and each sentence is tidied in place when
    /// you pause; in the pad, the paragraph you're on.
    pub tidy: bool,
    /// With `tidy`: fiction's dialog. Speech in the prose is put in quote
    /// marks and a different character's line starts a paragraph. Off,
    /// only dictated quote marks stay.
    pub dialog: bool,
    /// Where the model that tidies runs. A change loads the model again.
    pub tidy_device: TidyDevice,
    /// The bear on screen: a small floating window showing the state, with
    /// the tray's menu on a right-click.
    pub show_bear: bool,
    /// List mode ([`bd_pipeline::PipelineConfig::list_mode`]): for this run
    /// only, never saved.
    #[serde(skip)]
    pub list_mode: bool,
    /// Earlier versions' modes, moved into `destination` and `tidy` on load.
    #[serde(skip_serializing)]
    mode: Option<OldMode>,
    /// Earlier versions' Bear Pad switch, likewise.
    #[serde(skip_serializing)]
    bear_pad: bool,
    /// Keep the microphone open between dictations. The first syllable is
    /// never clipped, but the desktop shows the mic as in use all the time.
    pub keep_mic_open: bool,
    pub tray_theme: TrayTheme,
    /// Where the models are; empty for the default (or `BD_MODELS_DIR`).
    pub models_dir: String,
    /// OS notifications when something needs attention.
    pub notifications: bool,
    /// Look for new versions every few hours, download them, and install
    /// them at the next start. Off, only "Check for updates" looks.
    pub auto_update: bool,
    /// Names and invented words: the recognizer listens for them, and what
    /// it hears instead is replaced.
    pub word_groups: Vec<WordGroup>,
    /// The ungrouped list of earlier versions, moved into a group on load.
    #[serde(skip_serializing)]
    special_words: Vec<SpecialWord>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            hotkey: "Ctrl+Backquote".into(),
            cleanup: "auto".into(),
            paragraph_on_command: true,
            spoken_punctuation: true,
            numbers: NumberStyle::AsSaid,
            paragraph_on_speaker: true,
            paragraph_on_pause: false,
            paragraph_on_topic: false,
            pause_paragraph_secs: 3.0,
            quote_marks: QuoteMarks::Double,
            curly_quotes: true,
            voice_wake: true,
            microphone_on: true,
            sleep_after_mins: 10,
            blank_line_paragraphs: true,
            close_continued_speech: false,
            punctuation: Punctuation::Inside,
            spelling: Spelling::American,
            hotword_boost: 2.0,
            microphone: String::new(),
            keep_mic_open: false,
            destination: Destination::App,
            tidy: true,
            dialog: false,
            tidy_device: TidyDevice::Auto,
            show_bear: true,
            list_mode: false,
            mode: None,
            bear_pad: false,
            auto_gain: true,
            mic_gain_db: 0.0,
            tray_theme: TrayTheme::Auto,
            models_dir: String::new(),
            notifications: true,
            auto_update: true,
            word_groups: starter_groups(),
            special_words: Vec::new(),
        }
    }
}

/// The settings the models were loaded with: a change to any of them means
/// loading again. The rest reach a running pipeline at once (see
/// [`Settings::live`]).
#[derive(Debug, Clone, PartialEq)]
pub struct ModelSettings {
    cleanup: String,
    models_dir: String,
    tidy: bool,
    tidy_device: TidyDevice,
}

/// The settings a running pipeline takes without reloading.
#[derive(Debug, Clone, PartialEq)]
pub struct LiveSettings {
    pub style: Style,
    pub commands: CommandConfig,
    pub pause_paragraph: Option<Duration>,
    pub special_words: Vec<SpecialWord>,
    pub hotword_boost: f32,
    pub blank_line_paragraphs: bool,
    pub voice_wake: bool,
    pub auto_gain: bool,
    pub mic_gain_db: f32,
    pub pad: bool,
    pub list_mode: bool,
}

/// Whether `cleanup` names a catalog model id the catalog no longer has.
fn dropped_from_catalog(cleanup: &str) -> bool {
    cleanup.starts_with("cleanup-")
        && !cleanup.ends_with(".gguf")
        && !bd_models::Manifest::builtin().models.iter().any(|m| m.id == cleanup && m.available)
}

impl Settings {
    /// Brings settings from an earlier version up to date. Returns whether
    /// anything changed (and so wants saving).
    pub fn upgrade(&mut self) -> bool {
        let mut changed = false;
        if !self.special_words.is_empty() {
            // The old list replaces the starter group it never had.
            let words = std::mem::take(&mut self.special_words);
            self.word_groups = vec![WordGroup { words, ..WordGroup::new(FIRST_GROUP) }];
            changed = true;
        }
        if std::mem::take(&mut self.bear_pad) {
            self.mode = Some(OldMode::PadAuthor);
        }
        if let Some(mode) = self.mode.take() {
            (self.destination, self.tidy) = match mode {
                OldMode::Classic => (Destination::App, true),
                OldMode::Streaming => (Destination::App, false),
                OldMode::PadStreaming => (Destination::Pad, false),
                OldMode::PadAuthor => (Destination::Pad, true),
            };
            changed = true;
        }
        if self.cleanup == "off" {
            // The Post-processing page's own off switch is gone: Tidy up
            // says whether the model runs.
            (self.tidy, self.cleanup) = (false, "auto".into());
            changed = true;
        }
        if dropped_from_catalog(&self.cleanup) {
            // A catalog model that's since been removed: the best one there is.
            self.cleanup = "auto".into();
            changed = true;
        }
        changed
    }

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
            dialog: self.dialog,
            dialog_paragraphs: self.dialog && self.paragraph_on_speaker,
            topic_paragraphs: self.paragraph_on_topic,
            close_continued_speech: self.close_continued_speech,
            spelling: self.spelling,
        }
    }

    pub fn commands(&self) -> CommandConfig {
        CommandConfig { break_commands: self.paragraph_on_command, literal_punctuation: self.spoken_punctuation, numbers: self.numbers }
    }

    pub fn pause_paragraph(&self) -> Option<Duration> {
        self.paragraph_on_pause.then(|| Duration::from_secs_f32(self.pause_paragraph_secs.max(0.5)))
    }

    pub fn models(&self) -> ModelSettings {
        ModelSettings {
            cleanup: self.cleanup.clone(),
            models_dir: self.models_dir.clone(),
            tidy: self.tidy,
            tidy_device: self.tidy_device,
        }
    }

    pub fn live(&self) -> LiveSettings {
        LiveSettings {
            style: self.style(),
            commands: self.commands(),
            pause_paragraph: self.pause_paragraph(),
            special_words: active_words(&self.word_groups),
            hotword_boost: self.hotword_boost,
            blank_line_paragraphs: self.blank_line_paragraphs,
            voice_wake: self.voice_wake,
            auto_gain: self.auto_gain,
            mic_gain_db: self.mic_gain_db,
            pad: self.pad(),
            list_mode: self.list_mode,
        }
    }

    /// Words go to the Bear Pad.
    pub fn pad(&self) -> bool {
        self.destination == Destination::Pad
    }
}

/// The app's files.
#[derive(Debug, Clone)]
pub struct Paths {
    pub settings: PathBuf,
    /// The Wayland remote-input approval, so the desktop asks only once.
    pub portal_token: PathBuf,
    /// The microphone gain learned for each microphone.
    pub gains: PathBuf,
    /// Where the Bear Pad window was, and its size.
    pub pad_window: PathBuf,
    /// Where the bear on screen was left.
    pub bear_window: PathBuf,
    /// The Settings window's section when it was last left.
    pub settings_section: PathBuf,
    /// This run's log (replaced each start).
    pub log: PathBuf,
    /// Downloaded updates waiting to install (see [`crate::update`]).
    pub updates: PathBuf,
    /// Present while beardictate starts at login (not on Windows, which
    /// uses the registry: see [`crate::autostart`]).
    #[cfg_attr(windows, allow(dead_code))]
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
            portal_token: dirs.data_local_dir().join("portal-token"),
            log: dirs.state_dir().unwrap_or(dirs.data_local_dir()).join("beardictate.log"),
            gains: dirs.state_dir().unwrap_or(dirs.data_local_dir()).join("input-gain.toml"),
            pad_window: dirs.state_dir().unwrap_or(dirs.data_local_dir()).join("pad-window"),
            bear_window: dirs.state_dir().unwrap_or(dirs.data_local_dir()).join("bear-window"),
            settings_section: dirs.state_dir().unwrap_or(dirs.data_local_dir()).join("settings-section"),
            updates: dirs.data_local_dir().join("updates"),
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
        let mut settings: Settings = toml::from_str(&text).with_context(|| format!("in {}", self.settings.display()))?;
        if settings.upgrade() {
            self.save(&settings)?;
        }
        Ok(settings)
    }

    pub fn save(&self, settings: &Settings) -> Result<()> {
        let body = toml::to_string_pretty(settings).context("serializing settings")?;
        let text = format!("{SETTINGS_HEADER}\n{body}");
        write_atomic(&self.settings, &text)
    }

    /// Learned gains by microphone name ("" for the system default).
    pub fn load_gains(&self) -> std::collections::BTreeMap<String, f32> {
        fs::read_to_string(&self.gains).ok().and_then(|t| toml::from_str(&t).ok()).unwrap_or_default()
    }

    pub fn save_gains(&self, gains: &std::collections::BTreeMap<String, f32>) {
        let text = toml::to_string(gains).unwrap_or_default();
        if let Err(e) = write_atomic(&self.gains, &text) {
            tracing::warn!("saving the microphone gain: {e:#}");
        }
    }

    pub fn load_portal_token(&self) -> Option<String> {
        fs::read_to_string(&self.portal_token).ok().map(|t| t.trim().to_string()).filter(|t| !t.is_empty())
    }

    pub fn starts_at_login(&self) -> bool {
        #[cfg(windows)]
        {
            crate::autostart::enabled()
        }
        #[cfg(not(windows))]
        {
            self.autostart.exists()
        }
    }

    /// Adds or removes the autostart entry, copied from the installed
    /// desktop entry (or written fresh, pointing at this executable).
    pub fn set_start_at_login(&self, on: bool) -> Result<()> {
        #[cfg(windows)]
        {
            crate::autostart::set(on).context("changing the Run registry value")
        }
        #[cfg(not(windows))]
        {
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
# cleanup: the post-processing model: \"auto\" (the recommended one, else the largest you have), a model id from
#   `bd models list`, or the path to a .gguf file.
# quote_marks: \"double\" or \"single\"; curly_quotes: true or false.
# blank_line_paragraphs: true for a blank line between paragraphs, false for one return.
# close_continued_speech: true to close a quote at the end of each paragraph when
#   the same speaker carries on into the next; false (book style) leaves it open.
# numbers: \"as-said\" (words, unless you say \"numeral\" first) or \"style-guide\" (words up to
#   one hundred, digits above; $300, 12 percent, 4:30 p.m., May 21).
# spoken_punctuation: true to make \"comma\", \"period\", \"question mark\" and so on into marks.
# punctuation: \"inside\" or \"logical\".  tray_theme: \"auto\", \"light\", \"dark\".
# models_dir: where the models are; empty for the default location.
# auto_gain: learn the microphone gain from your speech; mic_gain_db: the gain
#   in dB (0 to 30) when auto_gain is false.
# destination: \"app\" (typed into the app you're in) or \"pad\" (collected in the Bear Pad;
#   \"bear, send it\" types it).
# show_bear: true to show the bear on screen (the state, and the tray's menu on a right-click).
# dialog: with tidy, true to put speech in quote marks (fiction); false keeps only dictated quotes.
# tidy: true to post-process with the model: words are typed as you say them, and each
#   sentence (in the pad, the paragraph) is tidied in place when you pause; false leaves them be.
# tidy_device: where the model that tidies runs: \"auto\" (the graphics card if there is one) or \"cpu\"
#   (the processor only; try it if tidying is slow).
# word_groups: [[word_groups]] with name and enabled, each with
#   [[word_groups.words]] entries: word = \"...\", heard = [\"...\"], enabled.
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
    fn tidy_device_defaults_to_auto_and_changing_it_reloads() {
        let s: Settings = toml::from_str("").unwrap();
        assert_eq!(s.tidy_device, TidyDevice::Auto);
        let cpu: Settings = toml::from_str("tidy_device = \"cpu\"\n").unwrap();
        assert_eq!(cpu.tidy_device, TidyDevice::Cpu);
        assert_ne!(s.models(), cpu.models());
    }

    #[test]
    fn post_processing_off_becomes_tidy_up_off() {
        let mut s: Settings = toml::from_str("cleanup = \"off\"\n").unwrap();
        assert!(s.upgrade());
        assert_eq!((s.tidy, s.cleanup.as_str()), (false, "auto"));
        assert!(s.show_bear && !s.dialog, "the bear on, dialog off, by default");
    }

    #[test]
    fn style_follows_the_choices() {
        let s = Settings { quote_marks: QuoteMarks::Single, punctuation: Punctuation::Logical, paragraph_on_topic: true, ..Settings::default() };
        let style = s.style();
        assert_eq!(style.quotes, QuoteStyle::CurlySingle);
        assert_eq!(style.punctuation, PunctuationPlacement::Logical);
        assert!(style.topic_paragraphs && !style.dialog && !style.dialog_paragraphs, "dialog is off by default");
        let fiction = Settings { dialog: true, ..s.clone() }.style();
        assert!(fiction.dialog && fiction.dialog_paragraphs);
        assert_ne!(s.live(), Settings::default().live());
        assert_eq!(s.models(), Settings::default().models());
    }

    #[test]
    fn word_groups_round_trip_and_old_lists_move_into_one() {
        let s = Settings::default();
        let text = toml::to_string_pretty(&s).unwrap();
        assert_eq!(toml::from_str::<Settings>(&text).unwrap().word_groups, s.word_groups);
        // An emptied list stays empty.
        let none: Settings = toml::from_str("word_groups = []\n").unwrap();
        assert!(none.word_groups.is_empty());

        let old = "[[special_words]]\nword = \"Beardictate\"\nheard = [\"bare dictate\"]\n";
        let mut s: Settings = toml::from_str(old).unwrap();
        assert!(s.upgrade());
        assert_eq!(s.word_groups.len(), 1);
        assert_eq!(s.word_groups[0].name, FIRST_GROUP);
        assert_eq!(s.word_groups[0].words, [SpecialWord::new("Beardictate", &["bare dictate"])]);
        assert!(!s.upgrade());
        assert!(!toml::to_string_pretty(&s).unwrap().contains("special_words"));

        // Bear Pad on in an earlier version: the pad, tidied.
        let mut pad: Settings = toml::from_str("bear_pad = true\n").unwrap();
        assert!(pad.upgrade());
        assert_eq!((pad.destination, pad.tidy), (Destination::Pad, true));
        assert!(!toml::to_string_pretty(&pad).unwrap().contains("bear_pad"));
        // The four modes of 0.1.8.
        for (mode, destination, tidy) in [
            ("classic", Destination::App, true),
            ("streaming", Destination::App, false),
            ("pad-streaming", Destination::Pad, false),
            ("pad-author", Destination::Pad, true),
        ] {
            let mut old: Settings = toml::from_str(&format!("mode = \"{mode}\"\n")).unwrap();
            assert!(old.upgrade());
            assert_eq!((old.destination, old.tidy), (destination, tidy), "{mode}");
            assert!(!toml::to_string_pretty(&old).unwrap().lines().any(|l| l.starts_with("mode ")));
            assert!(!old.upgrade());
        }

        let mut gone: Settings = toml::from_str("cleanup = \"cleanup-qwen3-4b-q4_k_m\"\n").unwrap();
        assert!(gone.upgrade());
        assert_eq!(gone.cleanup, "auto");
        let mut kept: Settings = toml::from_str("cleanup = \"/models/mine.gguf\"\n").unwrap();
        assert!(!kept.upgrade());
    }
}
