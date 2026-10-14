//! One HTTP transfer of one file from one URL into its `.part` file.
//!
//! Handles the server side of resume: `206` with a checked `Content-Range`
//! continues the part, `200` means the server ignored our `Range` and we start
//! over, and `416` means our part doesn't fit the server's file, so it is
//! discarded and the request repeated without a range.

use std::fs::File;
use std::io::{self, ErrorKind, Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::sync::atomic::AtomicBool;

use sha2::{Digest, Sha256};
use ureq::Agent;
use ureq::http::{HeaderMap, StatusCode, header};

use crate::error::{Error, SourceError};
use crate::hash::CHUNK;
use crate::progress::{OnEvent, Phase, Reporter, check_cancel};

/// A failed attempt: either the source's fault (try the next one) or ours
/// (stop everything).
#[derive(Debug)]
pub(crate) enum Failure {
    Fatal(Error),
    Source(SourceError),
}

impl From<Error> for Failure {
    fn from(e: Error) -> Self {
        Failure::Fatal(e)
    }
}

impl From<SourceError> for Failure {
    fn from(e: SourceError) -> Self {
        Failure::Source(e)
    }
}

/// Everything [`receive`] needs besides the URL.
pub(crate) struct Target<'a> {
    pub part: &'a Path,
    pub size: u64,
    pub cancel: &'a AtomicBool,
    pub reporter: &'a Reporter<'a>,
}

/// Downloads the rest of the file at `url`, appending to the part file from
/// byte `offset` (whose first `offset` bytes are already in `hasher`). On
/// success the part holds exactly `size` bytes, fsynced; the caller checks
/// the hash. A [`SourceError::TooLong`] deletes the part; everything else
/// leaves it for a later resume.
pub(crate) fn receive(
    agent: &Agent,
    url: &str,
    mut offset: u64,
    hasher: &mut Sha256,
    target: &Target<'_>,
    on_event: OnEvent<'_>,
) -> Result<(), Failure> {
    let mut retried_after_416 = false;
    let response = loop {
        let mut request = agent.get(url).header(header::ACCEPT_ENCODING, "identity");
        if offset > 0 {
            request = request.header(header::RANGE, format!("bytes={offset}-"));
        }
        let response = request.call().map_err(map_ureq_error)?;
        match response.status() {
            StatusCode::PARTIAL_CONTENT => {
                check_partial(response.headers(), offset, target.size)?;
                break response;
            }
            StatusCode::OK => {
                if offset > 0 {
                    tracing::info!(url, offset, "server ignored Range; restarting from zero");
                    offset = 0;
                    *hasher = Sha256::new();
                }
                check_length(response.headers(), target.size)?;
                break response;
            }
            StatusCode::RANGE_NOT_SATISFIABLE if offset > 0 && !retried_after_416 => {
                tracing::info!(url, offset, "416 for our partial file; restarting from zero");
                retried_after_416 = true;
                offset = 0;
                *hasher = Sha256::new();
            }
            status => return Err(SourceError::HttpStatus(status.as_u16()).into()),
        }
    };

    let part = target.part;
    let mut file = open_part(part, offset).map_err(Error::io("open", part))?;
    let mut reader = response.into_body().into_reader();
    let mut buf = vec![0u8; CHUNK];
    let mut done = offset;
    loop {
        check_cancel(target.cancel)?;
        let n = match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if e.kind() == ErrorKind::Interrupted => continue,
            Err(e) if e.kind() == ErrorKind::UnexpectedEof => {
                file.sync_all().map_err(Error::io("sync", part))?;
                return Err(SourceError::Truncated { expected: target.size, received: done }.into());
            }
            Err(e) => {
                file.sync_all().map_err(Error::io("sync", part))?;
                return Err(map_io_error(e).into());
            }
        };
        if done + n as u64 > target.size {
            drop(file);
            std::fs::remove_file(part).map_err(Error::io("remove", part))?;
            return Err(SourceError::TooLong { expected: target.size }.into());
        }
        file.write_all(&buf[..n]).map_err(Error::io("write", part))?;
        hasher.update(&buf[..n]);
        done += n as u64;
        target.reporter.emit(on_event, Phase::Downloading, done);
    }
    file.sync_all().map_err(Error::io("sync", part))?;
    if done < target.size {
        return Err(SourceError::Truncated { expected: target.size, received: done }.into());
    }
    Ok(())
}

/// Checks that `url` resolves and serves a file of `expected_size` bytes,
/// fetching at most one byte. Returns whether the server honours ranges.
pub(crate) fn probe(agent: &Agent, url: &str, expected_size: u64) -> Result<bool, SourceError> {
    let response = agent
        .get(url)
        .header(header::ACCEPT_ENCODING, "identity")
        .header(header::RANGE, "bytes=0-0")
        .call()
        .map_err(map_ureq_error)?;
    match response.status() {
        StatusCode::PARTIAL_CONTENT => {
            let range = content_range(response.headers())?;
            if range.start != 0 || range.end != 0 {
                return Err(SourceError::BadContentRange(range.raw));
            }
            match range.total {
                Some(total) if total != expected_size => {
                    Err(SourceError::SizeMismatch { expected: expected_size, actual: total })
                }
                _ => Ok(true),
            }
        }
        StatusCode::OK => check_length(response.headers(), expected_size).map(|()| false),
        status => Err(SourceError::HttpStatus(status.as_u16())),
    }
}

