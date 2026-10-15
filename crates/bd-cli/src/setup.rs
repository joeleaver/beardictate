//! Building pipeline stages from command-line options and the model store.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use bd_asr::{ParakeetAsr, ParakeetConfig, Segmenter, SegmenterConfig};
use bd_cleanup::prompt::DEFAULT_TEMPLATE;
use bd_cleanup::{CleanupError, CleanupOutcome, GpuLayers, HttpChat, HttpCleanup, HttpConfig, LlamaChat, LlamaCleanup, LlamaConfig};
use bd_core::{BoxError, Cleanup, CleanupRequest, PunctuationPlacement, QuoteStyle, Style};
use bd_models::{Manifest, ModelEntry, ModelKind, ModelStatus, Store};
use clap::{Args, ValueEnum};

pub const ASR_MODEL: &str = "asr-parakeet-tdt-0.6b-v3-int8";
pub const VAD_MODEL: &str = "vad-silero";

#[derive(Debug, Clone, Copy, Default, ValueEnum)]
pub enum StyleArg {
    /// “Curly double” quotes, commas and periods inside.
    #[default]
    Us,
    /// ‘Curly single’ quotes, logical punctuation.
    Uk,
}

impl StyleArg {
    pub fn style(self) -> Style {
        match self {
            StyleArg::Us => Style::default(),
            StyleArg::Uk => Style { quotes: QuoteStyle::CurlySingle, punctuation: PunctuationPlacement::Logical },
        }
    }
}

/// Which cleanup model to run, if any.
#[derive(Debug, Args)]
pub struct CleanupArgs {
    /// `off`, a catalog id (see `bd models list`), or a path to a .gguf
    /// file. Default: the largest installed catalog cleanup model, else off.
    #[arg(long, value_name = "off|ID|PATH")]
    pub cleanup: Option<String>,
    /// Use an OpenAI-compatible server instead, e.g. http://localhost:8080/v1.
    /// Your text is sent to it.
    #[arg(long, value_name = "URL", conflicts_with = "cleanup")]
    pub cleanup_url: Option<String>,
    /// Model name to send to --cleanup-url.
    #[arg(long, default_value = "local", requires = "cleanup_url")]
    pub cleanup_server_model: String,
    /// Prompt template id. Default: the catalog model's own, else generic-v1.
    #[arg(long)]
    pub template: Option<String>,
    /// Layers to put on the GPU: auto, none, or a count.
    #[arg(long, default_value = "auto", value_parser = parse_gpu_layers)]
    pub gpu_layers: GpuLayers,
    /// Context window for a local model, in tokens.
    #[arg(long, default_value_t = 4096)]
    pub ctx: u32,
}

fn parse_gpu_layers(s: &str) -> Result<GpuLayers, String> {
    match s {
        "auto" => Ok(GpuLayers::Auto),
        "none" | "0" => Ok(GpuLayers::None),
        n => n.parse().map(GpuLayers::Count).map_err(|_| format!("expected auto, none or a number, got {n:?}")),
    }
}

/// A cleanup backend behind the guardrail, keeping the concrete type so the
/// eval can see guardrail verdicts.
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
}

/// A loaded cleanup engine and a line describing it.
pub struct LoadedCleanup {
    pub engine: CleanupEngine,
    pub description: String,
}

/// Loads the cleanup model the options name. `Ok(None)` means cleanup is
/// off.
pub fn load_cleanup(args: &CleanupArgs, store: &Store) -> Result<Option<LoadedCleanup>> {
    if let Some(url) = &args.cleanup_url {
        let template = args.template.as_deref().unwrap_or(DEFAULT_TEMPLATE);
        let mut config = HttpConfig::new(url.clone(), args.cleanup_server_model.clone());
        config.disable_thinking = true;
        let engine = HttpCleanup::new(HttpChat::new(config), template)?;
        let description = format!("{} at {url} ({template})", args.cleanup_server_model);
        return Ok(Some(LoadedCleanup { engine: CleanupEngine::Http(engine), description }));
    }
    let Some((path, catalog)) = resolve_local(args.cleanup.as_deref(), store)? else {
        return Ok(None);
    };
    let catalog_template = catalog.and_then(|m| m.cleanup.as_ref()).map(|c| c.prompt_template.as_str());
    let template = args.template.as_deref().or(catalog_template).unwrap_or(DEFAULT_TEMPLATE);
    let mut config = LlamaConfig::new(&path);
    config.gpu_layers = args.gpu_layers;
    config.n_ctx = args.ctx;
    if let Some(params) = catalog.and_then(|m| m.cleanup.as_ref()) {
        config.sampling.temperature = params.sampling.temperature;
    }
    let chat = LlamaChat::load(config).with_context(|| format!("loading {}", path.display()))?;
    let info = chat.info();
    let name = catalog.map_or_else(|| path.display().to_string(), |m| m.name.clone());
    let description = format!(
        "{name} ({template}; {} of {} layers on GPU, {} threads)",
        info.gpu_layers, info.n_layer, info.threads
    );
    let engine = LlamaCleanup::new(chat, template)?;
    Ok(Some(LoadedCleanup { engine: CleanupEngine::Llama(engine), description }))
}

/// The GGUF to load for `--cleanup`, with its catalog entry if it has one.
fn resolve_local<'m>(choice: Option<&str>, store: &Store) -> Result<Option<(PathBuf, Option<&'m ModelEntry>)>> {
    let manifest = Manifest::builtin();
    let entry = match choice {
        Some("off") => return Ok(None),
        Some(c) if c.ends_with(".gguf") || Path::new(c).is_file() => {
            let path = PathBuf::from(c);
            if !path.is_file() {
                bail!("no such model file: {}", path.display());
            }
            return Ok(Some((path, None)));
        }
        Some(id) => {
            let entry = manifest.get(id).filter(|m| m.kind == ModelKind::Cleanup).ok_or_else(|| {
                anyhow!("{id:?} is not a cleanup model in the catalog; see `bd models list`")
            })?;
            if !installed(store, entry) {
                bail!("{id} is not installed; run `bd models download {id}`");
            }
            entry
        }
        // The catalog lists cleanup models smallest first; take the largest
        // installed one until the app's hardware probe picks for the user.
        None => match manifest.of_kind(ModelKind::Cleanup).filter(|m| m.available && installed(store, m)).last() {
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

/// Loads Parakeet, listening for `hotwords` at `boost` when both are given.
pub fn load_asr(store: &Store, hotwords: &[String], boost: f32) -> Result<ParakeetAsr> {
    required(store, ASR_MODEL)?;
    let dir = store.model_dir(ASR_MODEL);
    let use_hotwords = !hotwords.is_empty() && boost > 0.0;
    let config = ParakeetConfig { hotword_boost: use_hotwords.then_some(boost), ..ParakeetConfig::default() };
    let asr = ParakeetAsr::new(&dir, &config).with_context(|| format!("loading ASR from {}", dir.display()))?;
    if use_hotwords {
        asr.set_hotwords(hotwords)?;
    }
    Ok(asr)
}

pub fn load_segmenter(store: &Store) -> Result<Segmenter> {
    let vad = required(store, VAD_MODEL)?;
    let path = store
        .file_path(vad, "silero_vad.onnx")
        .ok_or_else(|| anyhow!("{VAD_MODEL} has no silero_vad.onnx"))?;
    Segmenter::new(&path, SegmenterConfig::default()).with_context(|| format!("loading VAD from {}", path.display()))
}
