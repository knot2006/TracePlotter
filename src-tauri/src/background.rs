use super::*;
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
#[cfg(windows)]
use tauri_plugin_autostart::ManagerExt;

#[derive(Clone, Copy, Default, Deserialize, Serialize)]
#[serde(default, rename_all = "camelCase")]
pub(super) struct Options {
    close_to_tray: bool,
    minimize_to_tray: bool,
}

#[derive(Default)]
pub(super) struct Background {
    options: Mutex<Options>,
    available: AtomicBool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Status {
    #[serde(flatten)]
    options: Options,
    available: bool,
    startup: bool,
    startup_available: bool,
}

pub(super) fn show(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

#[tauri::command]
pub(super) fn exit_app(app: AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        save_window_state(&window.as_ref().window());
    }
    app.exit(0);
}

pub(super) fn setup(app: &AppHandle) {
    let state = app.state::<Background>();
    if let Ok(text) = fs::read_to_string(data_dir().join("background.json")) {
        *state.options.lock().unwrap() = serde_json::from_str(&text).unwrap_or_default();
    }
    let create = || -> tauri::Result<()> {
        let show = MenuItem::with_id(app, "tray-show", "Show TracePlotter", true, None::<&str>)?;
        let quit = MenuItem::with_id(
            app,
            "tray-exit",
            "Exit and stop monitoring",
            true,
            None::<&str>,
        )?;
        let menu = Menu::with_items(app, &[&show, &quit])?;
        let icon = app
            .default_window_icon()
            .ok_or_else(|| tauri::Error::AssetNotFound("app icon".into()))?;
        TrayIconBuilder::with_id("monitor")
            .icon(icon.clone())
            .tooltip(if demo() {
                "TracePlotter (demo) — monitoring"
            } else {
                "TracePlotter — monitoring"
            })
            .menu(&menu)
            .show_menu_on_left_click(false)
            .on_menu_event(|app, event| match event.id.as_ref() {
                "tray-show" => self::show(app),
                "tray-exit" => exit_app(app.clone()),
                _ => {}
            })
            .on_tray_icon_event(|tray, event| {
                if matches!(
                    event,
                    TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    }
                ) {
                    self::show(tray.app_handle());
                }
            })
            .build(app)?;
        Ok(())
    };
    // A desktop without a tray must still have a usable window and normal close behavior.
    state.available.store(create().is_ok(), Ordering::Relaxed);
}

#[tauri::command]
pub(super) fn background_status(
    app: AppHandle,
    state: State<Background>,
) -> Result<Status, String> {
    #[cfg(windows)]
    let startup = app.autolaunch().is_enabled().map_err(|e| e.to_string())?;
    #[cfg(not(windows))]
    let startup = {
        let _ = app;
        false
    };
    Ok(Status {
        options: *state.options.lock().unwrap(),
        available: state.available.load(Ordering::Relaxed),
        startup,
        startup_available: cfg!(windows) && !demo(),
    })
}

#[tauri::command]
pub(super) fn set_background(options: Options, state: State<Background>) -> Result<(), String> {
    if !state.available.load(Ordering::Relaxed)
        && (options.close_to_tray || options.minimize_to_tray)
    {
        return Err("The system tray is unavailable on this desktop".into());
    }
    let text = serde_json::to_string(&options).map_err(|e| e.to_string())?;
    fs::write(data_dir().join("background.json"), text).map_err(|e| e.to_string())?;
    *state.options.lock().unwrap() = options;
    Ok(())
}

#[tauri::command]
pub(super) fn set_startup(app: AppHandle, enabled: bool) -> Result<(), String> {
    #[cfg(windows)]
    {
        if demo() {
            return Err("Startup registration is unavailable in demo mode".into());
        }
        let manager = app.autolaunch();
        if enabled {
            manager.enable()
        } else {
            manager.disable()
        }
        .map_err(|e| e.to_string())
    }
    #[cfg(not(windows))]
    {
        let _ = (app, enabled);
        Err("Start at login is currently available on Windows".into())
    }
}

#[tauri::command]
pub(super) fn hide_to_tray(app: AppHandle, state: State<Background>) -> Result<(), String> {
    if !state.available.load(Ordering::Relaxed) {
        return Err("The system tray is unavailable".into());
    }
    let window = app
        .get_webview_window("main")
        .ok_or("Window is unavailable")?;
    save_window_state(&window.as_ref().window());
    window.hide().map_err(|e| e.to_string())
}

pub(super) fn window_event(window: &tauri::Window, event: &WindowEvent) {
    let state = window.state::<Background>();
    if !state.available.load(Ordering::Relaxed) {
        return;
    }
    let options = *state.options.lock().unwrap();
    if let WindowEvent::CloseRequested { api, .. } = event
        && options.close_to_tray
        && window.hide().is_ok()
    {
        api.prevent_close();
    }
    if matches!(event, WindowEvent::Resized(_))
        && options.minimize_to_tray
        && window.is_minimized().unwrap_or(false)
    {
        let _ = window.hide();
    }
}
