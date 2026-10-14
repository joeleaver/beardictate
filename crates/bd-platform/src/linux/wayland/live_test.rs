//! Live libei test against a **nested, isolated** KWin. Ignored by default.
//!
//! Start `kwin_wayland --virtual --xwayland` under its own `dbus-run-session`
//! with scratch XDG dirs, open a client in it that writes typed text to a
//! file (e.g. `xterm -e sh -c 'stty -icanon -echo; cat > OUT'` on its
//! Xwayland display), then run:
//!
//! ```text
//! BD_TEST_NESTED_DBUS=<that bus address> BD_TEST_OUT=OUT \
//!     cargo test -p bd-platform -- --ignored live_libei
//! ```
//!
//! The test asks KWin's `org.kde.KWin.EIS.RemoteDesktop.connectToEIS` for a
//! keyboard-only EIS socket (what the KDE portal backend does after the
//! user approves) and refuses to run if that bus also hosts plasmashell,
//! i.e. if it is a real desktop session.
//!
//! `live_wayland_end_to_end` additionally drives the whole injector (worker
//! thread, data-control clipboard, Auto fallback to paste) and needs
//! `WAYLAND_DISPLAY`/`XDG_RUNTIME_DIR` of the nested compositor plus an
//! xterm whose Ctrl+Shift+V pastes CLIPBOARD
//! (`-xrm 'XTerm*VT100.translations: #override Ctrl Shift <Key>V:
//! insert-selection(CLIPBOARD)'`), started with `stty raw -echo`.

use std::os::fd::OwnedFd;
use std::thread;
use std::time::Duration;

use std::sync::Arc;

use bd_core::InjectOutcome;

use super::Connected;
use super::session::EiSession;
use crate::config::{InjectorConfig, NewlineKey};
use crate::config::{PasteChord, Strategy};
use crate::driver;
use crate::driver::Typed;
use crate::linux::LinuxBackend;
use crate::linux::keysym::chord_stroke;
use crate::text::tokenize;

/// Returns the EIS fd and the D-Bus connection, which must stay open: KWin
/// tears the EIS context down when the caller leaves the bus.
fn nested_eis(address: &str) -> (OwnedFd, zbus::Connection) {
    pollster::block_on(async {
        let conn = zbus::connection::Builder::address(address)
            .expect("bus address")
            .build()
            .await
            .expect("connect to the nested bus");
        let dbus = zbus::fdo::DBusProxy::new(&conn).await.unwrap();
        let names = dbus.list_names().await.unwrap();
        assert!(
            !names.iter().any(|n| n.as_str() == "org.kde.plasmashell"),
            "this bus hosts plasmashell: refusing to inject into a real desktop"
        );
        let reply = conn
            .call_method(
                Some("org.kde.KWin"),
                "/org/kde/KWin/EIS/RemoteDesktop",
                Some("org.kde.KWin.EIS.RemoteDesktop"),
                "connectToEIS",
                // Same bit layout as the portal's DeviceType: 1 = keyboard.
                &1i32,
            )
            .await
            .expect("connectToEIS");
        let (fd, _cookie): (zbus::zvariant::OwnedFd, i32) = reply.body().deserialize().unwrap();
        (fd.into(), conn)
    })
}

#[test]
#[ignore = "needs a nested KWin; see module docs"]
fn live_libei() {
    let address = std::env::var("BD_TEST_NESTED_DBUS").expect("BD_TEST_NESTED_DBUS");
    let out = std::env::var("BD_TEST_OUT").expect("BD_TEST_OUT");
    let (fd, _bus) = nested_eis(&address);
    let mut ei = EiSession::connect(fd).expect("libei session");
    let settings = InjectorConfig {
        newline: NewlineKey::Enter,
        ..InjectorConfig::default()
    }
    .resolve(None);

    assert!(ei.wait_modifiers_released(Duration::from_secs(1)).unwrap());

    let typed = "Hello, World! @#$ ~`{}|<>?\n\tdone";
    assert_eq!(
        ei.type_tokens(&tokenize(typed), &settings).unwrap(),
        Typed::Done
    );

    // A US layout has no curly quotes; without ei_text that must be refused
    // up front, with nothing sent.
    match ei.type_tokens(&tokenize("x “quoted”"), &settings).unwrap() {
        Typed::Impossible { reason } => assert!(reason.contains("U+201C"), "{reason}"),
        other => panic!("expected Impossible, got {other:?}"),
    }

    // The paste chord must at least be sendable.
    assert_eq!(
        ei.send_stroke(chord_stroke(crate::PasteChord::CtrlV))
            .unwrap(),
        None
    );

    thread::sleep(Duration::from_millis(500));
    let got = std::fs::read(&out).unwrap();
    let got = String::from_utf8_lossy(&got).replace('\0', "");
    // Enter reaches the terminal as CR; Ctrl+V as SYN (0x16).
    assert_eq!(got, "Hello, World! @#$ ~`{}|<>?\r\tdone\u{16}");
}

#[test]
#[ignore = "needs a nested KWin; see module docs"]
fn live_wayland_end_to_end() {
    let address = std::env::var("BD_TEST_NESTED_DBUS").expect("BD_TEST_NESTED_DBUS");
    let out = std::env::var("BD_TEST_OUT").expect("BD_TEST_OUT");
    assert_ne!(std::env::var("WAYLAND_DISPLAY").as_deref(), Ok("wayland-0"));
    let config = InjectorConfig {
        strategy: Strategy::Auto,
        paste_chord: PasteChord::CtrlShiftV,
        ..InjectorConfig::default()
    };
    let connector: super::Connector = Arc::new(move |_token| {
        let (eis, bus) = nested_eis(&address);
        Ok(Connected {
            eis,
            restore_token: None,
            keepalive: Box::new(bus),
        })
    });
    let mut backend = LinuxBackend::wayland_with_connector(&config, connector);
    use crate::driver::Backend as _;
    backend
        .clipboard_set("user's own clipboard", false)
        .unwrap();

    let typed = driver::inject(&mut backend, &config, "Typed line.\n").unwrap();
    assert_eq!(typed, InjectOutcome::Typed);
    let pasted = driver::inject(&mut backend, &config, "“Curly,” she said—").unwrap();
    assert_eq!(pasted, InjectOutcome::Pasted);
    assert!(
        backend
            .clipboard_still_ours("user's own clipboard")
            .unwrap(),
        "clipboard restored"
    );

    thread::sleep(Duration::from_millis(500));
    let got = String::from_utf8_lossy(&std::fs::read(&out).unwrap()).replace('\0', "");
    assert_eq!(got, "Typed line.\r“Curly,” she said—");
}
