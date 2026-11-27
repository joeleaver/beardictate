//! The app's files: where they are, and reading and writing them.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use super::Settings;

/// The app's files.
#[derive(Debug, Clone)]
pub struct Paths {
    pub settings: PathBuf,
    /// The Wayland remote-input approval, so the desktop asks only once.
    pub portal_token: PathBuf,
    /// The microphone gain learned for each microphone.
    pub gains: PathBuf,
    /// Your recent pauses mid-sentence, to suggest a sentence wait from
    /// (`engine/pauses.rs`): their lengths only, no words.
    pub pauses: PathBuf,
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
            pauses: dirs.state_dir().unwrap_or(dirs.data_local_dir()).join("sentence-pauses.json"),
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

pub(crate) fn write_atomic(path: &Path, text: &str) -> Result<()> {
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
# list_join_pauses: true to keep words said after a short pause on the same list item.
# sentence_wait_secs: how long (1 to 3 seconds) a pause waits for you to go on before
#   the sentence ends there; longer if you pause to think mid-sentence.
# word_groups: [[word_groups]] with name and enabled, each with
#   [[word_groups.words]] entries: word = \"...\", heard = [\"...\"], enabled.
";
