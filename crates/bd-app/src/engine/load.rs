//! Loading the models into a pipeline, and the pipeline settings that change
//! without loading.

use anyhow::Result;
use bd_cleanup::GpuLayers;
use bd_models::Store;
use bd_pipeline::setup::{self, CleanupOptions, CleanupSource};
use bd_pipeline::{Parts, Pipeline, PipelineConfig, Recognizer};

use std::thread;

use crossbeam_channel::bounded;

use super::{Loaded, Ui, Worker};
use crate::settings::{Settings, TidyDevice, vocabulary};

/// Why loading stopped on first run. The settings window is open with its
/// download card then, so this needs no notification.
pub(super) const NOT_DOWNLOADED: &str = "the speech models aren't downloaded yet (open Settings)";

/// Loads every model and starts a pipeline. Runs on its own thread.
pub(super) fn load(settings: &Settings, gain: f32) -> Result<Loaded> {
    let store = match settings.models_dir.trim() {
        "" => Store::from_env()?,
        dir => Store::at(dir),
    };
    if crate::models::speech().iter().any(|m| !bd_pipeline::setup::installed(&store, m)) {
        anyhow::bail!(NOT_DOWNLOADED);
    }
    let config = pipeline_config(settings, gain);
    let segmenter = setup::load_segmenter(&store)?;
    let hotwords = config.vocabulary.hotwords();
    let asr = Recognizer::Live(Box::new(setup::load_streaming_asr(&store, &hotwords, settings.hotword_boost)?));
    tracing::info!(destination = ?settings.destination, tidy = settings.tidy, "speech: Nemotron, streaming");
    let source = if settings.tidy { CleanupSource::parse(&settings.cleanup) } else { CleanupSource::Off };
    let cleanup = setup::load_cleanup(&source, &cleanup_options(settings), &store)?;
    let name = cleanup.as_ref().map_or_else(|| "off".to_string(), |c| c.name.clone());
    if let Some(c) = &cleanup {
        tracing::info!("{}", c.description);
    }
    let parts = Parts {
        segmenter: Box::new(segmenter),
        asr,
        cleanup: cleanup.map(|c| Box::new(c.engine) as _),
        focus: Some(focus_probe()),
    };
    let pipeline = Pipeline::spawn(parts, config).map_err(|e| anyhow::anyhow!(e))?;
    Ok(Loaded { pipeline, cleanup: name })
}

/// How the tidying model runs: on the graphics card if there is one, unless
/// the settings say the processor.
fn cleanup_options(settings: &Settings) -> CleanupOptions {
    let gpu_layers = match settings.tidy_device {
        TidyDevice::Auto => GpuLayers::Auto,
        TidyDevice::Cpu => GpuLayers::None,
    };
    CleanupOptions { gpu_layers, ..CleanupOptions::default() }
}

/// Which app has the focus, for the pipeline to start a new passage when
/// that changes: the executable's name, lowercased.
fn focus_probe() -> bd_pipeline::FocusProbe {
    let mut probe = bd_platform::FocusProbe::new();
    Box::new(move || probe.current().map(|app| app.exe_name.to_lowercase()))
}

/// The pipeline settings, with `gain` the microphone gain to start from.
pub(super) fn pipeline_config(settings: &Settings, gain: f32) -> PipelineConfig {
    let live = settings.live();
    PipelineConfig {
        style: live.style,
        commands: live.commands,
        vocabulary: vocabulary(&live.special_words),
        hotword_boost: live.hotword_boost,
        blank_line_paragraphs: live.blank_line_paragraphs,
        voice_wake: live.voice_wake,
        pause_paragraph: live.pause_paragraph,
        input_gain: gain,
        auto_gain: live.auto_gain,
        pad: live.pad,
        list_mode: live.list_mode,
        ..PipelineConfig::default()
    }
}

/// The cleanup model `settings` will load, by name; `None` for none.
pub(super) fn cleanup_label(settings: &Settings) -> Option<String> {
    let manifest = bd_models::Manifest::builtin();
    if !settings.tidy {
        return None;
    }
    match settings.cleanup.as_str() {
        "off" => None,
        "auto" => {
            let store = crate::models::store(&settings.models_dir)?;
            setup::auto_cleanup_model(&store).map(|m| m.name.clone())
        }
        id if !id.ends_with(".gguf") => manifest.get(id).map(|m| m.name.clone()),
        path => std::path::Path::new(path).file_stem().map(|s| s.to_string_lossy().into_owned()),
    }
}

impl<U: Ui> Worker<U> {
    pub(super) fn start_load(&mut self) {
        self.reload_when_idle = false;
        self.wake_after_load = self.standby && self.listening_since.is_some();
        self.load_error = None;
        let name = cleanup_label(&self.settings);
        self.loading_detail = match (self.cleanup_name.is_empty(), name) {
            (true, Some(name)) => format!("Loading {name}…"),
            (false, Some(name)) => format!("Switching to {name}…"),
            (_, None) => "Loading the speech models…".into(),
        };
        // Dropping the old pipeline joins its threads, which are idle.
        self.pipeline = None;
        let settings = self.settings.clone();
        let gain = self.starting_gain();
        self.gain = gain;
        let (tx, rx) = bounded(1);
        thread::Builder::new()
            .name("bd-load".into())
            .spawn(move || {
                let _ = tx.send(load(&settings, gain).map_err(|e| format!("{e:#}")));
            })
            .expect("spawning the loader");
        self.loading = Some(rx);
        self.publish();
    }

    pub(super) fn on_loaded(&mut self, loaded: Result<Loaded, String>) {
        self.loading = None;
        if std::mem::take(&mut self.reload_after_load) {
            // Loaded with settings that changed meanwhile: drop it, load anew.
            drop(loaded);
            return self.start_load();
        }
        match loaded {
            Ok(l) => {
                tracing::info!(cleanup = %l.cleanup, "models loaded");
                self.cleanup_name = format!("Post-processing: {}", l.cleanup);
                // Settings changed while loading reach it now.
                l.pipeline.reconfigure(pipeline_config(&self.settings, self.starting_gain()));
                self.pipeline = Some(l.pipeline);
                // The first load after starting: dictation comes on, in either
                // mode. Later loads (a settings change) keep what was.
                let starting = std::mem::take(&mut self.start_on_load);
                if starting && self.settings.voice_wake {
                    self.record_mic(true);
                }
                if self.wants_standby() {
                    let wake = starting || std::mem::take(&mut self.wake_after_load);
                    self.stand_by(wake);
                } else if starting
                    && !self.settings.voice_wake
                    && let Err(e) = self.start()
                {
                    self.notifier.notify("mic", "Microphone problem", &format!("{e:#}"));
                    self.capture = None;
                }
            }
            Err(e) => {
                if e != NOT_DOWNLOADED {
                    self.notifier.notify("load", "beardictate can't start", &e);
                }
                self.load_error = Some(e);
            }
        }
        self.publish();
    }
}
