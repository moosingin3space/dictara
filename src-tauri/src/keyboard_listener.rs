use crate::config::ShortcutsConfig;
use crate::recording::{RecordingCommand, RecordingStateManager};
#[cfg(not(target_os = "linux"))]
use crate::shortcuts::events::KeyCaptureEvent;
#[cfg(not(target_os = "linux"))]
use dictara_keyboard::{grab, Event, EventType};
use log::{error, info};
#[cfg(not(target_os = "linux"))]
use std::collections::HashSet;
use std::sync::Arc;
#[cfg(not(target_os = "linux"))]
use std::thread::{self, JoinHandle};
use tauri::AppHandle;
#[cfg(not(target_os = "linux"))]
use tauri_specta::Event as EventTrait;
#[cfg(not(target_os = "linux"))]
use tokio::sync::mpsc;

/// Operating mode for the keyboard listener
#[cfg(not(target_os = "linux"))]
enum ListenerMode {
    /// Normal mode: match shortcuts and trigger recording
    Normal { shortcuts: ShortcutsConfig },
    /// Capture mode: emit key events to frontend for configuration
    Capture { app_handle: AppHandle },
}

/// Keyboard listener that detects key events and emits recording commands
#[cfg(not(target_os = "linux"))]
pub struct KeyListener {
    _thread_handle: Option<JoinHandle<()>>,
    mode_tx: mpsc::Sender<ListenerMode>, // Send mode updates to thread
}

/// Keyboard listener backed by the XDG GlobalShortcuts portal.
///
/// Unlike the macOS CGEvent-tap listener, this never sees raw key events:
/// the compositor owns the actual bindings (configured through its own
/// dialog) and only tells us when our named shortcuts activate/deactivate.
/// Capture mode is therefore unsupported; the frontend branches on
/// `get_shortcut_capture_capability` instead.
#[cfg(target_os = "linux")]
pub struct KeyListener {
    // The portal session lives for the whole run: `BindShortcuts` is issued once
    // at startup, and reconfiguration goes through `ConfigureShortcuts` on this
    // same session (portal v2 / KDE) or is redirected to the system settings
    // (portal v1 / GNOME) — neither recreates it, so no interior mutability.
    portal: Option<dictara_keyboard::linux::PortalShortcuts>,
}

/// Portal shortcut ids (stable; the compositor persists bindings by id)
#[cfg(target_os = "linux")]
pub const PUSH_TO_RECORD_SHORTCUT_ID: &str = "push-to-record";
#[cfg(target_os = "linux")]
pub const HANDS_FREE_SHORTCUT_ID: &str = "hands-free";

impl KeyListener {
    /// Check if any shortcut uses Fn key (for globe key fix)
    pub fn uses_fn_key(config: &ShortcutsConfig) -> bool {
        let fn_code = dictara_keyboard::Key::Function.to_code();
        config
            .push_to_record
            .keys
            .iter()
            .any(|k| k.keycode == fn_code)
            || config.hands_free.keys.iter().any(|k| k.keycode == fn_code)
    }
}

#[cfg(not(target_os = "linux"))]
impl KeyListener {
    pub fn start(
        command_tx: mpsc::Sender<RecordingCommand>,
        state_manager: Arc<RecordingStateManager>,
        initial_config: ShortcutsConfig,
    ) -> Self {
        info!(
            "Starting KeyListener with initial config: push_to_record={:?}, hands_free={:?}",
            initial_config.push_to_record.keys, initial_config.hands_free.keys
        );

        let (mode_tx, mut mode_rx) = mpsc::channel(10);

        let thread_handle = thread::spawn(move || {
            let mut mode = ListenerMode::Normal {
                shortcuts: initial_config,
            };
            let mut pressed_keys: HashSet<u32> = HashSet::new();

            if let Err(err) = grab(move |event| {
                // Phase 1: Sync to latest mode from control channel
                Self::sync_mode(&mut mode, &mut mode_rx, &mut pressed_keys);

                // Phase 2: Process event with fresh mode
                match &mode {
                    ListenerMode::Normal { shortcuts } => Self::handle_normal_mode(
                        event,
                        shortcuts,
                        &mut pressed_keys,
                        &command_tx,
                        &state_manager,
                    ),
                    ListenerMode::Capture { app_handle } => {
                        Self::handle_capture_mode(event, app_handle)
                    }
                }
            }) {
                error!(
                    "Keyboard grab failed: {}. Keyboard shortcuts will not work.",
                    err
                );
            }
        });

        Self {
            _thread_handle: Some(thread_handle),
            mode_tx,
        }
    }

