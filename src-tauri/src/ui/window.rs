use log::{error, warn};
use std::sync::mpsc;
use tauri::window::Color;
use tauri::{Manager, Monitor};

type AnyError = Box<dyn std::error::Error + Send + Sync>;

const POPUP_WIDTH_NORMAL: u32 = 80;
const POPUP_WIDTH_ERROR: u32 = 400; // 5x wider for error display
const POPUP_HEIGHT: u32 = 74;
const BOTTOM_MARGIN: i32 = 100;

/// Show a window without stealing focus (macOS only).
/// Uses `orderFront:` instead of `makeKeyAndOrderFront:` to avoid activating the app.
#[cfg(target_os = "macos")]
fn show_window_without_focus(window: &tauri::WebviewWindow) -> Result<(), AnyError> {
    use objc2::msg_send;
    use objc2::runtime::AnyObject;
    use objc2_app_kit::{NSApplicationActivationOptions, NSWorkspace};
    use std::ptr;

    // Capture the currently frontmost app so we can restore focus after showing our popup.
    let frontmost_app = NSWorkspace::sharedWorkspace().frontmostApplication();

    // Get the raw NSWindow pointer from Tauri
    let ns_window_ptr = match window.ns_window() {
        Ok(ptr) => ptr as *mut AnyObject,
        Err(e) => {
            error!("Failed to get ns_window: {:?}, falling back to show()", e);
            window.show()?;
            return Ok(());
        }
    };

    // Safety: ns_window_ptr is a valid NSWindow pointer from Tauri
    unsafe {
        // First, make the window visible (setIsVisible:YES doesn't activate)
        let _: () = msg_send![ns_window_ptr, setIsVisible: true];
        // Then bring to front without making key (orderFront: vs makeKeyAndOrderFront:)
        let _: () = msg_send![ns_window_ptr, orderFront: ptr::null::<AnyObject>()];
    }

    // Give focus back to whoever had it before we showed the popup.
    if let Some(app) = frontmost_app {
        let app_handle = window.app_handle();
        let _ = app_handle.run_on_main_thread(move || {
            let _ = app.activateWithOptions(NSApplicationActivationOptions(0));
        });
    }

    Ok(())
}

#[cfg(not(target_os = "macos"))]
fn show_window_without_focus(window: &tauri::WebviewWindow) -> Result<(), AnyError> {
    window.show()?;
    Ok(())
}

/// Find the monitor containing the cursor position.
fn get_monitor_at_cursor(app_handle: &tauri::AppHandle) -> Option<Monitor> {
    let cursor_pos = app_handle.cursor_position().ok()?;

    let monitors = app_handle.available_monitors().ok()?;

    for monitor in monitors {
        let pos = monitor.position();
        let size = monitor.size();

        // Check if cursor is within this monitor's bounds (physical coordinates)
        if cursor_pos.x >= pos.x as f64
            && cursor_pos.x < (pos.x + size.width as i32) as f64
            && cursor_pos.y >= pos.y as f64
            && cursor_pos.y < (pos.y + size.height as i32) as f64
        {
            return Some(monitor);
        }
    }

    None
}

/// Pick the monitor to place the recording popup on.
///
/// Ordered fallbacks: the monitor under the cursor (macOS/X11), then the
/// primary monitor (macOS/X11), then simply the first available monitor.
/// The last tier is what makes this work on Wayland, where the compositor
/// exposes no cursor position or "primary" monitor but does list outputs.
fn select_popup_monitor(app_handle: &tauri::AppHandle) -> Option<Monitor> {
    get_monitor_at_cursor(app_handle)
        .or_else(|| app_handle.primary_monitor().ok().flatten())
        .or_else(|| app_handle.available_monitors().ok()?.into_iter().next())
}

fn run_on_main_thread_sync<T, F>(app_handle: &tauri::AppHandle, f: F) -> Result<T, AnyError>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, AnyError> + Send + 'static,
{
    let (tx, rx) = mpsc::channel();
    let app_handle_clone = app_handle.clone();

    // macOS AppKit APIs (NSWindow) must run on the main thread; wrap the work and
    // shuttle the result back through a channel so callers can stay synchronous.
    app_handle_clone.run_on_main_thread(move || {
        let _ = tx.send(f());
    })?;

    rx.recv()
        .unwrap_or_else(|_| Err("Failed to receive result from main thread task".into()))
}

pub fn open_recording_popup(app_handle: &tauri::AppHandle) -> Result<(), AnyError> {
    let app_handle_for_closure = app_handle.clone();
    run_on_main_thread_sync(app_handle, move || {
        open_recording_popup_inner(&app_handle_for_closure)
    })
}

pub fn close_recording_popup(app_handle: &tauri::AppHandle) -> Result<(), AnyError> {
    let app_handle_for_closure = app_handle.clone();
    run_on_main_thread_sync(app_handle, move || {
        close_recording_popup_inner(&app_handle_for_closure)
    })
}

