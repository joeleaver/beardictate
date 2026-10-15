//! Downloader behaviour against a local HTTP server. No network.

mod common;

use std::fs;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use bd_models::{Downloader, Error, Event, ModelEntry, Phase, SourceError, Store, PART_SUFFIX};
use common::{Mode, Server, bytes, local_http, model};

const SIZE: usize = 600_000; // a few CHUNKs, so progress and resume are exercised

fn part_of(store: &Store, m: &ModelEntry, file: &str) -> std::path::PathBuf {
    let mut p = store.file_path(m, file).unwrap().into_os_string();
    p.push(PART_SUFFIX);
    p.into()
}

fn run(store: &Store, m: &ModelEntry) -> (Result<(), Error>, Vec<String>) {
    let mut failed = Vec::new();
    let result = Downloader::new(&local_http()).download(store, m, &AtomicBool::new(false), &mut |e| {
        if let Event::SourceFailed { url, error, .. } = e {
            failed.push(format!("{url}: {error}"));
        }
    });
    (result, failed)
}

#[test]
fn full_download_with_progress() {
    let server = Server::start();
    let a = bytes(SIZE, 1);
    let b = bytes(1234, 2);
    server.put("m1/a.bin", &a, Mode::Normal);
    server.put("m1/sub/b.bin", &b, Mode::Normal);
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::at(tmp.path());
    let m = model("full", &[("a.bin", &a), ("sub/b.bin", &b)], vec![format!("{}m1/", server.base)]);

    let mut last = None;
    let mut done_files = Vec::new();
    Downloader::new(&local_http())
        .download(&store, &m, &AtomicBool::new(false), &mut |e| match e {
            Event::Progress(p) => {
                assert_eq!(p.phase, Phase::Downloading);
                assert!(p.file_done <= p.file_total && p.model_done <= p.model_total);
                last = Some((p.file_index, p.model_done, p.model_total));
            }
            Event::FileDone { file } => done_files.push(file.to_string()),
            Event::SourceFailed { .. } => panic!("no source should fail"),
        })
        .unwrap();

    assert_eq!(last, Some((1, (SIZE + 1234) as u64, (SIZE + 1234) as u64)));
    assert_eq!(done_files, ["a.bin", "sub/b.bin"]);
    assert_eq!(fs::read(store.file_path(&m, "a.bin").unwrap()).unwrap(), a);
    assert_eq!(fs::read(store.file_path(&m, "sub/b.bin").unwrap()).unwrap(), b);
    assert!(!part_of(&store, &m, "a.bin").exists());
    assert!(store.is_verified(&m));

    // Already verified: a second run makes no requests.
    let before = server.hits().len();
    run(&store, &m).0.unwrap();
    assert_eq!(server.hits().len(), before);
}

#[test]
fn resumes_after_an_interrupted_download() {
    let server = Server::start();
    let a = bytes(SIZE, 3);
    server.put("m/a.bin", &a, Mode::TruncateOnce(250_000));
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::at(tmp.path());
    let m = model("resume", &[("a.bin", &a)], vec![format!("{}m/", server.base)]);

    // First attempt: the connection dies after 250 kB.
    let (result, failed) = run(&store, &m);
    match result {
        Err(Error::AllSourcesFailed { file, failures }) => {
            assert_eq!(file, "a.bin");
            assert_eq!(failures.0[0].error, SourceError::Truncated { expected: SIZE as u64, received: 250_000 });
        }
        other => panic!("expected AllSourcesFailed, got {other:?}"),
    }
    assert_eq!(failed.len(), 1);
    assert_eq!(fs::metadata(part_of(&store, &m, "a.bin")).unwrap().len(), 250_000);

    // Second attempt re-hashes the 250 kB and asks only for the rest.
    run(&store, &m).0.unwrap();
    assert_eq!(server.hits_for("m/a.bin"), [None, Some("bytes=250000-".to_string())]);
    assert_eq!(fs::read(store.file_path(&m, "a.bin").unwrap()).unwrap(), a);
    assert!(store.is_verified(&m));
}

#[test]
fn restarts_when_the_server_ignores_range() {
    let server = Server::start();
    let a = bytes(SIZE, 4);
    server.put("m/a.bin", &a, Mode::IgnoreRange);
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::at(tmp.path());
    let m = model("norange", &[("a.bin", &a)], vec![format!("{}m/", server.base)]);
    fs::create_dir_all(store.model_dir("norange")).unwrap();
    fs::write(part_of(&store, &m, "a.bin"), &a[..100_000]).unwrap();

    run(&store, &m).0.unwrap();
    assert_eq!(server.hits_for("m/a.bin"), [Some("bytes=100000-".to_string())]);
    assert_eq!(fs::read(store.file_path(&m, "a.bin").unwrap()).unwrap(), a);
}

