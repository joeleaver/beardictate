//! Clipboard for both Linux backends, through arboard (X11 selections, or
//! Wayland `wlr`/`ext` data-control).
//!
//! A snapshot keeps the single richest representation arboard can read, in
//! this order: file list, HTML (with its plain-text alternative), plain
//! text, image. Other MIME types the source app offered (e.g. a word
//! processor's native format) are not restored; pasting the restored
//! content then yields the HTML/text/image form.

use std::path::PathBuf;

use arboard::{ImageData, SetExtLinux};

use crate::error::PlatformError;

pub(crate) enum Snapshot {
    Empty,
    Files(Vec<PathBuf>),
    Html { html: String, alt: Option<String> },
    Text(String),
    Image(ImageData<'static>),
}

/// Lazily created so a missing clipboard doesn't stop typing.
#[derive(Default)]
pub(crate) struct Clipboard {
    inner: Option<arboard::Clipboard>,
}

fn err(e: arboard::Error) -> PlatformError {
    PlatformError::Clipboard(e.to_string())
}

/// `Ok(None)` for "this kind of content isn't there".
fn optional<T>(r: Result<T, arboard::Error>) -> Result<Option<T>, PlatformError> {
    match r {
        Ok(v) => Ok(Some(v)),
        Err(arboard::Error::ContentNotAvailable | arboard::Error::ConversionFailure) => Ok(None),
        Err(e) => Err(err(e)),
    }
}

impl Clipboard {
    /// The arboard handle. It must stay alive while our content is on the
    /// clipboard: on X11 its thread serves paste requests.
    fn get(&mut self) -> Result<&mut arboard::Clipboard, PlatformError> {
        if self.inner.is_none() {
            self.inner = Some(arboard::Clipboard::new().map_err(err)?);
        }
        Ok(self.inner.as_mut().expect("just set"))
    }

    pub(crate) fn snapshot(&mut self) -> Result<Snapshot, PlatformError> {
        let cb = self.get()?;
        if let Some(files) = optional(cb.get().file_list())?
            && !files.is_empty()
        {
            return Ok(Snapshot::Files(files));
        }
        if let Some(html) = optional(cb.get().html())? {
            let alt = optional(cb.get_text())?;
            return Ok(Snapshot::Html { html, alt });
        }
        if let Some(text) = optional(cb.get_text())? {
            return Ok(Snapshot::Text(text));
        }
        if let Some(image) = optional(cb.get_image())? {
            return Ok(Snapshot::Image(image));
        }
        Ok(Snapshot::Empty)
    }

    /// `transient` marks it for clipboard managers (Klipper honours
    /// `x-kde-passwordManagerHint`) so it doesn't land in history.
    pub(crate) fn set_text(&mut self, text: &str, transient: bool) -> Result<(), PlatformError> {
        let cb = self.get()?;
        if transient {
            cb.set().exclude_from_history().text(text).map_err(err)
        } else {
            cb.set_text(text).map_err(err)
        }
    }

    pub(crate) fn holds_text(&mut self, text: &str) -> Result<bool, PlatformError> {
        Ok(optional(self.get()?.get_text())?.as_deref() == Some(text))
    }

    pub(crate) fn restore(&mut self, snapshot: Snapshot) -> Result<(), PlatformError> {
        let cb = self.get()?;
        let set = cb.set().exclude_from_history();
        match snapshot {
            Snapshot::Empty => cb.clear().map_err(err),
            Snapshot::Files(files) => set.file_list(&files).map_err(err),
            Snapshot::Html { html, alt } => set.html(html, alt).map_err(err),
            Snapshot::Text(text) => set.text(text).map_err(err),
            Snapshot::Image(image) => set.image(image).map_err(err),
        }
    }
}

#[cfg(test)]
mod live {
    use super::*;

    /// Round-trips the clipboard on a **nested** compositor or X server.
    /// Refuses to run unless `BD_TEST_NESTED_CLIPBOARD=1` and the display
    /// variables point somewhere other than the usual live session names.
    #[test]
    #[ignore = "needs a nested Wayland compositor or X server"]
    fn live_clipboard_roundtrip() {
        assert_eq!(
            std::env::var("BD_TEST_NESTED_CLIPBOARD").as_deref(),
            Ok("1")
        );
        let wl = std::env::var("WAYLAND_DISPLAY").unwrap_or_default();
        let x = std::env::var("DISPLAY").unwrap_or_default();
        assert!(
            wl != "wayland-0" && x != ":0",
            "refusing to touch a live session clipboard"
        );

        let mut cb = Clipboard::default();
        cb.set_text("before “x”", false).unwrap();
        let snap = cb.snapshot().unwrap();
        assert!(matches!(&snap, Snapshot::Text(t) if t == "before “x”"));
        cb.set_text("dictated", true).unwrap();
        assert!(cb.holds_text("dictated").unwrap());
        cb.restore(snap).unwrap();
        assert!(cb.holds_text("before “x”").unwrap());
        cb.restore(Snapshot::Empty).unwrap();
        assert!(!cb.holds_text("before “x”").unwrap());
    }
}
