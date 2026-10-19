//! Loading the pipeline's models from the model store: shared by the `bd`
//! CLI and the app, so both pick and configure models the same way.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use bd_asr::{NemotronAsr, NemotronConfig, Segmenter, SegmenterConfig};
use bd_cleanup::prompt::DEFAULT_TEMPLATE;
use bd_cleanup::{
    CleanupError, CleanupOutcome, GpuLayers, HttpChat, HttpCleanup, HttpConfig, LlamaChat, LlamaCleanup, LlamaConfig,
};
use bd_core::{BoxError, Cleanup, CleanupRequest};
use bd_models::{Manifest, ModelEntry, ModelKind, ModelStatus, Store};

/// The speech recognizer.
pub const STREAMING_ASR_MODEL: &str = "asr-nemotron-speech-streaming-en-0.6b-1120ms-int8";
pub const VAD_MODEL: &str = "vad-silero";

/// Which cleanup model to run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CleanupSource {
    /// No cleanup: ASR text with spoken commands applied.
    Off,
    /// The recommended catalog model if installed, else the largest installed, else off.
    Auto,
    /// A catalog model by id.
    Catalog(String),
    /// A GGUF file of the user's.
    File(PathBuf),
    /// An OpenAI-compatible server. The text leaves this program.
    Server { url: String, model: String },
}

impl CleanupSource {
    /// `off`, `auto` (or empty), a path to a `.gguf` file, or a catalog id.
    pub fn parse(s: &str) -> Self {
        match s.trim() {
            "off" => CleanupSource::Off,
            "" | "auto" => CleanupSource::Auto,
            p if p.ends_with(".gguf") || Path::new(p).is_file() => CleanupSource::File(PathBuf::from(p)),
            id => CleanupSource::Catalog(id.to_string()),
        }
    }
}

/// How to run a local cleanup model.
#[derive(Debug, Clone)]
pub struct CleanupOptions {
    /// Prompt template id. Default: the catalog model's own, else
    /// [`DEFAULT_TEMPLATE`].
    pub template: Option<String>,
    pub gpu_layers: GpuLayers,
    /// Context window in tokens.
    pub ctx: u32,
}

impl Default for CleanupOptions {
    fn default() -> Self {
        Self { template: None, gpu_layers: GpuLayers::Auto, ctx: 4096 }
    }
}

/// A cleanup backend behind the guardrail, keeping the concrete type so
/// callers can see guardrail verdicts and repairs.
pub enum CleanupEngine {
    Llama(LlamaCleanup),
    Http(HttpCleanup),
}

impl CleanupEngine {
    pub fn clean_detailed(&mut self, request: &CleanupRequest<'_>) -> Result<CleanupOutcome, CleanupError> {
        match self {
            CleanupEngine::Llama(c) => c.clean_detailed(request),
            CleanupEngine::Http(c) => c.clean_detailed(request),
        }
    }
}

impl Cleanup for CleanupEngine {
    fn clean(&mut self, request: &CleanupRequest<'_>) -> Result<String, BoxError> {
        match self {
            CleanupEngine::Llama(c) => c.clean(request),
            CleanupEngine::Http(c) => c.clean(request),
        }
    }

    fn last_report(&self) -> Option<bd_core::CleanupReport> {
        match self {
            CleanupEngine::Llama(c) => c.last_report(),
            CleanupEngine::Http(c) => c.last_report(),
        }
    }
}

/// A loaded cleanup engine, its name, and a line describing how it runs.
pub struct LoadedCleanup {
    pub engine: CleanupEngine,
    pub name: String,
    pub description: String,
    pub template: String,
    /// How a local model runs; `None` for a server.
    pub info: Option<bd_cleanup::ModelInfo>,
}

/// Loads the cleanup model `source` names. `Ok(None)` means cleanup is off
/// (asked for, or `Auto` with nothing installed).
pub fn load_cleanup(source: &CleanupSource, options: &CleanupOptions, store: &Store) -> Result<Option<LoadedCleanup>> {
    if let CleanupSource::Server { url, model } = source {
        let template = options.template.as_deref().unwrap_or(DEFAULT_TEMPLATE);
        let mut config = HttpConfig::new(url.clone(), model.clone());
        config.disable_thinking = true;
        let engine = HttpCleanup::new(HttpChat::new(config), template)?;
        let description = format!("{model} at {url} ({template})");
        return Ok(Some(LoadedCleanup { engine: CleanupEngine::Http(engine), name: model.clone(), description, template: template.to_string(), info: None }));
    }
    let Some((path, catalog)) = resolve_local(source, store)? else {
        return Ok(None);
    };
    let catalog_template = catalog.and_then(|m| m.cleanup.as_ref()).map(|c| c.prompt_template.as_str());
    let template = options.template.as_deref().or(catalog_template).unwrap_or(DEFAULT_TEMPLATE);
    let mut config = LlamaConfig::new(&path);
    config.gpu_layers = options.gpu_layers;
    config.n_ctx = options.ctx;
    if let Some(params) = catalog.and_then(|m| m.cleanup.as_ref()) {
        config.sampling.temperature = params.sampling.temperature;
    }
    let chat = LlamaChat::load(config).with_context(|| format!("loading {}", path.display()))?;
    let info = chat.info().clone();
    let name = catalog.map_or_else(|| path.display().to_string(), |m| m.name.clone());
    let description = format!(
        "{name} ({template}; {} of {} layers on GPU, {} threads)",
        info.gpu_layers, info.n_layer, info.threads
    );
    let mut engine = LlamaCleanup::new(chat, template)?;
    warm_up(&mut engine);
    Ok(Some(LoadedCleanup { engine: CleanupEngine::Llama(engine), name, description, template: template.to_string(), info: Some(info) }))
}

