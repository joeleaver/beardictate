//! `bd models` and `bd devices`.

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;

use anyhow::{Result, anyhow, bail};
use bd_models::{Downloader, Event, Manifest, ModelEntry, ModelStatus, Store, import_folder};
use clap::Subcommand;

#[derive(Debug, Subcommand)]
pub enum ModelsCommand {
    /// Show the catalog and what is installed.
    List,
    /// Download models (resumable; every file is hash-checked).
    Download {
        #[arg(required = true)]
        ids: Vec<String>,
    },
    /// Re-hash installed models. Default: every installed model.
    Verify { ids: Vec<String> },
    /// Install a model from a folder that already holds its files.
    Import { id: String, dir: PathBuf },
}

pub fn run(command: &ModelsCommand) -> Result<()> {
    let store = Store::from_env()?;
    let manifest = Manifest::builtin();
    let never = AtomicBool::new(false);
    match command {
        ModelsCommand::List => list(&store, manifest),
        ModelsCommand::Download { ids } => {
            let downloader = Downloader::default();
            for model in lookup(manifest, ids)? {
                eprintln!("downloading {} ({})", model.id, size(model.total_size()));
                let mut progress = Progress::default();
                downloader.download(&store, model, &never, &mut |e| progress.show(e))?;
                eprintln!("\r{} installed{:30}", model.id, "");
            }
            Ok(())
        }
        ModelsCommand::Verify { ids } => {
            let models = if ids.is_empty() {
                manifest.models.iter().filter(|m| matches!(store.status(m), ModelStatus::Installed { .. })).collect()
            } else {
                lookup(manifest, ids)?
            };
            let mut bad = 0;
            for model in models {
                let mut progress = Progress::default();
                let issues = store.verify(model, &never, &mut |e| progress.show(e))?;
                if issues.is_empty() {
                    eprintln!("\r{} ok{:40}", model.id, "");
                } else {
                    bad += 1;
                    eprintln!("\r{}:{:40}", model.id, "");
                    for issue in issues {
                        eprintln!("  {}: {:?}", issue.file, issue.problem);
                    }
                }
            }
            if bad > 0 {
                bail!("{bad} model(s) failed verification");
            }
            Ok(())
        }
        ModelsCommand::Import { id, dir } => {
            let model = lookup(manifest, std::slice::from_ref(id))?[0];
            let mut progress = Progress::default();
            let report = import_folder(&store, model, dir, &never, &mut |e| progress.show(e))?;
            eprintln!(
                "\r{id}: {} linked, {} copied, {} already present{:20}",
                report.linked, report.copied, report.already_present, ""
            );
            Ok(())
        }
    }
}

fn lookup<'m>(manifest: &'m Manifest, ids: &[String]) -> Result<Vec<&'m ModelEntry>> {
    ids.iter()
        .map(|id| manifest.get(id).ok_or_else(|| anyhow!("no model {id:?} in the catalog; see `bd models list`")))
        .collect()
}

fn list(store: &Store, manifest: &Manifest) -> Result<()> {
    println!("models in {}", store.root().display());
    for m in &manifest.models {
        let status = match store.status(m) {
            _ if !m.available => "not released yet".to_string(),
            ModelStatus::NotInstalled => "-".to_string(),
            ModelStatus::Partial { bytes_present } => format!("partial ({})", size(bytes_present)),
            ModelStatus::Installed { verified: true } => "installed, verified".to_string(),
            ModelStatus::Installed { verified: false } => "installed".to_string(),
        };
        let kind = format!("{:?}", m.kind).to_lowercase();
        println!("  {:<34} {:<8} {:>9}  {:<22} {}", m.id, kind, size(m.total_size()), status, m.name);
    }
    Ok(())
}

fn size(bytes: u64) -> String {
    match bytes {
        0 => "-".into(),
        b if b >= 1 << 30 => format!("{:.1} GB", b as f64 / f64::from(1u32 << 30)),
        b => format!("{:.0} MB", (b as f64 / f64::from(1u32 << 20)).max(1.0)),
    }
}

/// A one-line progress display on stderr, updated per whole percent.
#[derive(Default)]
struct Progress {
    last: Option<(String, u64)>,
}

impl Progress {
    fn show(&mut self, event: &Event<'_>) {
        match event {
            Event::Progress(p) => {
                let pct = (p.model_done * 100).checked_div(p.model_total).unwrap_or(100);
                let key = (format!("{:?} {}", p.phase, p.file), pct);
                if self.last.as_ref() != Some(&key) {
                    eprint!("\r  {:?} {} {pct:>3}%   ", p.phase, p.file);
                    self.last = Some(key);
                }
            }
            Event::SourceFailed { file, url, error } => eprintln!("\n  {file}: {url} failed ({error}); trying the next source"),
            Event::FileDone { .. } => {}
        }
    }
}

pub fn devices() -> Result<()> {
    for d in bd_audio::list_input_devices()? {
        println!("{}{}", d.name, if d.is_default { "  (default)" } else { "" });
    }
    Ok(())
}
