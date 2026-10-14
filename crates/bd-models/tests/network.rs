//! Checks against the real upstream URLs in the built-in manifest. Ignored
//! by default (they need the network); run with
//! `cargo test -p bd-models --test network -- --ignored`.

use std::sync::atomic::AtomicBool;

use bd_models::{Downloader, Event, Manifest, ModelEntry, Store};

/// Every source of every file resolves, has the manifest's size, and
/// honours ranges (so downloads can resume). Fetches one byte per URL.
#[test]
#[ignore = "network"]
fn every_manifest_url_resolves_with_the_right_size() {
    let downloader = Downloader::default();
    let mut failures = Vec::new();
    for model in Manifest::builtin().models.iter().filter(|m| m.available) {
        for source in &model.sources {
            for file in &model.files {
                let url = ModelEntry::file_url(source, file);
                match downloader.probe(&url, file.size) {
                    Ok(true) => println!("ok      {url}"),
                    Ok(false) => failures.push(format!("{url}: no range support")),
                    Err(e) => failures.push(format!("{url}: {e}")),
                }
            }
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
}

/// Downloads the small Silero VAD model for real into `BD_MODELS_DIR` (or
/// the platform store), proving its URL and hash end to end.
#[test]
#[ignore = "network"]
fn downloads_silero_vad_end_to_end() {
    let store = Store::from_env().unwrap();
    let vad = Manifest::builtin().get("vad-silero").unwrap();
    store.delete(&vad.id).unwrap();
    let mut events = 0usize;
    Downloader::default()
        .download(&store, vad, &AtomicBool::new(false), &mut |e| {
            if let Event::SourceFailed { url, error, .. } = e {
                panic!("{url}: {error}");
            }
            events += 1;
        })
        .unwrap();
    assert!(events > 1);
    assert!(store.is_verified(vad));
    let issues = store.verify(vad, &AtomicBool::new(false), &mut |_| {}).unwrap();
    assert!(issues.is_empty(), "{issues:?}");
    println!("verified at {}", store.file_path(vad, "silero_vad.onnx").unwrap().display());
}