/// Cleans one short line while the model loads, so the first sentence
/// dictated doesn't pay for setting it up: on a CPU that's many seconds
/// (21 s once, on a 12-thread laptop), long enough to look broken.
fn warm_up(engine: &mut dyn Cleanup) {
    let started = std::time::Instant::now();
    let style = bd_core::Style::default();
    let request = CleanupRequest { context: "", raw: "Hello there.", style: &style, names: &[] };
    match engine.clean(&request) {
        Ok(_) => tracing::info!(ms = started.elapsed().as_millis() as u64, "cleanup model warmed up"),
        Err(e) => tracing::warn!("warming up the cleanup model: {e}"),
    }
}

/// Installed, released catalog cleanup models, smallest first.
pub fn installed_cleanup_models(store: &Store) -> Vec<&'static ModelEntry> {
    Manifest::builtin().of_kind(ModelKind::Cleanup).filter(|m| m.available && installed(store, m)).collect()
}

/// The GGUF to load, with its catalog entry if it has one.
fn resolve_local(source: &CleanupSource, store: &Store) -> Result<Option<(PathBuf, Option<&'static ModelEntry>)>> {
    let manifest = Manifest::builtin();
    let entry = match source {
        CleanupSource::Off | CleanupSource::Server { .. } => return Ok(None),
        CleanupSource::File(path) => {
            if !path.is_file() {
                bail!("no such model file: {}", path.display());
            }
            return Ok(Some((path.clone(), None)));
        }
        CleanupSource::Catalog(id) => {
            let entry = manifest
                .get(id)
                .filter(|m| m.kind == ModelKind::Cleanup)
                .ok_or_else(|| anyhow!("{id:?} is not a cleanup model in the catalog"))?;
            if !installed(store, entry) {
                bail!("{id} is not installed; run `bd models download {id}`");
            }
            entry
        }
        CleanupSource::Auto => match auto_cleanup_model(store) {
            Some(entry) => entry,
            None => return Ok(None),
        },
    };
    let file = entry
        .files
        .iter()
        .find(|f| f.path.ends_with(".gguf"))
        .ok_or_else(|| anyhow!("catalog entry {} has no .gguf file", entry.id))?;
    let path = store.file_path(entry, &file.path).ok_or_else(|| anyhow!("{} is missing from the store", file.path))?;
    Ok(Some((path, Some(entry))))
}

/// What "auto" loads: the recommended cleanup model if it's downloaded,
/// otherwise the largest one that is (the catalog lists them smallest first).
pub fn auto_cleanup_model(store: &Store) -> Option<&'static ModelEntry> {
    let mut installed = installed_cleanup_models(store);
    match installed.iter().position(|m| m.recommended) {
        Some(i) => Some(installed.swap_remove(i)),
        None => installed.pop(),
    }
}

pub fn installed(store: &Store, model: &ModelEntry) -> bool {
    matches!(store.status(model), ModelStatus::Installed { .. })
}

fn required(store: &Store, id: &str) -> Result<&'static ModelEntry> {
    let entry = Manifest::builtin().get(id).ok_or_else(|| anyhow!("{id} is missing from the built-in catalog"))?;
    if !installed(store, entry) {
        bail!("{id} is not installed in {}; run `bd models download {id}`", store.root().display());
    }
    Ok(entry)
}

/// The hotword boost to use when the user hasn't picked one: a score per
/// matching token. On 22 invented names it doubled Parakeet's hits without
/// changing ordinary words, and took Nemotron from 8 to 13.
pub const DEFAULT_HOTWORD_BOOST: f32 = 2.0;

/// The streaming recognizer, listening for `hotwords` where the build can.
pub fn load_streaming_asr(store: &Store, hotwords: &[String], boost: f32) -> Result<NemotronAsr> {
    use bd_core::StreamingAsr;
    required(store, STREAMING_ASR_MODEL)?;
    let dir = store.model_dir(STREAMING_ASR_MODEL);
    let config = NemotronConfig { hotword_boost: Some(DEFAULT_HOTWORD_BOOST), ..NemotronConfig::default() };
    let mut asr = NemotronAsr::new(&dir, &config).with_context(|| format!("loading ASR from {}", dir.display()))?;
    asr.set_hotwords(hotwords, boost).map_err(|e| anyhow!("{e}"))?;
    Ok(asr)
}

pub fn load_segmenter(store: &Store) -> Result<Segmenter> {
    let vad = required(store, VAD_MODEL)?;
    let path = store
        .file_path(vad, "silero_vad.onnx")
        .ok_or_else(|| anyhow!("{VAD_MODEL} has no silero_vad.onnx"))?;
    Segmenter::new(&path, SegmenterConfig::default()).with_context(|| format!("loading VAD from {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cleanup_sources_parse() {
        assert_eq!(CleanupSource::parse("off"), CleanupSource::Off);
        assert_eq!(CleanupSource::parse(""), CleanupSource::Auto);
        assert_eq!(CleanupSource::parse("auto"), CleanupSource::Auto);
        assert_eq!(CleanupSource::parse("/m/x.gguf"), CleanupSource::File("/m/x.gguf".into()));
        assert_eq!(CleanupSource::parse("cleanup-qwen3-4b-q4_k_m"), CleanupSource::Catalog("cleanup-qwen3-4b-q4_k_m".into()));
    }
}
