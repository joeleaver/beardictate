//! A tiny HTTP/1.1 file server for downloader tests: std only, one thread
//! per connection, `Connection: close`, with switchable misbehaviours.

#![allow(dead_code)] // each test binary uses a different subset

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use bd_models::{HttpOptions, ModelEntry, ModelFile, ModelKind};
use sha2::{Digest, Sha256};

/// How the server answers requests for one path.
#[derive(Debug, Clone)]
pub enum Mode {
    /// Honour `Range` correctly.
    Normal,
    /// Always send the whole file with 200, ignoring `Range`.
    IgnoreRange,
    /// Answer every request with this status and an empty body.
    Status(u16),
    /// Answer every ranged request with 416; full requests work normally.
    Reject416,
    /// Advertise the full length but close after this many body bytes, on
    /// the first request only; afterwards behave normally.
    TruncateOnce(usize),
    /// Send the body in `chunk`-byte pieces with `delay` between them.
    Slow { chunk: usize, delay: Duration },
}

struct Route {
    data: Vec<u8>,
    mode: Mode,
    hits: usize,
}

/// One logged request: path and the raw `Range` header, if any.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hit {
    pub path: String,
    pub range: Option<String>,
}

#[derive(Clone)]
pub struct Server {
    pub base: String,
    routes: Arc<Mutex<HashMap<String, Route>>>,
    log: Arc<Mutex<Vec<Hit>>>,
}

impl Server {
    pub fn start() -> Server {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}/", listener.local_addr().unwrap());
        let server = Server { base, routes: Default::default(), log: Default::default() };
        let s = server.clone();
        thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { continue };
                let s = s.clone();
                thread::spawn(move || {
                    let _ = s.handle(stream);
                });
            }
        });
        server
    }

    /// Serves `data` at `/<prefix>/<path>` (prefix lets one server act as
    /// several mirrors).
    pub fn put(&self, url_path: &str, data: &[u8], mode: Mode) {
        self.routes
            .lock()
            .unwrap()
            .insert(url_path.to_string(), Route { data: data.to_vec(), mode, hits: 0 });
    }

    pub fn set_mode(&self, url_path: &str, mode: Mode) {
        self.routes.lock().unwrap().get_mut(url_path).unwrap().mode = mode;
    }

    pub fn hits(&self) -> Vec<Hit> {
        self.log.lock().unwrap().clone()
    }

    pub fn hits_for(&self, url_path: &str) -> Vec<Option<String>> {
        self.hits().into_iter().filter(|h| h.path == url_path).map(|h| h.range).collect()
    }

    fn handle(&self, stream: TcpStream) -> std::io::Result<()> {
        let mut reader = BufReader::new(stream.try_clone()?);
        let mut request_line = String::new();
        reader.read_line(&mut request_line)?;
        let path = request_line.split_whitespace().nth(1).unwrap_or("/").trim_start_matches('/').to_string();
        let mut range = None;
        loop {
            let mut line = String::new();
            if reader.read_line(&mut line)? == 0 || line == "\r\n" {
                break;
            }
            if let Some((name, value)) = line.split_once(':')
                && name.eq_ignore_ascii_case("range")
            {
                range = Some(value.trim().to_string());
            }
        }
        self.log.lock().unwrap().push(Hit { path: path.clone(), range: range.clone() });

        let (data, mode, hit) = {
            let mut routes = self.routes.lock().unwrap();
            match routes.get_mut(&path) {
                Some(r) => {
                    r.hits += 1;
                    (r.data.clone(), r.mode.clone(), r.hits)
                }
                None => return respond(stream, "404 Not Found", &[], &[], None),
            }
        };
        let len = data.len();
        let offset = match (&mode, &range) {
            (Mode::IgnoreRange, _) | (_, None) => None,
            (_, Some(r)) => r.strip_prefix("bytes=").and_then(|r| r.split_once('-')).and_then(|(n, _)| n.parse::<usize>().ok()),
        };
        // Only `bytes=N-` and `bytes=0-0` are ever sent; honour the end of the
        // latter so probes stay one byte.
        let end = match (&mode, &range) {
            (Mode::IgnoreRange, _) | (_, None) => None,
            (_, Some(r)) => r.rsplit_once('-').and_then(|(_, e)| e.parse::<usize>().ok()),
        };
        match mode {
            Mode::Status(code) => respond(stream, &format!("{code} Test"), &[], &[], None),
            Mode::Reject416 if offset.is_some() => {
                respond(stream, "416 Range Not Satisfiable", &[format!("Content-Range: bytes */{len}")], &[], None)
            }
            _ => {
                let (status, headers, body) = match offset {
                    Some(o) if o >= len => {
                        return respond(stream, "416 Range Not Satisfiable", &[format!("Content-Range: bytes */{len}")], &[], None);
                    }
                    Some(o) => {
                        let last = end.map_or(len - 1, |e| e.min(len - 1));
                        ("206 Partial Content", vec![format!("Content-Range: bytes {o}-{last}/{len}")], &data[o..=last])
                    }
                    None => ("200 OK", vec![], &data[..]),
                };
                let pacing = match mode {
                    Mode::TruncateOnce(n) if hit == 1 => Some((n.min(body.len()), usize::MAX, Duration::ZERO)),
                    Mode::Slow { chunk, delay } => Some((body.len(), chunk, delay)),
                    _ => None,
                };
                respond(stream, status, &headers, body, pacing)
            }
        }
    }
}

/// Writes a response. `pacing` = (bytes to actually send, chunk, delay).
fn respond(
    mut stream: TcpStream,
    status: &str,
    headers: &[String],
    body: &[u8],
    pacing: Option<(usize, usize, Duration)>,
) -> std::io::Result<()> {
    let mut head = format!("HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n", body.len());
    for h in headers {
        head.push_str(h);
        head.push_str("\r\n");
    }
    head.push_str("\r\n");
    stream.write_all(head.as_bytes())?;
    let (send, chunk, delay) = pacing.unwrap_or((body.len(), usize::MAX, Duration::ZERO));
    for piece in body[..send].chunks(chunk.max(1)) {
        stream.write_all(piece)?;
        stream.flush()?;
        if !delay.is_zero() {
            thread::sleep(delay);
        }
    }
    Ok(())
}

/// Deterministic pseudo-random bytes.
pub fn bytes(len: usize, seed: u64) -> Vec<u8> {
    let mut x = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
    (0..len)
        .map(|_| {
            x = x.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            (x >> 56) as u8
        })
        .collect()
}

/// A model entry over `files`, served from `sources` (base URLs).
pub fn model(id: &str, files: &[(&str, &[u8])], sources: Vec<String>) -> ModelEntry {
    ModelEntry {
        id: id.into(),
        kind: ModelKind::Asr,
        name: id.into(),
        license: "MIT".into(),
        attribution: None,
        homepage: None,
        required: false,
        available: true,
        recommended: false,
        retired: false,
        sources,
        files: files
            .iter()
            .map(|(path, data)| ModelFile {
                path: (*path).into(),
                size: data.len() as u64,
                sha256: hex::encode(Sha256::digest(data)),
            })
            .collect(),
        archive: None,
        cleanup: None,
    }
}

/// Options for talking to the local plain-http server.
pub fn local_http() -> HttpOptions {
    HttpOptions { https_only: false, idle_timeout: Duration::from_secs(5), ..HttpOptions::default() }
}
