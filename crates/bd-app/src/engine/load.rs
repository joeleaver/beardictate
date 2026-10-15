//! Loading the models into a pipeline, and the pipeline settings that change
//! without loading.

use anyhow::Result;
use bd_models::Store;
use bd_pipeline::setup::{self, CleanupOptions, CleanupSource};
use bd_pipeline::{Parts, Pipeline, PipelineConfig};

use super::Loaded;
use crate::settings::{Settings, vocabulary};

/// Loads every model and starts a pipeline. Runs on its own thread.
pub(super) fn load(settings: &Settings) -> Result<Loaded> {
    let store = match settings.models_dir.trim() {
        "" => Store::from_env()?,
        dir => Store::at(dir),
    };
    let config = pipeline_config(settings);
    let segmenter = setup::load_segmenter(&store)?;
    let asr = setup::load_asr(&store, &config.vocabulary.hotwords(), settings.hotword_boost)?;
    let source = CleanupSource::parse(&settings.cleanup);
    let cleanup = setup::load_cleanup(&source, &CleanupOptions::default(), &store)?;
    let name = cleanup.as_ref().map_or_else(|| "off".to_string(), |c| c.name.clone());
    if let Some(c) = &cleanup {
        tracing::info!("{}", c.description);
    }
    let parts = Parts {
        segmenter: Box::new(segmenter),
        asr: Box::new(asr),
        cleanup: cleanup.map(|c| Box::new(c.engine) as _),
    };
    let pipeline = Pipeline::spawn(parts, config).map_err(|e| anyhow::anyhow!(e))?;
    Ok(Loaded { pipeline, cleanup: name })
}

pub(super) fn pipeline_config(settings: &Settings) -> PipelineConfig {
    let live = settings.live();
    PipelineConfig {
        style: live.style,
        commands: live.commands,
        vocabulary: vocabulary(&live.special_words),
        hotword_boost: live.hotword_boost,
        blank_line_paragraphs: live.blank_line_paragraphs,
        voice_wake: live.voice_wake,
        pause_paragraph: live.pause_paragraph,
        ..PipelineConfig::default()
    }
}
