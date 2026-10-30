//! Background monitoring: a tray icon, closing or minimizing to it, and start at sign-in.
//! Probing and alerts run in the controller, so they carry on while the window is hidden.
use super::*;
use std::sync::atomic::{AtomicBool, Ordering};
use tray_icon::menu::{Menu, MenuEvent, MenuItem};
use tray_icon::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};

/// Kept in the same file as the legacy interface's, so the choice carries over.
#[derive(Clone, Copy, Default, Deserialize, Serialize)]
#[serde(default, rename_all = "camelCase")]
pub(super) struct Options {
    pub close_to_tray: bool,
    pub minimize_to_tray: bool,
}
impl Options {
    pub fn load() -> Self {
        fs::read_to_string(data_dir().join("background.json"))
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }
    pub fn save(&self) -> Result<(), String> {
        let text = serde_json::to_string(self).map_err(|e| e.to_string())?;
        model::atomic_write(&data_dir().join("background.json"), text.as_bytes())
    }
}

pub(super) struct Tray {
    _icon: TrayIcon,
    /// Set by the tray's exit item and File > Exit: the next close request really closes.
    pub quitting: Arc<AtomicBool>,
}

/// Winit cannot hide a window on Wayland, so there the window is minimized instead.
pub(super) fn wayland() -> bool {
    cfg!(target_os = "linux") && std::env::var_os("WAYLAND_DISPLAY").is_some()
}

pub(super) fn hide(ctx: &egui::Context) {
    ctx.send_viewport_cmd(if wayland() {
        egui::ViewportCommand::Minimized(true)
    } else {
        egui::ViewportCommand::Visible(false)
    });
}

pub(super) fn show(ctx: &egui::Context) {
    ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
    ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
    ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
    ctx.request_repaint();
}

/// Creates the tray icon, or `None` on a desktop without a tray: the window then keeps its
/// normal closing behaviour. On Windows this must run on the thread of the event loop.
pub(super) fn start(ctx: &egui::Context) -> Option<Tray> {
    let image = image::load_from_memory(include_bytes!("../../icons/128x128.png"))
        .ok()?
        .to_rgba8();
    let (width, height) = image.dimensions();
    let icon = tray_icon::Icon::from_rgba(image.into_raw(), width, height).ok()?;
    let show_item = MenuItem::new("Show TracePlotter", true, None);
    let exit_item = MenuItem::new("Exit and stop monitoring", true, None);
    let menu = Menu::new();
    menu.append_items(&[&show_item, &exit_item]).ok()?;
    let tray = TrayIconBuilder::new()
        .with_icon(icon)
        .with_tooltip(if demo() {
            "TracePlotter (demo) — monitoring"
        } else {
            "TracePlotter — monitoring"
        })
        .with_menu(Box::new(menu))
        .with_menu_on_left_click(false)
        .build()
        .map_err(|e| eprintln!("No tray icon: {e}"))
        .ok()?;
    let quitting = Arc::new(AtomicBool::new(false));
    // The handlers run on the tray's thread, also while the window is hidden and draws nothing.
    let (show_id, exit_id) = (show_item.id().clone(), exit_item.id().clone());
    let (menu_ctx, menu_quitting) = (ctx.clone(), quitting.clone());
    MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
        if event.id == show_id {
            show(&menu_ctx);
        } else if event.id == exit_id {
            menu_quitting.store(true, Ordering::Relaxed);
            menu_ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            menu_ctx.request_repaint();
        }
    }));
    let click_ctx = ctx.clone();
    TrayIconEvent::set_event_handler(Some(move |event: TrayIconEvent| {
        if matches!(
            event,
            TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            }
        ) {
            show(&click_ctx);
        }
    }));
    Some(Tray {
        _icon: tray,
        quitting,
    })
}

/// Start at sign-in is registered for the current Windows account, as the legacy plugin
/// did it, so either interface sees and changes the same entry.
pub(super) fn startup_available() -> bool {
    cfg!(windows) && !demo()
}

#[cfg(windows)]
fn launcher() -> Result<auto_launch::AutoLaunch, String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    auto_launch::AutoLaunchBuilder::new()
        .set_app_name("TracePlotter")
        .set_app_path(&exe.display().to_string())
        .set_args(&["--background"])
        .build()
        .map_err(|e| e.to_string())
}

pub(super) fn startup() -> bool {
    #[cfg(windows)]
    return launcher()
        .and_then(|l| l.is_enabled().map_err(|e| e.to_string()))
        .unwrap_or(false);
    #[cfg(not(windows))]
    false
}

pub(super) fn set_startup(enabled: bool) -> Result<(), String> {
    if !startup_available() {
        return Err(if demo() {
            "Startup registration is unavailable in demo mode".into()
        } else {
            "Start at login is currently available on Windows".into()
        });
    }
    #[cfg(windows)]
    {
        let launcher = launcher()?;
        if enabled {
            launcher.enable()
        } else {
            launcher.disable()
        }
        .map_err(|e| e.to_string())
    }
    #[cfg(not(windows))]
    {
        let _ = enabled;
        unreachable!("startup_available is false off Windows")
    }
}
