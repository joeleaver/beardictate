use std::sync::atomic::AtomicBool;

use super::*;
use crate::manifest::ModelKind;

fn entry(files: &[(&str, &[u8])]) -> ModelEntry {
    ModelEntry {
        id: "m".into(),
        kind: ModelKind::Asr,
        name: "M".into(),
        license: "MIT".into(),
        attribution: None,
        homepage: None,
        required: false,
        available: true,
        recommended: false,
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

fn write(dir: &Path, path: &str, data: &[u8]) {
    let p = dir.join(path);
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, data).unwrap();
}

fn no_cancel() -> AtomicBool {
    AtomicBool::new(false)
}

#[test]
fn imports_by_hard_link_on_same_filesystem() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("src");
    let store = Store::at(tmp.path().join("store"));
    let model = entry(&[("a.bin", b"aaaa"), ("sub/b.bin", b"bb")]);
    write(&src, "a.bin", b"aaaa");
    write(&src, "sub/b.bin", b"bb");

    let report = import_folder(&store, &model, &src, &no_cancel(), &mut |_| {}).unwrap();
    assert_eq!(report, ImportReport { linked: 2, copied: 0, already_present: 0 });
    assert!(store.is_verified(&model));
    assert_eq!(fs::read(store.file_path(&model, "sub/b.bin").unwrap()).unwrap(), b"bb");

    // Second import is a no-op.
    let again = import_folder(&store, &model, &src, &no_cancel(), &mut |_| {}).unwrap();
    assert_eq!(again.already_present, 2);
}

#[test]
fn copy_path_verifies_and_cleans_up() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("src");
    let model = entry(&[("a.bin", b"aaaa")]);
    write(&src, "a.bin", b"aaaa");
    let part = tmp.path().join("dest.part");
    let reporter = Reporter {
        model_id: "m",
        file: "a.bin",
        file_index: 0,
        file_count: 1,
        file_total: 4,
        model_base: 0,
        model_total: 4,
    };
    let mut phases = Vec::new();
    copy_verified(&src.join("a.bin"), &part, &model.files[0], &no_cancel(), &reporter, &mut |e| {
        if let Event::Progress(p) = e {
            phases.push(p.phase);
        }
    })
    .unwrap();
    assert_eq!(fs::read(&part).unwrap(), b"aaaa");
    assert_eq!(phases, vec![Phase::Copying]);

    write(&src, "a.bin", b"aaab");
    let err = copy_verified(&src.join("a.bin"), &part, &model.files[0], &no_cancel(), &reporter, &mut |_| {});
    assert!(matches!(err, Err(Error::Import { problem: FileProblem::WrongHash { .. }, .. })));
    assert!(!part.exists());
}

#[test]
fn accepts_a_store_copy_with_model_subfolder() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("usb");
    let store = Store::at(tmp.path().join("store"));
    let model = entry(&[("a.bin", b"aaaa")]);
    write(&src, "m/a.bin", b"aaaa");
    import_folder(&store, &model, &src, &no_cancel(), &mut |_| {}).unwrap();
    assert!(store.is_verified(&model));
}

#[test]
fn rejects_missing_wrong_size_and_wrong_hash() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("src");
    let store = Store::at(tmp.path().join("store"));
    let model = entry(&[("a.bin", b"aaaa"), ("b.bin", b"bb")]);
    write(&src, "a.bin", b"aaaa");

    let err = import_folder(&store, &model, &src, &no_cancel(), &mut |_| {}).unwrap_err();
    assert!(matches!(err, Error::Import { ref file, problem: FileProblem::Missing } if file == "b.bin"));
    assert!(!store.model_dir("m").exists(), "nothing touched before all files check out");

    write(&src, "b.bin", b"bbb");
    let err = import_folder(&store, &model, &src, &no_cancel(), &mut |_| {}).unwrap_err();
    assert!(matches!(err, Error::Import { problem: FileProblem::WrongSize { expected: 2, actual: 3 }, .. }));

    write(&src, "b.bin", b"XX");
    let err = import_folder(&store, &model, &src, &no_cancel(), &mut |_| {}).unwrap_err();
    assert!(matches!(err, Error::Import { ref file, problem: FileProblem::WrongHash { .. } } if file == "b.bin"));
    let b = store.file_path(&model, "b.bin").unwrap();
    assert!(!b.exists() && !part_path(&b).exists());
    assert!(!store.is_installed(&model));
}

#[test]
fn detects_importable_models() {
    let tmp = tempfile::tempdir().unwrap();
    let manifest = Manifest { models: vec![entry(&[("a.bin", b"aaaa")])] };
    assert!(detect_importable(&manifest, tmp.path()).is_empty());
    write(tmp.path(), "a.bin", b"aaaa");
    assert_eq!(detect_importable(&manifest, tmp.path()).len(), 1);
}

#[test]
fn importing_the_store_into_itself_just_verifies() {
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::at(tmp.path());
    let model = entry(&[("a.bin", b"aaaa")]);
    write(tmp.path(), "m/a.bin", b"aaaa");
    let report = import_folder(&store, &model, &store.model_dir("m"), &no_cancel(), &mut |_| {}).unwrap();
    assert_eq!(report.already_present, 1);
    assert!(store.is_verified(&model));
}