    /// Drain all pending mode updates from the control channel to ensure we always
    /// process events with the latest mode (avoids stale state)
    fn sync_mode(
        mode: &mut ListenerMode,
        mode_rx: &mut mpsc::Receiver<ListenerMode>,
        pressed_keys: &mut HashSet<u32>,
    ) {
        while let Ok(new_mode) = mode_rx.try_recv() {
            match &new_mode {
                ListenerMode::Normal { shortcuts } => {
                    info!(
                        "KeyListener mode updated: Normal (push_to_record={:?}, hands_free={:?})",
                        shortcuts.push_to_record.keys, shortcuts.hands_free.keys
                    );
                }
                ListenerMode::Capture { .. } => {
                    info!("KeyListener mode updated: Capture");
                }
            }
            *mode = new_mode;
            pressed_keys.clear(); // Reset on mode change
        }
    }

    /// Handle keyboard events in normal mode (shortcuts matching, recording triggers)
    fn handle_normal_mode(
        event: Event,
        shortcuts: &ShortcutsConfig,
        pressed_keys: &mut HashSet<u32>,
        command_tx: &mpsc::Sender<RecordingCommand>,
        state_manager: &Arc<RecordingStateManager>,
    ) -> Option<Event> {
        match event.event_type {
            EventType::KeyPress(key) => {
                let keycode = key.to_code();

                // Check if shortcut was matched BEFORE inserting new key (rising edge detection)
                let was_push_to_record = shortcuts.push_to_record.matches(pressed_keys);
                let was_hands_free = shortcuts.hands_free.matches(pressed_keys);

                pressed_keys.insert(keycode);

                // Push-to-talk: Rising edge detected
                if !was_push_to_record && shortcuts.push_to_record.matches(pressed_keys) {
                    if state_manager.is_recording_locked() {
                        // Stop hands-free mode (push-to-talk can stop hands-free)
                        let _ = command_tx.blocking_send(RecordingCommand::StopRecording);
                    } else {
                        // Start push-to-talk recording
                        let _ = command_tx.blocking_send(RecordingCommand::StartRecording);
                    }
                }

                // Hands-free: Rising edge detected (toggle behavior)
                if !was_hands_free && shortcuts.hands_free.matches(pressed_keys) {
                    if state_manager.is_recording_locked() {
                        // Toggle off: Stop hands-free
                        let _ = command_tx.blocking_send(RecordingCommand::StopRecording);
                    } else {
                        // Toggle on: Start hands-free
                        let _ = command_tx.blocking_send(RecordingCommand::StartRecording);
                        let _ = command_tx.blocking_send(RecordingCommand::LockRecording);
                    }

                    // Swallow Space if it's in the combo
                    let space_code = dictara_keyboard::Key::Space.to_code();
                    if shortcuts
                        .hands_free
                        .keys
                        .iter()
                        .any(|k| k.keycode == space_code)
                    {
                        return None;
                    }
                }

                // Swallow all keys while push-to-record is active
                if shortcuts.push_to_record.matches(pressed_keys) {
                    return None;
                }

                Some(event)
            }
            EventType::KeyRelease(key) => {
                let keycode = key.to_code();

                // Check push-to-record BEFORE removing key
                let was_push_to_record = shortcuts.push_to_record.matches(pressed_keys);

                pressed_keys.remove(&keycode);

                // Release stops recording (unless locked)
                if was_push_to_record && !state_manager.is_recording_locked() {
                    let _ = command_tx.blocking_send(RecordingCommand::StopRecording);
                }

                Some(event)
            }
        }
    }

