//! Linux hotkey backend using the XDG GlobalShortcuts portal.
//!
//! Wayland compositors don't allow global key grabbing, so shortcuts go
//! through `org.freedesktop.portal.GlobalShortcuts`: we register named
//! shortcuts with the portal and receive `Activated`/`Deactivated` signals
//! when the user presses whatever trigger the compositor has bound to them.
//!
//! Consequences compared to the macOS CGEvent-tap backend:
//! - The trigger key is *not* swallowed; it also reaches the focused app.
//! - We never see raw key events, so in-app "press your keys" capture is
//!   impossible — binding happens in the compositor's own dialog
//!   (`ConfigureShortcuts`, or the dialog shown on `BindShortcuts`).
//!
//! Compositor differences worth knowing:
//! - KDE (portal v2) shows a dialog on `BindShortcuts` where the user picks
//!   the key, remembers bindings per application, and supports
//!   `ConfigureShortcuts` for later changes — issued on the *existing* session,
//!   so reconfiguring never recreates anything.
//! - GNOME (portal v1) has no `ConfigureShortcuts`. Its backend
//!   (gnome-control-center) shows an "Add/Cancel" dialog that only accepts the
//!   app's `preferred_trigger` — so a preferred trigger is mandatory or nothing
//!   binds. `BindShortcuts` is allowed only once per session, and its dialog
//!   renders blank on gnome-control-center 50.x, so we do *not* try to re-open
//!   it; later changes happen in Settings → Apps → <app>. The portal version
//!   is surfaced (`supports_configure`) so callers can branch on this.

use ashpd::desktop::{
    global_shortcuts::{
        BindShortcutsOptions, ConfigureShortcutsOptions, GlobalShortcuts, ListShortcutsOptions,
        NewShortcut,
    },
    CreateSessionOptions, Session,
};
pub use ashpd::WindowIdentifier;
use futures_util::StreamExt;
use log::{error, info, warn};
use std::thread;
use tokio::sync::{mpsc, oneshot};

use crate::GrabError;

/// A shortcut to register with the portal.
#[derive(Debug, Clone)]
pub struct PortalShortcutSpec {
    /// Application-chosen stable id (e.g. "push-to-record")
    pub id: String,
    /// User-visible description shown in the compositor's dialog
    pub description: String,
    /// Preferred trigger in XDG shortcuts syntax (e.g. "CTRL+SHIFT+d"),
    /// if one can be suggested. The compositor may ignore it.
    pub preferred_trigger: Option<String>,
}

/// A shortcut as currently bound by the compositor.
#[derive(Debug, Clone)]
pub struct BoundShortcut {
    pub id: String,
    pub description: String,
    /// Human-readable trigger (e.g. "Ctrl+Alt+D"), for display in settings
    pub trigger_description: String,
}

/// Activation events delivered for registered shortcuts.
#[derive(Debug, Clone)]
pub enum PortalShortcutEvent {
    /// The shortcut's trigger was pressed
    Activated { id: String },
    /// The shortcut's trigger was released
    Deactivated { id: String },
}

enum Command {
    /// List currently bound shortcuts
    List(oneshot::Sender<Result<Vec<BoundShortcut>, String>>),
    /// Open the compositor's configuration UI (portal v2 / KDE only)
    Configure {
        parent: Option<WindowIdentifier>,
        reply: oneshot::Sender<Result<(), String>>,
    },
}

/// Handle to a running GlobalShortcuts portal session.
///
/// Dropping the handle shuts the background thread down.
pub struct PortalShortcuts {
    cmd_tx: mpsc::Sender<Command>,
    /// Portal interface version reported by the compositor (KDE is v2, GNOME v1)
    version: u32,
    _thread: thread::JoinHandle<()>,
}