fn open_recording_popup_inner(app_handle: &tauri::AppHandle) -> Result<(), AnyError> {
    if let Some(window) = app_handle.get_webview_window("recording-popup") {
        // Set size
        if let Err(e) = window.set_size(tauri::Size::Logical(tauri::LogicalSize {
            width: POPUP_WIDTH_NORMAL as f64,
            height: POPUP_HEIGHT as f64,
        })) {
            error!("Failed to set window size: {}", e);
        }

        // Get monitor for popup placement: cursor → primary → first available (Wayland fallback)
        let monitor = select_popup_monitor(app_handle);

        if let Some(monitor) = monitor {
            let scale_factor = monitor.scale_factor();
            let work_area = monitor.work_area();

            // Convert physical to logical coordinates using the work area
            let logical_width = work_area.size.width as f64 / scale_factor;
            let logical_height = work_area.size.height as f64 / scale_factor;
            let logical_x = work_area.position.x as f64 / scale_factor;
            let logical_y = work_area.position.y as f64 / scale_factor;

            // Calculate centered horizontal position
            let x = logical_x + (logical_width - POPUP_WIDTH_NORMAL as f64) / 2.0;

            // Calculate position from bottom
            let y = logical_y + logical_height - POPUP_HEIGHT as f64 - BOTTOM_MARGIN as f64;

            if let Err(e) =
                window.set_position(tauri::Position::Logical(tauri::LogicalPosition { x, y }))
            {
                error!("Failed to set window position: {}", e);
            }
        } else {
            warn!("No monitor found for popup placement; the compositor will decide where to put it (best-effort on Wayland)");
        }

        if let Err(e) = show_window_without_focus(&window) {
            error!("Failed to show recording popup: {}", e);
            return Err(e);
        }
    } else {
        return Err("Recording popup window not found".into());
    }

    Ok(())
}

fn close_recording_popup_inner(app_handle: &tauri::AppHandle) -> Result<(), AnyError> {
    if let Some(window) = app_handle.get_webview_window("recording-popup") {
        if let Err(e) = window.hide() {
            error!("Failed to hide recording popup: {}", e);
            return Err(Box::new(e));
        }
    } else {
        return Err("Recording popup window not found".into());
    }

    Ok(())
}

pub fn resize_recording_popup_for_error(app_handle: &tauri::AppHandle) -> Result<(), AnyError> {
    let app_handle_for_closure = app_handle.clone();
    run_on_main_thread_sync(app_handle, move || {
        resize_recording_popup_inner(&app_handle_for_closure, POPUP_WIDTH_ERROR)
    })
}

fn resize_recording_popup_inner(app_handle: &tauri::AppHandle, width: u32) -> Result<(), AnyError> {
    if let Some(window) = app_handle.get_webview_window("recording-popup") {
        // Set new size
        window.set_size(tauri::Size::Logical(tauri::LogicalSize {
            width: width as f64,
            height: POPUP_HEIGHT as f64,
        }))?;

        // Recalculate centered position: cursor → primary → first available (Wayland fallback)
        let monitor = select_popup_monitor(app_handle);

        if let Some(monitor) = monitor {
            let scale_factor = monitor.scale_factor();
            let work_area = monitor.work_area();

            let logical_width = work_area.size.width as f64 / scale_factor;
            let logical_height = work_area.size.height as f64 / scale_factor;
            let logical_x = work_area.position.x as f64 / scale_factor;
            let logical_y = work_area.position.y as f64 / scale_factor;

            // Center horizontally with new width
            let x = logical_x + (logical_width - width as f64) / 2.0;
            let y = logical_y + logical_height - POPUP_HEIGHT as f64 - BOTTOM_MARGIN as f64;

            window.set_position(tauri::Position::Logical(tauri::LogicalPosition { x, y }))?;
        }

        Ok(())
    } else {
        Err("Recording popup window not found".into())
    }
}

pub fn open_preferences_window(app_handle: &tauri::AppHandle) -> Result<(), AnyError> {
    let (width, height) = (750.0, 650.0);

    let window = match app_handle.get_webview_window("preferences") {
        Some(w) => w,
        None => tauri::WebviewWindowBuilder::new(
            app_handle,
            "preferences",
            tauri::WebviewUrl::App("preferences".into()),
        )
        .title("Preferences")
        .inner_size(width, height)
        .min_inner_size(width, height)
        // .max_inner_size(width, height)
        .background_color(Color(10, 10, 10, 255)) // Dark background to prevent white flash
        .visible(false)
        .build()?,
    };

    window.show()?;
    window.set_focus()?;
    window.center()?;

    Ok(())
}

pub fn open_onboarding_window(app_handle: &tauri::AppHandle) -> Result<(), AnyError> {
    let (width, height) = (800.0, 800.0);

    let window = match app_handle.get_webview_window("onboarding") {
        Some(w) => w,
        None => tauri::WebviewWindowBuilder::new(
            app_handle,
            "onboarding",
            tauri::WebviewUrl::App("onboarding".into()),
        )
        .title("Welcome to Dictara")
        .inner_size(width, height)
        .min_inner_size(width, height)
        .max_inner_size(width, height) // Fixed size for wizard
        .resizable(false)
        .background_color(Color(10, 10, 10, 255)) // Dark background to prevent white flash
        .visible(false)
        .build()?,
    };

    window.show()?;
    window.set_focus()?;
    window.center()?;

    Ok(())
}

#[allow(dead_code)]
pub fn close_onboarding_window(app_handle: &tauri::AppHandle) -> Result<(), AnyError> {
    if let Some(window) = app_handle.get_webview_window("onboarding") {
        window.close()?;
    }
    Ok(())
}