    /// Handle keyboard events in capture mode (emit to frontend, swallow all)
    fn handle_capture_mode(event: Event, app_handle: &AppHandle) -> Option<Event> {
        match event.event_type {
            EventType::KeyPress(key) => {
                let keycode = key.to_code();
                let label = key.to_label();
                let _ = KeyCaptureEvent::KeyDown { keycode, label }.emit(app_handle);
            }
            EventType::KeyRelease(key) => {
                let keycode = key.to_code();
                let label = key.to_label();
                let _ = KeyCaptureEvent::KeyUp { keycode, label }.emit(app_handle);
            }
        }

        // Swallow ALL events in capture mode (prevent Cmd+Q, etc.)
        None
    }

    /// Enter capture mode to configure shortcuts
    pub fn enter_capture_mode(&self, app_handle: AppHandle) -> Result<(), String> {
        info!("Sending mode change request: Capture");
        self.mode_tx
            .blocking_send(ListenerMode::Capture { app_handle })
            .map_err(|_| "KeyListener thread is not running".to_string())
    }

    /// Exit capture mode and return to normal mode with updated shortcuts
    pub fn exit_capture_mode(&self, shortcuts: ShortcutsConfig) -> Result<(), String> {
        info!(
            "Sending mode change request: Normal (push_to_record={:?}, hands_free={:?})",
            shortcuts.push_to_record.keys, shortcuts.hands_free.keys
        );
        self.mode_tx
            .blocking_send(ListenerMode::Normal { shortcuts })
            .map_err(|_| "KeyListener thread is not running".to_string())
    }

    /// Update shortcuts at runtime (no restart needed!)
    pub fn update_shortcuts(&self, new_config: ShortcutsConfig) -> Result<(), String> {
        info!(
            "Sending shortcuts update request: push_to_record={:?}, hands_free={:?}",
            new_config.push_to_record.keys, new_config.hands_free.keys
        );
        self.mode_tx
            .blocking_send(ListenerMode::Normal {
                shortcuts: new_config,
            })
            .map_err(|_| "KeyListener thread is not running".to_string())
    }
}

/// Start a GlobalShortcuts portal session that drives recording from the two
/// shortcuts, optionally parenting the bind dialog to `parent`. Returns `None`
/// (logged) if the portal is unavailable; recording still works from the tray.
#[cfg(target_os = "linux")]
fn spawn_portal_session(
    command_tx: tokio::sync::mpsc::Sender<RecordingCommand>,
    state_manager: Arc<RecordingStateManager>,
    parent: Option<dictara_keyboard::linux::WindowIdentifier>,
) -> Option<dictara_keyboard::linux::PortalShortcuts> {
    use dictara_keyboard::linux::{PortalShortcutEvent, PortalShortcutSpec, PortalShortcuts};

    // A preferred trigger is effectively mandatory on GNOME: its portal backend
    // (gnome-control-center) only offers an "Add/Cancel" dialog that accepts the
    // app's requested trigger — it does not let the user pick a key. With no
    // preferred trigger GNOME registers the shortcut name with no binding, so
    // nothing fires (BindShortcuts returns []). KDE ignores these and lets the
    // user choose in its own dialog. Values use XDG shortcut syntax; Super-based
    // combos avoid clashes. The app's Fn-based defaults are macOS-only and not
    // expressible here.
    let specs = vec![
        PortalShortcutSpec {
            id: PUSH_TO_RECORD_SHORTCUT_ID.to_string(),
            description: "Push to record (hold to dictate)".to_string(),
            preferred_trigger: Some("SUPER+SHIFT+d".to_string()),
        },
        PortalShortcutSpec {
            id: HANDS_FREE_SHORTCUT_ID.to_string(),
            description: "Hands-free recording (toggle)".to_string(),
            preferred_trigger: Some("SUPER+SHIFT+h".to_string()),
        },
    ];

    // The callback runs inside the portal's tokio runtime, so it must not
    // block; try_send is fine since the channel holds 100 commands.
    let portal = PortalShortcuts::start(specs, parent, move |event| match event {
        PortalShortcutEvent::Activated { id } if id == PUSH_TO_RECORD_SHORTCUT_ID => {
            if state_manager.is_recording_locked() {
                // Push-to-talk can stop hands-free mode
                let _ = command_tx.try_send(RecordingCommand::StopRecording);
            } else {
                let _ = command_tx.try_send(RecordingCommand::StartRecording);
            }
        }
        PortalShortcutEvent::Deactivated { id } if id == PUSH_TO_RECORD_SHORTCUT_ID => {
            // Release stops recording (unless locked by hands-free)
            if !state_manager.is_recording_locked() {
                let _ = command_tx.try_send(RecordingCommand::StopRecording);
            }
        }
        PortalShortcutEvent::Activated { id } if id == HANDS_FREE_SHORTCUT_ID => {
            if state_manager.is_recording_locked() {
                let _ = command_tx.try_send(RecordingCommand::StopRecording);
            } else {
                let _ = command_tx.try_send(RecordingCommand::StartRecording);
                let _ = command_tx.try_send(RecordingCommand::LockRecording);
            }
        }
        _ => {}
    });

    match portal {
        Ok(portal) => {
            info!("GlobalShortcuts portal listener started");
            Some(portal)
        }
        Err(e) => {
            error!(
                "Keyboard shortcuts unavailable on Linux: {}. \
                 Recording can still be started from the tray menu.",
                e
            );
            None
        }
    }
}

