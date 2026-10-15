//! The model manager: the speech models dictation needs and the catalog's
//! cleanup models, what's on disk for each, and downloads and deletes on
//! worker threads. The settings window's Cleanup page shows the cleanup
//! models; its first-run card (`window/setup.rs`) the speech models. The
//! engine hears when the set of downloaded models changes
//! (`Command::ModelsChanged`).

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use bd_models::{Downloader, Error, Event, Manifest, ModelEntry, ModelKind, ModelStatus, Store};
use rinch::prelude::*;

use crate::engine::Command;

/// Progress reaches the UI at most this often.
const PROGRESS_EVERY: Duration = Duration::from_millis(200);

/// What's on disk for one model, or what's being done to it.
#[derive(Debug, Clone, PartialEq)]
pub enum ModelState {
    Missing,
    /// An interrupted download: these bytes are kept, and Download resumes.
    Partial(u64),
    Installed,
    Downloading { done: u64, total: u64 },
    Deleting,
    Failed(String),
}

/// The cleanup models a user can download, smallest first.
pub fn catalog() -> Vec<&'static ModelEntry> {
    Manifest::builtin().of_kind(ModelKind::Cleanup).filter(|m| m.available).collect()
}

/// The models dictation can't start without (ASR, VAD).
pub fn speech() -> Vec<&'static ModelEntry> {
    Manifest::builtin().models.iter().filter(|m| m.required && m.available).collect()
}

/// Every model the manager tracks: the speech models, then the catalog.
fn tracked() -> impl Iterator<Item = &'static ModelEntry> {
    speech().into_iter().chain(catalog())
}

/// The speech models not yet downloaded and verified.
pub fn missing_speech(states: &HashMap<String, ModelState>) -> Vec<&'static ModelEntry> {
    speech().into_iter().filter(|m| states.get(&m.id) != Some(&ModelState::Installed)).collect()
}

/// The model store `models_dir` names, or the default one.
pub fn store(models_dir: &str) -> Option<Store> {
    match models_dir.trim() {
        "" => Store::from_env().ok(),
        dir => Some(Store::at(dir)),
    }
}

fn on_disk(store: Option<&Store>, model: &ModelEntry) -> ModelState {
    match store.map(|s| s.status(model)) {
        Some(ModelStatus::Installed { .. }) => ModelState::Installed,
        Some(ModelStatus::Partial { bytes_present }) => ModelState::Partial(bytes_present),
        _ => ModelState::Missing,
    }
}

/// The model `cleanup` (a setting: "auto", "off", an id or a path) loads,
/// if it is a catalog model: "auto" is the largest downloaded.
pub fn in_use(cleanup: &str, states: &HashMap<String, ModelState>) -> Option<String> {
    let installed = |id: &str| states.get(id) == Some(&ModelState::Installed);
    match cleanup {
        "auto" => catalog().iter().rev().map(|m| m.id.clone()).find(|id| installed(id)),
        id if installed(id) => Some(id.to_string()),
        _ => None,
    }
}

/// Shared by the page's closures: cheap to clone, used on the UI thread.
#[derive(Clone)]
pub struct Models {
    pub states: Signal<HashMap<String, ModelState>>,
    cancels: Arc<Mutex<HashMap<String, Arc<AtomicBool>>>>,
    downloader: Downloader,
}

impl Models {
    pub fn new(store: Option<&Store>) -> Models {
        let states = tracked().map(|m| (m.id.clone(), on_disk(store, m))).collect();
        Models { states: Signal::new(states), cancels: Arc::default(), downloader: Downloader::default() }
    }

    /// Reads the disk again (the models folder changed), leaving models
    /// being downloaded or deleted alone.
    pub fn refresh(&self, store: Option<&Store>) {
        self.states.update(|states| {
            for m in tracked() {
                let busy = matches!(states.get(&m.id), Some(ModelState::Downloading { .. } | ModelState::Deleting));
                if !busy {
                    states.insert(m.id.clone(), on_disk(store, m));
                }
            }
        });
    }

