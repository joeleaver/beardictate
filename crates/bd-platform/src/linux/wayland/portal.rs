//! The xdg-desktop-portal RemoteDesktop handshake that grants keyboard
//! access and hands us a libei socket.
//!
//! The first `Start` shows the compositor's permission dialog. We ask for
//! persistence ("until revoked"); the portal then returns a restore token,
//! and presenting it next time skips the dialog (KDE Plasma 6 and GNOME 46+
//! honour it; it is single-use, so every start yields a fresh one).

use std::os::fd::OwnedFd;

use ashpd::desktop::remote_desktop::{
    ConnectToEISOptions, DeviceType, RemoteDesktop, SelectDevicesOptions, StartOptions,
};
use ashpd::desktop::{CreateSessionOptions, PersistMode, Session};
use reis::enumflags2::BitFlags;

use crate::error::PlatformError;

/// Keeps the portal session alive; dropping it ends keyboard access.
struct PortalSession {
    proxy: Option<RemoteDesktop>,
    session: Option<Session<RemoteDesktop>>,
}

impl Drop for PortalSession {
    /// Ends the session explicitly rather than waiting for the D-Bus
    /// connection to go away.
    fn drop(&mut self) {
        if let Some(session) = self.session.take()
            && let Err(e) = pollster::block_on(session.close())
        {
            tracing::debug!(error = %e, "closing the portal session failed");
        }
        drop(self.proxy.take());
    }
}

/// A granted libei connection.
pub(crate) struct Connected {
    pub eis: OwnedFd,
    /// New restore token to persist, if the portal issued one.
    pub restore_token: Option<String>,
    /// Whatever must stay alive for the grant to last (the portal session).
    pub keepalive: Box<dyn Send>,
}

fn portal(what: &str) -> impl Fn(ashpd::Error) -> PlatformError + '_ {
    move |e| match e {
        ashpd::Error::Response(ashpd::desktop::ResponseError::Cancelled) => PlatformError::Portal(
            "keyboard access was declined in the remote-desktop permission dialog".into(),
        ),
        e => PlatformError::Portal(format!("{what}: {e}")),
    }
}

/// Runs CreateSession → SelectDevices(keyboard) → Start → ConnectToEIS.
/// Blocks until the user answers the dialog, if one is shown.
pub(crate) fn start(restore_token: Option<&str>) -> Result<Connected, PlatformError> {
    pollster::block_on(async {
        let proxy = RemoteDesktop::new().await.map_err(portal("connecting"))?;
        if proxy.version() < 2 {
            return Err(PlatformError::Portal(format!(
                "the RemoteDesktop portal is version {}, libei needs version 2 \
                 (xdg-desktop-portal 1.18+ with a KDE or GNOME backend)",
                proxy.version()
            )));
        }
        let session = proxy
            .create_session(CreateSessionOptions::default())
            .await
            .map_err(portal("CreateSession"))?;
        let options = SelectDevicesOptions::default()
            .set_devices(BitFlags::from(DeviceType::Keyboard))
            .set_persist_mode(PersistMode::ExplicitlyRevoked)
            .set_restore_token(restore_token);
        proxy
            .select_devices(&session, options)
            .await
            .map_err(portal("SelectDevices"))?
            .response()
            .map_err(portal("SelectDevices"))?;
        let selected = proxy
            .start(&session, None, StartOptions::default())
            .await
            .map_err(portal("Start"))?
            .response()
            .map_err(portal("Start"))?;
        if !selected.devices().contains(DeviceType::Keyboard) {
            return Err(PlatformError::Portal(
                "keyboard access was not granted".into(),
            ));
        }
        let restore_token = selected.restore_token().map(str::to_owned);
        let eis = proxy
            .connect_to_eis(&session, ConnectToEISOptions::default())
            .await
            .map_err(portal("ConnectToEIS"))?;
        Ok(Connected {
            eis,
            restore_token,
            keepalive: Box::new(PortalSession {
                proxy: Some(proxy),
                session: Some(session),
            }),
        })
    })
}