#[cfg(target_os = "linux")]
impl KeyListener {
    pub fn start(
        command_tx: tokio::sync::mpsc::Sender<RecordingCommand>,
        state_manager: Arc<RecordingStateManager>,
        _initial_config: ShortcutsConfig,
    ) -> Self {
        let portal = spawn_portal_session(command_tx, state_manager, None);
        Self { portal }
    }

    /// Whether the portal session is up and shortcuts are bound
    pub fn portal_active(&self) -> bool {
        self.portal.is_some()
    }

    /// Open the compositor's shortcut-configuration UI, parented to the
    /// requesting window (`identifier`) when one is available.
    ///
    /// Portal v2 (KDE) shows the dialog via `ConfigureShortcuts` on the running
    /// session. Portal v1 (GNOME) has no such call and its `BindShortcuts`
    /// dialog renders blank on gnome-control-center 50.x, so we don't re-open
    /// it — the shortcuts are already bound at their preferred triggers and are
    /// changed in the GNOME Settings app instead.
    pub fn configure_system_shortcuts(
        &self,
        identifier: Option<dictara_keyboard::linux::WindowIdentifier>,
    ) -> Result<(), String> {
        match self.portal.as_ref() {
            Some(portal) if portal.supports_configure() => portal.configure(identifier),
            Some(_) => Err(
                "On GNOME, change these shortcuts in Settings → Apps → Dictara — \
                             the in-app dialog isn't available for this desktop's shortcuts portal."
                    .into(),
            ),
            None => Err(
                "Could not open the shortcuts dialog (GlobalShortcuts portal unavailable)".into(),
            ),
        }
    }

    /// List shortcuts as currently bound by the compositor
    pub fn list_system_shortcuts(
        &self,
    ) -> Result<Vec<dictara_keyboard::linux::BoundShortcut>, String> {
        self.portal
            .as_ref()
            .ok_or("GlobalShortcuts portal is not available")?
            .list()
    }

    /// In-app key capture is impossible on Wayland (we never see raw keys);
    /// the frontend uses the system dialog via `configure_system_shortcuts`.
    pub fn enter_capture_mode(&self, _app_handle: AppHandle) -> Result<(), String> {
        Err("In-app key capture is not available on Linux; \
             shortcuts are configured through the system dialog"
            .to_string())
    }

    /// No-op on Linux (nothing to restore; capture mode never started)
    pub fn exit_capture_mode(&self, _shortcuts: ShortcutsConfig) -> Result<(), String> {
        Ok(())
    }

    /// No-op on Linux: bindings live in the compositor, keyed by shortcut id,
    /// not in the app's ShortcutsConfig keycodes.
    pub fn update_shortcuts(&self, _new_config: ShortcutsConfig) -> Result<(), String> {
        info!("Ignoring shortcuts keycode update on Linux (compositor owns portal bindings)");
        Ok(())
    }
}
