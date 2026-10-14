use std::sync::atomic::AtomicBool;

use sha2::{Digest, Sha256};

use super::*;
use crate::manifest::{ModelKind, ModelFile};

fn entry(id: &str, files: &[(&str, &[u8])]) -> ModelEntry {
    ModelEntry {
        id: id.into(),
        kind: ModelKind::Vad,
        name: id.into(),
        license: "MIT".into(),
        attribution: None,
        homepage: None,
        required: false,
        available: true,
        sources: vec!["https://example.invalid/".into()],
        files: files
            .iter()
            .map(|(path, data)| ModelFile {
                path: (*path).into(),
                size: data.len() as u64,
                sha256: hex::encode(Sha256::digest(data)),
            })
            .collect(),
        cleanup: None,
    }
}

fn put(store: &Store, model: &ModelEntry, path: &str, data: &[u8]) {
    let dest = store.file_path(model, path).unwrap();
    fs::create_dir_all(dest.parent().unwrap()).unwrap();
    fs::write(dest, data).unwrap();
}

fn no_cancel() -> AtomicBool {
    AtomicBool::new(false)
}

#[test]
fn layout_is_root_id_file() {
    let store = Store::at("/r");
    let model = entry("m", &[("sub/a.bin", b"a")]);
    assert_eq!(store.file_path(&model, "sub/a.bin").unwrap(), Path::new("/r/m/sub/a.bin"));
    assert!(store.file_path(&model, "nope").is_none());
    assert_eq!(part_path(Path::new("/r/m/a.bin")), Path::new("/r/m/a.bin.part"));
}

#[test]
fn env_var_overrides_default_root() {
    // The only test touching the variable, so no cross-test races.
    let dir = tempfile::tempdir().unwrap();
    // SAFETY: no other thread in this test binary reads or writes BD_MODELS_DIR.
    unsafe { std::env::set_var(MODELS_DIR_ENV, dir.path()) };
    assert_eq!(Store::from_env().unwrap().root(), dir.path());
    unsafe { std::env::set_var(MODELS_DIR_ENV, "") };
    let default = Store::default_root().unwrap();
    assert!(default.ends_with(Path::new("beardictate").join("models")), "{default:?}");
    unsafe { std::env::remove_var(MODELS_DIR_ENV) };
}

#[test]
fn status_progresses_from_missing_to_verified() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::at(dir.path());
    let model = entry("m", &[("a.bin", b"aaaa"), ("b.bin", b"bb")]);
    assert_eq!(store.status(&model), ModelStatus::NotInstalled);

    put(&store, &model, "a.bin", b"aaaa");
    fs::write(part_path(&store.file_path(&model, "b.bin").unwrap()), b"b").unwrap();
    assert_eq!(store.status(&model), ModelStatus::Partial { bytes_present: 5 });
    assert!(!store.is_installed(&model));

    put(&store, &model, "b.bin", b"bb");
    assert!(store.is_installed(&model));
    assert_eq!(store.status(&model), ModelStatus::Installed { verified: false });

    let issues = store.verify(&model, &no_cancel(), &mut |_| {}).unwrap();
    assert!(issues.is_empty(), "{issues:?}");
    assert!(store.is_verified(&model));
    assert_eq!(store.status(&model), ModelStatus::Installed { verified: true });
}

#[test]
fn wrong_size_is_not_installed() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::at(dir.path());
    let model = entry("m", &[("a.bin", b"aaaa")]);
    put(&store, &model, "a.bin", b"aaa");
    assert!(!store.is_installed(&model));
    assert_eq!(store.status(&model), ModelStatus::Partial { bytes_present: 3 });
}

#[test]
fn verify_reports_each_problem_and_forgets_bad_files() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::at(dir.path());
    let model = entry("m", &[("a.bin", b"aaaa"), ("b.bin", b"bb"), ("c.bin", b"c")]);
    put(&store, &model, "a.bin", b"aaaa");
    put(&store, &model, "b.bin", b"XX");
    assert!(store.verify(&model, &no_cancel(), &mut |_| {}).unwrap().len() == 2);

    put(&store, &model, "c.bin", b"cc");
    let issues = store.verify(&model, &no_cancel(), &mut |_| {}).unwrap();
    let problems: Vec<_> = issues.iter().map(|i| (i.file.as_str(), &i.problem)).collect();
    assert!(matches!(problems[0], ("b.bin", FileProblem::WrongHash { .. })));
    assert_eq!(problems[1], ("c.bin", &FileProblem::WrongSize { expected: 1, actual: 2 }));
    assert!(!store.is_verified(&model));
    // a.bin alone is still trusted.
    assert!(marker::is_verified(&store.model_dir("m"), &model.files[0]));
}

#[test]
fn verify_reports_progress_and_cancels() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::at(dir.path());
    let model = entry("m", &[("a.bin", b"aaaa")]);
    put(&store, &model, "a.bin", b"aaaa");
    let mut last = None;
    store
        .verify(&model, &no_cancel(), &mut |e| {
            if let crate::Event::Progress(p) = e {
                last = Some((p.phase, p.model_done, p.model_total));
            }
        })
        .unwrap();
    assert_eq!(last, Some((Phase::Checking, 4, 4)));
    let err = store.verify(&model, &AtomicBool::new(true), &mut |_| {}).unwrap_err();
    assert!(matches!(err, Error::Cancelled));
}

#[test]
fn delete_and_disk_usage() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::at(dir.path());
    let model = entry("m", &[("a.bin", b"aaaa")]);
    put(&store, &model, "a.bin", b"aaaa");
    fs::write(store.model_dir("m").join("x.part"), b"123").unwrap();
    put(&store, &entry("other", &[("o", b"o")]), "o", b"oo");
    assert_eq!(store.disk_usage("m").unwrap(), 7);
    assert_eq!(store.total_disk_usage().unwrap(), 9);

    store.delete("m").unwrap();
    assert!(!store.model_dir("m").exists());
    assert_eq!(store.disk_usage("m").unwrap(), 0);
    store.delete("m").unwrap(); // idempotent
    assert!(matches!(store.delete("../etc"), Err(Error::InvalidModelId(_))));
    assert!(matches!(store.disk_usage(""), Err(Error::InvalidModelId(_))));
}

#[test]
fn lock_is_exclusive_per_model() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::at(dir.path());
    let held = store.lock("m").unwrap();
    assert!(matches!(store.lock("m"), Err(Error::Busy(id)) if id == "m"));
    assert!(matches!(store.delete("m"), Err(Error::Busy(_))));
    let _other = store.lock("n").unwrap();
    drop(held);
    store.lock("m").unwrap();
}

#[test]
fn unknown_dirs_lists_leftovers_only() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::at(dir.path());
    assert!(store.unknown_dirs(Manifest::builtin()).unwrap().is_empty());
    for d in ["vad-silero", "old-model", ".locks"] {
        fs::create_dir_all(dir.path().join(d)).unwrap();
    }
    fs::write(dir.path().join("stray-file"), b"").unwrap();
    assert_eq!(store.unknown_dirs(Manifest::builtin()).unwrap(), vec!["old-model".to_string()]);
}

#[test]
fn unavailable_models_are_never_installed() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::at(dir.path());
    let bear = Manifest::builtin().get("cleanup-bear-small").unwrap();
    assert!(!store.is_installed(bear));
    assert!(!store.is_verified(bear));
    assert_eq!(store.status(bear), ModelStatus::NotInstalled);
    assert!(matches!(store.verify(bear, &no_cancel(), &mut |_| {}), Err(Error::NotAvailable(_))));
}