#[test]
fn restarts_after_416() {
    let server = Server::start();
    let a = bytes(SIZE, 5);
    server.put("m/a.bin", &a, Mode::Reject416);
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::at(tmp.path());
    let m = model("r416", &[("a.bin", &a)], vec![format!("{}m/", server.base)]);
    fs::create_dir_all(store.model_dir("r416")).unwrap();
    fs::write(part_of(&store, &m, "a.bin"), &a[..10]).unwrap();

    run(&store, &m).0.unwrap();
    assert_eq!(server.hits_for("m/a.bin"), [Some("bytes=10-".to_string()), None]);
    assert!(store.is_verified(&m));
}

#[test]
fn hash_mismatch_falls_back_to_the_next_source() {
    let server = Server::start();
    let a = bytes(SIZE, 6);
    let mut evil = a.clone();
    evil[SIZE / 2] ^= 0xff;
    server.put("bad/a.bin", &evil, Mode::Normal);
    server.put("good/a.bin", &a, Mode::Normal);
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::at(tmp.path());
    let sources = vec![format!("{}bad/", server.base), format!("{}good/", server.base)];
    let m = model("mirror", &[("a.bin", &a)], sources);

    let (result, failed) = run(&store, &m);
    result.unwrap();
    assert_eq!(failed.len(), 1);
    assert!(failed[0].contains("bad/a.bin") && failed[0].contains("sha256 mismatch"), "{failed:?}");
    // The bad part was deleted, so the good mirror started from zero.
    assert_eq!(server.hits_for("good/a.bin"), [None]);
    assert_eq!(fs::read(store.file_path(&m, "a.bin").unwrap()).unwrap(), a);
}

