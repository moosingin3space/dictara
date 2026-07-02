use crate::config::{self, ConfigKey, ConfigStore, ShortcutsConfig};
use crate::keyboard_listener::KeyListener;
use log::info;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, State};

/// Build an XDG portal `WindowIdentifier` for `window` so the compositor can
/// parent its shortcut dialog to us. Only Wayland is handled (our target); any
/// other/failing case yields `None`, and the dialog is shown unparented.
///
/// The surface/display pointers are exported via xdg-foreign, which is async.
/// We copy them across to a throwaway thread with its own runtime rather than
/// touching the app runtime or holding the non-`Send` handles across an await.
/// `window` outlives this call, so the surface stays valid during the export.
#[cfg(target_os = "linux")]
fn window_identifier(
    window: &tauri::WebviewWindow,
) -> Option<dictara_keyboard::linux::WindowIdentifier> {
    use raw_window_handle::{
        HasDisplayHandle, HasWindowHandle, RawDisplayHandle, RawWindowHandle, WaylandDisplayHandle,
        WaylandWindowHandle,
    };

    let (surface, display) = match (
        window.window_handle().ok()?.as_raw(),
        window.display_handle().ok()?.as_raw(),
    ) {
        (RawWindowHandle::Wayland(w), RawDisplayHandle::Wayland(d)) => {
            (w.surface.as_ptr() as usize, d.display.as_ptr() as usize)
        }
        _ => return None,
    };

    std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .ok()?;
        runtime.block_on(async move {
            let surface = std::ptr::NonNull::new(surface as *mut _)?;
            let display = std::ptr::NonNull::new(display as *mut _)?;
            let raw_window = RawWindowHandle::Wayland(WaylandWindowHandle::new(surface));
            let raw_display = RawDisplayHandle::Wayland(WaylandDisplayHandle::new(display));
            dictara_keyboard::linux::WindowIdentifier::from_raw_handle(
                &raw_window,
                Some(&raw_display),
            )
            .await
        })
    })
    .join()
    .ok()?
}

/// How shortcuts can be configured on this platform/session
#[derive(Debug, Clone, Copy, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub enum ShortcutCaptureCapability {
    /// In-app "press your keys now" capture (macOS CGEvent tap)
    RawCapture,
    /// Binding happens in the compositor's own dialog via the
    /// GlobalShortcuts portal (Linux/Wayland)
    PortalBind,
    /// No way to listen for global shortcuts in this session
    Unavailable,
}

/// A shortcut as bound by the compositor (GlobalShortcuts portal)
#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct SystemShortcut {
    pub id: String,
    pub description: String,
    /// Human-readable trigger for display (e.g. "Ctrl+Alt+D");
    /// empty if the user hasn't assigned a trigger yet
    pub trigger_description: String,
}

#[tauri::command]
#[specta::specta]
#[cfg_attr(not(target_os = "linux"), allow(unused_variables))]
pub fn get_shortcut_capture_capability(
    key_listener: State<KeyListener>,
) -> ShortcutCaptureCapability {
    #[cfg(target_os = "macos")]
    {
        ShortcutCaptureCapability::RawCapture
    }
    #[cfg(target_os = "linux")]
    {
        if key_listener.portal_active() {
            ShortcutCaptureCapability::PortalBind
        } else {
            ShortcutCaptureCapability::Unavailable
        }
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        ShortcutCaptureCapability::Unavailable
    }
}

/// Open the compositor's shortcut configuration dialog (Linux/Wayland only)
#[tauri::command]
#[specta::specta]
#[cfg_attr(not(target_os = "linux"), allow(unused_variables))]
pub fn configure_system_shortcuts(
    window: tauri::WebviewWindow,
    key_listener: State<KeyListener>,
) -> Result<(), String> {
    #[cfg(target_os = "linux")]
    {
        key_listener.configure_system_shortcuts(window_identifier(&window))
    }
    #[cfg(not(target_os = "linux"))]
    {
        Err("System shortcut configuration is only available on Linux".to_string())
    }
}

/// List shortcuts as currently bound by the compositor (Linux/Wayland only)
#[tauri::command]
#[specta::specta]
#[cfg_attr(not(target_os = "linux"), allow(unused_variables))]
pub fn list_system_shortcuts(
    key_listener: State<KeyListener>,
) -> Result<Vec<SystemShortcut>, String> {
    #[cfg(target_os = "linux")]
    {
        Ok(key_listener
            .list_system_shortcuts()?
            .into_iter()
            .map(|s| SystemShortcut {
                id: s.id,
                description: s.description,
                trigger_description: s.trigger_description,
            })
            .collect())
    }
    #[cfg(not(target_os = "linux"))]
    {
        Err("System shortcuts are only available on Linux".to_string())
    }
}

#[tauri::command]
#[specta::specta]
pub fn load_shortcuts_config(
    config_store: State<config::Config>,
) -> Result<ShortcutsConfig, String> {
    Ok(config_store.get(&ConfigKey::SHORTCUTS).unwrap_or_default())
}

#[tauri::command]
#[specta::specta]
pub fn save_shortcuts_config(
    config_store: State<config::Config>,
    key_listener: State<KeyListener>,
    config: ShortcutsConfig,
) -> Result<(), String> {
    // Validate all shortcuts
    config.push_to_record.validate()?;
    config.hands_free.validate()?;

    // Load old config for Fn key change detection
    let old_config = config_store.get(&ConfigKey::SHORTCUTS).unwrap_or_default();
    let old_uses_fn = KeyListener::uses_fn_key(&old_config);
    let new_uses_fn = KeyListener::uses_fn_key(&config);

    // Save to persistent storage
    config_store.set(&ConfigKey::SHORTCUTS, config.clone())?;
    info!(
        "Shortcuts config saved: push_to_record={:?}, hands_free={:?}",
        config.push_to_record.keys, config.hands_free.keys
    );

    // Hot-swap runtime config via channel (NO RESTART NEEDED!)
    key_listener.update_shortcuts(config)?;
    info!("Shortcuts config hot-swapped to KeyListener");

    // Update globe key fix if Fn usage changed
    if !old_uses_fn && new_uses_fn {
        crate::globe_key::fix_globe_key_if_needed();
        info!("Globe key fix applied (Fn key now in use)");
    }

    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn reset_shortcuts_config(
    config_store: State<config::Config>,
    key_listener: State<KeyListener>,
) -> Result<ShortcutsConfig, String> {
    let defaults = ShortcutsConfig::default();
    config_store.set(&ConfigKey::SHORTCUTS, defaults.clone())?;
    key_listener.update_shortcuts(defaults.clone())?;
    info!("Shortcuts config reset to defaults and hot-swapped to KeyListener");
    Ok(defaults)
}

#[tauri::command]
#[specta::specta]
pub fn start_key_capture(
    app_handle: AppHandle,
    key_listener: State<KeyListener>,
) -> Result<(), String> {
    info!("Entering key capture mode");
    // Switch KeyListener to capture mode
    key_listener.enter_capture_mode(app_handle)
}

#[tauri::command]
#[specta::specta]
pub fn stop_key_capture(
    key_listener: State<KeyListener>,
    config_store: State<config::Config>,
) -> Result<(), String> {
    // Load current shortcuts config
    let shortcuts = config_store
        .get(&ConfigKey::SHORTCUTS)
        .ok_or("Failed to load shortcuts config")?;

    info!("Exiting key capture mode, returning to normal mode");
    // Switch KeyListener back to normal mode
    key_listener.exit_capture_mode(shortcuts)
}
