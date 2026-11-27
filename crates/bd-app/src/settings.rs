//! The user's settings, kept as TOML in the platform config directory.

use std::time::Duration;

use bd_commands::{CommandConfig, NumberStyle};
use bd_core::{PunctuationPlacement, QuoteStyle, Spelling, Style};
use serde::{Deserialize, Serialize};

mod paths;
mod words;
pub use paths::Paths;
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
    /// In a list, words said after a short pause go on the same item
    /// rather than a new one ([`bd_pipeline::PipelineConfig::list_join`]).
    pub list_join_pauses: bool,
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
    /// Title case, lower case or all caps
    /// ([`bd_pipeline::PipelineConfig::case_mode`]): for this run only.
    #[serde(skip)]
    pub case_mode: bd_pipeline::CaseMode,
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
            list_join_pauses: true,
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
            case_mode: bd_pipeline::CaseMode::Normal,
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
    pub case_mode: bd_pipeline::CaseMode,
    pub list_join_pauses: bool,
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
            case_mode: self.case_mode,
            list_join_pauses: self.list_join_pauses,
        }
    }

    /// Words go to the Bear Pad.
    pub fn pad(&self) -> bool {
        self.destination == Destination::Pad
    }
}

#[cfg(test)]
mod tests;
