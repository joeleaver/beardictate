//! Talking to the running app from the command line: `beardictate --toggle`
//! starts or stops dictation, so any desktop's own shortcut settings can
//! drive it. The same socket keeps a second copy from starting.
//!
//! A Unix socket in `$XDG_RUNTIME_DIR` (or the temp directory), or on
//! Windows a named pipe ([`pipe`]), one line per request.

#[cfg(not(windows))]
use std::io;
#[cfg(unix)]
use std::io::{BufRead, BufReader, Write};
#[cfg(unix)]
use std::os::unix::net::{UnixListener, UnixStream};
#[cfg(unix)]
use std::path::PathBuf;
#[cfg(unix)]
use std::thread;

use crossbeam_channel::Sender;

use crate::engine::Command;

#[cfg(windows)]
mod pipe;
#[cfg(windows)]
pub use pipe::{listen, send};

/// A request from the command line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Request {
    Toggle,
    /// "Are you running?"
    Ping,
}

impl Request {
    fn as_str(self) -> &'static str {
        match self {
            Request::Toggle => "toggle",
            Request::Ping => "ping",
        }
    }

    fn parse(line: &str) -> Option<Request> {
        match line.trim() {
            "toggle" => Some(Request::Toggle),
            "ping" => Some(Request::Ping),
            _ => None,
        }
    }
}

/// The reply to one request line.
fn answer(line: &str, engine: &Sender<Command>) -> &'static str {
    match Request::parse(line) {
        Some(Request::Toggle) => {
            let _ = engine.send(Command::Toggle);
            "ok"
        }
        Some(Request::Ping) => "ok",
        None => "unknown request",
    }
}

#[cfg(unix)]
fn socket_path() -> PathBuf {
    let dir = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    dir.join("beardictate.sock")
}

/// Sends `request` to a running app. An error means none is running (or it
/// didn't answer).
#[cfg(unix)]
pub fn send(request: Request) -> io::Result<()> {
    let mut stream = UnixStream::connect(socket_path())?;
    writeln!(stream, "{}", request.as_str())?;
    let mut reply = String::new();
    BufReader::new(stream).read_line(&mut reply)?;
    if reply.trim() == "ok" { Ok(()) } else { Err(io::Error::other(format!("unexpected reply {reply:?}"))) }
}

#[cfg(not(any(unix, windows)))]
pub fn send(_request: Request) -> io::Result<()> {
    Err(io::Error::new(io::ErrorKind::Unsupported, "not supported on this platform yet"))
}

/// Listens for requests, forwarding them to the engine. Call only after
/// [`send`]`(Ping)` failed, i.e. no other copy is running: a socket left by
/// a crashed one is replaced.
#[cfg(unix)]
pub fn listen(engine: Sender<Command>) -> io::Result<()> {
    let path = socket_path();
    let _ = std::fs::remove_file(&path);
    let listener = UnixListener::bind(&path)?;
    thread::Builder::new().name("bd-control".into()).spawn(move || {
        for stream in listener.incoming().flatten() {
            let mut line = String::new();
            let mut reader = BufReader::new(&stream);
            if reader.read_line(&mut line).is_err() {
                continue;
            }
            let _ = writeln!(&stream, "{}", answer(&line, &engine));
        }
    })?;
    Ok(())
}

#[cfg(not(any(unix, windows)))]
pub fn listen(_engine: Sender<Command>) -> io::Result<()> {
    Ok(())
}