/// Opens the part file positioned at `offset`, dropping anything past it.
fn open_part(path: &Path, offset: u64) -> io::Result<File> {
    let mut file = File::options().create(true).write(true).truncate(offset == 0).open(path)?;
    if offset > 0 {
        file.set_len(offset)?;
        file.seek(SeekFrom::Start(offset))?;
    }
    Ok(file)
}

struct ContentRange {
    start: u64,
    end: u64,
    /// None for `*` (unknown total).
    total: Option<u64>,
    raw: String,
}

/// Parses `Content-Range: bytes <start>-<end>/<total|*>`.
fn content_range(headers: &HeaderMap) -> Result<ContentRange, SourceError> {
    let raw = headers
        .get(header::CONTENT_RANGE)
        .map(|v| String::from_utf8_lossy(v.as_bytes()).into_owned())
        .unwrap_or_default();
    let bad = || SourceError::BadContentRange(raw.clone());
    let spec = raw.trim().strip_prefix("bytes ").ok_or_else(bad)?;
    let (range, total) = spec.split_once('/').ok_or_else(bad)?;
    let (start, end) = range.split_once('-').ok_or_else(bad)?;
    let start: u64 = start.trim().parse().map_err(|_| bad())?;
    let end: u64 = end.trim().parse().map_err(|_| bad())?;
    let total = match total.trim() {
        "*" => None,
        t => Some(t.parse().map_err(|_| bad())?),
    };
    if end < start {
        return Err(bad());
    }
    Ok(ContentRange { start, end, total, raw })
}

/// A 206 must be exactly the rest of the file from `offset`.
fn check_partial(headers: &HeaderMap, offset: u64, size: u64) -> Result<(), SourceError> {
    let range = content_range(headers)?;
    if let Some(total) = range.total
        && total != size
    {
        return Err(SourceError::SizeMismatch { expected: size, actual: total });
    }
    if range.start != offset || range.end + 1 != size {
        return Err(SourceError::BadContentRange(range.raw));
    }
    check_length(headers, size - offset)
}

/// If the server states a body length, it must be the one we expect.
fn check_length(headers: &HeaderMap, expected: u64) -> Result<(), SourceError> {
    let length = headers
        .get(header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.trim().parse::<u64>().ok());
    match length {
        Some(actual) if actual != expected => Err(SourceError::SizeMismatch { expected, actual }),
        _ => Ok(()),
    }
}

fn map_ureq_error(e: ureq::Error) -> SourceError {
    match e {
        ureq::Error::Timeout(_) => SourceError::Timeout,
        ureq::Error::Io(io) => map_io_error(io),
        other => SourceError::Network(other.to_string()),
    }
}

/// I/O errors from the connection. Reading the body, ureq wraps its own
/// errors (including timeouts) in `io::Error`, so unwrap those first.
fn map_io_error(e: io::Error) -> SourceError {
    if let Some(inner) = e.get_ref().and_then(|i| i.downcast_ref::<ureq::Error>()) {
        return match inner {
            ureq::Error::Timeout(_) => SourceError::Timeout,
            other => SourceError::Network(other.to_string()),
        };
    }
    match e.kind() {
        ErrorKind::TimedOut | ErrorKind::WouldBlock => SourceError::Timeout,
        _ => SourceError::Network(e.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ureq::http::HeaderValue;

    fn headers(range: &str, length: Option<&str>) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert(header::CONTENT_RANGE, HeaderValue::from_str(range).unwrap());
        if let Some(l) = length {
            h.insert(header::CONTENT_LENGTH, HeaderValue::from_str(l).unwrap());
        }
        h
    }

    #[test]
    fn partial_content_must_cover_the_rest_of_the_file() {
        assert!(check_partial(&headers("bytes 4-9/10", Some("6")), 4, 10).is_ok());
        assert!(check_partial(&headers("bytes 4-9/*", None), 4, 10).is_ok());
        assert!(matches!(
            check_partial(&headers("bytes 3-9/10", None), 4, 10),
            Err(SourceError::BadContentRange(_))
        ));
        assert!(matches!(
            check_partial(&headers("bytes 4-8/10", None), 4, 10),
            Err(SourceError::BadContentRange(_))
        ));
        assert_eq!(
            check_partial(&headers("bytes 4-10/11", None), 4, 10),
            Err(SourceError::SizeMismatch { expected: 10, actual: 11 })
        );
        assert_eq!(
            check_partial(&headers("bytes 4-9/10", Some("5")), 4, 10),
            Err(SourceError::SizeMismatch { expected: 6, actual: 5 })
        );
        for junk in ["", "bytes", "bytes 4-9", "items 4-9/10", "bytes 9-4/10", "bytes a-9/10"] {
            assert!(matches!(content_range(&headers(junk, None)), Err(SourceError::BadContentRange(_))), "{junk}");
        }
    }
}
