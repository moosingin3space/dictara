//! Paste injection for Wayland via the XDG RemoteDesktop + Clipboard portals.
//!
//! Wayland compositors don't let arbitrary clients synthesize input or own the
//! clipboard directly, and the privileged `ext-/wlr-data-control` protocols are
//! withheld from sandboxed (Flatpak) clients — so `arboard`'s Wayland backend
//! can't reach the clipboard from inside the sandbox. We therefore go entirely
//! through portals: a `RemoteDesktop` session provides the Ctrl+V keystroke, and
//! the `Clipboard` portal — which rides on that same session — lets us own the
//! selection and hand the transcribed text to the focused app when it pastes.
//!
//! The first use triggers the compositor's permission dialog; we request
//! `PersistMode::ExplicitlyRevoked` and persist the returned restore token in
//! the keychain so later launches re-establish the session without re-prompting.

use ashpd::desktop::{
    clipboard::{Clipboard, RequestClipboardOptions, SetSelectionOptions},
    remote_desktop::{
        DeviceType, KeyState, NotifyKeyboardKeysymOptions, RemoteDesktop, SelectDevicesOptions,
        StartOptions,
    },
    CreateSessionOptions, PersistMode, Session,
};
use futures_util::StreamExt;
use log::{info, warn};
use std::io::Write;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::keychain::{self, SystemAccount};

/// X11 keysym values, which is what NotifyKeyboardKeysym expects
const XK_CONTROL_L: i32 = 0xffe3;
const XK_LOWERCASE_V: i32 = 0x0076;

/// Delay between modifier and letter events, mirroring the enigo path
const KEY_EVENT_DELAY: Duration = Duration::from_millis(25);

/// MIME types we advertise for the transcribed text, most specific first.
const CLIPBOARD_MIME_TYPES: &[&str] = &["text/plain;charset=utf-8", "text/plain", "UTF8_STRING"];