#[test]
fn http_errors_fall_back_and_all_failures_are_reported() {
    let server = Server::start();
    let a = bytes(1000, 7);
    server.put("e500/a.bin", &a, Mode::Status(500));
    server.put("good/a.bin", &a, Mode::Normal);
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::at(tmp.path());
    let base = &server.base;
    let m = model("errs", &[("a.bin", &a)], vec![format!("{base}missing/"), format!("{base}e500/"), format!("{base}good/")]);
    let (result, failed) = run(&store, &m);
    result.unwrap();
    assert_eq!(failed.len(), 2);
    assert!(failed[0].contains("HTTP 404") && failed[1].contains("HTTP 500"), "{failed:?}");

    let m2 = model("errs2", &[("a.bin", &a)], vec![format!("{base}missing/"), format!("{base}e500/")]);
    match run(&store, &m2).0 {
        Err(Error::AllSourcesFailed { failures, .. }) => {
            let errors: Vec<_> = failures.0.iter().map(|f| f.error.clone()).collect();
            assert_eq!(errors, [SourceError::HttpStatus(404), SourceError::HttpStatus(500)]);
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn wrong_size_on_server_is_rejected_without_downloading_it() {
    let server = Server::start();
    let a = bytes(1000, 8);
    server.put("m/a.bin", &a[..900], Mode::Normal);
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::at(tmp.path());
    let m = model("size", &[("a.bin", &a)], vec![format!("{}m/", server.base)]);
    match run(&store, &m).0 {
        Err(Error::AllSourcesFailed { failures, .. }) => {
            assert_eq!(failures.0[0].error, SourceError::SizeMismatch { expected: 1000, actual: 900 });
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn cancel_keeps_the_part_and_a_later_run_resumes() {
    let server = Server::start();
    let a = bytes(SIZE, 9);
    server.put("m/a.bin", &a, Mode::Slow { chunk: 16 * 1024, delay: Duration::from_millis(5) });
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::at(tmp.path());
    let m = model("cancel", &[("a.bin", &a)], vec![format!("{}m/", server.base)]);

    let cancel = AtomicBool::new(false);
    let result = Downloader::new(&local_http()).download(&store, &m, &cancel, &mut |e| {
        if let Event::Progress(p) = e
            && p.file_done >= 100_000
        {
            cancel.store(true, Ordering::Relaxed);
        }
    });
    assert!(matches!(result, Err(Error::Cancelled)), "{result:?}");
    let kept = fs::metadata(part_of(&store, &m, "a.bin")).unwrap().len();
    assert!((100_000..SIZE as u64).contains(&kept), "{kept}");
    assert!(!store.is_installed(&m));

    server.set_mode("m/a.bin", Mode::Normal);
    run(&store, &m).0.unwrap();
    assert_eq!(server.hits_for("m/a.bin")[1], Some(format!("bytes={kept}-")));
    assert_eq!(fs::read(store.file_path(&m, "a.bin").unwrap()).unwrap(), a);
}

#[test]
fn corrupt_part_is_discarded_and_refetched_from_the_same_source() {
    let server = Server::start();
    let a = bytes(SIZE, 10);
    server.put("m/a.bin", &a, Mode::Normal);
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::at(tmp.path());
    let m = model("badpart", &[("a.bin", &a)], vec![format!("{}m/", server.base)]);
    fs::create_dir_all(store.model_dir("badpart")).unwrap();
    fs::write(part_of(&store, &m, "a.bin"), vec![0u8; 50_000]).unwrap();

    let (result, failed) = run(&store, &m);
    result.unwrap();
    assert!(failed.is_empty(), "a bad local part is not the source's fault: {failed:?}");
    assert_eq!(server.hits_for("m/a.bin"), [Some("bytes=50000-".to_string()), None]);
    assert_eq!(fs::read(store.file_path(&m, "a.bin").unwrap()).unwrap(), a);
}

#[test]
fn corrupt_installed_file_is_replaced() {
    let server = Server::start();
    let a = bytes(1000, 11);
    server.put("m/a.bin", &a, Mode::Normal);
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::at(tmp.path());
    let m = model("badfile", &[("a.bin", &a)], vec![format!("{}m/", server.base)]);
    fs::create_dir_all(store.model_dir("badfile")).unwrap();
    fs::write(store.file_path(&m, "a.bin").unwrap(), vec![1u8; 1000]).unwrap();
    assert!(store.is_installed(&m) && !store.is_verified(&m));

    run(&store, &m).0.unwrap();
    assert_eq!(fs::read(store.file_path(&m, "a.bin").unwrap()).unwrap(), a);
    assert!(store.is_verified(&m));
}

#[test]
fn plain_http_is_refused_by_default() {
    let server = Server::start();
    let a = bytes(10, 12);
    server.put("m/a.bin", &a, Mode::Normal);
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::at(tmp.path());
    let m = model("https", &[("a.bin", &a)], vec![format!("{}m/", server.base)]);
    let result = Downloader::default().download(&store, &m, &AtomicBool::new(false), &mut |_| {});
    assert!(matches!(result, Err(Error::AllSourcesFailed { .. })), "{result:?}");
    assert!(server.hits().is_empty());
}

#[test]
fn probe_checks_size_and_range_support() {
    let server = Server::start();
    let a = bytes(5000, 13);
    server.put("r/a.bin", &a, Mode::Normal);
    server.put("n/a.bin", &a, Mode::IgnoreRange);
    let d = Downloader::new(&local_http());
    assert_eq!(d.probe(&format!("{}r/a.bin", server.base), 5000), Ok(true));
    assert_eq!(d.probe(&format!("{}n/a.bin", server.base), 5000), Ok(false));
    assert_eq!(
        d.probe(&format!("{}r/a.bin", server.base), 4000),
        Err(SourceError::SizeMismatch { expected: 4000, actual: 5000 })
    );
    assert_eq!(d.probe(&format!("{}x", server.base), 1), Err(SourceError::HttpStatus(404)));
}

#[test]
fn unavailable_models_are_refused() {
    let tmp = tempfile::tempdir().unwrap();
    let manifest = bd_models::Manifest::parse(
        "[[model]]\nid = \"cleanup-bear-medium\"\nkind = \"cleanup\"\nname = \"Bear Medium\"\nlicense = \"Apache-2.0\"\navailable = false\n",
    )
    .unwrap();
    let bear = manifest.get("cleanup-bear-medium").unwrap();
    let result = Downloader::default().download(&Store::at(tmp.path()), bear, &AtomicBool::new(false), &mut |_| {});
    assert!(matches!(result, Err(Error::NotAvailable(_))));
}

#[test]
fn a_stalled_connection_times_out_instead_of_hanging() {
    let server = Server::start();
    let a = bytes(10_000, 14);
    server.put("m/a.bin", &a, Mode::Slow { chunk: 1000, delay: Duration::from_secs(3) });
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::at(tmp.path());
    let m = model("stall", &[("a.bin", &a)], vec![format!("{}m/", server.base)]);
    let options = bd_models::HttpOptions { idle_timeout: Duration::from_millis(300), ..local_http() };
    let started = std::time::Instant::now();
    let result = Downloader::new(&options).download(&store, &m, &AtomicBool::new(false), &mut |_| {});
    match result {
        Err(Error::AllSourcesFailed { failures, .. }) => assert_eq!(failures.0[0].error, SourceError::Timeout),
        other => panic!("{other:?}"),
    }
    assert!(started.elapsed() < Duration::from_secs(2), "{:?}", started.elapsed());
    // The bytes that did arrive are kept for a resume.
    assert_eq!(fs::metadata(part_of(&store, &m, "a.bin")).unwrap().len(), 1000);
}
