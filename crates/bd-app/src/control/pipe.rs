//! The control channel on Windows: a named pipe per user,
//! `\\.\pipe\beardictate-<user>`. The first pipe instance is created with
//! `FILE_FLAG_FIRST_PIPE_INSTANCE`, so a second copy of the app can't take
//! the name, and remote clients are refused.

use std::fs::{File, OpenOptions};
use std::io::{self, BufRead, BufReader, Write};
use std::os::windows::io::{FromRawHandle, RawHandle};
use std::thread;

use crossbeam_channel::Sender;
use windows_sys::Win32::Foundation::{ERROR_PIPE_BUSY, ERROR_PIPE_CONNECTED, GetLastError, INVALID_HANDLE_VALUE};
use windows_sys::Win32::Storage::FileSystem::{FILE_FLAG_FIRST_PIPE_INSTANCE, PIPE_ACCESS_DUPLEX};
use windows_sys::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE,
    PIPE_UNLIMITED_INSTANCES, PIPE_WAIT, WaitNamedPipeW,
};

use super::{Request, answer};
use crate::engine::Command;

fn pipe_name() -> String {
    let user: String = std::env::var("USERNAME")
        .unwrap_or_default()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    format!(r"\\.\pipe\beardictate-{user}")
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

/// Sends `request` to a running app. An error means none is running (or it
/// didn't answer).
pub fn send(request: Request) -> io::Result<()> {
    let name = pipe_name();
    let open = || OpenOptions::new().read(true).write(true).open(&name);
    let mut pipe = match open() {
        Err(e) if e.raw_os_error() == Some(ERROR_PIPE_BUSY as i32) => {
            // Every instance is serving someone; wait for one to come free.
            // SAFETY: a NUL-terminated UTF-16 string that outlives the call.
            unsafe { WaitNamedPipeW(wide(&name).as_ptr(), 2000) };
            open()?
        }
        other => other?,
    };
    writeln!(pipe, "{}", request.as_str())?;
    let mut reply = String::new();
    BufReader::new(pipe).read_line(&mut reply)?;
    if reply.trim() == "ok" { Ok(()) } else { Err(io::Error::other(format!("unexpected reply {reply:?}"))) }
}

/// Creates one instance of the pipe, waiting for no one yet.
fn create(first: bool) -> io::Result<File> {
    let name = wide(&pipe_name());
    let flags = PIPE_ACCESS_DUPLEX | if first { FILE_FLAG_FIRST_PIPE_INSTANCE } else { 0 };
    // SAFETY: `name` is NUL-terminated and outlives the call; null security
    // attributes mean the default DACL (the creator, SYSTEM and admins).
    let handle = unsafe {
        CreateNamedPipeW(
            name.as_ptr(),
            flags,
            PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
            PIPE_UNLIMITED_INSTANCES,
            512,
            512,
            0,
            std::ptr::null(),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: a fresh handle that nothing else owns.
    Ok(unsafe { File::from_raw_handle(handle as RawHandle) })
}

/// Blocks until a client connects to `pipe`.
fn accept(pipe: &File) -> io::Result<()> {
    use std::os::windows::io::AsRawHandle;
    // SAFETY: a valid pipe handle, synchronous (no OVERLAPPED).
    let ok = unsafe { ConnectNamedPipe(pipe.as_raw_handle() as _, std::ptr::null_mut()) };
    // A client that connected between create and connect is fine too.
    if ok != 0 || unsafe { GetLastError() } == ERROR_PIPE_CONNECTED { Ok(()) } else { Err(io::Error::last_os_error()) }
}

/// Listens for requests, forwarding them to the engine. Fails if another
/// copy of the app already owns the pipe.
pub fn listen(engine: Sender<Command>) -> io::Result<()> {
    let first = create(true)?;
    thread::Builder::new().name("bd-control".into()).spawn(move || {
        let mut next = Some(first);
        loop {
            let pipe = match next.take().map_or_else(|| create(false), Ok) {
                Ok(p) => p,
                Err(e) => {
                    tracing::warn!("control pipe: {e}");
                    return;
                }
            };
            if let Err(e) = accept(&pipe) {
                tracing::warn!("control pipe: {e}");
                continue;
            }
            let mut line = String::new();
            let mut reader = BufReader::new(&pipe);
            if reader.read_line(&mut line).is_ok() {
                let mut writer = &pipe;
                let _ = writeln!(writer, "{}", answer(&line, &engine));
                // Let the client read the reply before the handle closes.
                let _ = writer.flush();
                let _ = pipe.sync_all();
            }
        }
    })?;
    Ok(())
}