    /// Starts (or resumes) downloading `id` into `store`.
    pub fn download(&self, store: Store, id: &str) {
        let Some(model) = tracked().find(|m| m.id == id) else { return };
        let cancel = Arc::new(AtomicBool::new(false));
        self.cancels.lock().unwrap_or_else(|e| e.into_inner()).insert(id.to_string(), cancel.clone());
        let total = model.total_size();
        self.states.update(|s| {
            s.insert(model.id.clone(), ModelState::Downloading { done: 0, total });
        });
        let (states, cancels, downloader) = (self.states, self.cancels.clone(), self.downloader.clone());
        let spawned = thread::Builder::new().name("bd-download".into()).spawn(move || {
            tracing::info!(model = %model.id, "downloading");
            let mut last = Instant::now() - PROGRESS_EVERY;
            let mut on_event = |event: &Event<'_>| {
                if let Event::Progress(p) = event
                    && last.elapsed() >= PROGRESS_EVERY
                {
                    last = Instant::now();
                    let (id, done) = (model.id.clone(), p.model_done);
                    on_ui(move || states.update(|s| {
                        s.insert(id, ModelState::Downloading { done, total });
                    }));
                }
            };
            let result = downloader.download(&store, model, &cancel, &mut on_event);
            cancels.lock().unwrap_or_else(|e| e.into_inner()).remove(&model.id);
            let state = match result {
                Ok(()) => {
                    tracing::info!(model = %model.id, "downloaded");
                    ModelState::Installed
                }
                Err(Error::Cancelled) => on_disk(Some(&store), model),
                Err(e) => {
                    tracing::warn!(model = %model.id, "download failed: {e}");
                    ModelState::Failed(e.to_string())
                }
            };
            let id = model.id.clone();
            on_ui(move || states.update(|s| {
                s.insert(id, state);
            }));
            rinch::run_on_main_thread(|| crate::engine_send(Command::ModelsChanged));
        });
        if let Err(e) = spawned {
            self.states.update(|s| {
                s.insert(id.to_string(), ModelState::Failed(e.to_string()));
            });
        }
    }

    /// Stops a download; what arrived is kept for a resume.
    pub fn cancel(&self, id: &str) {
        if let Some(flag) = self.cancels.lock().unwrap_or_else(|e| e.into_inner()).get(id) {
            flag.store(true, Ordering::Relaxed);
        }
    }

    /// Removes `id` from `store`, partial downloads included.
    pub fn delete(&self, store: Store, id: &str) {
        let id = id.to_string();
        self.states.update(|s| {
            s.insert(id.clone(), ModelState::Deleting);
        });
        let states = self.states;
        thread::spawn(move || {
            let state = match store.delete(&id) {
                Ok(()) => {
                    tracing::info!(model = %id, "deleted");
                    ModelState::Missing
                }
                Err(e) => ModelState::Failed(format!("couldn't delete: {e}")),
            };
            on_ui(move || states.update(|s| {
                s.insert(id, state);
            }));
            rinch::run_on_main_thread(|| crate::engine_send(Command::ModelsChanged));
        });
    }
}

/// Runs `f` on the UI thread, where signals live, and wakes it to repaint.
fn on_ui(f: impl FnOnce() + Send + 'static) {
    rinch::run_on_main_thread(f);
}

/// "2.3 GB", "610 MB".
pub fn size(bytes: u64) -> String {
    let mb = bytes as f64 / 1e6;
    if mb >= 1000.0 { format!("{:.1} GB", mb / 1000.0) } else { format!("{mb:.0} MB") }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_uses_the_largest_downloaded_model() {
        let ids: Vec<String> = catalog().iter().map(|m| m.id.clone()).collect();
        let largest = ids.last().expect("the catalog has a cleanup model").clone();
        let mut states: HashMap<String, ModelState> = ids.iter().map(|id| (id.clone(), ModelState::Missing)).collect();
        assert_eq!(in_use("auto", &states), None);
        for id in &ids {
            states.insert(id.clone(), ModelState::Installed);
        }
        assert_eq!(in_use("auto", &states), Some(largest));
        assert_eq!(in_use(&ids[0], &states), Some(ids[0].clone()));
        assert_eq!(in_use("off", &states), None);
        assert_eq!(in_use("cleanup-gone-from-the-catalog", &states), None);
        assert_eq!(size(2_300_000_000), "2.3 GB");
        assert_eq!(size(610_000_000), "610 MB");
    }

    #[test]
    fn speech_models_are_tracked_and_missing_until_installed() {
        let speech = speech();
        let kinds: Vec<ModelKind> = speech.iter().map(|m| m.kind).collect();
        assert!(kinds.contains(&ModelKind::Asr) && kinds.contains(&ModelKind::Vad));
        let models = Models::new(None);
        let mut states = models.states.get();
        assert!(speech.iter().all(|m| states.contains_key(&m.id)));
        assert_eq!(missing_speech(&states).len(), speech.len());
        for m in &speech {
            states.insert(m.id.clone(), ModelState::Installed);
        }
        assert!(missing_speech(&states).is_empty());
    }
}