#[derive(Debug, thiserror::Error)]
pub enum PortalPasteError {
    #[error("RemoteDesktop portal unavailable: {0}")]
    Unavailable(ashpd::Error),
    #[error("RemoteDesktop portal request failed (permission denied?): {0}")]
    Portal(#[from] ashpd::Error),
    #[error("Failed to create tokio runtime for portal connection: {0}")]
    Runtime(#[from] std::io::Error),
}

/// A live RemoteDesktop portal session (with clipboard access) plus the runtime
/// that keeps its D-Bus connection serviced between pastes.
struct PortalPaster {
    runtime: tokio::runtime::Runtime,
    remote_desktop: RemoteDesktop,
    clipboard: Clipboard,
    session: Arc<Session<RemoteDesktop>>,
    /// Bytes we currently own on the clipboard, served on `SelectionTransfer`.
    /// Shared with the background serving task.
    current: Arc<Mutex<Vec<u8>>>,
}

static PASTER: Mutex<Option<PortalPaster>> = Mutex::new(None);

impl PortalPaster {
    fn connect() -> Result<Self, PortalPasteError> {
        // zbus spawns background tasks on this runtime, so it must stay alive
        // (and running) for as long as the portal session is used.
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .thread_name("wayland-paste-portal")
            .enable_all()
            .build()?;

        let restore_token =
            keychain::load_system_secret(SystemAccount::WaylandRemoteDesktopRestoreToken)
                .unwrap_or_else(|e| {
                    warn!("Failed to load portal restore token from keychain: {}", e);
                    None
                });

        let current = Arc::new(Mutex::new(Vec::new()));

        let (remote_desktop, clipboard, session, new_token) = runtime.block_on(async {
            let remote_desktop = RemoteDesktop::new()
                .await
                .map_err(PortalPasteError::Unavailable)?;

            let session = remote_desktop
                .create_session(CreateSessionOptions::default())
                .await?;

            let mut options = SelectDevicesOptions::default()
                .set_devices(ashpd::enumflags2::BitFlags::from(DeviceType::Keyboard))
                .set_persist_mode(PersistMode::ExplicitlyRevoked);
            if let Some(token) = restore_token.as_deref() {
                options = options.set_restore_token(token);
            }
            remote_desktop.select_devices(&session, options).await?;

            // Clipboard access must be requested *before* the session starts.
            let clipboard = Clipboard::new()
                .await
                .map_err(PortalPasteError::Unavailable)?;
            clipboard
                .request(&session, RequestClipboardOptions::default())
                .await?;

            // This is the point where the compositor shows its permission
            // dialog (unless a valid restore token skipped it).
            let new_token = remote_desktop
                .start(&session, None, StartOptions::default())
                .await?
                .response()?
                .restore_token()
                .map(str::to_owned);

            // Serve the selection to whoever pastes it for the session's life.
            // The task owns its own Clipboard proxy (the signal stream borrows
            // it, so it can't share ours); it starts before any paste can set a
            // selection, so no transfer request is missed.
            let session = Arc::new(session);
            tokio::spawn(serve_selection(session.clone(), current.clone()));

            Ok::<_, PortalPasteError>((remote_desktop, clipboard, session, new_token))
        })?;

        // Tokens rotate on every session start; always persist the new one
        if let Some(token) = new_token {
            if let Err(e) = keychain::save_system_secret(
                SystemAccount::WaylandRemoteDesktopRestoreToken,
                &token,
            ) {
                warn!("Failed to save portal restore token to keychain: {}", e);
            }
        }

        info!("RemoteDesktop + Clipboard portal session established for paste injection");
        Ok(Self {
            runtime,
            remote_desktop,
            clipboard,
            session,
            current,
        })
    }

    /// Put `text` on the clipboard (as the portal selection) and press Ctrl+V.
    /// The focused app then requests the data, which the background task serves.
    fn paste(&self, text: &str) -> Result<(), PortalPasteError> {
        *self.current.lock().unwrap() = text.as_bytes().to_vec();
        self.runtime.block_on(async {
            self.clipboard
                .set_selection(
                    &self.session,
                    SetSelectionOptions::default().set_mime_types(CLIPBOARD_MIME_TYPES),
                )
                .await?;
            // Let the compositor register us as the selection owner before the
            // paste keystroke asks for the data.
            tokio::time::sleep(KEY_EVENT_DELAY).await;
            self.send_ctrl_v().await
        })
    }

    async fn notify_keysym(&self, keysym: i32, state: KeyState) -> Result<(), ashpd::Error> {
        self.remote_desktop
            .notify_keyboard_keysym(
                &self.session,
                keysym,
                state,
                NotifyKeyboardKeysymOptions::default(),
            )
            .await
    }

    async fn send_ctrl_v(&self) -> Result<(), PortalPasteError> {
        self.notify_keysym(XK_CONTROL_L, KeyState::Pressed).await?;
        tokio::time::sleep(KEY_EVENT_DELAY).await;
        self.notify_keysym(XK_LOWERCASE_V, KeyState::Pressed)
            .await?;
        self.notify_keysym(XK_LOWERCASE_V, KeyState::Released)
            .await?;
        tokio::time::sleep(KEY_EVENT_DELAY).await;
        self.notify_keysym(XK_CONTROL_L, KeyState::Released).await?;
        Ok(())
    }
}

/// Background task: whenever the compositor asks for our selection data (the
/// focused app is pasting), write the current bytes to the pipe it hands us.
async fn serve_selection(session: Arc<Session<RemoteDesktop>>, current: Arc<Mutex<Vec<u8>>>) {
    let clipboard = match Clipboard::new().await {
        Ok(clipboard) => clipboard,
        Err(e) => {
            warn!("Clipboard portal unavailable for serving selection: {e}");
            return;
        }
    };
    let transfer = match clipboard
        .receive_selection_transfer::<RemoteDesktop>()
        .await
    {
        Ok(transfer) => transfer,
        Err(e) => {
            warn!("Failed to subscribe to clipboard SelectionTransfer: {e}");
            return;
        }
    };
    futures_util::pin_mut!(transfer);
    while let Some((_session, mime_type, serial)) = transfer.next().await {
        let data = current.lock().unwrap().clone();
        let wrote = match clipboard.selection_write(&session, serial).await {
            Ok(fd) => match write_fd(fd, &data) {
                Ok(()) => true,
                Err(e) => {
                    warn!("Failed writing clipboard data ({mime_type}): {e}");
                    false
                }
            },
            Err(e) => {
                warn!("SelectionWrite failed: {e}");
                false
            }
        };
        if let Err(e) = clipboard
            .selection_write_done(&session, serial, wrote)
            .await
        {
            warn!("SelectionWriteDone failed: {e}");
        }
    }
    info!("Clipboard selection-transfer stream ended; session closed");
}

/// Write `data` to the portal-provided pipe fd, then close it (File drop).
fn write_fd(fd: ashpd::zvariant::OwnedFd, data: &[u8]) -> std::io::Result<()> {
    let mut file = std::fs::File::from(std::os::fd::OwnedFd::from(fd));
    file.write_all(data)?;
    file.flush()
}

/// Put `text` on the clipboard via the portal and paste it with Ctrl+V,
/// establishing (and caching) the portal session on first use.
pub fn paste_text(text: &str) -> Result<(), PortalPasteError> {
    let mut guard = PASTER.lock().unwrap();

    if guard.is_none() {
        *guard = Some(PortalPaster::connect()?);
    }

    match guard.as_ref().unwrap().paste(text) {
        Ok(()) => Ok(()),
        Err(e) => {
            // The session may have been revoked out from under us (e.g. via
            // the desktop's settings); rebuild it once and retry.
            warn!("Portal paste failed ({}); reconnecting and retrying", e);
            *guard = None;
            let paster = PortalPaster::connect()?;
            let result = paster.paste(text);
            *guard = Some(paster);
            result
        }
    }
}
