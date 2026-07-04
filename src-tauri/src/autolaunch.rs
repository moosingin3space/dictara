use crate::config::{self, AppConfig, ConfigKey, ConfigStore};
use log::{error, info, warn};
use tauri_plugin_autostart::ManagerExt;

/// Returns true when running inside a Flatpak sandbox.
#[cfg(target_os = "linux")]
fn is_flatpak() -> bool {
    std::path::Path::new("/.flatpak-info").exists()
}

/// How long to wait before issuing the Background portal request on first
/// launch. Issuing it while the GlobalShortcuts portal is still setting up its
/// session makes that session-creation time out (xdg-desktop-portal-gnome
/// serializes these), leaving the recording shortcuts unbound — the worst
/// possible first-run outcome. The GlobalShortcuts session bind window is
/// capped by its own 10s timeout, so waiting past that keeps the two portal
/// interactions from overlapping. Autostart isn't time-sensitive: it only has
/// to be registered sometime during this first session.
#[cfg(target_os = "linux")]
const AUTOSTART_PORTAL_DELAY: std::time::Duration = std::time::Duration::from_secs(12);

/// Spawns a background thread that requests autostart via the XDG Background
/// portal (`org.freedesktop.portal.Background`).  The thread builds its own
/// single-threaded Tokio runtime so the D-Bus round-trip (which may involve a
/// compositor dialog) never blocks the main app startup.  On success the
/// setup-done flag is persisted to the config store.
#[cfg(target_os = "linux")]
fn request_flatpak_autostart(config_store: &config::Config, app_config: &AppConfig) {
    let config_store = config_store.clone();
    let mut app_config = app_config.clone();
    std::thread::spawn(move || {
        // Stay clear of the GlobalShortcuts portal's startup session setup.
        std::thread::sleep(AUTOSTART_PORTAL_DELAY);

        let runtime = match tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            Ok(rt) => rt,
            Err(e) => {
                warn!("autostart: failed to build tokio runtime: {e}");
                return;
            }
        };
        let result = runtime.block_on(async {
            ashpd::desktop::background::Background::request()
                .reason(
                    "Dictara runs in the background so your dictation shortcut is always ready.",
                )
                .auto_start(true)
                .command(["dictara"])
                .dbus_activatable(false)
                .send()
                .await?
                .response()
        });
        match result {
            Ok(response) if response.auto_start() => {
                info!("Autostart enabled via the Background portal");
                app_config.autostart_initial_setup_done = true;
                if let Err(e) = config_store.set(&ConfigKey::APP, app_config.clone()) {
                    error!("Failed to save autostart setup flag: {e}");
                }
            }
            Ok(_) => info!("Background portal did not grant autostart (user declined?)"),
            Err(e) => warn!("Failed to request background autostart via portal: {e}"),
        }
    });
}

/// Setup autolaunch on first launch if not already done
///
/// This function:
/// - Checks if autolaunch has been set up before
/// - If not, enables autolaunch and marks it as done
/// - Logs appropriate messages for success/failure
///
/// On Linux inside a Flatpak sandbox the XDG Background portal is used instead
/// of the tauri-plugin-autostart plugin, because the plugin writes a
/// `~/.config/autostart/*.desktop` file that lives inside the sandbox and is
/// never read by the host session.  The portal creates a host-side autostart
/// entry that runs `flatpak run <app-id>`.
pub fn setup_autolaunch_if_needed(
    app: &tauri::AppHandle,
    config_store: &config::Config,
    app_config: &mut AppConfig,
) {
    // Skip if already set up
    if app_config.autostart_initial_setup_done {
        return;
    }

    // On Flatpak Linux, delegate to the XDG Background portal.
    #[cfg(target_os = "linux")]
    if is_flatpak() {
        info!("First launch detected - requesting background autostart via portal");
        request_flatpak_autostart(config_store, app_config);
        return; // flag is persisted by the spawned thread on success
    }

    // macOS, Windows, and non-Flatpak Linux: use the tauri-plugin-autostart plugin.
    info!("First launch detected - enabling autostart");
    let autostart_manager = app.autolaunch();

    if let Err(e) = autostart_manager.enable() {
        warn!("Failed to enable autostart on first launch: {}", e);
    } else {
        info!("Autostart enabled successfully");
        app_config.autostart_initial_setup_done = true;
        if let Err(e) = config_store.set(&ConfigKey::APP, app_config.clone()) {
            error!("Failed to save autostart setup flag: {}", e);
        }
    }
}
