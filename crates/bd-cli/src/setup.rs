//! Building pipeline stages from command-line options and the model store.

use anyhow::Result;
use bd_cleanup::GpuLayers;
use bd_core::{PunctuationPlacement, QuoteStyle, Style};
use bd_models::Store;
use bd_pipeline::setup::{self, CleanupOptions, CleanupSource};
use clap::{Args, ValueEnum};

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
            StyleArg::Uk => Style { quotes: QuoteStyle::CurlySingle, punctuation: PunctuationPlacement::Logical, ..Style::default() },
        }
    }
}

/// Which cleanup model to run, if any.
#[derive(Debug, Args)]
pub struct CleanupArgs {
    /// `off`, a catalog id (see `bd models list`), or a path to a .gguf
    /// file. Default: the recommended catalog cleanup model if installed, else the largest installed, else off.
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

pub use bd_pipeline::setup::{LoadedCleanup, load_asr, load_segmenter, load_streaming_asr};

/// Loads the cleanup model the options name. `Ok(None)` means cleanup is
/// off.
pub fn load_cleanup(args: &CleanupArgs, store: &Store) -> Result<Option<LoadedCleanup>> {
    let source = match &args.cleanup_url {
        Some(url) => CleanupSource::Server { url: url.clone(), model: args.cleanup_server_model.clone() },
        None => CleanupSource::parse(args.cleanup.as_deref().unwrap_or("auto")),
    };
    let options = CleanupOptions { template: args.template.clone(), gpu_layers: args.gpu_layers, ctx: args.ctx };
    setup::load_cleanup(&source, &options, store)
}