impl PortalShortcuts {
    /// Connect to the portal, register `specs` and start delivering events to
    /// `callback` from a background thread.
    ///
    /// Returns quickly after the portal session is created; the
    /// `BindShortcuts` request (which may pop up a compositor dialog) runs
    /// asynchronously so app startup is never blocked on user interaction.
    ///
    /// `parent` parents that dialog to the requesting window when available.
    /// `BindShortcuts` is issued once for the life of this session; later
    /// reconfiguration goes through [`configure`](Self::configure) on portal v2.
    pub fn start<F>(
        specs: Vec<PortalShortcutSpec>,
        parent: Option<WindowIdentifier>,
        callback: F,
    ) -> Result<Self, GrabError>
    where
        F: FnMut(PortalShortcutEvent) + Send + 'static,
    {
        let (cmd_tx, cmd_rx) = mpsc::channel(8);
        let (setup_tx, setup_rx) = std::sync::mpsc::channel();

        let thread = thread::Builder::new()
            .name("portal-shortcuts".into())
            .spawn(move || {
                // zbus needs a live runtime for its background tasks for as
                // long as the session exists
                let runtime = match tokio::runtime::Builder::new_multi_thread()
                    .worker_threads(1)
                    .enable_all()
                    .build()
                {
                    Ok(rt) => rt,
                    Err(e) => {
                        let _ = setup_tx.send(Err(GrabError::PortalUnavailable(format!(
                            "failed to create tokio runtime: {e}"
                        ))));
                        return;
                    }
                };
                runtime.block_on(run_session(specs, parent, callback, cmd_rx, setup_tx));
            })
            .map_err(|e| GrabError::PortalUnavailable(format!("failed to spawn thread: {e}")))?;

        // Bounded wait so a hung xdg-desktop-portal can't block app startup
        match setup_rx.recv_timeout(std::time::Duration::from_secs(10)) {
            Ok(Ok(version)) => Ok(Self {
                cmd_tx,
                version,
                _thread: thread,
            }),
            Ok(Err(e)) => Err(e),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => Err(GrabError::PortalUnavailable(
                "timed out waiting for the portal session".into(),
            )),
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => Err(
                GrabError::PortalUnavailable("portal thread exited during setup".into()),
            ),
        }
    }

    /// List the shortcuts as currently bound by the compositor.
    pub fn list(&self) -> Result<Vec<BoundShortcut>, String> {
        self.request(Command::List)
    }

    /// Portal interface version reported by the compositor.
    pub fn version(&self) -> u32 {
        self.version
    }

    /// Whether the compositor supports `ConfigureShortcuts` (portal v2, KDE).
    /// On v1 (GNOME) reconfiguration must happen in the system settings instead.
    pub fn supports_configure(&self) -> bool {
        self.version >= 2
    }

    /// Ask the compositor to show its shortcut-configuration UI, parented to
    /// `parent` when available. Only meaningful when [`supports_configure`] is
    /// true; on portal v1 the request errors (the interface has no such method).
    ///
    /// [`supports_configure`]: Self::supports_configure
    pub fn configure(&self, parent: Option<WindowIdentifier>) -> Result<(), String> {
        let (tx, rx) = oneshot::channel();
        self.cmd_tx
            .blocking_send(Command::Configure { parent, reply: tx })
            .map_err(|_| "portal shortcuts thread is not running".to_string())?;
        rx.blocking_recv()
            .map_err(|_| "portal shortcuts thread dropped the request".to_string())?
    }

    fn request<T>(
        &self,
        make: impl FnOnce(oneshot::Sender<Result<T, String>>) -> Command,
    ) -> Result<T, String> {
        let (tx, rx) = oneshot::channel();
        self.cmd_tx
            .blocking_send(make(tx))
            .map_err(|_| "portal shortcuts thread is not running".to_string())?;
        rx.blocking_recv()
            .map_err(|_| "portal shortcuts thread dropped the request".to_string())?
    }
}

fn to_new_shortcuts(specs: &[PortalShortcutSpec]) -> Vec<NewShortcut> {
    specs
        .iter()
        .map(|spec| {
            NewShortcut::new(spec.id.clone(), spec.description.clone())
                .preferred_trigger(spec.preferred_trigger.as_deref())
        })
        .collect()
}

fn to_bound(shortcut: &ashpd::desktop::global_shortcuts::Shortcut) -> BoundShortcut {
    BoundShortcut {
        id: shortcut.id().to_string(),
        description: shortcut.description().to_string(),
        trigger_description: shortcut.trigger_description().to_string(),
    }
}

