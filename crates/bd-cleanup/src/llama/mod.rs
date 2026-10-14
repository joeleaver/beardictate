//! In-process llama.cpp backend.
//!
//! The model and its context live on a dedicated worker thread for as long
//! as the [`LlamaChat`] exists: loaded once, resident, freed on drop. A
//! `LlamaContext` borrows its `LlamaModel`, so owning both on one thread's
//! stack avoids a self-referential struct. Requests reach the worker over a
//! channel, one at a time.

mod engine;

use std::path::PathBuf;
use std::sync::{OnceLock, mpsc};
use std::thread::JoinHandle;

use llama_cpp_2::llama_backend::LlamaBackend as Backend;

use crate::{ChatBackend, ChatMessage, CleanupError, Completion};

/// How many layers to put on the GPU.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum GpuLayers {
    /// Every layer when a GPU backend (`vulkan` feature) is compiled in and
    /// finds a device; none otherwise.
    #[default]
    Auto,
    /// CPU only.
    None,
    /// Exactly this many layers.
    Count(u32),
}

/// Sampling settings. Greedy by default: cleanup wants the most likely
/// text, not variety.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Sampling {
    /// 0 means greedy.
    pub temperature: f32,
    pub seed: u32,
}

impl Default for Sampling {
    fn default() -> Self {
        Self { temperature: 0.0, seed: 0 }
    }
}

/// What to load and how to run it.
#[derive(Debug, Clone, PartialEq)]
pub struct LlamaConfig {
    pub model_path: PathBuf,
    pub gpu_layers: GpuLayers,
    /// Context window in tokens. Prompt (system prompt, ~2 paragraphs of
    /// context, the segment) plus the answer must fit.
    pub n_ctx: u32,
    /// Prompt tokens decoded per batch.
    pub n_batch: u32,
    /// CPU threads; `None` picks half the logical CPUs (physical cores on
    /// SMT machines), at least 1 and at most 16.
    pub threads: Option<u32>,
    pub sampling: Sampling,
}

impl LlamaConfig {
    pub fn new(model_path: impl Into<PathBuf>) -> Self {
        Self {
            model_path: model_path.into(),
            gpu_layers: GpuLayers::Auto,
            n_ctx: 4096,
            n_batch: 512,
            threads: None,
            sampling: Sampling::default(),
        }
    }
}

/// Facts about the loaded model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelInfo {
    pub path: PathBuf,
    pub n_params: u64,
    pub size_bytes: u64,
    pub n_layer: u32,
    /// Layers placed on the GPU (0 on CPU).
    pub gpu_layers: u32,
    pub n_ctx: u32,
    pub threads: u32,
    /// The model's chat template has a thinking switch (Qwen3) and thinking
    /// is being suppressed by pre-filling an empty think block.
    pub thinking_suppressed: bool,
    /// The GGUF had no chat template, so ChatML is used.
    pub fallback_template: bool,
}

/// The process-wide llama.cpp backend. llama.cpp may be initialized only
/// once per process, so every user (this crate, a hardware probe) must share
/// this one. Its logs go to `tracing` (target `llama-cpp-2`).
pub fn shared_backend() -> Result<&'static Backend, CleanupError> {
    static BACKEND: OnceLock<Result<Backend, String>> = OnceLock::new();
    BACKEND
        .get_or_init(|| {
            llama_cpp_2::send_logs_to_tracing(llama_cpp_2::LogOptions::default());
            Backend::init().map_err(|e| e.to_string())
        })
        .as_ref()
        .map_err(|e| CleanupError::Llama(e.clone()))
}

struct Job {
    messages: Vec<ChatMessage>,
    max_tokens: usize,
    reply: mpsc::Sender<Result<Completion, CleanupError>>,
}

/// A GGUF model loaded in process, behind a worker thread.
pub struct LlamaChat {
    jobs: Option<mpsc::Sender<Job>>,
    worker: Option<JoinHandle<()>>,
    info: ModelInfo,
}

impl LlamaChat {
    /// Loads the model and creates its context, blocking until both are
    /// ready (or failed).
    pub fn load(config: LlamaConfig) -> Result<Self, CleanupError> {
        if !config.model_path.is_file() {
            return Err(CleanupError::ModelNotFound(config.model_path));
        }
        let backend = shared_backend()?;
        let (ready_tx, ready_rx) = mpsc::channel();
        let (jobs_tx, jobs_rx) = mpsc::channel::<Job>();
        let worker = std::thread::Builder::new()
            .name("bd-cleanup-llama".into())
            .spawn(move || engine::serve(backend, &config, &ready_tx, &jobs_rx))
            .map_err(|e| CleanupError::Llama(format!("could not start worker thread: {e}")))?;
        match ready_rx.recv() {
            Ok(Ok(info)) => Ok(Self { jobs: Some(jobs_tx), worker: Some(worker), info }),
            Ok(Err(e)) => {
                let _ = worker.join();
                Err(e)
            }
            Err(_) => {
                let _ = worker.join();
                Err(CleanupError::WorkerGone)
            }
        }
    }

    pub fn info(&self) -> &ModelInfo {
        &self.info
    }
}

impl ChatBackend for LlamaChat {
    fn complete(&mut self, messages: &[ChatMessage], max_tokens: usize) -> Result<Completion, CleanupError> {
        let (reply, answer) = mpsc::channel();
        let job = Job { messages: messages.to_vec(), max_tokens, reply };
        self.jobs
            .as_ref()
            .ok_or(CleanupError::WorkerGone)?
            .send(job)
            .map_err(|_| CleanupError::WorkerGone)?;
        answer.recv().map_err(|_| CleanupError::WorkerGone)?
    }
}

impl Drop for LlamaChat {
    /// Closes the job channel and waits for the worker to free the context
    /// and the model, so memory is back before the next model loads.
    fn drop(&mut self) {
        self.jobs.take();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

/// Default CPU thread count: half the logical CPUs, 1..=16.
fn default_threads() -> u32 {
    let logical = std::thread::available_parallelism().map_or(4, |n| n.get());
    u32::try_from((logical / 2).clamp(1, 16)).unwrap_or(4)
}