async fn run_session<F>(
    specs: Vec<PortalShortcutSpec>,
    parent: Option<WindowIdentifier>,
    mut callback: F,
    mut cmd_rx: mpsc::Receiver<Command>,
    setup_tx: std::sync::mpsc::Sender<Result<u32, GrabError>>,
) where
    F: FnMut(PortalShortcutEvent) + Send + 'static,
{
    let unavailable = |e: &dyn std::fmt::Display| {
        GrabError::PortalUnavailable(format!(
            "GlobalShortcuts portal unavailable (compositor may not support it): {e}"
        ))
    };

    let global_shortcuts = match GlobalShortcuts::new().await {
        Ok(gs) => gs,
        Err(e) => {
            let _ = setup_tx.send(Err(unavailable(&e)));
            return;
        }
    };

    let version = global_shortcuts.version();
    info!("GlobalShortcuts portal version {version}");

    let session: Session<GlobalShortcuts> = match global_shortcuts
        .create_session(CreateSessionOptions::default())
        .await
    {
        Ok(s) => s,
        Err(e) => {
            let _ = setup_tx.send(Err(unavailable(&e)));
            return;
        }
    };

    // Subscribe to signals before binding so no activation can be missed
    let mut activated = match global_shortcuts.receive_activated().await {
        Ok(stream) => stream,
        Err(e) => {
            let _ = setup_tx.send(Err(unavailable(&e)));
            return;
        }
    };
    let mut deactivated = match global_shortcuts.receive_deactivated().await {
        Ok(stream) => stream,
        Err(e) => {
            let _ = setup_tx.send(Err(unavailable(&e)));
            return;
        }
    };

    // Setup succeeded; report back so app startup can continue. The bind
    // below may block on a compositor dialog for arbitrarily long.
    let _ = setup_tx.send(Ok(version));

    let bind_result = async {
        global_shortcuts
            .bind_shortcuts(
                &session,
                &to_new_shortcuts(&specs),
                parent.as_ref(),
                BindShortcutsOptions::default(),
            )
            .await?
            .response()
    }
    .await;
    match bind_result {
        Ok(response) => {
            let bound: Vec<BoundShortcut> = response.shortcuts().iter().map(to_bound).collect();
            info!("GlobalShortcuts bound: {:?}", bound);
        }
        Err(e) => {
            // Not fatal: the user may have dismissed the dialog. Shortcuts
            // stay inactive until bound via the configure dialog.
            warn!("BindShortcuts failed or was dismissed: {e}");
        }
    }

    loop {
        tokio::select! {
            event = activated.next() => {
                match event {
                    Some(activation) => callback(PortalShortcutEvent::Activated {
                        id: activation.shortcut_id().to_string(),
                    }),
                    None => {
                        error!("GlobalShortcuts Activated signal stream closed");
                        break;
                    }
                }
            }
            event = deactivated.next() => {
                match event {
                    Some(deactivation) => callback(PortalShortcutEvent::Deactivated {
                        id: deactivation.shortcut_id().to_string(),
                    }),
                    None => {
                        error!("GlobalShortcuts Deactivated signal stream closed");
                        break;
                    }
                }
            }
            cmd = cmd_rx.recv() => {
                match cmd {
                    Some(Command::List(reply)) => {
                        let result = match global_shortcuts
                            .list_shortcuts(&session, ListShortcutsOptions::default())
                            .await
                        {
                            Ok(request) => match request.response() {
                                Ok(list) => Ok(list.shortcuts().iter().map(to_bound).collect()),
                                Err(e) => Err(format!("ListShortcuts response failed: {e}")),
                            },
                            Err(e) => Err(format!("ListShortcuts failed: {e}")),
                        };
                        let _ = reply.send(result);
                    }
                    Some(Command::Configure { parent, reply }) => {
                        let result = global_shortcuts
                            .configure_shortcuts(
                                &session,
                                parent.as_ref(),
                                ConfigureShortcutsOptions::default(),
                            )
                            .await
                            .map_err(|e| format!("ConfigureShortcuts failed: {e}"));
                        let _ = reply.send(result);
                    }
                    None => {
                        info!("PortalShortcuts handle dropped; shutting down session");
                        break;
                    }
                }
            }
        }
    }
}
